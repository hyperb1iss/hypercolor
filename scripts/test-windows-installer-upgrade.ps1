<#
.SYNOPSIS
    Prove that a Hypercolor installer lays down a working installation, from
    nothing or over what is already there, and takes all of it away again.

.DESCRIPTION
    Every run ends by uninstalling and checking nothing stayed behind, so
    runs follow each other on one machine.

    -Mode Fresh checks -CandidateInstaller refuses a folder standard users
    can change, then installs it on a clean machine and again over itself.

    -Mode Interactive and -Mode Silent install -PreviousInstaller, give it the
    state a real user would have (start with Windows on, the desktop shortcut
    removed, files from the previous release that the candidate no longer
    ships), then upgrade it. Interactive runs the candidate the way a person
    does and clicks through the wizard, which is the path where an upgrade
    used to uninstall first. Silent runs it with a bare /S, the way automation
    does.

    -Mode Killed upgrades the same way, but kills the installer and the
    program it runs while the unit is being published, while the transaction
    is open, and after it committed. After each kill the Start menu shortcut
    must still start Hypercolor, and one more run of the installer must finish
    the job. A round in which a kill misses its moment proves nothing, so it
    is thrown away and run again.

    -Mode Replace installs -PreviousInstaller, which already lays down units,
    and replaces it with the candidate.

    After an upgrade the release must run from a unit, the launcher must carry
    every name, the flat layout must be gone, and autostart and shortcuts must
    be on the launcher, with the shortcut the user removed still removed.
    Units and records must belong to Administrators, and a standard user must
    be able to start the launcher. -Managed says the candidate registers the
    update service; without it the service and its slots must not exist.

    Needs an elevated shell (the installers are per-machine) in a desktop
    session, on a machine where replacing the Hypercolor install is fine,
    such as a CI runner.
#>
param(
    [string] $PreviousInstaller = "",
    [Parameter(Mandatory)] [string] $CandidateInstaller,
    [ValidateSet("Fresh", "Interactive", "Silent", "Killed", "Replace")]
    [string] $Mode = "Silent",
    [switch] $Managed,
    [int] $TimeoutSeconds = 900
)

$ErrorActionPreference = "Stop"

