#!/usr/bin/env bash
# Build the Hypercolor macOS desktop bundle (.app + .dmg).
#
# Mirrors scripts/build-windows-installer.ps1 in shape: verify prereqs, build
# UI + effects + sidecars, stage assets, then run `cargo tauri build` against
# the hypercolor-app crate. By default the build is unsigned and unnotarized
# so the script Just Works on a fresh dev Mac.
#
# Signing + notarization activate automatically when the relevant env vars are
# present. To produce a release-ready artifact locally:
#
#   APPLE_SIGNING_IDENTITY="Developer ID Application: Stefanie Jane (TEAMID)" \
#   APPLE_ID="stef@hyperbliss.tech" \
#   APPLE_TEAM_ID="TEAMID" \
#   APPLE_APP_SPECIFIC_PASSWORD="xxxx-xxxx-xxxx-xxxx" \
#   scripts/build-mac-installer.sh --notarize
#
# Without those env vars the script still produces a fully usable DMG that
# Gatekeeper will warn on but the developer can right-click → Open to launch.

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

# Default Cargo artifacts to the workspace target tree. The Tauri bundle config
# references staged inputs through workspace-relative target/bundle-stage paths,
# while explicit CARGO_TARGET_DIR overrides remain supported for CI or one-off
# build isolation.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT_DIR}/target}"

PROFILE="release"
TARGET=""
BUNDLES="dmg,app"
SKIP_UI=0
SKIP_EFFECTS=0
NOTARIZE=0
CHECK_ONLY=0

CARGO_CACHE_BUILD="${ROOT_DIR}/scripts/cargo-cache-build.sh"
STAGE_ASSETS="${ROOT_DIR}/scripts/stage-app-bundle-assets.sh"

usage() {
  cat <<'EOF'
Usage: scripts/build-mac-installer.sh [options]

Options:
  --profile <preview|release>  Cargo build profile (default: release)
  --target <triple>            Rust target triple (default: host arch)
  --bundles <list>             Tauri bundle targets (default: dmg,app)
  --skip-ui                    Reuse existing UI build output
  --skip-effects               Reuse existing effects build output
  --notarize                   Submit DMG to Apple notary after build
  --check-only                 Verify prerequisites and exit
  -h, --help                   Show this help

Signing is driven entirely by APPLE_SIGNING_IDENTITY; if it is unset the
output is an unsigned bundle. Notarization additionally needs APPLE_ID,
APPLE_TEAM_ID, and APPLE_APP_SPECIFIC_PASSWORD (or APPLE_API_KEY_ID +
APPLE_API_ISSUER + APPLE_API_KEY_PATH for App Store Connect keys).
EOF
}

info()  { printf '\033[38;2;128;255;234m→\033[0m %s\n' "$*"; }
step()  { printf '\n\033[38;2;225;53;255m==>\033[0m %s\n' "$*"; }
ok()    { printf '\033[38;2;80;250;123m✅\033[0m %s\n' "$*"; }
warn()  { printf '\033[38;2;241;250;140m⚠\033[0m  %s\n' "$*" >&2; }
die()   { printf '\033[38;2;255;99;99m✗\033[0m %s\n' "$*" >&2; exit 1; }

require() {
  command -v "$1" >/dev/null 2>&1 || die "missing '$1' on PATH${2:+; $2}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile)     PROFILE="$2"; shift 2 ;;
    --target)      TARGET="$2"; shift 2 ;;
    --bundles)     BUNDLES="$2"; shift 2 ;;
    --skip-ui)     SKIP_UI=1; shift ;;
    --skip-effects) SKIP_EFFECTS=1; shift ;;
    --notarize)    NOTARIZE=1; shift ;;
    --check-only)  CHECK_ONLY=1; shift ;;
    -h|--help)     usage; exit 0 ;;
    *)             usage >&2; die "unknown option: $1" ;;
  esac
done

case "${PROFILE}" in
  preview|release) ;;
  *) die "profile must be 'preview' or 'release', got '${PROFILE}'" ;;
esac

[[ "$(uname -s)" == "Darwin" ]] || die "this script only runs on macOS"

