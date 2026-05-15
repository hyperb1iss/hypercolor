#!/usr/bin/env bash
# Build a Debian package from an assembled Hypercolor dist directory.

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

info() { printf '\033[38;2;128;255;234m→\033[0m %s\n' "$*" >&2; }
ok() { printf '\033[38;2;80;250;123mOK\033[0m %s\n' "$*" >&2; }
die() { printf '\033[38;2;255;99;99mERROR\033[0m %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: scripts/package-deb.sh <dist-dir> [output-dir]

Build a .deb package from an already assembled Hypercolor dist directory.
The dist directory must contain manifest.json from scripts/dist.sh.
EOF
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

copy_tree() {
  local source="$1"
  local target="$2"

  [[ -d "${source}" ]] || return 0
  mkdir -p "${target}"
  cp -a "${source}/." "${target}/"
}

cleanup() {
  if [[ -n "${PACKAGE_ROOT:-}" && -d "${PACKAGE_ROOT}" ]]; then
    rm -rf "${PACKAGE_ROOT}"
  fi
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

DIST_DIR="${1:-}"
OUTPUT_DIR="${2:-}"
[[ -n "${DIST_DIR}" ]] || { usage >&2; exit 2; }

DIST_DIR="${DIST_DIR%/}"
[[ -d "${DIST_DIR}" ]] || die "dist directory not found: ${DIST_DIR}"
[[ -f "${DIST_DIR}/manifest.json" ]] || die "missing manifest.json in ${DIST_DIR}"

if [[ -z "${OUTPUT_DIR}" ]]; then
  OUTPUT_DIR="$(dirname -- "${DIST_DIR}")"
fi
mkdir -p "${OUTPUT_DIR}"

require_cmd dpkg-deb
require_cmd jq

VERSION="$(jq -r '.version // empty' "${DIST_DIR}/manifest.json")"
PLATFORM="$(jq -r '.platform // empty' "${DIST_DIR}/manifest.json")"
[[ -n "${VERSION}" ]] || die "manifest is missing version"
[[ -n "${PLATFORM}" ]] || die "manifest is missing platform"

case "${PLATFORM}" in
  linux-amd64) DEB_ARCH="amd64" ;;
  linux-arm64) DEB_ARCH="arm64" ;;
  *) die "Debian packages are only supported for Linux dist payloads, got ${PLATFORM}" ;;
esac

DEB_VERSION="${VERSION/-/\~}"
DEB_NAME="hypercolor_${DEB_VERSION}_${DEB_ARCH}.deb"
DEB_PATH="${OUTPUT_DIR%/}/${DEB_NAME}"
PACKAGE_ROOT="$(mktemp -d "${ROOT_DIR}/.deb-build.XXXXXX")"
trap cleanup EXIT

info "Packaging ${DIST_DIR} as ${DEB_NAME}"

install -Dm755 "${DIST_DIR}/bin/hypercolor-daemon" "${PACKAGE_ROOT}/usr/bin/hypercolor-daemon"
install -Dm755 "${DIST_DIR}/bin/hypercolor" "${PACKAGE_ROOT}/usr/bin/hypercolor"
install -Dm755 "${DIST_DIR}/bin/hypercolor-app" "${PACKAGE_ROOT}/usr/bin/hypercolor-app"
install -Dm755 "${DIST_DIR}/bin/hypercolor-tray" "${PACKAGE_ROOT}/usr/bin/hypercolor-tray"
install -Dm755 "${DIST_DIR}/bin/hypercolor-tui" "${PACKAGE_ROOT}/usr/bin/hypercolor-tui"
install -Dm755 "${DIST_DIR}/bin/hypercolor-open" "${PACKAGE_ROOT}/usr/bin/hypercolor-open"

copy_tree "${DIST_DIR}/share/hypercolor" "${PACKAGE_ROOT}/usr/share/hypercolor"
copy_tree "${DIST_DIR}/share/applications" "${PACKAGE_ROOT}/usr/share/applications"
copy_tree "${DIST_DIR}/share/icons" "${PACKAGE_ROOT}/usr/share/icons"
copy_tree "${DIST_DIR}/share/bash-completion" "${PACKAGE_ROOT}/usr/share/bash-completion"
copy_tree "${DIST_DIR}/share/zsh" "${PACKAGE_ROOT}/usr/share/zsh"
copy_tree "${DIST_DIR}/share/fish" "${PACKAGE_ROOT}/usr/share/fish"