$UninstallKey = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Hypercolor"
$RunKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$MachineDirectory = Join-Path $env:ProgramData "Hypercolor"
$StateRoot = Join-Path $MachineDirectory "update"
$Administrators = [Security.Principal.SecurityIdentifier] "S-1-5-32-544"
# 3010 is the Windows "installed, restart required" code the installer
# returns when PawnIO needs a reboot.
$SuccessCodes = @(0, 3010)
$StartDeadlineSeconds = 60
$UninstallDeadlineSeconds = 180
$LauncherDeadlineMilliseconds = 30000
$KillRounds = 3

Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class InstallerWizard {
    delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lParam);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] static extern bool IsWindowEnabled(IntPtr hwnd);
    [DllImport("user32.dll")] static extern IntPtr GetDlgItem(IntPtr hwnd, int id);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder name, int size);
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr hwnd, uint msg, IntPtr wParam, IntPtr lParam);

    const uint WM_COMMAND = 0x0111;
    const int IDOK = 1;
    const int IDCANCEL = 2;
    const int IDNO = 7;

    static List<IntPtr> Dialogs(uint pid) {
        var found = new List<IntPtr>();
        EnumWindows((hwnd, _) => {
            uint owner;
            GetWindowThreadProcessId(hwnd, out owner);
            var name = new StringBuilder(64);
            GetClassName(hwnd, name, name.Capacity);
            if (owner == pid && IsWindowVisible(hwnd) && name.ToString() == "#32770") {
                found.Add(hwnd);
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    // Answer every message box with No (the PawnIO restart question, which
    // older installers ask even under /S), then Cancel, which also dismisses
    // an OK-only notice such as the SMBus broker warning and never means
    // Yes. When a wizard is given, press its Next or Finish button once it
    // is enabled. Posting WM_COMMAND reaches the installer's windows
    // directly, so nothing depends on keyboard focus.
    public static void Step(uint pid, IntPtr wizard) {
        foreach (var dialog in Dialogs(pid)) {
            if (dialog != wizard) {
                PostMessage(dialog, WM_COMMAND, (IntPtr)IDNO, IntPtr.Zero);
                PostMessage(dialog, WM_COMMAND, (IntPtr)IDCANCEL, IntPtr.Zero);
            }
        }
        if (wizard == IntPtr.Zero) {
            return;
        }
        var next = GetDlgItem(wizard, IDOK);
        if (next != IntPtr.Zero && IsWindowEnabled(next)) {
            PostMessage(wizard, WM_COMMAND, (IntPtr)IDOK, IntPtr.Zero);
        }
    }
}
"@

function Wait-Installer {
    param([System.Diagnostics.Process] $Process, [string] $Description, [switch] $ClickThrough)

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $windowDeadline = (Get-Date).AddSeconds(60)
    $sawWizard = $false
    while (-not $Process.HasExited) {
        if ((Get-Date) -gt $deadline) {
            Stop-Process -Id $Process.Id -Force
            throw "$Description did not finish within $TimeoutSeconds seconds"
        }
        $wizard = [IntPtr]::Zero
        if ($ClickThrough) {
            $Process.Refresh()
            $wizard = $Process.MainWindowHandle
            $sawWizard = $sawWizard -or $wizard -ne [IntPtr]::Zero
            if (-not $sawWizard -and (Get-Date) -gt $windowDeadline) {
                Stop-Process -Id $Process.Id -Force
                throw "$Description showed no window within 60 seconds; -Mode Interactive needs a desktop session"
            }
        }
        if (-not $ClickThrough -or $wizard -ne [IntPtr]::Zero) {
            [InstallerWizard]::Step([uint32]$Process.Id, $wizard)
        }
        Start-Sleep -Milliseconds 1500
    }
    $Process.WaitForExit()
    if ($SuccessCodes -notcontains $Process.ExitCode) {
        throw "$Description exited with $($Process.ExitCode)"
    }
}

function Stop-InstalledHypercolor {
    param([string] $InstallDir)

    Get-Process -Name hypercolor-app, hypercolor-daemon -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path.StartsWith($InstallDir, [StringComparison]::OrdinalIgnoreCase) } |
        Stop-Process -Force -ErrorAction SilentlyContinue
}

function Get-InstalledVersion {
    (Get-ItemProperty -Path $UninstallKey -ErrorAction Stop).DisplayVersion
}

function Get-InstallDirectory {
    (Get-ItemProperty -Path $UninstallKey).InstallLocation.Trim('"')
}

function Install-Silently {
    param([string] $Installer, [string] $Description)

    Write-Host "Installing $Description with: $Installer /S"
    Wait-Installer -Process (Start-Process -FilePath $Installer -ArgumentList "/S" -PassThru) -Description $Description
}

function Read-Record {
    param([string] $Name)

    Get-Content -Raw -LiteralPath (Join-Path $StateRoot $Name) | ConvertFrom-Json
}

function Get-ActiveUnit {
    (Read-Record "active.json").unit
}

function Test-FlatLayout {
    param([string] $InstallDir)

    Test-Path -LiteralPath (Join-Path $InstallDir "hypercolor-app.exe")
}

function Find-Failures {
    param([string] $InstallDir)

    $failures = @()
    $launcher = Join-Path $InstallDir "Hypercolor.exe"
    if (-not (Test-Path -LiteralPath $launcher)) {
        $failures += "the launcher is not at $launcher"
    }
    # NTFS ignores case, so a listing is the only place the spelling shows,
    # and nothing depends on it.
    $spelled = (Get-ChildItem -LiteralPath $InstallDir -Force | Where-Object { $_.Name -ieq "Hypercolor.exe" }).Name
    if ($spelled -cne "Hypercolor.exe") {
        Write-Host "::warning::the launcher is spelled '$spelled'"
    } else {
        Write-Host "the launcher is spelled $spelled"
    }
    foreach ($flat in "hypercolor-app.exe", "hypercolor-daemon.exe", "libEGL.dll", "vcruntime140.dll", "ui", "effects", "tools") {
        if (Test-Path -LiteralPath (Join-Path $InstallDir $flat)) {
            $failures += "$flat survived from the flat layout"
        }
    }

    $unit = Get-ActiveUnit
    $unitDirectory = Join-Path $InstallDir "units\$unit"
    foreach ($member in "manifest.json", "hypercolor-app.exe", "hypercolor-daemon.exe", "hypercolor.exe", "tools\hypercolor-smbus-service.exe", "tools\hypercolor-update-bootstrap.exe", "tools\pawnio\modules\SmbusI801.bin") {
        if (-not (Test-Path -LiteralPath (Join-Path $unitDirectory $member))) {
            $failures += "unit $unit has no $member"
        }
    }
    $units = @(Get-ChildItem -LiteralPath (Join-Path $InstallDir "units") -Force | ForEach-Object { $_.Name })
    if ($units.Count -ne 1 -or $units[0] -ne $unit) {
        $failures += "units holds $($units -join ', '), not just the active unit"
    }

    foreach ($path in @(
        $InstallDir,
        (Join-Path $InstallDir "units"),
        $unitDirectory,
        (Join-Path $unitDirectory "hypercolor-app.exe"),
        $MachineDirectory,
        $StateRoot,
        (Join-Path $StateRoot "active.json"),
        (Join-Path $StateRoot "install-journal.json"),
        (Join-Path $StateRoot "installation.json")
    )) {
        $owner = (Get-Acl -LiteralPath $path).GetOwner([Security.Principal.SecurityIdentifier])
        if ($owner -ne $Administrators) {
            $failures += "$path is owned by $owner, not Administrators"
        }
    }

    $journal = Read-Record "install-journal.json"
    if ($journal.disposition -ne "committed") {
        $failures += "the journal ended $($journal.disposition)"
    }
    $installation = Read-Record "installation.json"
    if ($installation.runtime_mode -ne "app_supervised") {
        $failures += "the installation records $($installation.runtime_mode)"
    }
    $me = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    if ($installation.user_sid -ne $me) {
        $failures += "the installation records $($installation.user_sid), not the installing user $me"
    }

    $broker = Get-CimInstance Win32_Service -Filter "Name = 'HypercolorSmBus'"
    if ($broker -and -not $broker.PathName.Contains("units\$unit\")) {
        $failures += "the broker runs $($broker.PathName), outside the active unit"
    }
    if ((Get-Service -Name mpssvc -ErrorAction SilentlyContinue).Status -eq "Running") {
        foreach ($rule in @(
            @{ Name = "Hypercolor Daemon"; Program = "hypercolor-daemon.exe" },
            @{ Name = "Hypercolor App"; Program = "hypercolor-app.exe" }
        )) {
            $program = (Get-NetFirewallRule -DisplayName $rule.Name -ErrorAction SilentlyContinue |
                Get-NetFirewallApplicationFilter).Program
            if ($program -ne (Join-Path $unitDirectory $rule.Program)) {
                $failures += "firewall rule $($rule.Name) allows '$program'"
            }
        }
    }

    $service = Get-CimInstance Win32_Service -Filter "Name = 'HypercolorUpdate'"
    if ($Managed) {
        $failures += Find-UpdateServiceFailures -InstallDir $InstallDir -Service $service -User $me
    } else {
        if ($service) {
            $failures += "an open source build left HypercolorUpdate registered"
        }
        if (Test-Path -LiteralPath (Join-Path $InstallDir "update")) {
            $failures += "an open source build left the bootstrap slots behind"
        }
    }
    $failures
}

function Find-UpdateServiceFailures {
    param([string] $InstallDir, $Service, [string] $User)

    if (-not $Service) {
        return @("the update service is not registered")
    }
    $failures = @()
    if ($Service.PathName -notmatch 'update\\slot-[ab]\.exe') {
        $failures += "the update service runs $($Service.PathName)"
    }
    if ($Service.StartMode -ne "Auto" -or -not $Service.DelayedAutoStart) {
        $failures += "the update service starts $($Service.StartMode), delayed $($Service.DelayedAutoStart)"
    }
    $granted = 0
    foreach ($ace in (Get-Acl -LiteralPath (Join-Path $StateRoot "coordinator")).Access) {
        if ($ace.AccessControlType -eq "Allow" -and
            $ace.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value -eq $User) {
            $granted = $granted -bor [int] $ace.FileSystemRights
        }
    }
    $modify = [int] [Security.AccessControl.FileSystemRights]::Modify
    if (($granted -band $modify) -ne $modify) {
        $failures += "the installing user may not write the coordinator's records"
    }
    foreach ($slot in "slot-a.exe", "slot-b.exe") {
        if (-not (Test-Path -LiteralPath (Join-Path $InstallDir "update\$slot"))) {
            $failures += "the update service has no $slot"
        }
    }
    $actions = (& sc.exe qfailure HypercolorUpdate | Out-String)
    foreach ($expected in "RESTART", "RUN PROCESS", "--recover-fallback") {
        if (-not $actions.Contains($expected)) {
            $failures += "the update service's failure actions lack ${expected}: $actions"
        }
    }
    if (-not (& sc.exe qfailureflag HypercolorUpdate | Out-String).Contains("TRUE")) {
        $failures += "a non-zero exit of the update service takes no failure action"
    }
    $descriptor = (& sc.exe sdshow HypercolorUpdate | Out-String)
    # The descriptor spells the machine's built-in administrator as LA.
    if ($descriptor -notmatch "\(A;;[A-Z]*RP[A-Z]*;;;($User|LA)\)") {
        $failures += "the installing user may not start the update service: $descriptor"
    }
    $failures
}

function Wait-ForProgram {
    param([string] $Name, [string] $Directory)

    $deadline = (Get-Date).AddSeconds($StartDeadlineSeconds)
    while ((Get-Date) -lt $deadline) {
        $running = Get-Process -Name $Name -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and $_.Path.StartsWith($Directory, [StringComparison]::OrdinalIgnoreCase) }
        if ($running) {
            return $true
        }
        Start-Sleep -Milliseconds 250
    }
    $false
}

# The launcher reads the state root and the units through ACLs that only an
# account outside Administrators puts to the test.
function Find-StandardUserFailures {
    param([string] $InstallDir)

    $name = "hc-proof"
    $password = "Hc$(([Guid]::NewGuid()).ToString('N').Substring(0, 8))aA1!"
    & net.exe user $name $password /add | Out-Null
    if ($LASTEXITCODE -ne 0) {
        return @("could not create a standard user")
    }
    try {
        & net.exe localgroup Users $name /add 2>$null | Out-Null
        $secret = ConvertTo-SecureString $password -AsPlainText -Force
        $credential = [pscredential]::new($name, $secret)
        # -Wait would also wait for the app the launcher leaves running. The
        # process's own wait needs its handle read first, or its exit code
        # is lost with it.
        $launch = Start-Process -FilePath (Join-Path $InstallDir "Hypercolor.exe") -ArgumentList "--minimized" `
            -WorkingDirectory $env:SystemRoot -Credential $credential -LoadUserProfile -PassThru
        $null = $launch.Handle
        $finished = $launch.WaitForExit($LauncherDeadlineMilliseconds)
        Stop-InstalledHypercolor -InstallDir $InstallDir
        if (-not $finished) {
            Stop-Process -Id $launch.Id -Force -ErrorAction SilentlyContinue
            return @("the launcher did not finish for a standard user")
        }
        if ($launch.ExitCode -ne 0) {
            return @("the launcher refused a standard user with exit code $($launch.ExitCode)")
        }
        @()
    } finally {
        & net.exe user $name /delete | Out-Null
    }
}

function Find-LaunchFailures {
    param([string] $InstallDir)

    $unitDirectory = Join-Path $InstallDir "units\$(Get-ActiveUnit)"
    Start-Process -FilePath (Join-Path $InstallDir "Hypercolor.exe") -ArgumentList "--minimized" | Out-Null
    $failures = @()
    if (-not (Wait-ForProgram -Name hypercolor-app -Directory $unitDirectory)) {
        $failures += "the launcher did not start the active unit's app"
    }
    if (-not (Wait-ForProgram -Name hypercolor-daemon -Directory $unitDirectory)) {
        $failures += "the app did not start the active unit's daemon"
    }
    Stop-InstalledHypercolor -InstallDir $InstallDir
    $failures
}

function Find-Leftovers {
    $failures = @()
    if (Test-Path -LiteralPath $UninstallKey) {
        $failures += "the uninstall entry stayed"
    }
    foreach ($service in "HypercolorSmBus", "HypercolorUpdate") {
        if (Get-Service -Name $service -ErrorAction SilentlyContinue) {
            $failures += "$service stayed registered"
        }
    }
    if ((Get-Service -Name mpssvc -ErrorAction SilentlyContinue).Status -eq "Running") {
        foreach ($rule in "Hypercolor Daemon", "Hypercolor App") {
            if (Get-NetFirewallRule -DisplayName $rule -ErrorAction SilentlyContinue) {
                $failures += "firewall rule $rule stayed"
            }
        }
    }
    if (Test-Path -LiteralPath $MachineDirectory) {
        $failures += "$MachineDirectory stayed"
    }
    if ((Get-ItemProperty -Path $RunKey -Name "Hypercolor" -ErrorAction SilentlyContinue).Hypercolor) {
        $failures += "the autostart value stayed"
    }
    $failures
}

function Assert-Clean {
    param([string] $Description)

    $failures = @(Find-Leftovers)
    if ($failures) {
        foreach ($failure in $failures) {
            Write-Host "::error::$failure"
        }
        throw "$Description left the machine dirty"
    }
}

function Invoke-Uninstall {
    param([string] $InstallDir)

    $uninstaller = Join-Path $InstallDir "uninstall.exe"
    Write-Host "Uninstalling with: $uninstaller /S"
    # NSIS copies the uninstaller away and starts the copy, so the process
    # this starts says nothing, and the registry goes after the files. Only
    # everything having gone says it finished.
    Start-Process -FilePath $uninstaller -ArgumentList "/S" | Out-Null
    $deadline = (Get-Date).AddSeconds($UninstallDeadlineSeconds)
    while ((Test-Path -LiteralPath $InstallDir) -or @(Find-Leftovers).Count -gt 0) {
        if ((Get-Date) -gt $deadline) {
            if (Test-Path -LiteralPath $InstallDir) {
                $left = @(Get-ChildItem -LiteralPath $InstallDir -Force | ForEach-Object { $_.Name }) -join ", "
                throw "uninstalling left $InstallDir holding: $left"
            }
            Assert-Clean -Description "uninstalling"
        }
        Start-Sleep -Milliseconds 500
    }
}

function Show-BrokerState {
    # The broker is not part of the install contract above, because a runner
    # may not take the PawnIO driver, but a broker that is not running after
    # an install is exactly what users hit, so put it in the run summary with
    # the setup and broker logs.
    $broker = Get-Service -Name HypercolorSmBus -ErrorAction SilentlyContinue
    if (-not $broker -or $broker.Status -ne "Running") {
        $brokerState = if ($broker) { "$($broker.Status)" } else { "not registered" }
        Write-Host "::warning::the HypercolorSmBus broker is $brokerState after the install ($Mode)"
        foreach ($log in @("hardware-support.log", "smbus-broker.log")) {
            $logPath = Join-Path $env:ProgramData "Hypercolor\logs\$log"
            if (Test-Path -LiteralPath $logPath) {
                Write-Host "--- $logPath"
                Get-Content -LiteralPath $logPath -Tail 40 | ForEach-Object { Write-Host $_ }
            }
        }
    }
}

function Get-JournalDisposition {
    try {
        (Read-Record "install-journal.json").disposition
    } catch {
        "none"
    }
}

# The pointer names something other than the release this run started from.
function Test-PointerMoved {
    try {
        (Get-ActiveUnit) -ne $script:previousUnit
    } catch {
        $false
    }
}

# Start the candidate and kill it and the program it runs the moment
# $Reached says so. Returns whether it was still running by then.
function Invoke-KilledInstall {
    param([string] $Moment, [scriptblock] $Reached)

    Write-Host "Killing the installer $Moment"
    $installer = Start-Process -FilePath $CandidateInstaller -ArgumentList "/S" -PassThru
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while (-not $installer.HasExited -and -not (& $Reached)) {
        if ((Get-Date) -gt $deadline) {
            Stop-Process -Id $installer.Id -Force
            throw "the installer never reached the moment $Moment"
        }
    }
    if ($installer.HasExited) {
        Write-Host "the installer finished before it could be killed $Moment"
        return $false
    }
    Stop-Process -Id $installer.Id -Force -ErrorAction SilentlyContinue
    $installer.WaitForExit()
    # The records are read next, so nothing may still be writing them.
    $running = @(Get-Process -Name hypercolor -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path.EndsWith("\unit\hypercolor.exe", [StringComparison]::OrdinalIgnoreCase) })
    $running | Stop-Process -Force -ErrorAction SilentlyContinue
    foreach ($process in $running) {
        if (-not $process.WaitForExit($LauncherDeadlineMilliseconds)) {
            throw "the program the installer ran outlived its kill $Moment"
        }
    }
    Write-Host "killed $Moment; journal $(Get-JournalDisposition), pointer moved $(Test-PointerMoved)"
    $true
}

# A folder a standard user could change never holds what the update service
# runs, so the installer has to stop before it lays anything down there.
function Assert-RefusesUnsafeFolder {
    $folder = Join-Path $env:SystemDrive "hypercolor-unsafe-folder"
    New-Item -ItemType Directory -Force -Path $folder | Out-Null
    try {
        & icacls.exe $folder /grant "*S-1-5-32-545:(OI)(CI)M" | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "could not open $folder to Users"
        }
        Write-Host "Installing into a folder Users can change: $folder"
        $refused = Start-Process -FilePath $CandidateInstaller -ArgumentList "/S", "/D=$folder" -PassThru
        $refused.WaitForExit()
        if ($SuccessCodes -contains $refused.ExitCode) {
            throw "the installer accepted $folder, which Users can change"
        }
        if (Test-Path -LiteralPath (Join-Path $folder "units")) {
            throw "the installer refused $folder only after writing to it"
        }
        if (Test-Path -LiteralPath $UninstallKey) {
            throw "a refused install left an uninstall entry"
        }
    } finally {
        Remove-Item -LiteralPath $folder -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# What the person at this machine clicks has to work after any kill.
function Assert-ShortcutStarts {
    param([string] $InstallDir, [string] $Moment)

    $target = (New-Object -ComObject WScript.Shell).CreateShortcut($startMenuLink).TargetPath
    Start-Process -FilePath $target -ArgumentList "--minimized" | Out-Null
    $started = Wait-ForProgram -Name hypercolor-app -Directory $InstallDir
    Stop-InstalledHypercolor -InstallDir $InstallDir
    if (-not $started) {
        throw "killed $Moment, the Start menu shortcut ran $target and nothing started"
    }
}

# One run through every moment. Returns false when a kill missed its moment,
# which leaves the run open for the caller to finish.
function Invoke-KillRound {
    param([string] $InstallDir)

    $moments = @(
        @{ Name = "while publishing the unit"; Open = $false; Reached = {
            @(Get-ChildItem -Path (Join-Path $InstallDir "units") -Filter ".hypercolor-stage-*" -Force -ErrorAction SilentlyContinue).Count -gt 0 } },
        @{ Name = "inside the transaction"; Open = $true; Reached = {
            (Get-JournalDisposition) -eq "forward" } },
        @{ Name = "after the transaction committed"; Open = $false; Reached = {
            (Get-JournalDisposition) -eq "committed" -and (Test-PointerMoved) } }
    )
    foreach ($moment in $moments) {
        if (-not (Invoke-KilledInstall -Moment $moment.Name -Reached $moment.Reached)) {
            return $false
        }
        if ($moment.Open -and (Get-JournalDisposition) -notin "forward", "rollback") {
            Write-Host "the kill landed after the transaction ended"
            return $false
        }
        Assert-ShortcutStarts -InstallDir $InstallDir -Moment $moment.Name
    }
    $true
}

# Lay down the previous release and give it a user's state.
function Install-Previous {
    Install-Silently -Installer $PreviousInstaller -Description "the previous release"
    $script:from = Get-InstalledVersion
    $script:installDir = Get-InstallDirectory
    $script:wasFlat = Test-FlatLayout -InstallDir $script:installDir
    $previousApp = Join-Path $script:installDir $(if ($script:wasFlat) { "hypercolor-app.exe" } else { "Hypercolor.exe" })
    # A running Hypercolor would make the wizard ask to close it.
    Stop-InstalledHypercolor -InstallDir $script:installDir
    if (-not $script:wasFlat) {
        $script:previousUnit = Get-ActiveUnit
        $script:previousInstallation = (Read-Record "installation.json").installation_id
    }

    Set-ItemProperty -Path $RunKey -Name "Hypercolor" -Value "`"$previousApp`" --minimized"
    Remove-Item -Path $desktopLink -ErrorAction SilentlyContinue
    if ($script:wasFlat) {
        $script:staleFiles = @(
            (Join-Path $script:installDir "ui\previous-release-asset.js"),
            (Join-Path $script:installDir "effects\bundled\removed-effect.html")
        )
        foreach ($stale in $script:staleFiles) {
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $stale) | Out-Null
            Set-Content -Path $stale -Value "left behind by the previous release"
        }
    }
}

if ($Mode -ne "Fresh" -and -not $PreviousInstaller) {
    throw "-Mode $Mode needs -PreviousInstaller"
}
Assert-Clean -Description "the machine before this run"

$from = $null
$installDir = $null
$previousUnit = $null
$previousInstallation = $null
$desktopLink = Join-Path ([Environment]::GetFolderPath("CommonDesktopDirectory")) "Hypercolor.lnk"
$startMenuLink = Join-Path ([Environment]::GetFolderPath("CommonPrograms")) "Hypercolor.lnk"
$staleFiles = @()
$wasFlat = $false

if ($Mode -ne "Fresh") {
    Install-Previous
}

switch ($Mode) {
    "Interactive" {
        Write-Host "Upgrading Hypercolor $from by clicking through: $CandidateInstaller"
        $candidate = Start-Process -FilePath $CandidateInstaller -PassThru
        Wait-Installer -Process $candidate -Description "the candidate installer" -ClickThrough
        # The finish page starts Hypercolor; leave the machine as we found it.
        Start-Sleep -Seconds 3
        Stop-InstalledHypercolor -InstallDir (Get-InstallDirectory)
    }
    "Killed" {
        for ($round = 1; -not (Invoke-KillRound -InstallDir $installDir); $round += 1) {
            if ($round -ge $KillRounds) {
                throw "$KillRounds rounds could not land every kill"
            }
            Write-Host "Starting over: a kill missed its moment"
            Install-Silently -Installer $CandidateInstaller -Description "the candidate, to end the round"
            Invoke-Uninstall -InstallDir $installDir
            Install-Previous
        }
        Install-Silently -Installer $CandidateInstaller -Description "the candidate, after being killed"
    }
    "Fresh" {
        Assert-RefusesUnsafeFolder
        Install-Silently -Installer $CandidateInstaller -Description "the candidate"
        $failures = @(Find-Failures -InstallDir (Get-InstallDirectory))
        if ($failures) {
            foreach ($failure in $failures) {
                Write-Host "::error::$failure"
            }
            throw "the first install failed"
        }
        Install-Silently -Installer $CandidateInstaller -Description "the candidate, over itself"
    }
    default {
        Install-Silently -Installer $CandidateInstaller -Description "the candidate"
    }
}

$to = Get-InstalledVersion
$installDir = Get-InstallDirectory

$failures = @(Find-Failures -InstallDir $installDir)
if ($Mode -in "Interactive", "Silent", "Killed" -and $to -eq $from) {
    $failures += "the installed version stayed at $from"
}
if ($Mode -ne "Fresh") {
    $launcherAutostart = "`"$(Join-Path $installDir 'Hypercolor.exe')`" --minimized"
    $runValue = (Get-ItemProperty -Path $RunKey -Name "Hypercolor" -ErrorAction SilentlyContinue).Hypercolor
    if ($runValue -ne $launcherAutostart) {
        $failures += "the autostart Run value is not on the launcher (now '$runValue')"
    }
    if ($Mode -ne "Replace" -and (Test-Path $desktopLink)) {
        $failures += "the upgrade recreated the desktop shortcut the user had removed"
    }
    foreach ($stale in $staleFiles) {
        if (Test-Path $stale) {
            $failures += "a previous release's file survived the upgrade: $stale"
        }
    }
}
if ($Mode -eq "Replace") {
    if ((Get-ActiveUnit) -eq $previousUnit) {
        $failures += "the candidate did not replace unit $previousUnit"
    }
    if ((Read-Record "installation.json").installation_id -ne $previousInstallation) {
        $failures += "replacing the release changed the installation's identity"
    }
}
if (Test-Path $startMenuLink) {
    $target = (New-Object -ComObject WScript.Shell).CreateShortcut($startMenuLink).TargetPath
    if ($target -ne (Join-Path $installDir "Hypercolor.exe")) {
        $failures += "the Start menu shortcut names $target"
    }
}
if (-not $failures) {
    $failures = @(Find-LaunchFailures -InstallDir $installDir) + @(Find-StandardUserFailures -InstallDir $installDir)
}
Show-BrokerState

if ($failures) {
    foreach ($failure in $failures) {
        Write-Host "::error::$failure"
    }
    throw "Hypercolor $from -> $to failed ($Mode)"
}
Write-Host "Hypercolor $from -> $to holds ($Mode): unit active, names on the launcher, nothing left of the old layout"

Invoke-Uninstall -InstallDir $installDir
Write-Host "Uninstalled cleanly ($Mode)"
