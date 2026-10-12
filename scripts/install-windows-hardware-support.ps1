param(
    [string]$AssetRoot = "",
    [string]$BrokerExe = "",
    [string]$ModuleDestination = "",
    [switch]$ForcePawnIo,
    [switch]$Silent,
    [switch]$ReinstallService,
    [switch]$NoStartService
)

$ErrorActionPreference = "Stop"

if (-not $AssetRoot) {
    $AssetRoot = Join-Path $PSScriptRoot "pawnio"
}
if (-not $BrokerExe) {
    $BrokerExe = Join-Path $PSScriptRoot "hypercolor-smbus-service.exe"
}
if (-not $ModuleDestination) {
    $ModuleDestination = Join-Path $env:LOCALAPPDATA "hypercolor\pawnio\modules"
}

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Quote-ProcessArgument {
    param([string]$Value)

    if ($Value -eq "") {
        return '""'
    }
    if ($Value -notmatch '[\s"]') {
        return $Value
    }

    $escaped = $Value -replace '(\\*)"', '$1$1\"'
    $escaped = $escaped -replace '(\\+)$', '$1$1'
    return '"' + $escaped + '"'
}

if (-not (Test-IsAdministrator)) {
    $arguments = @(
        "-NoLogo",
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        $PSCommandPath,
        "-AssetRoot",
        $AssetRoot,
        "-BrokerExe",
        $BrokerExe,
        "-ModuleDestination",
        $ModuleDestination
    )
    if ($ForcePawnIo) {
        $arguments += "-ForcePawnIo"
    }
    if ($Silent) {
        $arguments += "-Silent"
    }
    if ($ReinstallService) {
        $arguments += "-ReinstallService"
    }
    if ($NoStartService) {
        $arguments += "-NoStartService"
    }

    $argumentLine = ($arguments | ForEach-Object { Quote-ProcessArgument $_ }) -join " "
    $process = Start-Process -FilePath "powershell.exe" -ArgumentList $argumentLine -Verb RunAs -Wait -PassThru
    exit $process.ExitCode
}

$pawnIoInstaller = Join-Path $PSScriptRoot "install-bundled-pawnio.ps1"
$serviceInstaller = Join-Path $PSScriptRoot "install-windows-smbus-service.ps1"

# Keep a transcript beside the broker's own log. The installer only shows
# this script's output in its details pane, which is gone once the wizard
# closes; the diagnostics bundle collects this directory. The previous run
# is kept once the log passes a megabyte.
$transcriptPath = Join-Path (Split-Path -Parent $BrokerExe) "logs\hardware-support.log"
$transcribing = $false
try {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $transcriptPath) | Out-Null
    if ((Test-Path -LiteralPath $transcriptPath) -and (Get-Item -LiteralPath $transcriptPath).Length -gt 1MB) {
        Move-Item -LiteralPath $transcriptPath -Destination "$transcriptPath.1" -Force
    }
    Start-Transcript -LiteralPath $transcriptPath -Append | Out-Null
    $transcribing = $true
} catch {
    Write-Host "Could not keep a setup log at ${transcriptPath}: $($_.Exception.Message)"
}

$pawnIoExit = 0
$failures = @()
try {
    Write-Host "Hypercolor hardware support setup, $(Get-Date -Format o), $([IntPtr]::Size * 8)-bit PowerShell"

    foreach ($path in @($pawnIoInstaller, $serviceInstaller, $BrokerExe)) {
        if (-not (Test-Path -LiteralPath $path)) {
            throw "Required hardware support file was not found: $path"
        }
    }

    # Hashtable splatting, not array splatting. Splatting an array into a
    # PowerShell script binds every element positionally, so "-AssetRoot"
    # lands in $AssetRoot as a literal value and switches never bind at all.
    # Only native executables parse "-Name value" pairs out of a splatted
    # array.
    $pawnIoArgs = @{
        AssetRoot         = $AssetRoot
        ModuleDestination = $ModuleDestination
    }
    if ($ForcePawnIo) {
        $pawnIoArgs["Force"] = $true
    }
    if ($Silent) {
        $pawnIoArgs["Silent"] = $true
    }

    # The broker step runs whatever happened to PawnIO. The broker starts
    # without PawnIO and finds it once it is installed, while a broker left
    # unregistered or stopped by a failed PawnIO step stays dark until
    # someone repairs it by hand.
    try {
        & $pawnIoInstaller @pawnIoArgs
        $pawnIoExit = $LASTEXITCODE
    } catch {
        $failures += "PawnIO setup failed: $($_.Exception.Message)"
        Write-Host $failures[-1]
    }

    $serviceArgs = @{
        BrokerExe   = $BrokerExe
        StartupType = "Automatic"
    }
    if ($ReinstallService) {
        $serviceArgs["Reinstall"] = $true
    }
    if (-not $NoStartService) {
        $serviceArgs["Start"] = $true
    }
    try {
        & $serviceInstaller @serviceArgs
    } catch {
        $failures += "SMBus broker setup failed: $($_.Exception.Message)"
        Write-Host $failures[-1]
    }
} catch {
    $failures += "Hypercolor Windows hardware support failed: $($_.Exception.Message)"
    Write-Host $failures[-1]
} finally {
    if ($failures.Count -eq 0) {
        Write-Host "Hypercolor Windows hardware support is ready"
    }
    if ($transcribing) {
        Stop-Transcript | Out-Null
    }
}

# Propagate PawnIO's reboot-required signal (3010) up to the parent Tauri
# command so the UI can surface a persistent "Restart Windows" banner
# instead of silently pretending the install is fully live. It wins over a
# failure because the restart is what finishes the driver; the installer
# checks the broker on its own either way.
if ($pawnIoExit -eq 3010) {
    Write-Host "Restart required: PawnIO driver activation needs a reboot to finish."
    exit 3010
}
if ($failures.Count -gt 0) {
    exit 1
}
