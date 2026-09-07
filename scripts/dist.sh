#!/usr/bin/env bash
# Build a ready-to-ship Hypercolor release bundle.
# Includes the daemon, CLI, unified desktop app, TUI launcher, UI, bundled effects/faces,
# docs, agent skills, and host integration files in one directory.
#
# Usage:
#   ./scripts/dist.sh                    # full bundle for the host platform
#   ./scripts/dist.sh --target linux-amd64
#   ./scripts/dist.sh --target x86_64-unknown-linux-gnu
#   ./scripts/dist.sh --version 0.1.0-rc.1
#   ./scripts/dist.sh --skip-effects     # reuse existing bundled effects/faces
#   ./scripts/dist.sh --skip-docs        # reuse docs or omit them from the bundle
#   ./scripts/dist.sh --ci               # use pre-built web assets from --web-assets
#   ./scripts/dist.sh --bin-dir /abs/dir # package pre-built binaries from a directory

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

SKIP_EFFECTS=0
SKIP_DOCS=0
CI_MODE=0
WEB_ASSETS_DIR=""
BIN_DIR=""
RUST_TARGET=""
RELEASE_VERSION=""
BUILD_ROOT=""
TCC_CANARY=0

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT_DIR}/target}"
CARGO_CACHE_BUILD="${ROOT_DIR}/scripts/cargo-cache-build.sh"
MACOS_SIGNING_ACTOR="${ROOT_DIR}/scripts/sign-macos-artifacts.sh"

info()  { printf '\033[38;2;128;255;234m→\033[0m %s\n' "$*"; }
ok()    { printf '\033[38;2;80;250;123m✅\033[0m %s\n' "$*"; }
warn()  { printf '\033[38;2;241;250;140m⚠\033[0m  %s\n' "$*" >&2; }
die()   { printf '\033[38;2;255;99;99m✗\033[0m %s\n' "$*" >&2; exit 1; }

cleanup() {
  if [[ -n "${BUILD_ROOT}" && -d "${BUILD_ROOT}" ]]; then
    rm -rf "${BUILD_ROOT}"
  fi
}

normalize_target() {
  case "$1" in
    linux-amd64) echo "x86_64-unknown-linux-gnu" ;;
    linux-arm64) echo "aarch64-unknown-linux-gnu" ;;
    macos-arm64) echo "aarch64-apple-darwin" ;;
    macos-amd64) echo "x86_64-apple-darwin" ;;
    *) echo "$1" ;;
  esac
}

count_files() {
  local dir="$1"
  if [[ -d "${dir}" ]]; then
    find "${dir}" -type f | wc -l | tr -d ' '
  else
    printf '0'
  fi
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

trap cleanup EXIT

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-effects)     SKIP_EFFECTS=1; shift ;;
    --skip-docs)        SKIP_DOCS=1; shift ;;
    --ci)               CI_MODE=1; shift ;;
    --web-assets)       WEB_ASSETS_DIR="$2"; shift 2 ;;
    --bin-dir)          BIN_DIR="$2"; shift 2 ;;
    --tcc-canary)       TCC_CANARY=1; shift ;;
    --target)           RUST_TARGET="$(normalize_target "$2")"; shift 2 ;;
    --version)          RELEASE_VERSION="$2"; shift 2 ;;
    -h|--help)
      cat <<'EOF'
Usage: ./scripts/dist.sh [options]

Options:
  --target <triple|alias>
                       Rust target triple or release alias (default: host)
  --version <version>   Override artifact version (default: Cargo package version)
  --skip-effects       Skip SDK effect/face compilation
  --skip-docs          Skip Zola docs compilation
  --ci                 CI mode (expect --web-assets for pre-built UI/effects)
  --web-assets <dir>   Path to pre-built web assets (ui/ + effects/)
  --bin-dir <dir>      Package pre-built binaries from <dir> instead of
                       building them (absolute path; must contain the three
                       release binaries)
  --tcc-canary         Include the signed physical TCC canary surface
  -h, --help           Show this help
