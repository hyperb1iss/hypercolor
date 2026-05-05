param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $DaemonArgs
)

$ErrorActionPreference = 'Stop'

$DaemonArgs = @($DaemonArgs | Where-Object { $null -ne $_ -and $_ -ne '' })

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

function Add-PathPrefix {
    param([string] $Path)

    if ((Test-Path $Path) -and (($env:Path -split ';') -notcontains $Path)) {
        $env:Path = "$Path;$env:Path"
    }
}

function Enter-HypercolorVsDevShell {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) {
        throw 'vswhere.exe not found. Install Visual Studio 2026 Build Tools with the C++ desktop workload.'
    }

    $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (-not $vs) {
        throw 'No Visual Studio C++ toolchain found. Install the C++ desktop workload.'
    }

    $devShell = Join-Path $vs 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll'
    Import-Module $devShell
    Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64' | Out-Null
}

function Start-HypercolorChild {
    param(
        [string] $FilePath,
        [string[]] $Arguments,
        [string] $WorkingDirectory
    )

    $command = Get-Command $FilePath -ErrorAction Stop
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $command.Source
    $startInfo.WorkingDirectory = $WorkingDirectory
    $startInfo.UseShellExecute = $false
    $startInfo.Arguments = Join-HypercolorWindowsArguments $Arguments

    $process = [System.Diagnostics.Process]::Start($startInfo)
    if ($null -eq $process) {
        throw "Failed to start $FilePath"
    }

    return $process
}

function Join-HypercolorWindowsArguments {
    param([string[]] $Arguments)

    return (($Arguments | ForEach-Object { ConvertTo-HypercolorWindowsArgument $_ }) -join ' ')
}

function ConvertTo-HypercolorWindowsArgument {
    param([string] $Argument)

    if ($null -eq $Argument -or $Argument.Length -eq 0) {
        return '""'
    }

    if ($Argument -notmatch '[\s"]') {
        return $Argument
    }

    $quoted = '"'
    $backslashes = 0
    foreach ($char in $Argument.ToCharArray()) {
        if ($char -eq '\') {
            $backslashes += 1
            continue
        }

        if ($char -eq '"') {
            $quoted += ('\' * (($backslashes * 2) + 1))
            $quoted += '"'
            $backslashes = 0
            continue
        }

        if ($backslashes -gt 0) {
            $quoted += ('\' * $backslashes)
            $backslashes = 0
        }
        $quoted += $char
    }

    if ($backslashes -gt 0) {
        $quoted += ('\' * ($backslashes * 2))
    }
    $quoted += '"'

    return $quoted
}

function Stop-HypercolorChild {
    param([System.Diagnostics.Process] $Process)

    if ($null -eq $Process -or $Process.HasExited) {
        return
    }

    & taskkill.exe /PID $Process.Id /T /F | Out-Null
}

function Add-HypercolorAngleRuntimePath {
    $buildDir = Join-Path $RepoRoot 'target\preview\build'
    if (-not (Test-Path $buildDir)) {
        return
    }

    $eglDll = Get-ChildItem -Path $buildDir -Recurse -Filter 'libEGL.dll' -ErrorAction SilentlyContinue |
        Where-Object { Test-Path (Join-Path $_.DirectoryName 'libGLESv2.dll') } |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1

    if ($null -eq $eglDll) {
        return
    }

    Add-PathPrefix $eglDll.DirectoryName
    $env:HYPERCOLOR_ANGLE_DIR = $eglDll.DirectoryName
}

Enter-HypercolorVsDevShell

$nasmDir = 'C:\Program Files\NASM'
$nasmExe = Join-Path $nasmDir 'nasm.exe'
if (Test-Path $nasmExe) {
    Add-PathPrefix $nasmDir
    $env:ASM_NASM = $nasmExe
}

Remove-Item Env:NO_COLOR -ErrorAction SilentlyContinue

Write-Host '[dev] building bundled effects'
Push-Location (Join-Path $RepoRoot 'sdk')
try {
    & bun scripts/build-effect.ts --all
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
} finally {
    Pop-Location
}

if ([string]::IsNullOrWhiteSpace($env:HYPERCOLOR_COMPOSITOR_ACCELERATION_MODE)) {
    $compositorAccelerationMode = 'auto'
} else {
    $compositorAccelerationMode = $env:HYPERCOLOR_COMPOSITOR_ACCELERATION_MODE
}
Write-Host "[dev] compositor acceleration mode: $compositorAccelerationMode"

$daemonArguments = @(
    '--log-level',
    'debug',
    '--compositor-acceleration-mode',
    $compositorAccelerationMode,
    '--bind',
    '127.0.0.1:9420'
) + $DaemonArgs

$cargoBuildArguments = @(
    'build',
    '-p',
    'hypercolor-daemon',
    '--bin',
    'hypercolor-daemon',
    '--profile',
    'preview',
    '--features',
    'servo wgpu'
)

$daemonExe = Join-Path $RepoRoot 'target\preview\hypercolor-daemon.exe'

$daemon = $null
$ui = $null

try {
    Write-Host '[dev] building daemon'
    & cargo @cargoBuildArguments
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }

    Add-HypercolorAngleRuntimePath

    Write-Host '[dev] starting daemon on 127.0.0.1:9420'
    $daemon = Start-HypercolorChild -FilePath $daemonExe -Arguments $daemonArguments -WorkingDirectory $RepoRoot

    Start-Sleep -Seconds 2

    Write-Host '[dev] starting UI on 127.0.0.1:9430'
    $ui = Start-HypercolorChild `
        -FilePath 'trunk.exe' `
        -Arguments @('serve', '--dist', '.dist-dev') `
        -WorkingDirectory (Join-Path $RepoRoot 'crates\hypercolor-ui')

    while ($true) {
        if ($daemon.HasExited) {
            exit $daemon.ExitCode
        }

        if ($ui.HasExited) {
            exit $ui.ExitCode
        }

        Start-Sleep -Seconds 1
    }
} finally {
    Stop-HypercolorChild $ui
    Stop-HypercolorChild $daemon
}
