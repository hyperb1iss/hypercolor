# Hypercolor — cross-platform developer environment bootstrap (Windows).
#
# Installs every tool needed to build the daemon, UI, SDK, and Python client,
# idempotently. Safe to re-run; skips anything already installed.
#
# Usage:
#   scripts\setup.ps1                 # full setup, prompt before installs
#   scripts\setup.ps1 -Yes            # full setup, no prompts
#   scripts\setup.ps1 -Minimal        # rust + wasm target only
#   scripts\setup.ps1 -WithServo      # extra deps for Servo HTML renderer

[CmdletBinding()]
param(
    [switch]$Yes,
    [switch]$Minimal,
    [switch]$WithServo,
    [switch]$NoSystem
)

$ErrorActionPreference = 'Stop'

# ─── SilkCircuit palette ─────────────────────────────────────────────
$ESC = [char]27
$ElectricPurple = "$ESC[38;2;225;53;255m"
$NeonCyan       = "$ESC[38;2;128;255;234m"
$Coral          = "$ESC[38;2;255;106;193m"
$ElectricYellow = "$ESC[38;2;241;250;140m"
$SuccessGreen   = "$ESC[38;2;80;250;123m"
$ErrorRed       = "$ESC[38;2;255;99;99m"
$Dim            = "$ESC[2m"
$Bold           = "$ESC[1m"
$Reset          = "$ESC[0m"

if ($env:NO_COLOR) {
    $ElectricPurple = $NeonCyan = $Coral = $ElectricYellow = ''
    $SuccessGreen = $ErrorRed = $Dim = $Bold = $Reset = ''
}

# ─── Output helpers ──────────────────────────────────────────────────
function Section { param($msg) Write-Host "`n$ElectricPurple$Bold▶$Reset $Bold$msg$Reset" }
function Ok      { param($msg) Write-Host "  $SuccessGreen✓$Reset $msg" }
function Info    { param($msg) Write-Host "  $NeonCyan→$Reset $msg" }
function Warn    { param($msg) Write-Host "  $ElectricYellow!$Reset $msg" }
function Err     { param($msg) Write-Host "  $ErrorRed✗$Reset $msg" -ForegroundColor Red }
function Note    { param($msg) Write-Host "    $Dim$msg$Reset" }

function Has-Cmd { param($name) $null -ne (Get-Command $name -ErrorAction SilentlyContinue) }

function Confirm-Action {
    param($prompt)
    if ($Yes) { return $true }
    $reply = Read-Host "    $ElectricYellow?$Reset $prompt [y/N]"
    return ($reply -match '^(y|yes)$')
}

function Bin-Version {
    param($name)
    try {
        $output = & $name --version 2>$null | Select-Object -First 1
        if ($output -match '\S+\s+(\S+)') { return $matches[1] }
    } catch {}
    return ''
}

function Winget-Install {
    param($id, $displayName = $id)
    if (-not (Has-Cmd winget)) {
        Warn "winget not available — install $displayName manually"
        return $false
    }
    Info "winget install $id"
    $result = winget install --id $id --silent --accept-package-agreements --accept-source-agreements 2>&1
    if ($LASTEXITCODE -eq 0 -or $LASTEXITCODE -eq -1978335189) {
        # -1978335189 = APPINSTALLER_CLI_ERROR_UPDATE_NOT_APPLICABLE (already installed)
        return $true
    }
    Warn "winget install $id failed (exit $LASTEXITCODE)"
    return $false
}

$Root = Resolve-Path (Join-Path $PSScriptRoot '..')

# ─── Banner ──────────────────────────────────────────────────────────
$winVer = (Get-CimInstance Win32_OperatingSystem -ErrorAction SilentlyContinue).Caption
if (-not $winVer) { $winVer = 'Windows' }

Write-Host ''
Write-Host "$ElectricPurple$Bold    ╭───────────────────────────────────────╮$Reset"
Write-Host "$ElectricPurple$Bold    │     Hypercolor Developer Setup        │$Reset"
Write-Host "$ElectricPurple$Bold    ╰───────────────────────────────────────╯$Reset"
Write-Host "    ${Dim}host$Reset $NeonCyan$winVer$Reset    ${Dim}pkg$Reset $Coral$(if (Has-Cmd winget) { 'winget' } else { 'manual' })$Reset"
Write-Host ''

