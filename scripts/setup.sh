#!/usr/bin/env bash
# Hypercolor — cross-platform developer environment bootstrap.
#
# Installs every tool needed to build the daemon, UI, SDK, and Python client,
# idempotently. Safe to re-run; skips anything already installed.
#
# Supported hosts:
#   Linux: Debian/Ubuntu (apt), Fedora/RHEL (dnf), Arch (pacman)
#   macOS: Homebrew + Xcode Command Line Tools
#
# Usage:
#   scripts/setup.sh                 # full setup, prompt before sudo
#   scripts/setup.sh -y              # full setup, no prompts
#   scripts/setup.sh --no-system     # skip system packages (no sudo)
#   scripts/setup.sh --with-servo    # extra deps for Servo HTML renderer
#   scripts/setup.sh --minimal       # rust + wasm target only

set -euo pipefail

# ─── SilkCircuit palette ─────────────────────────────────────────────
ELECTRIC_PURPLE=$'\033[38;2;225;53;255m'
NEON_CYAN=$'\033[38;2;128;255;234m'
CORAL=$'\033[38;2;255;106;193m'
ELECTRIC_YELLOW=$'\033[38;2;241;250;140m'
SUCCESS_GREEN=$'\033[38;2;80;250;123m'
ERROR_RED=$'\033[38;2;255;99;99m'
DIM=$'\033[2m'
BOLD=$'\033[1m'
RESET=$'\033[0m'

if [ ! -t 1 ] || [ "${NO_COLOR:-}" = "1" ]; then
  ELECTRIC_PURPLE='' NEON_CYAN='' CORAL='' ELECTRIC_YELLOW=''
  SUCCESS_GREEN='' ERROR_RED='' DIM='' BOLD='' RESET=''
fi

# ─── Output helpers ──────────────────────────────────────────────────
section() { printf "\n%s%s▶%s %s%s%s\n" "$ELECTRIC_PURPLE" "$BOLD" "$RESET" "$BOLD" "$1" "$RESET"; }
ok()      { printf "  %s✓%s %s\n" "$SUCCESS_GREEN" "$RESET" "$1"; }
info()    { printf "  %s→%s %s\n" "$NEON_CYAN" "$RESET" "$1"; }
warn()    { printf "  %s!%s %s\n" "$ELECTRIC_YELLOW" "$RESET" "$1"; }
err()     { printf "  %s✗%s %s\n" "$ERROR_RED" "$RESET" "$1" >&2; }
note()    { printf "    %s%s%s\n" "$DIM" "$1" "$RESET"; }
need()    { command -v "$1" >/dev/null 2>&1; }

# ─── Flags ───────────────────────────────────────────────────────────
SKIP_SYSTEM=0
WITH_SERVO=0
ASSUME_YES=0
MINIMAL=0

while [ $# -gt 0 ]; do
  case "$1" in
    --no-system|--skip-system) SKIP_SYSTEM=1 ;;
    --with-servo) WITH_SERVO=1 ;;
    --minimal) MINIMAL=1 ;;
    -y|--yes) ASSUME_YES=1 ;;
    -h|--help)
      sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) err "unknown flag: $1"; exit 2 ;;
  esac
  shift
done

# ─── Platform detection ──────────────────────────────────────────────
OS="$(uname -s)"
case "$OS" in
  Linux)
    if [ -r /etc/os-release ]; then
      # shellcheck disable=SC1091
      . /etc/os-release
      DISTRO="${ID:-unknown}"
      DISTRO_LIKE="${ID_LIKE:-}"
    else
      DISTRO=unknown DISTRO_LIKE=
    fi
    ;;
  Darwin) DISTRO=macos DISTRO_LIKE= ;;
  *) err "unsupported OS: $OS — Windows users: run scripts/setup.ps1"; exit 1 ;;
esac

PKG_MGR=
case "$DISTRO" in
  ubuntu|debian|linuxmint|pop|raspbian) PKG_MGR=apt ;;
  fedora|rhel|centos|rocky|almalinux) PKG_MGR=dnf ;;
  arch|manjaro|endeavouros|cachyos) PKG_MGR=pacman ;;
  macos) PKG_MGR=brew ;;
  *)
    case "$DISTRO_LIKE" in
      *debian*) PKG_MGR=apt ;;
      *fedora*|*rhel*) PKG_MGR=dnf ;;
      *arch*) PKG_MGR=pacman ;;
    esac
    ;;
esac