if [[ -f "${DIST_DIR}/lib/systemd/user/hypercolor.service.system" ]]; then
  install -Dm644 "${DIST_DIR}/lib/systemd/user/hypercolor.service.system" \
    "${PACKAGE_ROOT}/usr/lib/systemd/user/hypercolor.service"
fi

if [[ -f "${DIST_DIR}/lib/udev/rules.d/99-hypercolor.rules" ]]; then
  install -Dm644 "${DIST_DIR}/lib/udev/rules.d/99-hypercolor.rules" \
    "${PACKAGE_ROOT}/usr/lib/udev/rules.d/99-hypercolor.rules"
fi

if [[ -f "${DIST_DIR}/etc/modules-load.d/i2c-dev.conf" ]]; then
  install -Dm644 "${DIST_DIR}/etc/modules-load.d/i2c-dev.conf" \
    "${PACKAGE_ROOT}/usr/lib/modules-load.d/i2c-dev.conf"
fi

install -Dm644 "${DIST_DIR}/LICENSE" "${PACKAGE_ROOT}/usr/share/doc/hypercolor/copyright"
install -Dm644 "${DIST_DIR}/NOTICE" "${PACKAGE_ROOT}/usr/share/doc/hypercolor/NOTICE"
install -Dm644 "${DIST_DIR}/README.md" "${PACKAGE_ROOT}/usr/share/doc/hypercolor/README.md"

INSTALLED_SIZE="$(du -sk "${PACKAGE_ROOT}" | cut -f1)"
mkdir -p "${PACKAGE_ROOT}/DEBIAN"
cat > "${PACKAGE_ROOT}/DEBIAN/control" <<EOF
Package: hypercolor
Version: ${DEB_VERSION}
Section: utils
Priority: optional
Architecture: ${DEB_ARCH}
Maintainer: Stefanie Jane <stef@hyperbliss.tech>
Installed-Size: ${INSTALLED_SIZE}
Depends: libc6, libgcc-s1, libstdc++6, libdbus-1-3, libudev1, libasound2t64 | libasound2, libpulse0, libpipewire-0.3-0t64 | libpipewire-0.3-0, libgtk-3-0t64 | libgtk-3-0, libwebkit2gtk-4.1-0, libayatana-appindicator3-1, libfontconfig1, libxcb1, libxdo3, libegl1, libssl3t64 | libssl3 | libssl1.1
Recommends: systemd, udev
Suggests: i2c-tools
Homepage: https://github.com/hyperb1iss/hypercolor
Description: Open-source RGB lighting orchestration engine
 Hypercolor orchestrates RGB lighting devices with a daemon, CLI,
 tray applet, bundled web UI, and native desktop shell.
EOF

cat > "${PACKAGE_ROOT}/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e

if command -v udevadm >/dev/null 2>&1; then
  udevadm control --reload-rules 2>/dev/null || true
  udevadm trigger --action=add --subsystem-match=hidraw 2>/dev/null || true
  udevadm trigger --action=add --subsystem-match=usb 2>/dev/null || true
fi

if command -v modprobe >/dev/null 2>&1; then
  modprobe i2c-dev 2>/dev/null || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t /usr/share/icons/hicolor 2>/dev/null || true
fi

cat <<'MSG'

To start Hypercolor as a user service:
  systemctl --user enable --now hypercolor.service

To open the web UI:
  hypercolor-open

MSG
EOF

cat > "${PACKAGE_ROOT}/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e

if command -v udevadm >/dev/null 2>&1; then
  udevadm control --reload-rules 2>/dev/null || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t /usr/share/icons/hicolor 2>/dev/null || true
fi
EOF

chmod 0755 "${PACKAGE_ROOT}/DEBIAN/postinst" "${PACKAGE_ROOT}/DEBIAN/postrm"

rm -f "${DEB_PATH}"
dpkg-deb --root-owner-group --build "${PACKAGE_ROOT}" "${DEB_PATH}" >/dev/null
ok "Debian package ready: ${DEB_PATH}"
printf '%s\n' "${DEB_PATH}"
