#!/usr/bin/env bash
# Re-sign the dev bundle so the daemon carries the sidecar identity the
# launcher authority verifies.
#
# Tauri signs every nested binary with a filename-derived identifier
# (hypercolor-daemon), but the app-sidecar ownership handshake requires
# the daemon's designated requirement to open with
#   identifier "tech.hyperbliss.hypercolor.sidecar"
# and share its certificate tail with the app. The release lane fixes
# identifiers up after the Tauri build the same way
# (scripts/sign-macos-artifacts.sh); this is the minimal dev-bundle
# equivalent. Re-signing the daemon breaks the outer bundle seal, so
# the app is resealed afterward. The DMG Tauri produced before this
# pass keeps the unpatched app; dev iteration launches the .app
# directly.
#
# No-op on non-macOS hosts and for ad-hoc builds, whose bare cdhash
# requirement takes the launcher authority's structural fallback
# instead.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  exit 0
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"
app_path="${1:-${repo_root}/target/release/bundle/macos/Hypercolor.app}"
app_entitlements="${repo_root}/crates/hypercolor-app/entitlements.plist"
sidecar_entitlements="${repo_root}/packaging/macos/daemon-sidecar.entitlements.plist"

identity="$("${script_dir}/macos-dev-signing-identity.sh")"
if [[ "${identity}" == "-" ]]; then
  exit 0
fi
[[ -d "${app_path}" ]] || { echo "app bundle not found: ${app_path}" >&2; exit 1; }

codesign --force --options runtime --timestamp=none \
  --identifier tech.hyperbliss.hypercolor.sidecar \
  --entitlements "${sidecar_entitlements}" \
  --sign "${identity}" \
  "${app_path}/Contents/MacOS/hypercolor-daemon"

codesign --force --options runtime --timestamp=none \
  --entitlements "${app_entitlements}" \
  --sign "${identity}" \
  "${app_path}"

codesign --verify --deep --strict "${app_path}"
echo "dev bundle resealed: daemon carries the sidecar identity"
