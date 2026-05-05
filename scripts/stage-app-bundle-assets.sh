#!/usr/bin/env bash
# Stage Tauri sidecars and platform-conditional resources for hypercolor-app bundling.
#
# Static web assets are copied into target/bundle-stage so clean checkouts can
# compile the app without generated UI/effect output in the source tree. This
# script also handles artifacts that are platform-conditional or need
# triple-suffixed sidecars: built binaries and the Windows SMBus + PawnIO payloads.
#
# All staging output lives under ${target}/bundle-stage/, which is gitignored
# along with the rest of target/. The source tree never receives generated files.

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

PROFILE="release"
RUST_TARGET=""
SKIP_PAWNIO=0
PAWNIO_SETUP_VERSION="2.2.0"
PAWNIO_SETUP_SHA256="1F519A22E47187F70A1379A48CA604981C4FCF694F4E65B734AAA74A9FBA3032"
PAWNIO_MODULES_VERSION="0.2.5"
PAWNIO_MODULES_SHA256="1149B87F4DC757E72654D5A402863251815EBFC8AD4E3BB030DBCFFB3DE74153"

usage() {
  cat <<'EOF'
Usage: scripts/stage-app-bundle-assets.sh [options]

Options:
  --profile <name>  Cargo profile containing built binaries (default: release)
  --target <triple> Rust target triple (default: host)
  --skip-pawnio     Do not download/stage PawnIO payloads for Windows bundles
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
    --skip-pawnio)
      SKIP_PAWNIO=1
      shift
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

STAGE_DIR="${TARGET_DIR}/bundle-stage"
STAGE_BIN="${STAGE_DIR}/binaries"
STAGE_UI="${STAGE_DIR}/ui"
STAGE_EFFECTS="${STAGE_DIR}/effects"
STAGE_TOOLS="${STAGE_DIR}/tools"

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

stage_tool_binary() {
  local name="$1"
  local source="${PROFILE_DIR}/${name}${EXE}"
  local target="${STAGE_TOOLS}/${name}${EXE}"
  require_file "${source}"
  install -m755 "${source}" "${target}"
}

is_windows_target() {
  case "${RUST_TARGET}" in
    *windows*|*-pc-windows-*) return 0 ;;
    *) return 1 ;;
  esac
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print toupper($1)}'
  else
    shasum -a 256 "$1" | awk '{print toupper($1)}'
  fi
}

download_verified() {
  local url="$1"
  local output="$2"
  local expected="$3"

  if [[ ! -f "${output}" || "$(sha256_file "${output}")" != "${expected}" ]]; then
    curl -fsSL "${url}" -o "${output}"
  fi

  local actual
  actual="$(sha256_file "${output}")"
  if [[ "${actual}" != "${expected}" ]]; then
    echo "SHA256 mismatch for ${output}; expected ${expected}, got ${actual}" >&2
    exit 1
  fi
}

stage_pawnio_assets() {
  local dest="${STAGE_TOOLS}/pawnio"
  local cache="${ROOT_DIR}/target/pawnio"
  local extract="${cache}/modules-${PAWNIO_MODULES_VERSION}"
  local setup_url="https://github.com/namazso/PawnIO.Setup/releases/download/${PAWNIO_SETUP_VERSION}/PawnIO_setup.exe"
  local modules_url="https://github.com/namazso/PawnIO.Modules/releases/download/${PAWNIO_MODULES_VERSION}/release_${PAWNIO_MODULES_VERSION//./_}.zip"
  local setup_cache="${cache}/PawnIO_setup-${PAWNIO_SETUP_VERSION}.exe"
  local modules_cache="${cache}/PawnIO.Modules-${PAWNIO_MODULES_VERSION}.zip"
  local staged_modules=()

  mkdir -p "${cache}"
  rm -rf "${dest}" "${extract}"
  mkdir -p "${dest}/modules" "${extract}"

  download_verified "${setup_url}" "${setup_cache}" "${PAWNIO_SETUP_SHA256}"
  download_verified "${modules_url}" "${modules_cache}" "${PAWNIO_MODULES_SHA256}"

  cp "${setup_cache}" "${dest}/PawnIO_setup.exe"
  unzip -q "${modules_cache}" -d "${extract}"

  for module in SmbusI801.bin SmbusPIIX4.bin SmbusNCT6793.bin; do
    local source
    source="$(find "${extract}" -type f -name "${module}" -print -quit)"
    if [[ -z "${source}" ]]; then
      echo "PawnIO module ${module} was not found in release archive" >&2
      exit 1
    fi
    local target="${dest}/modules/${module}"
    cp "${source}" "${target}"
    staged_modules+=("${module}:$(sha256_file "${target}")")
  done

  local license
  license="$(find "${extract}" -type f -name COPYING -print -quit)"
  if [[ -n "${license}" ]]; then
    cp "${license}" "${dest}/modules/COPYING"
  fi

  {
    cat <<EOF
{
  "pawnio_setup": {
    "version": "${PAWNIO_SETUP_VERSION}",
    "url": "${setup_url}",
    "sha256": "${PAWNIO_SETUP_SHA256}"
  },
  "pawnio_modules": {
    "version": "${PAWNIO_MODULES_VERSION}",
    "url": "${modules_url}",
    "sha256": "${PAWNIO_MODULES_SHA256}",
    "installed_modules": [
      "SmbusI801.bin",
      "SmbusPIIX4.bin",
      "SmbusNCT6793.bin"
    ],
    "modules": [
EOF

    local first=1
    local entry name hash
    for entry in "${staged_modules[@]}"; do
      name="${entry%%:*}"
      hash="${entry#*:}"
      if [[ "${first}" -ne 1 ]]; then
        printf ',\n'
      fi
      printf '      {"name": "%s", "sha256": "%s"}' "${name}" "${hash}"
      first=0
    done
    printf '\n'

    cat <<EOF
    ]
  }
}
EOF
  } >"${dest}/manifest.json"
}

if [[ ! -f crates/hypercolor-ui/dist/index.html ]]; then
  echo "error: crates/hypercolor-ui/dist is missing or incomplete" >&2
  echo "build the production UI first: just ui-build" >&2
  exit 1
fi

if [[ ! -d effects/hypercolor ]] || [[ -z "$(ls -A effects/hypercolor 2>/dev/null)" ]]; then
  echo "error: effects/hypercolor is missing or empty" >&2
  echo "build bundled effects first: just effects-build" >&2
  exit 1
fi

rm -rf "${STAGE_BIN}" "${STAGE_UI}" "${STAGE_EFFECTS}" "${STAGE_TOOLS}"
mkdir -p "${STAGE_BIN}" "${STAGE_UI}" "${STAGE_EFFECTS}" "${STAGE_TOOLS}"
cp -R crates/hypercolor-ui/dist/. "${STAGE_UI}/"
cp -R effects/hypercolor/. "${STAGE_EFFECTS}/"

stage_binary hypercolor-daemon
stage_binary hypercolor

if is_windows_target; then
  stage_tool_binary hypercolor-smbus-service

  if [[ "${SKIP_PAWNIO}" -ne 1 ]]; then
    stage_pawnio_assets
  fi
fi

echo "staged hypercolor-app bundle assets for ${RUST_TARGET} (${PROFILE}) -> ${STAGE_DIR}"
