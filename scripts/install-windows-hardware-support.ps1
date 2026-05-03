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

foreach ($path in @($pawnIoInstaller, $serviceInstaller, $BrokerExe)) {
    if (-not (Test-Path -LiteralPath $path)) {
        throw "Required hardware support file was not found: $path"
    }
}

$pawnIoArgs = @(
    "-AssetRoot",
    $AssetRoot,
    "-ModuleDestination",
    $ModuleDestination
)
if ($ForcePawnIo) {
    $pawnIoArgs += "-Force"
}
if ($Silent) {
    $pawnIoArgs += "-Silent"
}
& $pawnIoInstaller @pawnIoArgs

$serviceArgs = @(
    "-BrokerExe",
    $BrokerExe,
    "-StartupType",
    "Automatic"
)
if ($ReinstallService) {
    $serviceArgs += "-Reinstall"
}
if (-not $NoStartService) {
    $serviceArgs += "-Start"
}
& $serviceInstaller @serviceArgs

Write-Host "Hypercolor Windows hardware support is ready"
