param(
    [switch] $Check
)

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

$metadata = & cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed with exit code $LASTEXITCODE"
}

$workspaceIds = [System.Collections.Generic.HashSet[string]]::new()
foreach ($id in $metadata.workspace_members) {
    [void] $workspaceIds.Add($id)
}

$packages = $metadata.packages |
    Where-Object { $workspaceIds.Contains($_.id) } |
    Sort-Object name

foreach ($package in $packages) {
    $cargoArgs = @('fmt', '-p', $package.name)
    if ($Check) {
        $cargoArgs += @('--', '--check')
    }

    Write-Host "[cargo-fmt] $($package.name)"
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw "cargo fmt failed for $($package.name) with exit code $LASTEXITCODE"
    }
}
