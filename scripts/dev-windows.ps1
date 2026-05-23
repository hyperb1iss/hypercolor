param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $DaemonArgs
)

$ErrorActionPreference = 'Stop'

$DaemonArgs = @($DaemonArgs | Where-Object { $null -ne $_ -and $_ -ne '' })

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

$CacheRoot = if ($env:HYPERCOLOR_CACHE_DIR) {
    $env:HYPERCOLOR_CACHE_DIR
} else {
    Join-Path $env:USERPROFILE '.cache\hypercolor'
}

if (-not $env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR = Join-Path $CacheRoot 'target'
}
if (-not $env:MOZBUILD_STATE_PATH) {
    $env:MOZBUILD_STATE_PATH = Join-Path $CacheRoot 'mozbuild'
}

New-Item -ItemType Directory -Force -Path $env:CARGO_TARGET_DIR, $env:MOZBUILD_STATE_PATH | Out-Null

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
    $buildDir = Join-Path $env:CARGO_TARGET_DIR 'preview\build'
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

function Set-HypercolorPawnIoModuleDir {
    # Make the bundled PawnIO modules (SMBus, IntelMSR, AMDFamily17) visible
    # to the daemon without requiring the user to also run the installer.
    # `pawnio_module_dirs()` in hypercolor-windows-pawnio checks this env var
    # first, so a `just dev` loop gets motherboard RGB enumeration AND CPU
    # temperature reads identical to a real install.
    $stagedModuleDir = Join-Path $RepoRoot 'crates\hypercolor-app\resources\tools\pawnio\modules'
    $intelMsr = Join-Path $stagedModuleDir 'IntelMSR.bin'

    if (-not (Test-Path $intelMsr)) {
        Write-Host '[dev] staging PawnIO modules (one-time)'
        $fetchScript = Join-Path $RepoRoot 'scripts\fetch-pawnio-assets.ps1'
        & powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File $fetchScript
        if ($LASTEXITCODE -ne 0) {
            Write-Warning "[dev] PawnIO module fetch failed; CPU temps will be unavailable in this session"
            return
        }
    }

    if (Test-Path $stagedModuleDir) {
        $env:HYPERCOLOR_PAWNIO_MODULE_DIR = $stagedModuleDir
        Write-Host "[dev] HYPERCOLOR_PAWNIO_MODULE_DIR=$stagedModuleDir"
    }
}

function Test-HypercolorSmbusBroker {
    # CPU temperature reads and SMBus motherboard / DRAM device
    # discovery both route through HypercolorSmBus (which runs as
    # LocalSystem because PawnIO requires admin context). If the
    # service exists but is stopped, surface a one-line fix instead of
    # letting the user dig through daemon warnings for "access denied".
    $query = & sc.exe query HypercolorSmBus 2>&1
    if ($LASTEXITCODE -ne 0) {
        # Service not installed; nothing to warn about. The Settings
        # UI's "Install Support" panel handles first-time setup.
        return
    }

    $stateLine = $query | Where-Object { $_ -match '^\s*STATE\s*:' } | Select-Object -First 1
    if ($stateLine -match 'RUNNING') {
        return
    }

    Write-Host ""
    Write-Warning "HypercolorSmBus broker is installed but not running."
    Write-Warning "CPU temperature and motherboard RGB discovery need the broker."
    Write-Warning "Fix: open an elevated PowerShell and run:"
    Write-Warning "    sc.exe start HypercolorSmBus"
    Write-Warning "(Or reboot - broker startup type is Automatic.)"
    Write-Host ""
}

function Build-HypercolorWindowsHelper {
    # Build the signed elevated helper that the Tauri app shells out to
    # for privileged operations (repair SMBus, install/uninstall hardware
    # support, swap broker binary). The app's helper_client resolves the
    # binary via HYPERCOLOR_HELPER_PATH first, so we set that to the
    # freshly built debug binary and `just dev` gets a single-prompt UAC
    # repair flow identical to a real bundle.
    Write-Host '[dev] building hypercolor-windows-helper'
    $cargoCacheBuild = Join-Path $RepoRoot 'scripts\cargo-cache-build.ps1'
    & powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File $cargoCacheBuild `
        cargo build --bin hypercolor-windows-helper
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "[dev] hypercolor-windows-helper build failed; Repair button will be disabled"
        return
    }

    $helperExe = Join-Path $env:CARGO_TARGET_DIR 'debug\hypercolor-windows-helper.exe'
    if (Test-Path $helperExe) {
        $env:HYPERCOLOR_HELPER_PATH = $helperExe
        Write-Host "[dev] HYPERCOLOR_HELPER_PATH=$helperExe"
    } else {
        Write-Warning "[dev] hypercolor-windows-helper.exe not found at $helperExe"
    }
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

if ([string]::IsNullOrWhiteSpace($env:HYPERCOLOR_SERVO_GPU_IMPORT_MODE)) {
    $servoGpuImportMode = 'auto'
    Write-Host "[dev] Servo GPU import mode: $servoGpuImportMode (default)"
} else {
    $servoGpuImportMode = $env:HYPERCOLOR_SERVO_GPU_IMPORT_MODE
    Write-Host "[dev] Servo GPU import mode: $servoGpuImportMode"
}

$daemonArguments = @(
    '--log-level',
    'debug',
    '--compositor-acceleration-mode',
    $compositorAccelerationMode,
    '--servo-gpu-import-mode',
    $servoGpuImportMode,
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
    'servo wgpu servo-gpu-import'
)

$daemonExe = Join-Path $env:CARGO_TARGET_DIR 'preview\hypercolor-daemon.exe'
$cargoCacheBuild = Join-Path $RepoRoot 'scripts\cargo-cache-build.ps1'

$daemon = $null
$ui = $null

try {
    Write-Host '[dev] building daemon'
    Write-Host "[dev] CARGO_TARGET_DIR=$env:CARGO_TARGET_DIR"
    & powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File $cargoCacheBuild cargo @cargoBuildArguments
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }

    Add-HypercolorAngleRuntimePath
    Set-HypercolorPawnIoModuleDir
    Build-HypercolorWindowsHelper
    Test-HypercolorSmbusBroker

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