EOF
      exit 0
      ;;
    *) die "unknown option: $1" ;;
  esac
done

if [[ -n "${BIN_DIR}" ]]; then
  [[ "${BIN_DIR}" == /* ]] || die "--bin-dir must be an absolute path (the script runs from the repo root): ${BIN_DIR}"
  [[ -d "${BIN_DIR}" ]] || die "--bin-dir does not exist: ${BIN_DIR}"
  MISSING_BINS=()
  for bin in hypercolor-daemon hypercolor hypercolor-app; do
    [[ -f "${BIN_DIR}/${bin}" && -x "${BIN_DIR}/${bin}" ]] || MISSING_BINS+=("${bin}")
  done
  if [[ ${#MISSING_BINS[@]} -ne 0 ]]; then
    die "--bin-dir is missing executable binaries: ${MISSING_BINS[*]}"
  fi
fi

require_cmd cargo
require_cmd jq
require_cmd tar

VERSION="${RELEASE_VERSION}"
if [[ -z "${VERSION}" ]]; then
  VERSION=$(cargo metadata --format-version 1 --no-deps \
    | jq -r '.packages[] | select(.name == "hypercolor-daemon") | .version')
fi
[[ -n "${VERSION}" ]] || die "could not determine version from Cargo.toml"

HOST_TARGET="$(rustc -vV | sed -n 's/host: //p')"
if [[ -z "${RUST_TARGET}" ]]; then
  RUST_TARGET="${HOST_TARGET}"
fi

case "${RUST_TARGET}" in
  x86_64-unknown-linux-gnu)    PLATFORM="linux-amd64" ;;
  aarch64-unknown-linux-gnu)   PLATFORM="linux-arm64" ;;
  aarch64-apple-darwin)        PLATFORM="macos-arm64" ;;
  x86_64-apple-darwin)         PLATFORM="macos-amd64" ;;
  *)                           PLATFORM="${RUST_TARGET}" ;;
esac

IS_LINUX=0
IS_MACOS=0
case "${RUST_TARGET}" in
  *linux*) IS_LINUX=1 ;;
  *apple*|*darwin*) IS_MACOS=1 ;;
esac

if [[ "${TCC_CANARY}" -eq 1 && "${IS_MACOS}" -ne 1 ]]; then
  die "--tcc-canary requires a macOS target"
fi
if [[ "${TCC_CANARY}" -eq 1 && -n "${BIN_DIR}" ]]; then
  die "--tcc-canary cannot verify pre-built binaries from --bin-dir"
fi

TARGET_FLAG=()
if [[ "${RUST_TARGET}" != "${HOST_TARGET}" ]]; then
  TARGET_FLAG=(--target "${RUST_TARGET}")
fi

RELEASE_DIR="${CARGO_TARGET_DIR}/release"
if [[ ${#TARGET_FLAG[@]} -ne 0 ]]; then
  RELEASE_DIR="${CARGO_TARGET_DIR}/${RUST_TARGET}/release"
fi
if [[ -n "${BIN_DIR}" ]]; then
  RELEASE_DIR="${BIN_DIR}"
fi

DIST_NAME="hypercolor-${VERSION}-${PLATFORM}"
DIST_DIR="${ROOT_DIR}/dist/${DIST_NAME}"
BUILD_ROOT="$(mktemp -d "${ROOT_DIR}/.dist-build.XXXXXX")"
DOCS_BUILD_DIR="${BUILD_ROOT}/docs"
SITE_BUILD_DIR=""

info "Building Hypercolor v${VERSION} for ${PLATFORM} (${RUST_TARGET})"
info "Rust artifacts will land in ${CARGO_TARGET_DIR}"

if [[ "${CI_MODE}" -eq 1 ]]; then
  [[ -n "${WEB_ASSETS_DIR}" ]] || die "--ci requires --web-assets <dir>"
  [[ -f "${WEB_ASSETS_DIR}/ui/index.html" ]] \
    || die "pre-built web assets are missing ui/index.html: ${WEB_ASSETS_DIR}"
  info "Using pre-built web assets from ${WEB_ASSETS_DIR}"
  rm -rf crates/hypercolor-ui/dist
  mkdir -p crates/hypercolor-ui/dist
  cp -R "${WEB_ASSETS_DIR}/ui/." crates/hypercolor-ui/dist/
else
  require_cmd trunk
  require_cmd bun

  info "Building web UI (Leptos/Trunk)"
  (
    cd crates/hypercolor-ui
    bun install --frozen-lockfile
    if command -v rustup >/dev/null 2>&1; then
      rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
    fi
    HYPERCOLOR_FORCE_SCCACHE=1 env -u NO_COLOR \
      "${CARGO_CACHE_BUILD}" trunk build --release --locked
  )

  if [[ "${SKIP_EFFECTS}" -eq 0 ]]; then
    info "Building bundled effects and faces"
    (
      cd sdk
      bun install --frozen-lockfile
      bun run build:effects
    )
  fi

  WEB_ASSETS_DIR=""
fi

if [[ -n "${BIN_DIR}" ]]; then
  info "Using pre-built binaries from ${BIN_DIR}"
else
  info "Building release binaries"
  # The ${arr[@]+...} guard keeps macOS bash 3.2 from treating an empty
  # array expansion as an unbound-variable error under set -u.
  #
  # Two invocations on purpose. Built together, feature unification turns
  # on hypercolor-core's servo feature for every binary, so the CLI and app
  # each fat-LTO-merge the full Servo bitcode only for the link to
  # dead-strip it again (their shipped binaries are 10-18MB; the daemon,
  # which actually uses Servo, is 144MB). Splitting keeps Servo's LTO cost
  # to the daemon: one Servo-sized merge at peak instead of four racing.
  # The daemon's merge alone still overruns a 15Gi arm64 runner into swap,
  # so this trims the peak rather than eliminating it; CI provisions swap
  # for the remainder.
  DAEMON_FEATURE_FLAG=()
  if [[ "${TCC_CANARY}" -eq 1 ]]; then
    DAEMON_FEATURE_FLAG=(--features macos-tcc-canary)
  fi
  ./scripts/cargo-cache-build.sh cargo build --release --locked \
    -p hypercolor-daemon --bin hypercolor-daemon \
    ${DAEMON_FEATURE_FLAG[@]+"${DAEMON_FEATURE_FLAG[@]}"} \
    ${TARGET_FLAG[@]+"${TARGET_FLAG[@]}"}
  ./scripts/cargo-cache-build.sh cargo build --release --locked \
    -p hypercolor-cli --bin hypercolor \
    -p hypercolor-app --bin hypercolor-app \
    ${TARGET_FLAG[@]+"${TARGET_FLAG[@]}"}
fi

if [[ "${SKIP_DOCS}" -eq 0 ]]; then
  require_cmd zola
  info "Building docs site"
  (
    cd docs
    zola build --output-dir "${DOCS_BUILD_DIR}"
  )
fi

if [[ -d site ]]; then
  require_cmd pnpm
  if [[ ! -d site/node_modules ]]; then
    info "Installing marketing site dependencies"
    (
      cd site
      pnpm install
    )
  fi
  info "Building marketing site"
  (
    cd site
    pnpm build
  )

  for candidate in out dist build; do
    if [[ -d "site/${candidate}" ]]; then
      SITE_BUILD_DIR="${ROOT_DIR}/site/${candidate}"
      break
    fi
  done

  if [[ -z "${SITE_BUILD_DIR}" ]]; then
    warn "marketing site built, but no static output directory was found"
  fi
else
  warn "site/ is not present in this checkout; skipping marketing site bundle"
fi

info "Assembling distribution at ${DIST_DIR}"
rm -rf "${DIST_DIR}"
mkdir -p "${DIST_DIR}/bin"
mkdir -p "${DIST_DIR}/share/hypercolor"/{ui,effects/bundled,docs,site,agents/skills,agents/agents}
mkdir -p "${DIST_DIR}/share"/{applications,bash-completion/completions,zsh/site-functions,fish/vendor_completions.d}
mkdir -p "${DIST_DIR}/share/icons/hicolor"/{scalable,48x48,128x128,256x256}/apps

if [[ "${IS_LINUX}" -eq 1 ]]; then
  mkdir -p "${DIST_DIR}"/{lib/systemd/user,lib/udev/rules.d,etc/modules-load.d}
fi
if [[ "${IS_MACOS}" -eq 1 ]]; then
  mkdir -p "${DIST_DIR}/share/hypercolor/launchd"
fi

install -m755 "${RELEASE_DIR}/hypercolor-daemon" "${DIST_DIR}/bin/hypercolor-daemon"
install -m755 "${RELEASE_DIR}/hypercolor" "${DIST_DIR}/bin/hypercolor"
install -m755 "${RELEASE_DIR}/hypercolor-app" "${DIST_DIR}/bin/hypercolor-app"
install -m755 packaging/bin/hypercolor-tui "${DIST_DIR}/bin/hypercolor-tui"
install -m755 packaging/bin/hypercolor-open "${DIST_DIR}/bin/hypercolor-open"

if [[ -n "${WEB_ASSETS_DIR}" ]]; then
  cp -R "${WEB_ASSETS_DIR}/ui/." "${DIST_DIR}/share/hypercolor/ui/"
else
  cp -R crates/hypercolor-ui/dist/. "${DIST_DIR}/share/hypercolor/ui/"
fi

if [[ -n "${WEB_ASSETS_DIR}" && -d "${WEB_ASSETS_DIR}/effects" ]]; then
  cp -R "${WEB_ASSETS_DIR}/effects/." "${DIST_DIR}/share/hypercolor/effects/bundled/"
elif [[ -d effects/hypercolor ]]; then
  cp -R effects/hypercolor/. "${DIST_DIR}/share/hypercolor/effects/bundled/"
else
  warn "no bundled effects/faces found — run 'just effects-build' or pass --web-assets"
fi

if [[ "${SKIP_DOCS}" -eq 0 && -d "${DOCS_BUILD_DIR}" ]]; then
  cp -R "${DOCS_BUILD_DIR}/." "${DIST_DIR}/share/hypercolor/docs/"
else
  warn "docs were skipped; the bundle will not include the generated docs site"
fi

if [[ -d .agents/skills ]]; then
  cp -R .agents/skills "${DIST_DIR}/share/hypercolor/agents/"
fi
if [[ -d .agents/agents ]]; then
  cp -R .agents/agents "${DIST_DIR}/share/hypercolor/agents/"
fi

if [[ -n "${SITE_BUILD_DIR}" ]]; then
  cp -R "${SITE_BUILD_DIR}/." "${DIST_DIR}/share/hypercolor/site/"
fi

sed "s|@BIN_DIR@|/usr/bin|g" packaging/desktop/hypercolor.desktop.in \
  > "${DIST_DIR}/share/applications/hypercolor.desktop"

# The brand mark ships as pre-rendered PNGs; the traced SVG is still TBD
# (e1bd5c14) and is bundled only once it exists.
for size in 48 128 256; do
  install -m644 "packaging/icons/hypercolor-${size}.png" \
    "${DIST_DIR}/share/icons/hicolor/${size}x${size}/apps/hypercolor.png"
done
if [[ -f packaging/icons/hypercolor.svg ]]; then
  cp packaging/icons/hypercolor.svg "${DIST_DIR}/share/icons/hicolor/scalable/apps/hypercolor.svg"
fi

if [[ ${#TARGET_FLAG[@]} -eq 0 ]]; then
  info "Generating shell completions"
  "${DIST_DIR}/bin/hypercolor" completions bash > "${DIST_DIR}/share/bash-completion/completions/hypercolor"
  "${DIST_DIR}/bin/hypercolor" completions zsh > "${DIST_DIR}/share/zsh/site-functions/_hypercolor"
  "${DIST_DIR}/bin/hypercolor" completions fish > "${DIST_DIR}/share/fish/vendor_completions.d/hypercolor.fish"
else
  warn "cross-compiling — skipping shell completion generation"
fi

if [[ "${IS_LINUX}" -eq 1 ]]; then
  cp packaging/systemd/user/hypercolor.service "${DIST_DIR}/lib/systemd/user/"
  cp packaging/systemd/user/hypercolor.service.system "${DIST_DIR}/lib/systemd/user/"
  cp udev/99-hypercolor.rules udev/70-hypercolor-input.rules "${DIST_DIR}/lib/udev/rules.d/"
  cp packaging/modules-load/i2c-dev.conf "${DIST_DIR}/etc/modules-load.d/"
fi

if [[ "${IS_MACOS}" -eq 1 ]]; then
  cp packaging/launchd/tech.hyperbliss.hypercolor.plist \
    "${DIST_DIR}/share/hypercolor/launchd/"
  info "Signing and notarizing standalone macOS artifacts"
  "${MACOS_SIGNING_ACTOR}" standalone \
    --directory "${DIST_DIR}" \
    --target "${RUST_TARGET}"
fi

cp LICENSE NOTICE README.md "${DIST_DIR}/"

DIST_DIR="${DIST_DIR}" VERSION="${VERSION}" PLATFORM="${PLATFORM}" \
RUST_TARGET="${RUST_TARGET}" python3 - <<'PY'
import hashlib
import json
import os
import stat
from pathlib import Path

root = Path(os.environ["DIST_DIR"])
members = []
for path in sorted(root.rglob("*")):
    relative = path.relative_to(root).as_posix()
    if relative == "manifest.json":
        continue
    metadata = path.lstat()
    mode = stat.S_IMODE(metadata.st_mode)
    if stat.S_ISDIR(metadata.st_mode):
        members.append({"path": relative, "type": "directory", "mode": mode})
        continue
    if not stat.S_ISREG(metadata.st_mode):
        raise SystemExit(f"release payload contains unsupported member: {relative}")
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    members.append(
        {
            "path": relative,
            "type": "file",
            "mode": mode,
            "size": metadata.st_size,
            "sha256": digest.hexdigest(),
        }
    )

def count_files(relative: str) -> int:
    directory = root / relative
    return sum(1 for path in directory.rglob("*") if path.is_file())

manifest = {
    "name": "hypercolor",
    "version": os.environ["VERSION"],
    "platform": os.environ["PLATFORM"],
    "rust_target": os.environ["RUST_TARGET"],
    "binaries": [
        "hypercolor-daemon",
        "hypercolor",
        "hypercolor-app",
        "hypercolor-tui",
        "hypercolor-open",
    ],
    "assets": {
        "ui_files": count_files("share/hypercolor/ui"),
        "bundled_effect_files": count_files("share/hypercolor/effects/bundled"),
        "docs_files": count_files("share/hypercolor/docs"),
        "skill_files": count_files("share/hypercolor/agents/skills"),
        "agent_files": count_files("share/hypercolor/agents/agents"),
        "site_files": count_files("share/hypercolor/site"),
    },
    "members": members,
}
(root / "manifest.json").write_text(
    json.dumps(manifest, indent=2, sort_keys=True) + "\n",
    encoding="utf-8",
)
PY

info "Creating tarball"
(cd dist && tar czf "${DIST_NAME}.tar.gz" "${DIST_NAME}")

TARBALL_SIZE=$(du -sh "dist/${DIST_NAME}.tar.gz" | cut -f1)
ok "Distribution ready: dist/${DIST_NAME}.tar.gz (${TARBALL_SIZE})"