# ─── Banner ──────────────────────────────────────────────────────────
printf "\n"
printf "%s%s    ╭───────────────────────────────────────╮%s\n" "$ELECTRIC_PURPLE" "$BOLD" "$RESET"
printf "%s%s    │     Hypercolor Developer Setup        │%s\n" "$ELECTRIC_PURPLE" "$BOLD" "$RESET"
printf "%s%s    ╰───────────────────────────────────────╯%s\n" "$ELECTRIC_PURPLE" "$BOLD" "$RESET"
printf "    %shost%s %s%s%s    %sdistro%s %s%s%s    %spkg%s %s%s%s\n\n" \
  "$DIM" "$RESET" "$NEON_CYAN" "$OS" "$RESET" \
  "$DIM" "$RESET" "$NEON_CYAN" "$DISTRO" "$RESET" \
  "$DIM" "$RESET" "$CORAL" "${PKG_MGR:-none}" "$RESET"

# ─── Helpers ─────────────────────────────────────────────────────────
confirm() {
  if [ "$ASSUME_YES" -eq 1 ]; then return 0; fi
  printf "    %s?%s %s [y/N] " "$ELECTRIC_YELLOW" "$RESET" "$1"
  read -r reply
  case "$reply" in y|Y|yes|YES) return 0 ;; *) return 1 ;; esac
}

run_sudo() {
  if [ "${EUID:-$(id -u)}" -eq 0 ]; then "$@"; return; fi
  if ! need sudo; then err "sudo required but not installed"; return 1; fi
  sudo "$@"
}

# Prompt for sudo password upfront and cache credentials so the
# rest of the section runs without surprise prompts.
sudo_warmup() {
  if [ "${EUID:-$(id -u)}" -eq 0 ]; then return 0; fi
  if ! need sudo; then return 1; fi
  info "authenticating sudo (caches credentials for the rest of setup)..."
  sudo -v
}

bin_version() {
  "$1" --version 2>/dev/null | head -1 | awk '{print $2}' || true
}

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# ─── 1. Rust toolchain ───────────────────────────────────────────────
section "rust toolchain"

if ! need rustup; then
  warn "rustup not found"
  if confirm "install rustup via https://sh.rustup.rs?"; then
    info "installing rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none
    export PATH="$HOME/.cargo/bin:$PATH"
  else
    err "rustup is required — re-run after installing"
    exit 1
  fi
fi
ok "rustup $(bin_version rustup)"

# rust-toolchain.toml pins stable + clippy/rustfmt/rust-analyzer; trigger fetch
rustup show >/dev/null
ok "toolchain $(rustup show active-toolchain 2>/dev/null | awk '{print $1}')"

# Add wasm32 to every installed toolchain. Some shells (e.g. proto's activate
# hook) prepend a toolchain's bin dir to PATH, bypassing the rustup proxy and
# making non-active toolchains' cargo win — so the project's rust-toolchain.toml
# override is silently ignored. Installing wasm32 across the board makes trunk
# build the UI cleanly no matter which cargo runs.
while IFS= read -r tc; do
  [ -z "$tc" ] && continue
  if rustup target list --installed --toolchain "$tc" 2>/dev/null | grep -q '^wasm32-unknown-unknown$'; then
    ok "wasm32-unknown-unknown installed for $tc"
  else
    info "adding wasm32-unknown-unknown to $tc..."
    rustup target add wasm32-unknown-unknown --toolchain "$tc"
    ok "wasm32-unknown-unknown added to $tc"
  fi
done < <(rustup toolchain list 2>/dev/null | awk '{print $1}')

if [ "$MINIMAL" -eq 1 ]; then
  printf "\n%s%s✓ minimal setup complete%s — re-run without --minimal for the full toolchain\n\n" \
    "$SUCCESS_GREEN" "$BOLD" "$RESET"
  exit 0
fi

# ─── 2. System packages ──────────────────────────────────────────────
if [ "$SKIP_SYSTEM" -eq 1 ]; then
  section "system packages"
  warn "skipped (--no-system)"
elif [ -z "${PKG_MGR:-}" ]; then
  section "system packages"
  warn "unsupported distro $DISTRO — install manually (see docs/content/guide/installation.md)"
