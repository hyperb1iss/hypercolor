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

    $resolvedPath = [System.IO.Path]::GetFullPath($Path).TrimEnd(
        [System.IO.Path]::DirectorySeparatorChar,
        [System.IO.Path]::AltDirectorySeparatorChar
    )

    # The stage dir intentionally lives under CARGO_TARGET_DIR (which is
    # commonly set to a cache outside the repo, e.g. ~/.cache/hypercolor/
    # target/). Treat both repo root and the cargo target dir as safe
    # roots — the entire purpose of the check is to prevent stray
    # destruction outside controlled build/cache surfaces.
    $allowedRoots = @($RepoRoot)
    if ($env:CARGO_TARGET_DIR) {
        $allowedRoots += $env:CARGO_TARGET_DIR
    }

    foreach ($root in $allowedRoots) {
        if (-not $root) { continue }
        $resolvedRoot = [System.IO.Path]::GetFullPath($root).TrimEnd(
            [System.IO.Path]::DirectorySeparatorChar,
            [System.IO.Path]::AltDirectorySeparatorChar
        )
        $rootPrefix = "$resolvedRoot$([System.IO.Path]::DirectorySeparatorChar)"
        if (
            $resolvedPath -eq $resolvedRoot -or
            $resolvedPath.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)
        ) {
            return
        }
    }

    throw "refusing to modify path outside repo or CARGO_TARGET_DIR: $resolvedPath"
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

function Copy-AngleRuntime {
    # Servo's WebGL backend goes through mozangle's libEGL.dll +
    # libGLESv2.dll. Without these next to hypercolor-daemon.exe the
    # render thread panics at startup with "egl function was not
    # loaded" and the daemon dies before serving its first frame.
    # `cargo build` produces them under <profile>/build/mozangle-*/out/
    # for the host triple or <target>/<profile>/build/... for a
    # cross-target build. Mirror $ProfileDir's derivation so a
    # preview/cross-target build doesn't accidentally pull stale
    # release-profile DLLs.
    $buildRoot = Join-Path $ProfileDir 'build'
    if (-not (Test-Path -LiteralPath $buildRoot)) {
        throw "missing $Profile build dir: $buildRoot; build hypercolor-daemon before staging"
    }

    $eglDll = Get-ChildItem -LiteralPath $buildRoot -Recurse -Filter 'libEGL.dll' -ErrorAction SilentlyContinue |
        Where-Object { Test-Path -LiteralPath (Join-Path $_.DirectoryName 'libGLESv2.dll') } |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1

    if ($null -eq $eglDll) {
        throw "could not locate libEGL.dll + libGLESv2.dll under $buildRoot; rebuild hypercolor-daemon with the servo feature for profile $Profile"
    }

    Copy-Item -LiteralPath $eglDll.FullName -Destination (Join-Path $StageDlls 'libEGL.dll') -Force
    Copy-Item -LiteralPath (Join-Path $eglDll.DirectoryName 'libGLESv2.dll') -Destination (Join-Path $StageDlls 'libGLESv2.dll') -Force
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

# Tauri bundle config references the stage dir via paths relative to
# crates/hypercolor-app/ (`../../target/bundle-stage/...`), so the dir
# must live at `<repo>/target/bundle-stage/` regardless of where
# CARGO_TARGET_DIR puts the actual cargo build outputs.
$StageDir = Join-Path $RepoRoot 'target\bundle-stage'
$StageBin = Join-Path $StageDir 'binaries'
$StageUi = Join-Path $StageDir 'ui'
$StageEffects = Join-Path $StageDir 'effects'
$StageTools = Join-Path $StageDir 'tools'
$StageDlls = Join-Path $StageDir 'dlls'

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
Reset-Directory $StageUi
Reset-Directory $StageEffects
Reset-Directory $StageTools
if (Test-WindowsTarget) {
    Reset-Directory $StageDlls
}

Get-ChildItem -LiteralPath $uiDist -Force |
    Copy-Item -Destination $StageUi -Recurse -Force
Get-ChildItem -LiteralPath $effectsDist -Force |
    Copy-Item -Destination $StageEffects -Recurse -Force

Copy-Sidecar 'hypercolor-daemon'
Copy-Sidecar 'hypercolor'

if (Test-WindowsTarget) {
    Copy-WindowsToolBinary 'hypercolor-smbus-service'
    Copy-WindowsToolBinary 'hypercolor-windows-helper'

    Copy-AngleRuntime

    if (-not $SkipPawnIo) {
        & (Join-Path $RepoRoot 'scripts\fetch-pawnio-assets.ps1') `
            -Destination (Join-Path $StageTools 'pawnio')
    }
}

Write-Host "staged hypercolor-app bundle assets for $Target ($Profile) -> $StageDir"
