#!/usr/bin/env bash
# Build the Hypercolor macOS desktop bundle.
#
# Mirrors scripts/build-windows-installer.ps1 in shape: verify prereqs, build
# UI + effects + sidecars, stage assets, then build the hypercolor-app crate.
# The default produces an unsigned development app. Release-ready builds route
# signing, notarization, and separate DMG creation through the signing actor.
#
# Signing + notarization activate automatically when the relevant env vars are
# present. To produce a release-ready artifact locally:
#
#   APPLE_SIGNING_IDENTITY="Developer ID Application: Stefanie Jane (TEAMID)" \
#   APPLE_TEAM_ID="TEAMID" \
#   APPLE_API_KEY_ID="KEYID" \
#   APPLE_API_ISSUER="issuer-uuid" \
#   APPLE_API_KEY_PATH="${HOME}/private_keys/AuthKey_KEYID.p8" \
#   scripts/build-mac-installer.sh --notarize
#
# Without those env vars the script produces an unsigned development app.

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
SKIP_UI=0
SKIP_EFFECTS=0
NOTARIZE=0
CHECK_ONLY=0
TCC_CANARY=0

CARGO_CACHE_BUILD="${ROOT_DIR}/scripts/cargo-cache-build.sh"
STAGE_ASSETS="${ROOT_DIR}/scripts/stage-app-bundle-assets.sh"
SIGNING_ACTOR="${ROOT_DIR}/scripts/sign-macos-artifacts.sh"

usage() {
  cat <<'EOF'
Usage: scripts/build-mac-installer.sh [options]

Options:
  --profile <preview|release>  Cargo build profile (default: release)
  --target <triple>            Rust target triple (default: host arch)
  --skip-ui                    Reuse existing UI build output
  --skip-effects               Reuse existing effects build output
  --notarize                   Produce signed, notarized app and DMG artifacts
  --tcc-canary                 Include the signed physical TCC canary surface
  --check-only                 Verify prerequisites and exit
  -h, --help                   Show this help

Release signing is driven by APPLE_SIGNING_IDENTITY. Notarization additionally
needs APPLE_API_KEY_ID + APPLE_API_ISSUER + APPLE_API_KEY_PATH, or a
preconfigured APPLE_NOTARY_KEYCHAIN_PROFILE. Raw Apple ID passwords are not
accepted.
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
    --skip-ui)     SKIP_UI=1; shift ;;
    --skip-effects) SKIP_EFFECTS=1; shift ;;
    --notarize)    NOTARIZE=1; shift ;;
    --tcc-canary)  TCC_CANARY=1; shift ;;
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
if [[ "${TCC_CANARY}" -eq 1 && "${NOTARIZE}" -ne 1 ]]; then
  die "--tcc-canary requires --notarize"
fi

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
    [[ "${NOTARIZE}" -eq 1 ]] \
      || die "APPLE_SIGNING_IDENTITY requires --notarize for manifest-driven signing"
  else
    warn "APPLE_SIGNING_IDENTITY not set; app will be unsigned"
  fi

  if [[ "${NOTARIZE}" -eq 1 ]]; then
    [[ "${PROFILE}" == "release" ]] || die "--notarize requires the release profile"
    require jq "install with: brew install jq"
    [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]] || die "--notarize requires APPLE_SIGNING_IDENTITY"
    if [[ -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" || -n "${APPLE_ID:-}" ]]; then
      die "raw Apple ID credentials are unsupported; use a notarytool keychain profile"
    elif [[ -n "${APPLE_API_KEY_ID:-}" && -n "${APPLE_API_ISSUER:-}" && -n "${APPLE_API_KEY_PATH:-}" ]]; then
      info "notarization will use App Store Connect API key ${APPLE_API_KEY_ID}"
    elif [[ -n "${APPLE_NOTARY_KEYCHAIN_PROFILE:-}" ]]; then
      info "notarization will use keychain profile ${APPLE_NOTARY_KEYCHAIN_PROFILE}"
    else
      die "--notarize needs the App Store Connect API key trio or APPLE_NOTARY_KEYCHAIN_PROFILE"
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
    --bundles app
    --no-sign
  )
  if [[ -n "${TARGET}" ]]; then
    args+=(--target "${TARGET}")
  fi
  step "Build unsigned Tauri macOS app"
  (
    cd "${ROOT_DIR}/crates/hypercolor-app"
    HYPERCOLOR_FORCE_SCCACHE=1 "${CARGO_CACHE_BUILD}" cargo "${args[@]}"
  )
}

build_ui_bundle() {
  cd "${ROOT_DIR}/crates/hypercolor-ui"
  HYPERCOLOR_FORCE_SCCACHE=1 env -u NO_COLOR \
    "${CARGO_CACHE_BUILD}" trunk build --release --locked
}

resolve_target_dir() {
  local base="${CARGO_TARGET_DIR:-${ROOT_DIR}/target}"
  if [[ -n "${TARGET}" ]]; then
    printf '%s/%s/%s' "${base}" "${TARGET}" "${PROFILE}"
  else
    printf '%s/%s' "${base}" "${PROFILE}"
  fi
}

show_artifacts() {
  step "Artifacts"
  local profile_dir
  profile_dir="$(resolve_target_dir)"
  find "${profile_dir}/bundle/dmg" -maxdepth 1 -type f -name '*.dmg' -print 2>/dev/null || true
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
  run_step "Install UI dependencies" bun install --frozen-lockfile \
    --cwd "${ROOT_DIR}/crates/hypercolor-ui"
  run_step "Build production UI" build_ui_bundle
fi

if [[ "${SKIP_EFFECTS}" -ne 1 ]]; then
  run_step "Install SDK dependencies" bun install --cwd "${ROOT_DIR}/sdk"
  run_step "Build bundled effects" bash -c "cd '${ROOT_DIR}/sdk' && bun run build:effects"
fi

daemon_features="servo"
if [[ "${TCC_CANARY}" -eq 1 ]]; then
  daemon_features="${daemon_features},macos-tcc-canary"
fi
build_cargo "Build daemon sidecar (with servo)" \
  -p hypercolor-daemon --features "${daemon_features}"
build_cargo "Build CLI sidecar" -p hypercolor-cli

stage_assets
if [[ "${NOTARIZE}" -eq 1 ]]; then
  signing_target="${TARGET}"
  if [[ -z "${signing_target}" ]]; then
    signing_target="$(rustc --print host-tuple 2>/dev/null || rustc -vV | sed -n 's/^host: //p')"
  fi
  case "${signing_target}" in
    aarch64-apple-darwin) signing_arch="arm64" ;;
    x86_64-apple-darwin) signing_arch="x86_64" ;;
    *) die "unsupported macOS signing target: ${signing_target}" ;;
  esac
  signing_version="$(cargo metadata --format-version 1 --no-deps \
    | jq -r '.packages[] | select(.name == "hypercolor-app") | .version')"
  run_step "Sign, notarize, and package macOS artifacts" \
    "${SIGNING_ACTOR}" app \
    --target "${signing_target}" \
    --version "${signing_version}" \
    --arch "${signing_arch}"
else
  build_tauri_bundle
fi

show_artifacts
ok "macOS bundle build complete"
