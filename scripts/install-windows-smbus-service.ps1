param(
    [string]$ServiceName = "HypercolorSmBus",
    [string]$DisplayName = "Hypercolor SMBus Broker",
    [string]$Description = "Tiny privileged Hypercolor broker for Windows SMBus access through PawnIO.",
    [string]$BrokerExe = "",
    [string]$PawnIoHome = "",
    [string]$PawnIoModuleDir = "",
    [ValidateSet("Automatic", "Manual", "Disabled")]
    [string]$StartupType = "Automatic",
    [switch]$Reinstall,
    [switch]$Start
)

$ErrorActionPreference = "Stop"

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Test-SmbusBrokerBinary {
    param([string]$Path)

    & $Path --help *> $null
    return $LASTEXITCODE -eq 0
}

function Resolve-SmbusBroker {
    param([string]$ExplicitPath)

    if ($ExplicitPath) {
        $resolved = Resolve-Path -LiteralPath $ExplicitPath -ErrorAction Stop
        if (-not (Test-SmbusBrokerBinary $resolved.Path)) {
            throw "BrokerExe '$($resolved.Path)' does not look like hypercolor-smbus-service.exe. Build the current broker first."
        }
        return $resolved.Path
    }

    $repoRoot = Split-Path -Parent $PSScriptRoot
    $candidates = @(
        (Join-Path $env:USERPROFILE ".cache\hypercolor\target\preview\hypercolor-smbus-service.exe"),
        (Join-Path $repoRoot "target\preview\hypercolor-smbus-service.exe"),
        (Join-Path $env:USERPROFILE ".cache\hypercolor\target\release\hypercolor-smbus-service.exe"),
        (Join-Path $repoRoot "target\release\hypercolor-smbus-service.exe")
    ) |
        Where-Object { Test-Path -LiteralPath $_ } |
        Get-Item |
        Sort-Object LastWriteTime -Descending

    foreach ($candidate in $candidates) {
        if (Test-SmbusBrokerBinary $candidate.FullName) {
            return $candidate.FullName
        }
    }

    if ($candidates) {
        $found = ($candidates | ForEach-Object { $_.FullName }) -join ", "
        throw "Found hypercolor-smbus-service.exe, but none passed --help: $found. Build the current broker with `just windows-smbus-service-build`, or pass -BrokerExe."
    }

    throw "Could not find hypercolor-smbus-service.exe. Build it first with `just windows-smbus-service-build`, or pass -BrokerExe."
}

function Resolve-PawnIoHome {
    param([string]$ExplicitPath)

    if ($ExplicitPath) {
        return (Resolve-Path -LiteralPath $ExplicitPath -ErrorAction Stop).Path
    }

    foreach ($candidate in @(
        (Join-Path $env:ProgramFiles "PawnIO"),
        (Join-Path ${env:ProgramFiles(x86)} "PawnIO")
    )) {
        if (Test-Path -LiteralPath (Join-Path $candidate "PawnIOLib.dll")) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }

    return ""
}

function Resolve-PawnIoModuleDir {
    param([string]$ExplicitPath)

    if ($ExplicitPath) {
        return (Resolve-Path -LiteralPath $ExplicitPath -ErrorAction Stop).Path
    }

    $candidates = @()
    if ($env:LOCALAPPDATA) {
        $candidates += (Join-Path $env:LOCALAPPDATA "hypercolor\pawnio\modules")
    }
    $pawnIoHomePath = Resolve-PawnIoHome $PawnIoHome
    if ($pawnIoHomePath) {
        $candidates += (Join-Path $pawnIoHomePath "modules")
        $candidates += $pawnIoHomePath
    }

    foreach ($candidate in $candidates) {
        foreach ($module in @("SmbusI801.bin", "SmbusPIIX4.bin", "SmbusNCT6793.bin")) {
            if (Test-Path -LiteralPath (Join-Path $candidate $module)) {
                return (Resolve-Path -LiteralPath $candidate).Path
            }
        }
    }

    return ""
}

if (-not (Test-IsAdministrator)) {
    throw "Install must run from an elevated PowerShell session."
}

$brokerPath = Resolve-SmbusBroker $BrokerExe
$resolvedPawnIoHome = Resolve-PawnIoHome $PawnIoHome
$resolvedPawnIoModuleDir = Resolve-PawnIoModuleDir $PawnIoModuleDir
$binaryPath = '"' + $brokerPath + '"'

$existing = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($existing) {
    if (-not $Reinstall) {
        throw "Service '$ServiceName' already exists. Pass -Reinstall to replace it."
    }

    if ($existing.Status -ne "Stopped") {
        Stop-Service -Name $ServiceName -Force -ErrorAction Stop
    }
    sc.exe delete $ServiceName | Out-Null
    Start-Sleep -Milliseconds 500
}

New-Service `
    -Name $ServiceName `
    -DisplayName $DisplayName `
    -BinaryPathName $binaryPath `
    -StartupType $StartupType | Out-Null

Set-ItemProperty `
    -Path "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName" `
    -Name Description `
    -Value $Description

$serviceEnvironment = @()
if ($resolvedPawnIoHome) {
    $serviceEnvironment += "HYPERCOLOR_PAWNIO_HOME=$resolvedPawnIoHome"
}
if ($resolvedPawnIoModuleDir) {
    $serviceEnvironment += "HYPERCOLOR_PAWNIO_MODULE_DIR=$resolvedPawnIoModuleDir"
}
if ($serviceEnvironment.Count -gt 0) {
    New-ItemProperty `
        -Path "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName" `
        -Name Environment `
        -PropertyType MultiString `
        -Value $serviceEnvironment `
        -Force | Out-Null
}

sc.exe failure $ServiceName reset= 86400 actions= restart/5000/restart/15000/""/60000 | Out-Null

Write-Host "Installed $ServiceName"
Write-Host "  Binary: $brokerPath"
Write-Host "  Account: LocalSystem (SMBus broker only)"
if ($resolvedPawnIoHome) {
    Write-Host "  PawnIO: $resolvedPawnIoHome"
}
if ($resolvedPawnIoModuleDir) {
    Write-Host "  Modules: $resolvedPawnIoModuleDir"
}
Write-Host "  Start:  Start-Service $ServiceName"

if ($Start) {
    Start-Service -Name $ServiceName
    Write-Host "Started $ServiceName"
}
