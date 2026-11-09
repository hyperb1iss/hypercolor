<#
.SYNOPSIS
    Produce the release unit an installer carries, and the digest that names
    it, from a build's outputs.

.DESCRIPTION
    A unit holds every program's exact bytes, so this runs after the programs
    are built, and after they are signed when the build signs. The tree lands
    in target\bundle-stage\windows-unit and the digest beside it in
    windows-unit.digest, where installer-hooks.nsh compiles them in. Running
    it again replaces both. The version comes from tauri.conf.json, which a
    release build has already patched.
#>
param(
    # The directory under the Cargo target directory holding the programs.
    [string] $ProfileDirectory = 'release',
    [string] $RustTarget = 'x86_64-pc-windows-msvc',
    # Store inventories a downstream build adds.
    [string[]] $DurableStores = @(),
    # Notices of third-party code a downstream build links.
    [string] $ThirdPartyNotices = ''
)

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $RepoRoot 'target' }
$BinDir = Join-Path $TargetDir $ProfileDirectory
$StageDir = Join-Path $RepoRoot 'target\bundle-stage'
$Unit = Join-Path $StageDir 'windows-unit'
$DigestFile = Join-Path $StageDir 'windows-unit.digest'

$config = Get-Content -Raw -LiteralPath (Join-Path $RepoRoot 'crates\hypercolor-app\tauri.conf.json') |
    ConvertFrom-Json

foreach ($path in @($Unit, $DigestFile)) {
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Recurse -Force
    }
}

$arguments = @(
    '--version', $config.version,
    '--rust-target', $RustTarget,
    '--bin-dir', $BinDir,
    '--stage-dir', $StageDir,
    '--source-root', $RepoRoot,
    '--output', $Unit
)
foreach ($inventory in $DurableStores) {
    $arguments += @('--durable-stores', $inventory)
}
if ($ThirdPartyNotices) {
    $arguments += @('--third-party-notices', $ThirdPartyNotices)
}

$digest = & (Join-Path $BinDir 'hypercolor-windows-unit.exe') @arguments
if ($LASTEXITCODE -ne 0) {
    throw "hypercolor-windows-unit failed with exit code $LASTEXITCODE"
}
if ("$digest" -notmatch '^[0-9a-f]{64}$') {
    throw "hypercolor-windows-unit printed '$digest', not a digest"
}
Set-Content -NoNewline -Encoding ascii -LiteralPath $DigestFile -Value $digest
Write-Host "produced Windows unit $digest -> $Unit"
