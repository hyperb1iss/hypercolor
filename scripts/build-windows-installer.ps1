param(
    [ValidateSet("preview", "release")]
    [string] $Profile = "release",
    [string] $Target = "",
    [string] $Bundles = "nsis",
    [switch] $SkipPawnIo,
    [switch] $SkipUi,
    [switch] $SkipEffects,
    [switch] $CheckOnly
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$CargoCacheBuild = Join-Path $RepoRoot "scripts\cargo-cache-build.ps1"
$StageAssets = Join-Path $RepoRoot "scripts\stage-app-bundle-assets.ps1"
# crates/hypercolor-app/installer.nsi forks the NSIS template of the
# tauri-bundler this CLI version pins, so installers build with exactly it.
$PinnedTauriCli = "2.12.1"

function Write-Step {
    param([string] $Message)

    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Require-Command {
    param(
        [string] $Name,
        [string] $InstallHint
    )

    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Missing '$Name'. $InstallHint"
    }
}

function Invoke-Checked {
    param(
        [string] $Description,
        [string] $File,
        [string[]] $Arguments,
        [string] $WorkingDirectory = $RepoRoot
    )

    Write-Step $Description
    Push-Location $WorkingDirectory
    try {
        & $File @Arguments
        if ($LASTEXITCODE -ne 0) {
            throw "$Description failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
}

function Invoke-CargoBuild {
    param(
        [string] $Description,
        [string[]] $CargoArgs
    )

    $arguments = @("cargo", "build", "--locked", "--profile", $Profile)
    if ($Target) {
        $arguments += @("--target", $Target)
    }
    $arguments += $CargoArgs

    Invoke-Checked $Description $CargoCacheBuild $arguments
}

function Invoke-StageAssets {
    Write-Step "Stage app bundle assets"
    if ($Target) {
        & $StageAssets -Profile $Profile -Target $Target -SkipPawnIo:$SkipPawnIo
    } else {
        & $StageAssets -Profile $Profile -SkipPawnIo:$SkipPawnIo
    }
    if ($LASTEXITCODE -ne 0) {
        throw "Stage app bundle assets failed with exit code $LASTEXITCODE"
    }
}

function Assert-Prerequisites {
    Require-Command "cargo" "Install Rust from https://rustup.rs/."
    Require-Command "rustc" "Install Rust from https://rustup.rs/."
    Require-Command "bun" "Install Bun from https://bun.sh/."
    Require-Command "trunk" "Install with: cargo install trunk --locked"

    $installHint = "cargo install tauri-cli --version '=$PinnedTauriCli' --locked"
    $tauriVersion = & cargo tauri --version 2>$null
    if ($LASTEXITCODE -ne 0) {
        throw "Missing cargo-tauri. Install with: $installHint"
    }
    Write-Host "cargo-tauri: $tauriVersion"
    if ($Bundles -match "(^|,)nsis(,|$)" -and "$tauriVersion".Trim() -ne "tauri-cli $PinnedTauriCli") {
        throw "NSIS installers need tauri-cli $PinnedTauriCli, the version installer.nsi was forked from; found '$tauriVersion'. Install with: $installHint"
    }

    if ($Bundles -match "(^|,)nsis(,|$)" -and -not (Get-Command "makensis" -ErrorAction SilentlyContinue)) {
        Write-Warning "makensis was not found on PATH. Tauri may provide NSIS itself; install NSIS if the bundle step fails."
    }
}

function Show-InstallerArtifacts {
    $roots = @()
    if ($env:CARGO_TARGET_DIR) {
        $roots += (Join-Path $env:CARGO_TARGET_DIR "release\bundle\nsis")
    }

    $userProfile = [Environment]::GetFolderPath("UserProfile")
    if ($userProfile) {
        $roots += (Join-Path $userProfile ".cache\hypercolor\target\release\bundle\nsis")
    }

    $roots += (Join-Path $RepoRoot "target\release\bundle\nsis")
    $roots += (Join-Path $RepoRoot "crates\hypercolor-app\target\release\bundle\nsis")
    $roots = $roots | Select-Object -Unique

    $artifacts = @()
    foreach ($root in $roots) {
        if (Test-Path -LiteralPath $root) {
            $artifacts += @(Get-ChildItem -LiteralPath $root -Filter "*.exe" -File)
        }
    }

    Write-Step "Installer artifacts"
    if ($artifacts.Count -eq 0) {
        Write-Warning "No NSIS installer artifact found in the known bundle output directories."
        return
    }

    foreach ($artifact in $artifacts) {
        Write-Host $artifact.FullName
    }
}

Set-Location $RepoRoot
Assert-Prerequisites

if ($CheckOnly) {
    Write-Step "Prerequisite check complete"
    exit 0
}

if (-not $SkipUi) {
    Invoke-Checked "Install UI dependencies" "bun" @("install") (Join-Path $RepoRoot "crates\hypercolor-ui")
    Invoke-Checked "Build production UI" "trunk" @("build", "--release") (Join-Path $RepoRoot "crates\hypercolor-ui")
}

if (-not $SkipEffects) {
    Invoke-Checked "Install SDK dependencies" "bun" @("install") (Join-Path $RepoRoot "sdk")
    Invoke-Checked "Build bundled effects" "bun" @("run", "build:effects") (Join-Path $RepoRoot "sdk")
}

Invoke-CargoBuild "Build daemon sidecar" @("-p", "hypercolor-daemon", "--features", "servo")
Invoke-CargoBuild "Build CLI sidecar" @("-p", "hypercolor")
Invoke-CargoBuild "Build Windows SMBus broker" @("-p", "hypercolor-windows-pawnio", "--bin", "hypercolor-smbus-service")
Invoke-CargoBuild "Build Windows privileged helper" @("-p", "hypercolor-windows-helper", "--bin", "hypercolor-windows-helper")
Invoke-StageAssets

Invoke-Checked `
    "Build unsigned Tauri Windows installer" `
    "cargo" `
    @(
        "tauri", "build",
        "--config", "tauri.bundle.conf.json",
        "--config", "tauri.windows.bundle.conf.json",
        "--bundles", $Bundles
    ) `
    (Join-Path $RepoRoot "crates\hypercolor-app")

Show-InstallerArtifacts