# ─── 1. Rust toolchain ───────────────────────────────────────────────
Section 'rust toolchain'

if (-not (Has-Cmd rustup)) {
    Warn 'rustup not found'
    if (Confirm-Action 'install rustup via winget?') {
        if (-not (Winget-Install 'Rustlang.Rustup' 'rustup')) {
            Err 'rustup install failed — install manually from https://rustup.rs'
            exit 1
        }
        $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
    } else {
        Err 'rustup is required — re-run after installing'
        exit 1
    }
}
Ok "rustup $(Bin-Version rustup)"

# rust-toolchain.toml will pull stable + components on first cargo invocation
& rustup show *>$null
$activeToolchain = (& rustup show active-toolchain 2>$null) -split '\s+' | Select-Object -First 1
Ok "toolchain $activeToolchain"

$installedTargets = & rustup target list --installed 2>$null
if ($installedTargets -contains 'wasm32-unknown-unknown') {
    Ok 'wasm32-unknown-unknown target installed'
} else {
    Info 'adding wasm32-unknown-unknown target...'
    & rustup target add wasm32-unknown-unknown
    Ok 'wasm32-unknown-unknown target installed'
}

if ($Minimal) {
    Write-Host ''
    Write-Host "$SuccessGreen$Bold✓ minimal setup complete$Reset — re-run without -Minimal for the full toolchain"
    Write-Host ''
    exit 0
}

# ─── 2. System build tools (MSVC) ────────────────────────────────────
if ($NoSystem) {
    Section 'system build tools'
    Warn 'skipped (-NoSystem)'
} else {
    Section 'system build tools (MSVC)'

    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $hasVcTools = $false
    if (Test-Path $vswhere) {
        $vsInstall = & $vswhere -latest -products * `
            -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
            -property installationPath 2>$null
        if ($vsInstall) { $hasVcTools = $true }
    }

    if ($hasVcTools) {
        Ok 'Visual Studio C++ Build Tools'
    } else {
        Warn 'Visual Studio C++ Build Tools not found (required for Rust on Windows)'
        if (Confirm-Action 'install Microsoft.VisualStudio.2022.BuildTools via winget? (~2GB)') {
            if (Has-Cmd winget) {
                Info 'installing VS Build Tools (this takes a while)...'
                winget install --id Microsoft.VisualStudio.2022.BuildTools --silent `
                    --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended" `
                    --accept-package-agreements --accept-source-agreements
                if ($LASTEXITCODE -eq 0) {
                    Ok 'Visual Studio C++ Build Tools'
                } else {
                    Warn 'install failed — see https://visualstudio.microsoft.com/downloads/'
                }
            } else {
                Warn 'winget not available — install manually from https://visualstudio.microsoft.com/downloads/'
            }
        } else {
            Note 'manual: https://visualstudio.microsoft.com/downloads/ → "Build Tools for Visual Studio 2022"'
        }
    }
}

# ─── 3. Cargo-installed dev tools ────────────────────────────────────
Section 'cargo tools'

