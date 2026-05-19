param(
    [string]$ServiceName = "Hypercolor",
    [string]$DisplayName = "Hypercolor RGB Daemon",
    [string]$Description = "Hypercolor RGB lighting daemon and Windows hardware access service.",
    [string]$DaemonExe = "",
    [string]$Bind = "127.0.0.1:9420",
    [string]$LogLevel = "info",
    [string]$Config = "",
    [string]$UiDir = "",
    [string]$PawnIoHome = "",
    [string]$PawnIoModuleDir = "",
    [ValidateSet("Automatic", "Manual", "Disabled")]
    [string]$StartupType = "Automatic",
    [switch]$Reinstall,
    [switch]$Start,
    [switch]$AllowSystemDaemon
)

$ErrorActionPreference = "Stop"

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Test-HypercolorDaemonSupportsService {
    param([string]$Path)

    & $Path --windows-service --help *> $null
    return $LASTEXITCODE -eq 0
}

function Resolve-HypercolorDaemon {
    param([string]$ExplicitPath)

    if ($ExplicitPath) {
        $resolved = Resolve-Path -LiteralPath $ExplicitPath -ErrorAction Stop
        if (-not (Test-HypercolorDaemonSupportsService $resolved.Path)) {
            throw "DaemonExe '$($resolved.Path)' does not support --windows-service. Build the current daemon first."
        }
        return $resolved.Path
    }

    $repoRoot = Split-Path -Parent $PSScriptRoot
    $candidates = @(
        (Join-Path $env:USERPROFILE ".cache\hypercolor\target\preview\hypercolor-daemon.exe"),
        (Join-Path $repoRoot "target\preview\hypercolor-daemon.exe"),
        (Join-Path $env:USERPROFILE ".cache\hypercolor\target\release\hypercolor-daemon.exe"),
        (Join-Path $repoRoot "target\release\hypercolor-daemon.exe")
    ) |
        Where-Object { Test-Path -LiteralPath $_ } |
        Get-Item |
        Sort-Object LastWriteTime -Descending

    foreach ($candidate in $candidates) {
        if (Test-HypercolorDaemonSupportsService $candidate.FullName) {
            return $candidate.FullName
        }
    }

    if ($candidates) {
        $found = ($candidates | ForEach-Object { $_.FullName }) -join ", "
        throw "Found hypercolor-daemon.exe, but none support --windows-service: $found. Build the current daemon with `just build-preview -p hypercolor-daemon --bin hypercolor-daemon`, or pass -DaemonExe."
    }

    throw "Could not find hypercolor-daemon.exe. Build it first with `just build-preview -p hypercolor-daemon --bin hypercolor-daemon`, or pass -DaemonExe."
}

function Resolve-PawnIoHome {
    param([string]$ExplicitPath)

    if ($ExplicitPath) {
        $resolved = (Resolve-Path -LiteralPath $ExplicitPath -ErrorAction Stop).Path
        Assert-ServicePawnIoPath $resolved
        return $resolved
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

function Assert-ServicePawnIoPath {
    param([string]$Path)

    $resolved = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).Path
    $userWritableRoots = @(
        $env:LOCALAPPDATA,
        $env:APPDATA,
        $env:USERPROFILE,
        (Join-Path $env:SystemDrive 'Users')
    ) |
        Where-Object { $_ } |
        ForEach-Object { (Resolve-Path -LiteralPath $_ -ErrorAction SilentlyContinue).Path } |
        Where-Object { $_ }

    foreach ($root in $userWritableRoots) {
        if ($resolved.Equals($root, [System.StringComparison]::OrdinalIgnoreCase) -or
            $resolved.StartsWith($root + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "PawnIO service path '$resolved' is under a per-user profile directory ('$root'). Windows services run elevated, so PawnIO install and module directories must be administrator-owned (for example under %ProgramFiles%)."
        }
    }
}

function Resolve-PawnIoModuleDir {
    param([string]$ExplicitPath)

    if ($ExplicitPath) {
        $resolved = (Resolve-Path -LiteralPath $ExplicitPath -ErrorAction Stop).Path
        Assert-ServicePawnIoPath $resolved
        return $resolved
    }

    $candidates = @()
    $pawnIoHomePath = Resolve-PawnIoHome $PawnIoHome
    if ($pawnIoHomePath) {
        $candidates += (Join-Path $pawnIoHomePath "modules")
        $candidates += $pawnIoHomePath
    }

    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath (Join-Path $candidate "SmbusI801.bin")) {
            $resolved = (Resolve-Path -LiteralPath $candidate).Path
            Assert-ServicePawnIoPath $resolved
            return $resolved
        }
    }

    return ""
}

function Get-BindPort {
    param([string]$BindAddress)

    if ($BindAddress -match ':(\d+)$') {
        return [int]$Matches[1]
    }

    throw "Cannot parse port from bind address '$BindAddress'."
}

function Assert-BindPortAvailable {
    param([string]$BindAddress)

    $port = Get-BindPort $BindAddress
    $listeners = @(Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue)
    if ($listeners.Count -eq 0) {
        return
    }

    $details = $listeners | ForEach-Object {
        $process = Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue
        $processName = if ($process) { $process.ProcessName } else { "unknown" }
        "pid=$($_.OwningProcess) process=$processName local=$($_.LocalAddress):$($_.LocalPort)"
    }
    throw "Cannot start $ServiceName because port $port is already in use: $($details -join '; '). Stop the foreground daemon first, then run Start-Service $ServiceName."
}

if (-not (Test-IsAdministrator)) {
    throw "Install must run from an elevated PowerShell session."
}

if (-not $AllowSystemDaemon) {
    throw "This service mode runs the full Hypercolor daemon as LocalSystem. It is intended only as a temporary Windows SMBus test path. Pass -AllowSystemDaemon to opt in, or keep using the foreground daemon while we split SMBus into a narrow hardware broker."
}

$daemonPath = Resolve-HypercolorDaemon $DaemonExe
$resolvedPawnIoHome = Resolve-PawnIoHome $PawnIoHome
$resolvedPawnIoModuleDir = Resolve-PawnIoModuleDir $PawnIoModuleDir

$arguments = @("--windows-service", "--bind", $Bind, "--log-level", $LogLevel)
if ($Config) {
    $arguments += @("--config", (Resolve-Path -LiteralPath $Config).Path)
}
if ($UiDir) {
    $arguments += @("--ui-dir", (Resolve-Path -LiteralPath $UiDir).Path)
}

$quotedExe = '"' + $daemonPath + '"'
$quotedArgs = ($arguments | ForEach-Object {
    if ($_ -match '\s') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ }
}) -join " "
$binaryPath = "$quotedExe $quotedArgs"

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
Write-Host "  Binary: $daemonPath"
Write-Host "  Args:   $quotedArgs"
if ($resolvedPawnIoHome) {
    Write-Host "  PawnIO: $resolvedPawnIoHome"
}
if ($resolvedPawnIoModuleDir) {
    Write-Host "  Modules: $resolvedPawnIoModuleDir"
}
Write-Host "  Start:  Start-Service $ServiceName"

if ($Start) {
    Assert-BindPortAvailable $Bind
    Start-Service -Name $ServiceName
    Write-Host "Started $ServiceName"
}
