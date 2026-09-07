[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Build', 'Serve')]
    [string] $Mode,

    [ValidateRange(1, 65535)]
    [int] $Port = 9430,

    [string] $BindAddress = '127.0.0.1'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$uiRoot = Join-Path $repoRoot 'crates\hypercolor-ui'
$cargoRunner = Join-Path $PSScriptRoot 'cargo-cache-build.ps1'

Set-Location $uiRoot

if (-not $env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR = Join-Path $uiRoot 'target'
}

Remove-Item Env:NO_COLOR -ErrorAction SilentlyContinue

$trunkArgs = if ($Mode -eq 'Build') {
    # web-sys feature sets exceed Windows' process argument ceiling when
    # sccache re-spawns rustc. Cargo handles the direct compiler invocation.
    $env:HYPERCOLOR_NO_SCCACHE = '1'
    $env:CARGO_INCREMENTAL = '0'
    @('build', '--release', '--locked')
} else {
    $env:HYPERCOLOR_ITERATE = '1'
    @('serve', '--dist', '.dist-dev', '--port', "$Port", '--address', $BindAddress)
}

& $cargoRunner trunk @trunkArgs
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