assert_prerequisites() {
  require cargo "install Rust from https://rustup.rs/"
  require rustc "install Rust from https://rustup.rs/"
  require bun "install Bun from https://bun.sh/"
  require trunk "install with: cargo install trunk --locked"
  require xcrun "ships with the Xcode Command Line Tools (xcode-select --install)"

  if ! cargo tauri --version >/dev/null 2>&1; then
    die "missing cargo-tauri; install with: cargo install tauri-cli --version '^2.0.0' --locked"
  fi
  info "cargo-tauri: $(cargo tauri --version 2>/dev/null | head -1)"

  if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
    info "signing with identity: ${APPLE_SIGNING_IDENTITY}"
  else
    warn "APPLE_SIGNING_IDENTITY not set; bundle will be unsigned"
  fi

  if [[ "${NOTARIZE}" -eq 1 ]]; then
    [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]] || die "--notarize requires APPLE_SIGNING_IDENTITY"
    if [[ -n "${APPLE_API_KEY_ID:-}" && -n "${APPLE_API_ISSUER:-}" && -n "${APPLE_API_KEY_PATH:-}" ]]; then
      info "notarization will use App Store Connect API key ${APPLE_API_KEY_ID}"
    elif [[ -n "${APPLE_ID:-}" && -n "${APPLE_TEAM_ID:-}" && -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" ]]; then
      info "notarization will use Apple ID ${APPLE_ID}"
    else
      die "--notarize needs APPLE_ID + APPLE_TEAM_ID + APPLE_APP_SPECIFIC_PASSWORD, or the API key trio (APPLE_API_KEY_ID, APPLE_API_ISSUER, APPLE_API_KEY_PATH)"
    fi
  fi
}

run_step() {
  local desc="$1"; shift
  step "${desc}"
  "$@"
}

build_cargo() {
  local desc="$1"; shift
  local args=(cargo build --locked --profile "${PROFILE}")
  if [[ -n "${TARGET}" ]]; then
    args+=(--target "${TARGET}")
  fi
  args+=("$@")
  run_step "${desc}" "${CARGO_CACHE_BUILD}" "${args[@]}"
}

stage_assets() {
  local args=(--profile "${PROFILE}" --skip-pawnio)
  if [[ -n "${TARGET}" ]]; then
    args+=(--target "${TARGET}")
  fi
  run_step "Stage app bundle assets" "${STAGE_ASSETS}" "${args[@]}"
}

build_tauri_bundle() {
  local args=(
    tauri build
    --config tauri.bundle.conf.json
    --bundles "${BUNDLES}"
  )
  if [[ -n "${TARGET}" ]]; then
    args+=(--target "${TARGET}")
  fi
  if [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
    args+=(--no-sign)
  fi
  step "Build Tauri macOS bundle"
  (cd "${ROOT_DIR}/crates/hypercolor-app" && cargo "${args[@]}")
}

resolve_target_dir() {
  local base="${CARGO_TARGET_DIR:-${ROOT_DIR}/target}"
  if [[ -n "${TARGET}" ]]; then
    printf '%s/%s/%s' "${base}" "${TARGET}" "${PROFILE}"
  else
    printf '%s/%s' "${base}" "${PROFILE}"
  fi
}

find_dmg() {
  local profile_dir="$1"
  local candidates=(
    "${profile_dir}/bundle/dmg"
    "${ROOT_DIR}/crates/hypercolor-app/target/${PROFILE}/bundle/dmg"
  )
  local d
  for d in "${candidates[@]}"; do
    if [[ -d "${d}" ]]; then
      find "${d}" -maxdepth 1 -type f -name "*.dmg" -print
    fi
  done
}

notarize_dmg() {
  local dmg="$1"

  step "Submit ${dmg##*/} to Apple notary"
  local submit_args=(notarytool submit "${dmg}" --wait --timeout 30m)
  if [[ -n "${APPLE_API_KEY_ID:-}" ]]; then
    submit_args+=(--key "${APPLE_API_KEY_PATH}" --key-id "${APPLE_API_KEY_ID}" --issuer "${APPLE_API_ISSUER}")
  else
    submit_args+=(--apple-id "${APPLE_ID}" --team-id "${APPLE_TEAM_ID}" --password "${APPLE_APP_SPECIFIC_PASSWORD}")
  fi
  xcrun "${submit_args[@]}"

  step "Staple notarization ticket"
  xcrun stapler staple "${dmg}"

  step "Verify notarization"
  xcrun stapler validate "${dmg}"
  spctl --assess --type install --verbose "${dmg}" || warn "spctl assess returned non-zero (preview spctl rules are flaky locally — verify on a clean Mac)"
}

show_artifacts() {
  step "Artifacts"
  local profile_dir
  profile_dir="$(resolve_target_dir)"
  local dmgs
  dmgs="$(find_dmg "${profile_dir}")"
  if [[ -n "${dmgs}" ]]; then
    printf '%s\n' "${dmgs}"
  else
    warn "no DMG produced under ${profile_dir}/bundle/dmg"
  fi
  local app
  app="$(find "${profile_dir}/bundle/macos" -maxdepth 1 -type d -name "*.app" 2>/dev/null | head -1)"
  if [[ -n "${app}" ]]; then
    printf '%s\n' "${app}"
  fi
}

assert_prerequisites

if [[ "${CHECK_ONLY}" -eq 1 ]]; then
  ok "prerequisites check complete"
  exit 0
fi

if [[ "${SKIP_UI}" -ne 1 ]]; then
  run_step "Install UI dependencies" bun install --cwd "${ROOT_DIR}/crates/hypercolor-ui"
  run_step "Build production UI" bash -c "cd '${ROOT_DIR}/crates/hypercolor-ui' && trunk build --release"
fi

if [[ "${SKIP_EFFECTS}" -ne 1 ]]; then
  run_step "Install SDK dependencies" bun install --cwd "${ROOT_DIR}/sdk"
  run_step "Build bundled effects" bash -c "cd '${ROOT_DIR}/sdk' && bun run build:effects"
fi

build_cargo "Build daemon sidecar (with servo)" -p hypercolor-daemon --features servo
build_cargo "Build CLI sidecar" -p hypercolor-cli

stage_assets
build_tauri_bundle

if [[ "${NOTARIZE}" -eq 1 ]]; then
  profile_dir="$(resolve_target_dir)"
  mapfile -t dmgs < <(find_dmg "${profile_dir}")
  if [[ "${#dmgs[@]}" -eq 0 ]]; then
    die "--notarize requested but no DMG was produced"
  fi
  for dmg in "${dmgs[@]}"; do
    notarize_dmg "${dmg}"
  done
fi

show_artifacts
ok "macOS bundle build complete"
