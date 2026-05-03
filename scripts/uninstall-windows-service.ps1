param(
    [string]$ServiceName = "Hypercolor"
)

$ErrorActionPreference = "Stop"

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not (Test-IsAdministrator)) {
    throw "Uninstall must run from an elevated PowerShell session."
}

$service = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($null -eq $service) {
    Write-Host "Service '$ServiceName' is not installed."
    exit 0
}

if ($service.Status -ne "Stopped") {
    Stop-Service -Name $ServiceName -Force -ErrorAction Stop
}

sc.exe delete $ServiceName | Out-Null
Write-Host "Removed $ServiceName"