$hasBinstall = $false
if (Has-Cmd cargo-binstall) {
    Ok "cargo-binstall $(Bin-Version cargo-binstall)"
    $hasBinstall = $true
} elseif (Confirm-Action 'install cargo-binstall for fast prebuilt binary installs? (recommended)') {
    $installer = Invoke-WebRequest -UseBasicParsing `
        'https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.ps1'
    Invoke-Expression $installer.Content
    if (Has-Cmd cargo-binstall) {
        Ok 'cargo-binstall installed'
        $hasBinstall = $true
    } else {
        Warn 'cargo-binstall install failed — falling back to source builds'
    }
}

function Cargo-Get {
    param($bin, $pkg = $bin)
    if (Has-Cmd $bin) {
        Ok "$bin $(Bin-Version $bin)"
        return
    }
    if ($script:hasBinstall) {
        Info "cargo binstall $pkg"
        & cargo binstall --no-confirm --quiet $pkg 2>&1 | Out-Null
        if (-not (Has-Cmd $bin)) {
            & cargo install --locked $pkg
        }
    } else {
        Info "cargo install $pkg (compiling from source — grab a coffee)"
        & cargo install --locked $pkg
    }
    Ok "$bin installed"
}

Cargo-Get just
Cargo-Get trunk
Cargo-Get cargo-deny

if (Has-Cmd sccache) {
    Ok "sccache $(Bin-Version sccache)"
} elseif (Confirm-Action 'install sccache for compilation caching? (highly recommended)') {
    Cargo-Get sccache
}

# ─── 4. Bun (SDK runtime) ────────────────────────────────────────────
Section 'bun'
if (Has-Cmd bun) {
    Ok "bun $(& bun --version)"
} else {
    Info 'installing bun via winget...'
    if (-not (Winget-Install 'Oven-sh.Bun' 'bun')) {
        Info 'falling back to powershell installer...'
        try {
            Invoke-Expression "& {$(Invoke-RestMethod https://bun.sh/install.ps1)}"
        } catch {
            Err 'bun install failed — see https://bun.sh'
        }
    }
    $env:Path = "$env:USERPROFILE\.bun\bin;$env:Path"
    if (Has-Cmd bun) { Ok "bun $(& bun --version)" }
}

# ─── 5. Frontend dependencies ────────────────────────────────────────
Section 'frontend dependencies'

if (-not (Has-Cmd npm)) {
    Warn 'npm not found — install Node.js (winget install OpenJS.NodeJS.LTS)'
} else {
    Info 'npm ci in crates/hypercolor-ui (Tailwind v4)'
    Push-Location (Join-Path $Root 'crates/hypercolor-ui')
    try { & npm ci --silent --no-audit --no-fund | Out-Null; Ok 'crates/hypercolor-ui ready' }
    catch { Warn 'hypercolor-ui npm ci failed' }
    finally { Pop-Location }
}

if (Has-Cmd bun) {
    Info 'bun install in sdk/'
    Push-Location (Join-Path $Root 'sdk')
    try { & bun install --silent | Out-Null; Ok 'sdk/ ready' }
    catch { Warn 'sdk/ bun install failed' }
    finally { Pop-Location }
}

if ((Test-Path (Join-Path $Root 'e2e/package.json')) -and (Has-Cmd npm)) {
    Info 'npm ci in e2e/'
    Push-Location (Join-Path $Root 'e2e')
    try { & npm ci --silent --no-audit --no-fund | Out-Null; Ok 'e2e/ ready' }
    catch { Warn 'e2e/ npm ci failed (non-fatal — needed for ''just e2e'')' }
    finally { Pop-Location }
}

# ─── 6. Python client (optional) ─────────────────────────────────────
Section 'python client'
if (Has-Cmd uv) {
    Ok "uv $(Bin-Version uv)"
    Info 'uv sync in python/'
    Push-Location (Join-Path $Root 'python')
    try { & uv sync --quiet; Ok 'python/ ready' }
    catch { Warn 'python/ uv sync failed' }
    finally { Pop-Location }
} else {
    Warn 'uv not installed — needed only for ''just python-*'' recipes'
    Note 'install with: winget install astral-sh.uv'
}

# ─── Final summary ───────────────────────────────────────────────────
Write-Host ''
Write-Host "$SuccessGreen$Bold    ╭───────────────────────────────────────╮$Reset"
Write-Host "$SuccessGreen$Bold    │           ✓  All set                  │$Reset"
Write-Host "$SuccessGreen$Bold    ╰───────────────────────────────────────╯$Reset"
Write-Host ''
Write-Host "    ${Bold}Next:$Reset"
Write-Host "      $ElectricPurple•$Reset ${NeonCyan}just verify$Reset    run lint + tests"
Write-Host "      $ElectricPurple•$Reset ${NeonCyan}just daemon$Reset    start the daemon on :9420"
Write-Host "      $ElectricPurple•$Reset ${NeonCyan}just dev$Reset       daemon + UI dev server together"
Write-Host ''
