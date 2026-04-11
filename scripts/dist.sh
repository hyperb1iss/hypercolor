#!/usr/bin/env bash
# Build everything end-to-end and assemble a distribution tarball.
# This is THE one command for a complete release build.
#
# Usage:
#   ./scripts/dist.sh                    # release build for host platform
#   ./scripts/dist.sh --target linux-amd64
#   ./scripts/dist.sh --target x86_64-unknown-linux-gnu
#   ./scripts/dist.sh --skip-effects     # skip SDK effect build
#   ./scripts/dist.sh --ci               # CI mode: use pre-built web assets

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

# ── Defaults ─────────────────────────────────────────────────
SKIP_EFFECTS=0
CI_MODE=0
WEB_ASSETS_DIR=""  # for CI: path to pre-built UI + effects
RUST_TARGET=""     # empty = host target

info()  { printf '\033[38;2;128;255;234m→\033[0m %s\n' "$*"; }
ok()    { printf '\033[38;2;80;250;123m✅\033[0m %s\n' "$*"; }
warn()  { printf '\033[38;2;241;250;140m⚠\033[0m  %s\n' "$*" >&2; }
die()   { printf '\033[38;2;255;99;99m✗\033[0m %s\n' "$*" >&2; exit 1; }

normalize_target() {
  case "$1" in
    linux-amd64) echo "x86_64-unknown-linux-gnu" ;;
    linux-arm64) echo "aarch64-unknown-linux-gnu" ;;
    macos-arm64) echo "aarch64-apple-darwin" ;;
    macos-amd64) echo "x86_64-apple-darwin" ;;
    *) echo "$1" ;;
  esac
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-effects)     SKIP_EFFECTS=1; shift ;;
    --ci)               CI_MODE=1; shift ;;
    --web-assets)       WEB_ASSETS_DIR="$2"; shift 2 ;;
    --target)           RUST_TARGET="$(normalize_target "$2")"; shift 2 ;;
    -h|--help)
      cat <<'EOF'
Usage: ./scripts/dist.sh [options]

Options:
  --target <triple|alias>
                       Rust target triple or release alias (default: host)
  --skip-effects       Skip SDK effect compilation
  --ci                 CI mode (expect --web-assets for pre-built UI/effects)
  --web-assets <dir>   Path to pre-built web assets (ui/ + effects/)
  -h, --help           Show this help
EOF
      exit 0
      ;;
    *) die "unknown option: $1" ;;
  esac
done

# ── Version & Target ─────────────────────────────────────────
VERSION=$(cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | jq -r '.packages[] | select(.name == "hypercolor-daemon") | .version')
[[ -n "${VERSION}" ]] || die "could not determine version from Cargo.toml"

if [[ -z "${RUST_TARGET}" ]]; then
  RUST_TARGET=$(rustc -vV | sed -n 's/host: //p')
fi

# Map rust target to friendly name for tarball
case "${RUST_TARGET}" in
  x86_64-unknown-linux-gnu)    PLATFORM="linux-amd64" ;;
  aarch64-unknown-linux-gnu)   PLATFORM="linux-arm64" ;;
  aarch64-apple-darwin)        PLATFORM="macos-arm64" ;;
  x86_64-apple-darwin)         PLATFORM="macos-amd64" ;;
  *)                           PLATFORM="${RUST_TARGET}" ;;
esac

IS_LINUX=0; IS_MACOS=0
case "${RUST_TARGET}" in
  *linux*) IS_LINUX=1 ;;
  *apple*|*darwin*) IS_MACOS=1 ;;
esac

DIST_NAME="hypercolor-${VERSION}-${PLATFORM}"
DIST_DIR="${ROOT_DIR}/dist/${DIST_NAME}"

info "Building Hypercolor v${VERSION} for ${PLATFORM} (${RUST_TARGET})"

# ── Phase 1: Rust Binaries ───────────────────────────────────
TARGET_FLAG=()
if [[ "${RUST_TARGET}" != "$(rustc -vV | sed -n 's/host: //p')" ]]; then
  TARGET_FLAG=(--target "${RUST_TARGET}")
fi

