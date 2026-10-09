<#
.SYNOPSIS
    Prove that a Hypercolor installer upgrades an older install in place.

.DESCRIPTION
    Installs the previous release silently, gives it the state a real user
    would have (start with Windows on, the desktop shortcut removed, and files
    from the previous release that the candidate no longer ships), then runs
    the candidate installer and checks that the upgrade:

      - replaced the installed version,
      - kept the autostart Run value,
      - did not recreate the desktop shortcut the user removed,
      - cleared the previous release's UI and bundled effect files.

    -Mode Interactive runs the candidate the way a person does and clicks
    through the wizard, which is the path where an upgrade used to uninstall
    first. -Mode Silent runs it with a bare /S, the way automation does.

    Needs an elevated shell (the installers are per-machine) in a desktop
    session, on a machine where replacing the Hypercolor install is fine,
    such as a CI runner.
#>
param(
    [Parameter(Mandatory)] [string] $PreviousInstaller,
    [Parameter(Mandatory)] [string] $CandidateInstaller,
    [ValidateSet("Interactive", "Silent")]
    [string] $Mode = "Silent",
    [int] $TimeoutSeconds = 900
)

$ErrorActionPreference = "Stop"

$UninstallKey = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Hypercolor"
$RunKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
# 3010 is the Windows "installed, restart required" code the installer
# returns when PawnIO needs a reboot.
$SuccessCodes = @(0, 3010)

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

    // Answer every message box with No (the PawnIO restart question is the
    // only one an upgrade asks, and older installers ask it even under /S),
    // and when a wizard is given, press its Next or Finish button once it is
    // enabled. Posting WM_COMMAND reaches the installer's windows directly,
    // so nothing depends on keyboard focus.
    public static void Step(uint pid, IntPtr wizard) {
        foreach (var dialog in Dialogs(pid)) {
            if (dialog != wizard) {
                PostMessage(dialog, WM_COMMAND, (IntPtr)IDNO, IntPtr.Zero);
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

Write-Host "Installing the previous release: $PreviousInstaller /S"
Wait-Installer -Process (Start-Process -FilePath $PreviousInstaller -ArgumentList "/S" -PassThru) -Description "the previous installer"
$from = Get-InstalledVersion
$installDir = (Get-ItemProperty -Path $UninstallKey).InstallLocation.Trim('"')
$appExe = Join-Path $installDir "hypercolor-app.exe"
# A running Hypercolor would make the wizard ask to close it.
Stop-InstalledHypercolor -InstallDir $installDir

$autostart = "`"$appExe`" --minimized"
Set-ItemProperty -Path $RunKey -Name "Hypercolor" -Value $autostart
$desktopLink = Join-Path ([Environment]::GetFolderPath("CommonDesktopDirectory")) "Hypercolor.lnk"
Remove-Item -Path $desktopLink -ErrorAction SilentlyContinue
$staleFiles = @(
    (Join-Path $installDir "ui\previous-release-asset.js"),
    (Join-Path $installDir "effects\bundled\removed-effect.html")
)
foreach ($stale in $staleFiles) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $stale) | Out-Null
    Set-Content -Path $stale -Value "left behind by the previous release"
}

if ($Mode -eq "Interactive") {
    Write-Host "Upgrading Hypercolor $from by clicking through: $CandidateInstaller"
    $candidate = Start-Process -FilePath $CandidateInstaller -PassThru
    Wait-Installer -Process $candidate -Description "the candidate installer" -ClickThrough
    # The finish page starts Hypercolor; leave the machine as we found it.
    Start-Sleep -Seconds 3
    Stop-InstalledHypercolor -InstallDir $installDir
} else {
    Write-Host "Upgrading Hypercolor $from with: $CandidateInstaller /S"
    Wait-Installer -Process (Start-Process -FilePath $CandidateInstaller -ArgumentList "/S" -PassThru) -Description "the candidate installer"
}
$to = Get-InstalledVersion

$failures = @()
if ($to -eq $from) {
    $failures += "the installed version stayed at $from"
}
$runValue = (Get-ItemProperty -Path $RunKey -Name "Hypercolor" -ErrorAction SilentlyContinue).Hypercolor
if ($runValue -ne $autostart) {
    $failures += "the autostart Run value did not survive the upgrade (now '$runValue')"
}
if (Test-Path $desktopLink) {
    $failures += "the upgrade recreated the desktop shortcut the user had removed"
}
foreach ($stale in $staleFiles) {
    if (Test-Path $stale) {
        $failures += "a previous release's file survived the upgrade: $stale"
    }
}
if (-not (Test-Path $appExe)) {
    $failures += "hypercolor-app.exe is missing after the upgrade"
}
if (-not (Get-ChildItem -Path (Join-Path $installDir "ui") -File -ErrorAction SilentlyContinue)) {
    $failures += "the UI folder is empty after the upgrade"
}

if ($failures) {
    foreach ($failure in $failures) {
        Write-Host "::error::$failure"
    }
    throw "Hypercolor $from -> $to did not upgrade in place ($Mode)"
}
Write-Host "Hypercolor $from -> $to upgraded in place ($Mode): autostart kept, shortcuts untouched, stale files cleared"
