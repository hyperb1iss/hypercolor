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

    # Filename match instead of runtime exec: the release broker binary
    # is GUI-subsystem (no console output) and has no clap CLI of its
    # own — running it with --help either silently no-ops or returns
    # non-zero from the service-dispatcher path, both of which falsely
    # fail this check. Trust the caller-supplied path as long as it
    # looks like the expected binary name.
    if (-not (Test-Path -LiteralPath $Path)) {
        return $false
    }
    $name = Split-Path -Leaf $Path
    return $name -match '^hypercolor-smbus-service(\.exe)?$'
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

function Get-PawnIoRegisteredHome {
    # PawnIO records its install directory in the 64-bit uninstall key. A
    # 32-bit PowerShell reading HKLM:\SOFTWARE sees the WOW6432Node copy
    # instead, so open the 64-bit registry view explicitly.
    try {
        $hklm = [Microsoft.Win32.RegistryKey]::OpenBaseKey(
            [Microsoft.Win32.RegistryHive]::LocalMachine,
            [Microsoft.Win32.RegistryView]::Registry64)
        $key = $hklm.OpenSubKey("SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO")
        if ($key) {
            return ([string]$key.GetValue("InstallLocation")).Trim('"')
        }
    } catch {
        return ""
    }
    return ""
}

function Resolve-PawnIoHome {
    param([string]$ExplicitPath)

    if ($ExplicitPath) {
        $resolved = (Resolve-Path -LiteralPath $ExplicitPath -ErrorAction Stop).Path
        Assert-ServicePawnIoPath $resolved
        return $resolved
    }

    # In a 32-bit PowerShell ProgramFiles names "Program Files (x86)",
    # where a 64-bit PawnIO never lives. ProgramW6432 names the 64-bit
    # directory from either bitness.
    $candidates = @(
        (Get-PawnIoRegisteredHome),
        $(if ($env:ProgramW6432) { Join-Path $env:ProgramW6432 "PawnIO" }),
        $(if ($env:ProgramFiles) { Join-Path $env:ProgramFiles "PawnIO" }),
        $(if (${env:ProgramFiles(x86)}) { Join-Path ${env:ProgramFiles(x86)} "PawnIO" })
    ) | Where-Object { $_ }

    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath (Join-Path $candidate "PawnIOLib.dll")) {
            $resolved = (Resolve-Path -LiteralPath $candidate).Path
            # The broker loads PawnIOLib.dll from this directory as
            # LocalSystem, so a user-writable candidate is skipped, not used.
            try {
                Assert-ServicePawnIoPath $resolved
                return $resolved
            } catch {
                Write-Host "Ignoring PawnIO at ${resolved}: $($_.Exception.Message)"
            }
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

function Assert-ServiceBrokerPath {
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
            throw "Broker executable path '$resolved' is under a per-user profile directory ('$root'). Windows services run elevated, so BrokerExe must be administrator-owned (for example under %ProgramFiles%)."
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
        foreach ($module in @(
            "SmbusI801.bin",
            "SmbusPIIX4.bin",
            "SmbusNCT6793.bin",
            "IntelMSR.bin",
            "AMDFamily17.bin"
        )) {
            if (Test-Path -LiteralPath (Join-Path $candidate $module)) {
                $resolved = (Resolve-Path -LiteralPath $candidate).Path
                Assert-ServicePawnIoPath $resolved
                return $resolved
            }
        }
    }

    return ""
}

function Get-RegisteredBrokerPath {
    param([string]$Name)

    $service = Get-CimInstance Win32_Service -Filter "Name='$Name'" -ErrorAction SilentlyContinue
    if (-not $service -or -not $service.PathName) {
        return ""
    }
    return $service.PathName.Trim().Trim('"')
}

function Get-BrokerLogPath {
    param([string]$BrokerPath)

    # The broker writes its own log beside its executable; see
    # crates/hypercolor-windows-pawnio/src/windows/service_log.rs.
    Join-Path (Split-Path -Parent $BrokerPath) "logs\smbus-broker.log"
}

function Get-ServiceExitSummary {
    param([string]$Name)

    $service = Get-CimInstance Win32_Service -Filter "Name='$Name'" -ErrorAction SilentlyContinue
    if (-not $service) {
        return "the service is not registered"
    }
    return "state $($service.State), exit code $($service.ExitCode), service-specific exit code $($service.ServiceSpecificExitCode)"
}