RELEASE_DIR="target/${RUST_TARGET}/release"
if [[ ${#TARGET_FLAG[@]} -eq 0 ]]; then
  RELEASE_DIR="target/release"
fi

info "Building daemon (with Servo)"
cargo build --release -p hypercolor-daemon --features servo "${TARGET_FLAG[@]}"

info "Building CLI"
cargo build --release -p hypercolor-cli "${TARGET_FLAG[@]}"

info "Building TUI"
cargo build --release -p hypercolor-tui "${TARGET_FLAG[@]}"

# Tray applet — Linux needs GTK, macOS uses native AppKit
info "Building tray applet"
cargo build --release -p hypercolor-tray "${TARGET_FLAG[@]}"

# ── Phase 2: Web Assets (platform-independent) ──────────────
if [[ "${CI_MODE}" -eq 1 && -n "${WEB_ASSETS_DIR}" ]]; then
  info "Using pre-built web assets from ${WEB_ASSETS_DIR}"
else
  # Build UI
  info "Building web UI (Leptos/Trunk)"
  (
    cd crates/hypercolor-ui
    if [[ ! -d node_modules ]]; then
      npm install
    fi
    if command -v rustup &>/dev/null; then
      rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
    fi
    env -u NO_COLOR trunk build --release
  )

  # Build effects
  if [[ "${SKIP_EFFECTS}" -eq 0 ]]; then
    info "Building SDK effects"
    (
      cd sdk
      if [[ ! -d node_modules ]]; then
        bun install
      fi
      bun run build:effects
    )
  fi

  WEB_ASSETS_DIR=""  # use local paths
fi

# ── Phase 3: Assemble Distribution ──────────────────────────
info "Assembling distribution at ${DIST_DIR}"
rm -rf "${DIST_DIR}"
mkdir -p "${DIST_DIR}"/{bin,share/hypercolor/{ui,effects/bundled,overlay-templates}}
mkdir -p "${DIST_DIR}"/share/{applications,bash-completion/completions,zsh/site-functions}
mkdir -p "${DIST_DIR}"/share/icons/hicolor/{scalable,48x48,128x128,256x256}/apps

if [[ "${IS_LINUX}" -eq 1 ]]; then
  mkdir -p "${DIST_DIR}"/{lib/systemd/user,lib/udev/rules.d,etc/modules-load.d}
fi
if [[ "${IS_MACOS}" -eq 1 ]]; then
  mkdir -p "${DIST_DIR}"/share/hypercolor/launchd
fi
mkdir -p "${DIST_DIR}"/share/fish/vendor_completions.d

# Binaries
for bin in hypercolor hyper hypercolor-tray hypercolor-tui; do
  cp "${RELEASE_DIR}/${bin}" "${DIST_DIR}/bin/"
done
cp packaging/bin/hypercolor-open "${DIST_DIR}/bin/"
chmod 755 "${DIST_DIR}/bin/"*

# Web UI
if [[ -n "${WEB_ASSETS_DIR}" ]]; then
  cp -R "${WEB_ASSETS_DIR}/ui/." "${DIST_DIR}/share/hypercolor/ui/"
else
  cp -R crates/hypercolor-ui/dist/. "${DIST_DIR}/share/hypercolor/ui/"
fi

# Effects
if [[ -n "${WEB_ASSETS_DIR}" && -d "${WEB_ASSETS_DIR}/effects" ]]; then
  cp -R "${WEB_ASSETS_DIR}/effects/." "${DIST_DIR}/share/hypercolor/effects/bundled/"
elif [[ -d effects/hypercolor ]]; then
  cp -R effects/hypercolor/. "${DIST_DIR}/share/hypercolor/effects/bundled/"
else
  warn "No built effects found — run 'just effects-build' or pass --web-assets"
fi

# Overlay templates
if [[ -d assets/overlay-templates ]]; then
  cp -R assets/overlay-templates/. "${DIST_DIR}/share/hypercolor/overlay-templates/"
else
  warn "No overlay templates found at assets/overlay-templates"
fi

# Desktop entry
sed "s|@BIN_DIR@|/usr/bin|g" packaging/desktop/hypercolor.desktop.in \
  > "${DIST_DIR}/share/applications/hypercolor.desktop"

# Icons
cp packaging/icons/hypercolor.svg "${DIST_DIR}/share/icons/hicolor/scalable/apps/hypercolor.svg"
if command -v rsvg-convert &>/dev/null; then
  for size in 48 128 256; do
    rsvg-convert -w "${size}" -h "${size}" packaging/icons/hypercolor.svg \
      -o "${DIST_DIR}/share/icons/hicolor/${size}x${size}/apps/hypercolor.png"
  done
else
  warn "rsvg-convert not found — skipping PNG icon generation"
fi

# Shell completions (only works when building for host platform)
if [[ ${#TARGET_FLAG[@]} -eq 0 ]]; then
  info "Generating shell completions"
  "${DIST_DIR}/bin/hyper" completions bash > "${DIST_DIR}/share/bash-completion/completions/hyper" 2>/dev/null || true
  "${DIST_DIR}/bin/hyper" completions zsh  > "${DIST_DIR}/share/zsh/site-functions/_hyper" 2>/dev/null || true
  "${DIST_DIR}/bin/hyper" completions fish > "${DIST_DIR}/share/fish/vendor_completions.d/hyper.fish" 2>/dev/null || true
else
  warn "Cross-compiling — skipping shell completion generation"
fi

# System integration (Linux)
if [[ "${IS_LINUX}" -eq 1 ]]; then
  cp packaging/systemd/user/hypercolor.service "${DIST_DIR}/lib/systemd/user/"
  cp udev/99-hypercolor.rules "${DIST_DIR}/lib/udev/rules.d/"
  cp packaging/modules-load/i2c-dev.conf "${DIST_DIR}/etc/modules-load.d/"
fi

# macOS integration
if [[ "${IS_MACOS}" -eq 1 ]]; then
  cp packaging/launchd/tech.hyperbliss.hypercolor.plist \
    "${DIST_DIR}/share/hypercolor/launchd/"
fi

# License
cp LICENSE "${DIST_DIR}/"

# ── Phase 4: Create Tarball ─────────────────────────────────
info "Creating tarball"
(cd dist && tar czf "${DIST_NAME}.tar.gz" "${DIST_NAME}")

TARBALL_SIZE=$(du -sh "dist/${DIST_NAME}.tar.gz" | cut -f1)
ok "Distribution ready: dist/${DIST_NAME}.tar.gz (${TARBALL_SIZE})"
