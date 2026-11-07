<#
.SYNOPSIS
    Stops every Hypercolor daemon running from an install directory before an
    install switches releases, and removes the daemon services registered
    there once it has.

.DESCRIPTION
    installer-hooks.nsh embeds this script in the installer and runs it from
    the installer's plugins directory. It lives in a file because NSIS caps
    every string, command lines included, at 1024 characters.

    -Action Stop asks each service whose binary is a hypercolor-daemon.exe
    under the install directory to stop, and gives it up to twenty seconds,
    like the broker. Daemon processes still running from the directory after
    that, a service that would not stop included, get ten seconds to exit
    before they are ended.

    -Action Remove deletes those services, because the program each one names
    does not outlive the install and the app supervises the daemon from then
    on. The installer only asks once the new release is in: an install that
    fails leaves the registration, and the service back at the next boot.
#>
param(
    [Parameter(Mandatory)] [ValidateSet("Stop", "Remove")] [string] $Action,
    [Parameter(Mandatory)] [string] $InstallDir
)

$root = "$($InstallDir.TrimEnd('\'))\".ToLowerInvariant()

$services = @(Get-CimInstance Win32_Service | Where-Object {
    $path = "$($_.PathName)".ToLowerInvariant()
    $path.Contains($root) -and $path.Contains('hypercolor-daemon.exe')
})
if ($Action -eq "Remove") {
    foreach ($svc in $services) {
        Write-Output ("Removing the daemon service {0}" -f $svc.Name)
        & sc.exe delete $svc.Name | Out-Null
    }
    return
}

foreach ($svc in $services) {
    try {
        $service = Get-Service -Name $svc.Name
        if ($svc.State -ne "Stopped") {
            if ($svc.State -ne "Stop Pending") {
                $service.Stop()
            }
            $service.WaitForStatus("Stopped", [TimeSpan]::FromSeconds(20))
        }
    } catch {
        Write-Output ("{0} did not stop: {1}" -f $svc.Name, $_.Exception.Message)
    }
}

# The installer is 32-bit, so this runs in 32-bit PowerShell, where
# Get-Process cannot read a 64-bit process's path. WMI can, from either.
function Get-DaemonProcessIds {
    @(Get-CimInstance Win32_Process -Filter "Name = 'hypercolor-daemon.exe'" |
        Where-Object { "$($_.ExecutablePath)".ToLowerInvariant().StartsWith($root) } |
        ForEach-Object { [int] $_.ProcessId })
}

$daemonIds = Get-DaemonProcessIds
if ($daemonIds.Count -gt 0) {
    Wait-Process -Id $daemonIds -Timeout 10 -ErrorAction SilentlyContinue
    foreach ($id in Get-DaemonProcessIds) {
        Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
    }
}
