#!/usr/bin/env bash
# Verify a Hypercolor Debian package has the expected metadata and payload.

set -euo pipefail

die() { printf 'error: %s\n' "$*" >&2; exit 1; }

require_path() {
  local path="$1"
  grep -Fxq "${path}" "${CONTENTS_FILE}" || die "missing package path: ${path}"
}

DEB_PATH="${1:-}"
[[ -n "${DEB_PATH}" ]] || die "usage: scripts/verify-deb-package.sh <package.deb>"
[[ -f "${DEB_PATH}" ]] || die "package not found: ${DEB_PATH}"
command -v dpkg-deb >/dev/null 2>&1 || die "missing required command: dpkg-deb"

PACKAGE="$(dpkg-deb --field "${DEB_PATH}" Package)"
VERSION="$(dpkg-deb --field "${DEB_PATH}" Version)"
ARCH="$(dpkg-deb --field "${DEB_PATH}" Architecture)"
DEPENDS="$(dpkg-deb --field "${DEB_PATH}" Depends)"

[[ "${PACKAGE}" == "hypercolor" ]] || die "unexpected package name: ${PACKAGE}"
[[ -n "${VERSION}" ]] || die "missing package version"
case "${ARCH}" in
  amd64|arm64) ;;
  *) die "unexpected architecture: ${ARCH}" ;;
esac

for dependency in libc6 libudev1 libdbus-1-3 libgtk-3-0 libwebkit2gtk-4.1-0; do
  [[ "${DEPENDS}" == *"${dependency}"* ]] || die "missing dependency: ${dependency}"
done

CONTENTS_FILE="$(mktemp)"
trap 'rm -f "${CONTENTS_FILE}"' EXIT
dpkg-deb --contents "${DEB_PATH}" | awk '{ print $NF }' > "${CONTENTS_FILE}"

require_path "./usr/bin/hypercolor"
require_path "./usr/bin/hypercolor-daemon"
require_path "./usr/bin/hypercolor-app"
require_path "./usr/bin/hypercolor-tray"
require_path "./usr/bin/hypercolor-tui"
require_path "./usr/bin/hypercolor-open"
require_path "./usr/share/applications/hypercolor.desktop"
require_path "./usr/share/doc/hypercolor/copyright"
require_path "./usr/share/hypercolor/ui/index.html"
require_path "./usr/lib/systemd/user/hypercolor.service"
require_path "./usr/lib/udev/rules.d/99-hypercolor.rules"
require_path "./usr/lib/modules-load.d/i2c-dev.conf"

printf 'verified Debian package hypercolor_%s_%s\n' "${VERSION}" "${ARCH}"
