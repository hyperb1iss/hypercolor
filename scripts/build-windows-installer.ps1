param(
    [ValidateSet("dev", "release")]
    [string] $Profile = "release",
    [string] $Bundles = "nsis",
    [switch] $SkipPawnIo,
    [switch] $SkipUi,
    [switch] $SkipEffects,
    # Build the daemon without Servo. The result installs and runs but renders
    # no HTML effects: it exists to prove the installer in minutes, not hours.
    [switch] $NoServo,
    # Build the installer that registers the update service. Only the official
    # release sets this.
    [switch] $ManagedUpdate,
    [switch] $CheckOnly
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$CargoCacheBuild = Join-Path $RepoRoot "scripts\cargo-cache-build.ps1"
$StageAssets = Join-Path $RepoRoot "scripts\stage-app-bundle-assets.ps1"
$ProduceUnit = Join-Path $RepoRoot "scripts\produce-windows-unit.ps1"
# Cargo builds the dev profile into a directory with another name.
$ProfileDirectory = if ($Profile -eq "dev") { "debug" } else { $Profile }
$TauriProfileArgs = if ($Profile -eq "dev") { @("--debug") } else { @() }
$TauriConfigArgs = @("--config", "tauri.bundle.conf.json", "--config", "tauri.windows.bundle.conf.json")
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
    $arguments += $CargoArgs

    Invoke-Checked $Description $CargoCacheBuild $arguments
}

function Invoke-StageAssets {
    Write-Step "Stage app bundle assets"
    & $StageAssets -Profile $ProfileDirectory -SkipPawnIo:$SkipPawnIo -StandInAngle:$NoServo
    if ($LASTEXITCODE -ne 0) {
        throw "Stage app bundle assets failed with exit code $LASTEXITCODE"
    }
}

function Assert-Prerequisites {
    Require-Command "cargo" "Install Rust from https://rustup.rs/."
    Require-Command "rustc" "Install Rust from https://rustup.rs/."
    if (-not ($SkipUi -and $SkipEffects)) {
        Require-Command "bun" "Install Bun from https://bun.sh/."
    }
    if (-not $SkipUi) {
        Require-Command "trunk" "Install with: cargo install trunk --locked"
    }

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
        $roots += (Join-Path $env:CARGO_TARGET_DIR "$ProfileDirectory\bundle\nsis")
    }

    $userProfile = [Environment]::GetFolderPath("UserProfile")
    if ($userProfile) {
        $roots += (Join-Path $userProfile ".cache\hypercolor\target\$ProfileDirectory\bundle\nsis")
    }

    $roots += (Join-Path $RepoRoot "target\$ProfileDirectory\bundle\nsis")
    $roots += (Join-Path $RepoRoot "crates\hypercolor-app\target\$ProfileDirectory\bundle\nsis")
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

$DaemonArgs = @("-p", "hypercolor-daemon", "--bin", "hypercolor-daemon")
if (-not $NoServo) {
    $DaemonArgs += @("--features", "servo")
}
$CliArgs = @("-p", "hypercolor")
if ($ManagedUpdate) {
    $CliArgs += @("--features", "managed-update")
}
Invoke-CargoBuild "Build daemon" $DaemonArgs
Invoke-CargoBuild "Build CLI" $CliArgs
Invoke-CargoBuild "Build Windows SMBus broker" @("-p", "hypercolor-windows-pawnio", "--bin", "hypercolor-smbus-service")
Invoke-CargoBuild "Build Windows privileged helper" @("-p", "hypercolor-windows-helper", "--bin", "hypercolor-windows-helper")
Invoke-CargoBuild "Build the stable launcher and recovery bootstrap" @("-p", "hypercolor-windows-launcher", "--bins")
Invoke-CargoBuild "Build the Windows unit producer" @("-p", "hypercolor-install", "--bin", "hypercolor-windows-unit")
Invoke-StageAssets

$AppDirectory = Join-Path $RepoRoot "crates\hypercolor-app"
Invoke-Checked "Build the desktop app" "cargo" (@("tauri", "build", "--no-bundle") + $TauriProfileArgs + $TauriConfigArgs) $AppDirectory

Write-Step "Produce the release unit"
& $ProduceUnit -ProfileDirectory $ProfileDirectory
if ($LASTEXITCODE -ne 0) {
    throw "Produce the release unit failed with exit code $LASTEXITCODE"
}

Invoke-Checked `
    "Bundle unsigned Windows installer" `
    "cargo" `
    (@("tauri", "bundle", "--bundles", $Bundles) + $TauriProfileArgs + $TauriConfigArgs) `
    $AppDirectory

Show-InstallerArtifacts
