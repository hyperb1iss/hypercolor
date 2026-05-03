param(
    [string]$ServiceName = "Hypercolor",
    [string]$DisplayName = "Hypercolor RGB Daemon",
    [string]$Description = "Hypercolor RGB lighting daemon and Windows hardware access service.",
    [string]$DaemonExe = "",
    [string]$Bind = "127.0.0.1:9420",
    [string]$LogLevel = "info",
    [string]$Config = "",
    [string]$UiDir = "",
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

function Resolve-HypercolorDaemon {
    param([string]$ExplicitPath)

    if ($ExplicitPath) {
        $resolved = Resolve-Path -LiteralPath $ExplicitPath -ErrorAction Stop
        return $resolved.Path
    }

    $repoRoot = Split-Path -Parent $PSScriptRoot
    $candidates = @(
        (Join-Path $repoRoot "target\preview\hypercolor-daemon.exe"),
        (Join-Path $env:USERPROFILE ".cache\hypercolor\target\preview\hypercolor-daemon.exe"),
        (Join-Path $repoRoot "target\release\hypercolor-daemon.exe"),
        (Join-Path $env:USERPROFILE ".cache\hypercolor\target\release\hypercolor-daemon.exe")
    )

    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }

    throw "Could not find hypercolor-daemon.exe. Build it first with `just build-preview`, or pass -DaemonExe."
}

if (-not (Test-IsAdministrator)) {
    throw "Install must run from an elevated PowerShell session."
}

$daemonPath = Resolve-HypercolorDaemon $DaemonExe

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

sc.exe failure $ServiceName reset= 86400 actions= restart/5000/restart/15000/""/60000 | Out-Null

Write-Host "Installed $ServiceName"
Write-Host "  Binary: $daemonPath"
Write-Host "  Args:   $quotedArgs"
Write-Host "  Start:  Start-Service $ServiceName"

if ($Start) {
    Start-Service -Name $ServiceName
    Write-Host "Started $ServiceName"
}
