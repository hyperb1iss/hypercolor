# Copy the bundled PawnIO modules into PawnIO's install dir so the
# HypercolorSmBus broker (running as LocalSystem) can find them, then
# restart the broker to pick them up.
#
# Self-elevates on first run. Idempotent — safe to re-run any time the
# bundled modules in resources/tools/pawnio/modules/ change.
#
# Symptom this fixes:
#   "Hypercolor SMBus broker read_msr failed: PawnIO SMBus module
#    IntelMSR.bin was not found"
#   "Windows PawnIO loaded, but no supported SMBus modules exposed a bus"
# Both of those mean the broker's PawnIO module search paths don't
# contain the module blobs. Easiest fix is to put them where PawnIO
# expects them by default: <PawnIO install>\modules\.

param(
    [switch] $RestartBroker
)

$ErrorActionPreference = 'Stop'

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

$RepoRoot = Split-Path -Parent $PSScriptRoot
$Source = Join-Path $RepoRoot 'crates\hypercolor-app\resources\tools\pawnio\modules'

if (-not (Test-Path $Source)) {
    throw "PawnIO module staging dir not found: $Source. Run scripts\fetch-pawnio-assets.ps1 first."
}

$Modules = Get-ChildItem -Path $Source -Filter '*.bin' -File
if ($Modules.Count -eq 0) {
    throw "No .bin modules in $Source. Re-run scripts\fetch-pawnio-assets.ps1."
}

# Locate PawnIO install dir. PawnIO_setup.exe registers itself in
# Program Files by default; honor an override env var if set.
$Destination = $null
if ($env:HYPERCOLOR_PAWNIO_HOME) {
    $Destination = Join-Path $env:HYPERCOLOR_PAWNIO_HOME 'modules'
} else {
    foreach ($root in @($env:ProgramFiles, ${env:ProgramFiles(x86)})) {
        if (-not $root) { continue }
        $candidate = Join-Path $root 'PawnIO'
        if (Test-Path (Join-Path $candidate 'PawnIOLib.dll')) {
            $Destination = Join-Path $candidate 'modules'
            break
        }
    }
}

if (-not $Destination) {
    throw "PawnIO install not found. Install PawnIO first via the Settings > Hardware support panel."
}

if (-not (Test-IsAdministrator)) {
    Write-Host "[deploy-pawnio-modules] elevating to write to $Destination"
    $arguments = @(
        '-NoLogo', '-NoProfile', '-ExecutionPolicy', 'Bypass',
        '-File', $PSCommandPath
    )
    if ($RestartBroker) { $arguments += '-RestartBroker' }
    $process = Start-Process -FilePath 'powershell.exe' -ArgumentList $arguments -Verb RunAs -Wait -PassThru
    exit $process.ExitCode
}

New-Item -ItemType Directory -Force -Path $Destination | Out-Null

foreach ($module in $Modules) {
    $target = Join-Path $Destination $module.Name
    Copy-Item -LiteralPath $module.FullName -Destination $target -Force
    Write-Host "[deploy-pawnio-modules] $($module.Name) -> $target"
}

if ($RestartBroker -or (Get-Service -Name 'HypercolorSmBus' -ErrorAction SilentlyContinue)) {
    $service = Get-Service -Name 'HypercolorSmBus' -ErrorAction SilentlyContinue
    if ($service) {
        if ($service.Status -eq 'Running') {
            Write-Host "[deploy-pawnio-modules] restarting HypercolorSmBus broker"
            Restart-Service -Name 'HypercolorSmBus' -Force
        } else {
            Write-Host "[deploy-pawnio-modules] starting HypercolorSmBus broker"
            Start-Service -Name 'HypercolorSmBus'
        }
    }
}

Write-Host "[deploy-pawnio-modules] done. $($Modules.Count) modules deployed to $Destination."