else
  section "system packages ($PKG_MGR)"
  case "$PKG_MGR" in
    apt)
      pkgs=(build-essential pkg-config cmake nasm
            libudev-dev libusb-1.0-0-dev libhidapi-dev
            libasound2-dev libpulse-dev libpipewire-0.3-dev
            libxdo-dev libgtk-3-dev libwebkit2gtk-4.1-dev
            libayatana-appindicator3-dev librsvg2-dev libssl-dev
            clang lld)
      [ "$WITH_SERVO" -eq 1 ] && pkgs+=(gperf libgtk-3-dev libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev)
      missing=()
      for p in "${pkgs[@]}"; do
        if dpkg -s "$p" >/dev/null 2>&1; then ok "$p"; else missing+=("$p"); fi
      done
      if [ "${#missing[@]}" -gt 0 ]; then
        info "installing: ${missing[*]}"
        if confirm "run sudo apt-get install -y for ${#missing[@]} package(s)?"; then
          sudo_warmup
          info "apt-get update (this can take a moment)..."
          run_sudo apt-get update
          info "apt-get install ${missing[*]}"
          run_sudo apt-get install -y "${missing[@]}"
          for p in "${missing[@]}"; do ok "$p"; done
        else
          warn "skipped — install manually before building"
        fi
      fi
      ;;
    dnf)
      pkgs=(gcc gcc-c++ pkg-config cmake nasm
            systemd-devel libusb1-devel hidapi-devel
            alsa-lib-devel pulseaudio-libs-devel pipewire-devel
            libxdo-devel gtk3-devel webkit2gtk4.1-devel
            libappindicator-gtk3-devel librsvg2-devel openssl-devel
            clang lld)
      [ "$WITH_SERVO" -eq 1 ] && pkgs+=(gperf gtk3-devel libxcb-devel libxkbcommon-devel libxkbcommon-x11-devel)
      missing=()
      for p in "${pkgs[@]}"; do
        if rpm -q "$p" >/dev/null 2>&1; then ok "$p"; else missing+=("$p"); fi
      done
      if [ "${#missing[@]}" -gt 0 ]; then
        info "installing: ${missing[*]}"
        if confirm "run sudo dnf install -y for ${#missing[@]} package(s)?"; then
          sudo_warmup
          info "dnf install ${missing[*]}"
          run_sudo dnf install -y "${missing[@]}"
          for p in "${missing[@]}"; do ok "$p"; done
        else
          warn "skipped"
        fi
      fi
      ;;
    pacman)
      pkgs=(base-devel pkgconf cmake nasm libusb hidapi alsa-lib libpulse pipewire
            xdotool gtk3 webkit2gtk-4.1 appmenu-gtk-module libappindicator-gtk3
            librsvg openssl clang lld)
      [ "$WITH_SERVO" -eq 1 ] && pkgs+=(gperf gtk3 libxcb libxkbcommon libxkbcommon-x11)
      missing=()
      for p in "${pkgs[@]}"; do
        if pacman -Q "$p" >/dev/null 2>&1; then ok "$p"; else missing+=("$p"); fi
      done
      if [ "${#missing[@]}" -gt 0 ]; then
        info "installing: ${missing[*]}"
        if confirm "run sudo pacman -S --needed for ${#missing[@]} package(s)?"; then
          sudo_warmup
          info "pacman -S ${missing[*]}"
          run_sudo pacman -S --needed --noconfirm "${missing[@]}"
          for p in "${missing[@]}"; do ok "$p"; done
        else
          warn "skipped"
        fi
      fi
      ;;
    brew)
      if ! need brew; then
        warn "Homebrew not found"
        if confirm "install Homebrew? (https://brew.sh)"; then
          /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
          eval "$(/opt/homebrew/bin/brew shellenv 2>/dev/null || /usr/local/bin/brew shellenv)"
        else
          warn "skipped — install manually before building"
        fi
      fi
      if need brew; then
        if xcode-select -p >/dev/null 2>&1; then
          ok "Xcode Command Line Tools"
        else
          warn "Xcode Command Line Tools missing — run: xcode-select --install"
        fi
        pkgs=(hidapi pkg-config cmake nasm)
        [ "$WITH_SERVO" -eq 1 ] && pkgs+=(gperf)
        for p in "${pkgs[@]}"; do
          if brew list --formula "$p" >/dev/null 2>&1; then
            ok "$p"
          else
            info "brew install $p"
            brew install "$p" >/dev/null
            ok "$p"
          fi
        done
      fi
      ;;
  esac
fi

# ─── 3. Cargo-installed dev tools ────────────────────────────────────
section "cargo tools"

