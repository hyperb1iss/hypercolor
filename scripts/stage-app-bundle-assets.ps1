param(
    [string] $Profile = 'release',
    [string] $Target = '',
    [switch] $SkipPawnIo
)

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot

function Get-HypercolorHostTriple {
    $hostTuple = & rustc --print host-tuple 2>$null
    if ($LASTEXITCODE -eq 0 -and $hostTuple) {
        return $hostTuple.Trim()
    }

    $verbose = & rustc -vV
    $hostLine = $verbose | Where-Object { $_ -like 'host: *' } | Select-Object -First 1
    if (-not $hostLine) {
        throw 'failed to determine Rust host triple'
    }
    return ($hostLine -replace '^host:\s*', '').Trim()
}

function Assert-UnderRepo {
    param([string] $Path)

    $resolvedRepo = [System.IO.Path]::GetFullPath($RepoRoot).TrimEnd(
        [System.IO.Path]::DirectorySeparatorChar,
        [System.IO.Path]::AltDirectorySeparatorChar
    )
    $resolvedPath = [System.IO.Path]::GetFullPath($Path).TrimEnd(
        [System.IO.Path]::DirectorySeparatorChar,
        [System.IO.Path]::AltDirectorySeparatorChar
    )
    $repoPrefix = "$resolvedRepo$([System.IO.Path]::DirectorySeparatorChar)"
    if (
        $resolvedPath -ne $resolvedRepo -and
        -not $resolvedPath.StartsWith($repoPrefix, [System.StringComparison]::OrdinalIgnoreCase)
    ) {
        throw "refusing to modify path outside repository: $resolvedPath"
    }
}

function Reset-Directory {
    param([string] $Path)

    Assert-UnderRepo $Path
    if (Test-Path -LiteralPath $Path) {
        Remove-Item -LiteralPath $Path -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $Path | Out-Null
}

function Copy-Sidecar {
    param([string] $Name)

    $source = Join-Path $ProfileDir "$Name$Exe"
    if (-not (Test-Path -LiteralPath $source)) {
        throw "missing built binary: $source; build release binaries before staging app bundle assets"
    }

    $targetPath = Join-Path $StageBin "$Name-$Target$Exe"
    Copy-Item -LiteralPath $source -Destination $targetPath -Force
}

function Copy-WindowsToolBinary {
    param([string] $Name)

    $source = Join-Path $ProfileDir "$Name$Exe"
    if (-not (Test-Path -LiteralPath $source)) {
        throw "missing built Windows tool binary: $source; build release binaries before staging app bundle assets"
    }

    Copy-Item -LiteralPath $source -Destination (Join-Path $StageTools "$Name$Exe") -Force
}

function Test-WindowsTarget {
    $Target -like '*windows*' -or $Target -like '*-pc-windows-*'
}

Set-Location $RepoRoot

$HostTarget = Get-HypercolorHostTriple
if (-not $Target) {
    $Target = $HostTarget
}

$TargetDir = if ($env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR
} else {
    Join-Path $RepoRoot 'target'
}

$ProfileDir = Join-Path $TargetDir $Profile
if ($Target -ne $HostTarget) {
    $ProfileDir = Join-Path (Join-Path $TargetDir $Target) $Profile
}

$Exe = if ($Target -like '*windows*' -or $Target -like '*-pc-windows-*') { '.exe' } else { '' }

$StageDir = Join-Path $TargetDir 'bundle-stage'
$StageBin = Join-Path $StageDir 'binaries'
$StageTools = Join-Path $StageDir 'tools'

$uiDist = Join-Path $RepoRoot 'crates\hypercolor-ui\dist'
$uiIndex = Join-Path $uiDist 'index.html'
if (-not (Test-Path -LiteralPath $uiIndex)) {
    Write-Error 'crates/hypercolor-ui/dist is missing or incomplete; run "just ui-build" first'
    exit 1
}

$effectsDist = Join-Path $RepoRoot 'effects\hypercolor'
$effectsEmpty = (-not (Test-Path -LiteralPath $effectsDist)) -or `
    (-not (Get-ChildItem -LiteralPath $effectsDist -Force -ErrorAction SilentlyContinue))
if ($effectsEmpty) {
    Write-Error 'effects/hypercolor is missing or empty; run "just effects-build" first'
    exit 1
}

Reset-Directory $StageBin
Reset-Directory $StageTools

Copy-Sidecar 'hypercolor-daemon'
Copy-Sidecar 'hypercolor'

if (Test-WindowsTarget) {
    Copy-WindowsToolBinary 'hypercolor-smbus-service'

    if (-not $SkipPawnIo) {
        & (Join-Path $RepoRoot 'scripts\fetch-pawnio-assets.ps1') `
            -Destination (Join-Path $StageTools 'pawnio')
    }
}

Write-Host "staged hypercolor-app bundle assets for $Target ($Profile) -> $StageDir"
