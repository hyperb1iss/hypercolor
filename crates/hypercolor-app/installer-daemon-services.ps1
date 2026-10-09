<#
.SYNOPSIS
    Stops Hypercolor daemons before an in-place upgrade copies files, and
    restores the daemon services it stopped afterwards.

.DESCRIPTION
    installer-hooks.nsh embeds this script in the installer and runs it from
    the installer's plugins directory. It lives in a file because NSIS caps
    every string, command lines included, at 1024 characters.

    -Action Stop lists each running service whose binary is this install's
    hypercolor-daemon.exe, asks it to stop, and waits up to twenty seconds,
    like the broker. A service already stopping is waited for but not
    listed, since nobody wanted it running. Daemon processes still running
    from the install directory after that, a service that would not stop
    included, get ten seconds to exit before they are ended.

    -Action Restore starts exactly the services -Action Stop listed and waits
    up to twenty seconds for each to report Running.
#>
param(
    [Parameter(Mandatory)] [ValidateSet("Stop", "Restore")] [string] $Action,
    [Parameter(Mandatory)] [string] $InstallDir,
    [Parameter(Mandatory)] [string] $StateFile
)

$daemonExe = Join-Path $InstallDir "hypercolor-daemon.exe"

if ($Action -eq "Restore") {
    if (Test-Path -LiteralPath $StateFile) {
        foreach ($name in @(Get-Content -LiteralPath $StateFile | Where-Object { $_ })) {
            try {
                $service = Get-Service -Name $name -ErrorAction Stop
                $service.Start()
                $service.WaitForStatus("Running", [TimeSpan]::FromSeconds(20))
            } catch {
                Write-Output ("{0} did not start: {1}" -f $name, $_.Exception.Message)
            }
        }
    }
    return
}

$stopped = @()
$services = @(Get-CimInstance Win32_Service | Where-Object {
    $_.PathName -and
    $_.PathName.ToLowerInvariant().Contains($daemonExe.ToLowerInvariant()) -and
    $_.State -ne "Stopped"
})
foreach ($svc in $services) {
    try {
        $service = Get-Service -Name $svc.Name
        if ($svc.State -ne "Stop Pending") {
            $stopped += $svc.Name
            $service.Stop()
        }
        $service.WaitForStatus("Stopped", [TimeSpan]::FromSeconds(20))
    } catch {
        Write-Output ("{0} did not stop: {1}" -f $svc.Name, $_.Exception.Message)
    }
}
Set-Content -LiteralPath $StateFile -Value $stopped

# The installer is 32-bit, so this runs in 32-bit PowerShell, where
# Get-Process cannot read a 64-bit process's path. WMI can, from either.
function Get-DaemonProcessIds {
    @(Get-CimInstance Win32_Process -Filter "Name = 'hypercolor-daemon.exe'" |
        Where-Object { $_.ExecutablePath -eq $daemonExe } |
        ForEach-Object { [int] $_.ProcessId })
}

$daemonIds = Get-DaemonProcessIds
if ($daemonIds.Count -gt 0) {
    Wait-Process -Id $daemonIds -Timeout 10 -ErrorAction SilentlyContinue
    foreach ($id in Get-DaemonProcessIds) {
        Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
    }
}