function Start-SmbusBroker {
    param(
        [string]$Name,
        [string]$BrokerPath
    )

    # Start-Service waits without a bound while a service reports
    # START_PENDING, which would hang the installer, so start through the
    # controller and bound the wait. The broker reports Running before it
    # touches PawnIO or its pipe, so check again a moment later that it
    # stayed up.
    $service = Get-Service -Name $Name -ErrorAction Stop
    $failure = ""
    try {
        $service.Start()
        $service.WaitForStatus("Running", [TimeSpan]::FromSeconds(20))
        Start-Sleep -Seconds 2
        $service.Refresh()
        if ($service.Status -ne "Running") {
            $failure = "it reported Running and then stopped"
        }
    } catch {
        $failure = $_.Exception.GetBaseException().Message
    }

    if (-not $failure) {
        Write-Host "Started $Name"
        return
    }

    Write-Host "Could not start ${Name}: $failure"
    Write-Host "  Service: $(Get-ServiceExitSummary $Name)"
    $logPath = Get-BrokerLogPath $BrokerPath
    if (Test-Path -LiteralPath $logPath) {
        Write-Host "  Last lines of ${logPath}:"
        Get-Content -LiteralPath $logPath -Tail 20 | ForEach-Object { Write-Host "    $_" }
    } else {
        Write-Host "  The broker wrote no log at $logPath, so its process never reached its own startup."
    }
    throw "$Name did not start: $failure"
}

if (-not (Test-IsAdministrator)) {
    throw "Install must run from an elevated PowerShell session."
}

$brokerPath = Resolve-SmbusBroker $BrokerExe
Assert-ServiceBrokerPath $brokerPath
$resolvedPawnIoHome = Resolve-PawnIoHome $PawnIoHome
$resolvedPawnIoModuleDir = Resolve-PawnIoModuleDir $PawnIoModuleDir
$binaryPath = '"' + $brokerPath + '"'

$existing = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($existing) {
    # Re-registering the same broker binary is how a stopped or broken
    # registration gets repaired (the app's Install support button runs
    # this without -Reinstall), so only a registration that points at a
    # different binary needs the explicit switch.
    $registeredPath = Get-RegisteredBrokerPath $ServiceName
    $sameBroker = $registeredPath -and $registeredPath.Equals($brokerPath, [System.StringComparison]::OrdinalIgnoreCase)
    if (-not $Reinstall -and -not $sameBroker) {
        throw "Service '$ServiceName' already exists for '$registeredPath'. Pass -Reinstall to replace it."
    }

    # Stop-Service also waits without a bound, so stop through the
    # controller and give it twenty seconds, like the installer hooks do.
    if ($existing.Status -ne "Stopped") {
        if ($existing.Status -ne "StopPending") {
            $existing.Stop()
        }
        $existing.WaitForStatus("Stopped", [TimeSpan]::FromSeconds(20))
    }
    sc.exe delete $ServiceName | Out-Null

    # DeleteService only marks the service for deletion while anything
    # holds a handle to it (an open Services console, a stuck stop), and
    # New-Service then fails with "marked for deletion". Wait for it to go.
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
    }
    if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
        throw "Service '$ServiceName' is still marked for deletion. Close the Services console or anything else holding it open, or restart Windows, then try again."
    }
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
# Apply the restart actions when the broker stops itself with an error
# exit code too, not only when its process crashes.
sc.exe failureflag $ServiceName 1 | Out-Null

Write-Host "Installed $ServiceName"
Write-Host "  Binary: $brokerPath"
Write-Host "  Account: LocalSystem (SMBus broker only)"
if ($resolvedPawnIoHome) {
    Write-Host "  PawnIO: $resolvedPawnIoHome"
} else {
    Write-Host "  PawnIO: not found; the broker searches the PawnIO install location itself"
}
if ($resolvedPawnIoModuleDir) {
    Write-Host "  Modules: $resolvedPawnIoModuleDir"
}
Write-Host "  Log:    $(Get-BrokerLogPath $brokerPath)"
Write-Host "  Start:  Start-Service $ServiceName"

if ($Start) {
    Start-SmbusBroker -Name $ServiceName -BrokerPath $brokerPath
}
