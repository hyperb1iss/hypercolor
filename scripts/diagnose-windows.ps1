param(
    [string]$Api = "http://127.0.0.1:9420"
)

$ErrorActionPreference = "Continue"

$Utf8NoBom = New-Object System.Text.UTF8Encoding $false
[Console]::OutputEncoding = $Utf8NoBom
$OutputEncoding = $Utf8NoBom

$Esc = [char]27
$Purple = "$Esc[38;2;225;53;255m"
$Cyan = "$Esc[38;2;128;255;234m"
$Yellow = "$Esc[38;2;241;250;140m"
$Green = "$Esc[38;2;80;250;123m"
$Red = "$Esc[38;2;255;99;99m"
$Reset = "$Esc[0m"

function Write-Section {
    param([string]$Title)
    Write-Host ""
    Write-Host "$Purple-- $Title $Reset"
}

function Write-Check {
    param(
        [string]$Name,
        [bool]$Ok,
        [string]$Detail = ""
    )
    $color = if ($Ok) { $Green } else { $Yellow }
    $marker = if ($Ok) { "ok" } else { "warn" }
    Write-Host ("  {0}{1,-4}{2} {3}{4}{2}" -f $color, $marker, $Reset, $Cyan, $Name) -NoNewline
    if ($Detail) {
        Write-Host " $Detail"
    } else {
        Write-Host ""
    }
}

function Get-ApiItems {
    param([object]$Envelope)

    if ($null -eq $Envelope -or $null -eq $Envelope.data) {
        return @()
    }

    $itemsProperty = $Envelope.data.PSObject.Properties["items"]
    if ($itemsProperty) {
        return @($itemsProperty.Value)
    }

    return @($Envelope.data)
}

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

Write-Host "$Purple Hypercolor Windows Diagnostics $Reset"

Write-Section "Privileges"
Write-Check "Elevated shell" (Test-IsAdministrator) "current shell admin token"

Write-Section "Services"
foreach ($name in @("HypercolorSmBus", "Hypercolor", "PawnIO", "pawnio", "SignalRgb.Service")) {
    $svc = Get-Service -Name $name -ErrorAction SilentlyContinue
    if ($svc) {
        Write-Check $name ($svc.Status -eq "Running") "$($svc.Status), $($svc.StartType)"
        if ($name -eq "Hypercolor" -or $name -eq "HypercolorSmBus") {
            $svcReg = "HKLM:\SYSTEM\CurrentControlSet\Services\$name"
            $envValue = (Get-ItemProperty -Path $svcReg -Name Environment -ErrorAction SilentlyContinue).Environment
            foreach ($entry in @($envValue)) {
                Write-Host "       env $entry"
            }
        }
    } else {
        Write-Check $name $false "not installed"
    }
}

Write-Section "PawnIO Files"
$pawnIoDll = "C:\Program Files\PawnIO\PawnIOLib.dll"
Write-Check "PawnIOLib.dll" (Test-Path -LiteralPath $pawnIoDll) $pawnIoDll

$moduleDir = Join-Path $env:LOCALAPPDATA "hypercolor\pawnio\modules"
foreach ($module in @("SmbusI801.bin", "SmbusPIIX4.bin", "SmbusNCT6793.bin")) {
    $path = Join-Path $moduleDir $module
    Write-Check $module (Test-Path -LiteralPath $path) $path
}

Write-Section "Hypercolor Processes"
$processes = @()
$processes += @(Get-CimInstance Win32_Process -Filter "name='hypercolor-daemon.exe'" -ErrorAction SilentlyContinue)
$processes += @(Get-CimInstance Win32_Process -Filter "name='hypercolor-smbus-service.exe'" -ErrorAction SilentlyContinue)
if ($processes) {
    foreach ($process in $processes) {
        $owner = "unknown"
        try {
            $ownerInfo = Invoke-CimMethod -InputObject $process -MethodName GetOwner
            if ($ownerInfo.ReturnValue -eq 0) {
                $owner = "$($ownerInfo.Domain)\$($ownerInfo.User)"
            }
        } catch {}
        Write-Check $process.Name $true "pid=$($process.ProcessId) owner=$owner"
        Write-Host "       $($process.CommandLine)"
    }
} else {
    Write-Check "hypercolor-daemon.exe / hypercolor-smbus-service.exe" $false "not running"
}

Write-Section "API"
try {
    $status = Invoke-RestMethod -Uri "$Api/api/v1/status" -TimeoutSec 2
    Write-Check "Daemon status" $true "$Api/api/v1/status"
    if ($status.data.version) {
        Write-Host "       version=$($status.data.version)"
    }
} catch {
    Write-Check "Daemon status" $false $_.Exception.Message
}

try {
    $devices = Invoke-RestMethod -Uri "$Api/api/v1/devices" -TimeoutSec 2
    $items = Get-ApiItems $devices
    $count = @($items).Count
    Write-Check "Devices endpoint" $true "$count device(s)"
    foreach ($device in @($items)) {
        $backend = if ($device.origin.backend_id) { $device.origin.backend_id } else { "unknown" }
        Write-Host "       $($device.name) [$backend] $($device.id)"
    }
} catch {
    Write-Check "Devices endpoint" $false $_.Exception.Message
}

Write-Host ""
Write-Host "$Yellow Tip:$Reset Hypercolor should usually run as your user. Only the SMBus broker should be a service:"
Write-Host "      Start-Service HypercolorSmBus"