# cargo-binstall installs prebuilt binaries (much faster than `cargo install`)
HAS_BINSTALL=0
if need cargo-binstall; then
  ok "cargo-binstall $(bin_version cargo-binstall)"
  HAS_BINSTALL=1
elif confirm "install cargo-binstall for fast prebuilt binary installs? (recommended)"; then
  curl -L --proto '=https' --tlsv1.2 -sSf \
    https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash
  if need cargo-binstall; then
    ok "cargo-binstall installed"
    HAS_BINSTALL=1
  else
    warn "cargo-binstall install failed — falling back to source builds"
  fi
fi

cargo_get() {
  local bin="$1" pkg="${2:-$1}"
  if need "$bin"; then
    ok "${bin} $(bin_version "$bin")"
    return
  fi
  if [ "$HAS_BINSTALL" -eq 1 ]; then
    info "cargo binstall $pkg"
    cargo binstall --no-confirm --quiet "$pkg" || cargo install --locked "$pkg"
  else
    info "cargo install $pkg (compiling from source — grab a coffee ☕)"
    cargo install --locked "$pkg"
  fi
  ok "$bin installed"
}

cargo_get just
cargo_get trunk
cargo_get cargo-deny

if need sccache; then
  ok "sccache $(bin_version sccache)"
elif confirm "install sccache for compilation caching? (highly recommended)"; then
  cargo_get sccache
fi

# ─── 4. Bun (SDK runtime) ────────────────────────────────────────────
section "bun"
if need bun; then
  ok "bun $(bun --version)"
else
  info "installing bun..."
  if [ "$DISTRO" = macos ] && need brew; then
    brew install oven-sh/bun/bun
  else
    curl -fsSL https://bun.sh/install | bash
    export PATH="$HOME/.bun/bin:$PATH"
  fi
  if need bun; then
    ok "bun $(bun --version)"
  else
    err "bun install failed — see https://bun.sh"
  fi
fi

# ─── 5. Frontend dependencies ────────────────────────────────────────
section "frontend dependencies"

info "npm ci in crates/hypercolor-ui (Tailwind v4)"
(cd "$ROOT/crates/hypercolor-ui" && npm ci --silent --no-audit --no-fund 2>&1 | tail -5) || \
  warn "hypercolor-ui npm ci failed"
ok "crates/hypercolor-ui ready"

info "bun install in sdk/"
(cd "$ROOT/sdk" && bun install --silent)
ok "sdk/ ready"

if [ -f "$ROOT/e2e/package.json" ]; then
  info "npm ci in e2e/"
  if (cd "$ROOT/e2e" && npm ci --silent --no-audit --no-fund 2>&1 | tail -3); then
    ok "e2e/ ready"
  else
    warn "e2e/ npm ci failed (non-fatal — needed for 'just e2e')"
  fi
fi

# ─── 6. Python client (optional) ─────────────────────────────────────
section "python client"
if need uv; then
  ok "uv $(bin_version uv)"
  info "uv sync in python/"
  if (cd "$ROOT/python" && uv sync --quiet); then
    ok "python/ ready"
  else
    warn "python/ uv sync failed"
  fi
else
  warn "uv not installed — needed only for 'just python-*' recipes"
  note "install with: curl -LsSf https://astral.sh/uv/install.sh | sh"
fi

# ─── Final summary ───────────────────────────────────────────────────
printf "\n%s%s    ╭───────────────────────────────────────╮%s\n" "$SUCCESS_GREEN" "$BOLD" "$RESET"
printf "%s%s    │           ✓  All set                  │%s\n" "$SUCCESS_GREEN" "$BOLD" "$RESET"
printf "%s%s    ╰───────────────────────────────────────╯%s\n" "$SUCCESS_GREEN" "$BOLD" "$RESET"
printf "\n    %sNext:%s\n" "$BOLD" "$RESET"
printf "      %s•%s %sjust verify%s          run lint + tests\n" "$ELECTRIC_PURPLE" "$RESET" "$NEON_CYAN" "$RESET"
printf "      %s•%s %sjust daemon%s          start the daemon on :9420\n" "$ELECTRIC_PURPLE" "$RESET" "$NEON_CYAN" "$RESET"
printf "      %s•%s %sjust dev%s             daemon + UI dev server together\n" "$ELECTRIC_PURPLE" "$RESET" "$NEON_CYAN" "$RESET"
printf "      %s•%s %sjust udev-install%s    install USB device rules (Linux, sudo)\n\n" "$ELECTRIC_PURPLE" "$RESET" "$NEON_CYAN" "$RESET"
