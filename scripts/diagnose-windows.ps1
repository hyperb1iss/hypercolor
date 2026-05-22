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
$diagnose = $null
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

Write-Section "Daemon Diagnostics"
try {
    $body = @{ system = $true } | ConvertTo-Json -Compress
    $diagnose = Invoke-RestMethod -Method Post -Uri "$Api/api/v1/diagnose" -Body $body -ContentType "application/json" -TimeoutSec 3
    foreach ($check in @($diagnose.data.checks)) {
        $ok = $check.status -eq "pass"
        Write-Check "$($check.category).$($check.name)" $ok "$($check.status): $($check.detail)"
    }

    $latest = $diagnose.data.snapshot.render.latest_frame
    if ($latest) {
        Write-Host "       frame=$($latest.frame_token) source=$($latest.output_frame_source) stale=$($latest.gpu_sample_stale) sample_us=$($latest.sample_us) push_us=$($latest.push_us) devices=$($latest.devices_written) leds=$($latest.total_leds)"
    }

    $window = $diagnose.data.snapshot.render.recent_window
    if ($window) {
        Write-Host "       recent frames=$($window.frames) current=$($window.output_current_frame) published=$($window.output_published_frame) routed_reuse=$($window.output_routed_reuse) stale=$($window.gpu_sample_stale)"
    }

    $usb = $diagnose.data.snapshot.usb
    if ($usb) {
        Write-Host "       usb display lane frames=$($usb.display_frames_total) delayed_for_led=$($usb.display_frames_delayed_for_led_total) wait_max_ms=$($usb.display_led_priority_wait_max_ms)"
    }
} catch {
    Write-Check "Daemon diagnostics" $false $_.Exception.Message
}

Write-Section "Device Output Queues"
if ($diagnose -and $diagnose.data.snapshot.device_output) {
    $output = $diagnose.data.snapshot.device_output
    Write-Check "Queue snapshot" ($output.lagging_queues -eq 0 -and $output.errors_total -eq 0) "queues=$($output.queues) usb=$($output.usb_queues) lagging=$($output.lagging_queues) dropped_total=$($output.dropped_frames_total) errors_total=$($output.errors_total)"
    foreach ($queue in @($output.items)) {
        $ok = -not $queue.worker_finished -and $queue.errors_total -eq 0
        $fps = "{0:n1}/{1:n1}" -f [double]$queue.fps_sent, [double]$queue.fps_queued
        Write-Check "$($queue.backend_id):$($queue.id)" $ok "fps sent/queued=$fps target=$($queue.fps_target) dropped=$($queue.frames_dropped) queue_wait_ms=$($queue.avg_queue_wait_ms) write_ms=$($queue.avg_write_ms) last_sent_ms=$($queue.last_sent_ago_ms)"
        if ($queue.last_error) {
            Write-Host "       last_error=$($queue.last_error)"
        }
    }
} else {
    Write-Check "Queue snapshot" $false "diagnose snapshot unavailable"
}

Write-Host ""
Write-Host "$Yellow Tip:$Reset Hypercolor should usually run as your user. Only the SMBus broker should be a service:"
Write-Host "      Start-Service HypercolorSmBus"
