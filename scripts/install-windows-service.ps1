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
    [string]$InstallDir = "",
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

function Resolve-HypercolorInstallDir {
    param([string]$ExplicitPath)

    if ($ExplicitPath) {
        return $ExplicitPath
    }

    return (Join-Path $env:ProgramFiles "Hypercolor")
}

function Test-IsPathUnder {
    param(
        [string]$Path,
        [string]$Root
    )

    if (-not $Root) {
        return $false
    }

    $fullPath = [System.IO.Path]::GetFullPath($Path).TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    $fullRoot = [System.IO.Path]::GetFullPath($Root).TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    return $fullPath.Equals($fullRoot, [System.StringComparison]::OrdinalIgnoreCase) -or $fullPath.StartsWith($fullRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)
}

function Assert-HypercolorInstallDirIsProtectedRoot {
    param([string]$Path)

    $programRoots = @($env:ProgramFiles, ${env:ProgramFiles(x86)}) |
        Where-Object { $_ }

    foreach ($root in $programRoots) {
        if (Test-IsPathUnder $Path $root) {
            return
        }
    }

    throw "InstallDir '$Path' must be under ProgramFiles so the LocalSystem service executable is not installed below a user-writable parent directory."
}

function Invoke-CheckedProcess {
    param(
        [string]$FilePath,
        [string[]]$Arguments
    )

    & $FilePath @Arguments | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath failed with exit code $LASTEXITCODE while applying secure service executable ACLs."
    }
}

function Protect-HypercolorInstallDir {
    param([string]$Path)

    Invoke-CheckedProcess "icacls.exe" @(
        $Path,
        "/inheritance:r",
        "/grant:r",
        "*S-1-5-18:(OI)(CI)(F)",
        "*S-1-5-32-544:(OI)(CI)(F)",
        "/remove:g",
        "*S-1-1-0",
        "*S-1-5-11",
        "*S-1-5-32-545"
    )
}

function Protect-HypercolorServiceExecutable {
    param([string]$Path)

    Invoke-CheckedProcess "icacls.exe" @(
        $Path,
        "/inheritance:r",
        "/grant:r",
        "*S-1-5-18:(F)",
        "*S-1-5-32-544:(F)",
        "/remove:g",
        "*S-1-1-0",
        "*S-1-5-11",
        "*S-1-5-32-545"
    )
}

function Install-HypercolorServiceExecutable {
    param(
        [string]$SourcePath,
        [string]$DestinationDir
    )

    if (-not $DestinationDir) {
        throw "InstallDir must not be empty."
    }

    New-Item -ItemType Directory -Force -Path $DestinationDir | Out-Null
    $resolvedDestinationDir = (Resolve-Path -LiteralPath $DestinationDir -ErrorAction Stop).Path
    Assert-HypercolorInstallDirIsProtectedRoot $resolvedDestinationDir
    Protect-HypercolorInstallDir $resolvedDestinationDir

    $destinationPath = Join-Path $resolvedDestinationDir "hypercolor-daemon.exe"
    $resolvedSourcePath = (Resolve-Path -LiteralPath $SourcePath -ErrorAction Stop).Path
    $sourceFullPath = [System.IO.Path]::GetFullPath($resolvedSourcePath)
    $destinationFullPath = [System.IO.Path]::GetFullPath($destinationPath)

    if (-not [System.String]::Equals($sourceFullPath, $destinationFullPath, [System.StringComparison]::OrdinalIgnoreCase)) {
        Copy-Item -LiteralPath $resolvedSourcePath -Destination $destinationPath -Force
    }

    Protect-HypercolorServiceExecutable $destinationPath
    if (-not (Test-HypercolorDaemonSupportsService $destinationPath)) {
        throw "Installed daemon '$destinationPath' does not support --windows-service."
    }

    return (Resolve-Path -LiteralPath $destinationPath -ErrorAction Stop).Path
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

$daemonSourcePath = Resolve-HypercolorDaemon $DaemonExe
$serviceInstallDir = Resolve-HypercolorInstallDir $InstallDir
$resolvedPawnIoHome = Resolve-PawnIoHome $PawnIoHome
$resolvedPawnIoModuleDir = Resolve-PawnIoModuleDir $PawnIoModuleDir

$arguments = @("--windows-service", "--bind", $Bind, "--log-level", $LogLevel)
if ($Config) {
    $arguments += @("--config", (Resolve-Path -LiteralPath $Config).Path)
}
if ($UiDir) {
    $arguments += @("--ui-dir", (Resolve-Path -LiteralPath $UiDir).Path)
}

$quotedArgs = ($arguments | ForEach-Object {
    if ($_ -match '\s') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ }
}) -join " "

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

$daemonPath = Install-HypercolorServiceExecutable $daemonSourcePath $serviceInstallDir
$quotedExe = '"' + $daemonPath + '"'
$binaryPath = "$quotedExe $quotedArgs"

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
Write-Host "  Source: $daemonSourcePath"
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
