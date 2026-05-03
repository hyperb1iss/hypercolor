#!/usr/bin/env bash
# Stage Tauri sidecars and resources for hypercolor-app bundling.

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

PROFILE="release"
RUST_TARGET=""

usage() {
  cat <<'EOF'
Usage: scripts/stage-app-bundle-assets.sh [options]

Options:
  --profile <name>  Cargo profile containing built binaries (default: release)
  --target <triple> Rust target triple (default: host)
  -h, --help        Show this help
EOF
}

host_triple() {
  rustc --print host-tuple 2>/dev/null || rustc -vV | sed -n 's/^host: //p'
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile)
      PROFILE="$2"
      shift 2
      ;;
    --target)
      RUST_TARGET="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

HOST_TARGET="$(host_triple)"
RUST_TARGET="${RUST_TARGET:-${HOST_TARGET}}"

TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT_DIR}/target}"
PROFILE_DIR="${TARGET_DIR}/${PROFILE}"
if [[ "${RUST_TARGET}" != "${HOST_TARGET}" ]]; then
  PROFILE_DIR="${TARGET_DIR}/${RUST_TARGET}/${PROFILE}"
fi

case "${RUST_TARGET}" in
  *windows*|*-pc-windows-*) EXE=".exe" ;;
  *) EXE="" ;;
esac

STAGE_BIN="${ROOT_DIR}/crates/hypercolor-app/binaries"
STAGE_RES="${ROOT_DIR}/crates/hypercolor-app/resources"

require_file() {
  local path="$1"
  [[ -f "${path}" ]] || {
    echo "missing built binary: ${path}" >&2
    echo "build release binaries before staging app bundle assets" >&2
    exit 1
  }
}

stage_binary() {
  local name="$1"
  local source="${PROFILE_DIR}/${name}${EXE}"
  local target="${STAGE_BIN}/${name}-${RUST_TARGET}${EXE}"
  require_file "${source}"
  install -m755 "${source}" "${target}"
}

rm -rf "${STAGE_BIN}" "${STAGE_RES}/tools"
mkdir -p "${STAGE_BIN}" "${STAGE_RES}/ui" "${STAGE_RES}/effects/bundled" "${STAGE_RES}/tools"

stage_binary hypercolor-daemon
stage_binary hypercolor

if [[ -d crates/hypercolor-ui/dist ]]; then
  rm -rf "${STAGE_RES}/ui"
  mkdir -p "${STAGE_RES}/ui"
  cp -R crates/hypercolor-ui/dist/. "${STAGE_RES}/ui/"
else
  echo "warning: crates/hypercolor-ui/dist not found; UI resources left as-is" >&2
fi

if [[ -d effects/hypercolor ]]; then
  rm -rf "${STAGE_RES}/effects/bundled"
  mkdir -p "${STAGE_RES}/effects/bundled"
  cp -R effects/hypercolor/. "${STAGE_RES}/effects/bundled/"
else
  echo "warning: effects/hypercolor not found; bundled effects left as-is" >&2
fi

install -m644 scripts/install-windows-service.ps1 "${STAGE_RES}/tools/"
install -m644 scripts/uninstall-windows-service.ps1 "${STAGE_RES}/tools/"
install -m644 scripts/diagnose-windows.ps1 "${STAGE_RES}/tools/"

echo "staged hypercolor-app bundle assets for ${RUST_TARGET} (${PROFILE})"
