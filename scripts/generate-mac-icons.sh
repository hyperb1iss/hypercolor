#!/usr/bin/env bash
# Generate the macOS icon ladder (PNG sizes + .icns) for hypercolor-app from
# packaging/icons/hypercolor.svg. Run on macOS; depends on qlmanage (Quick Look
# preview tool, ships with macOS), sips (built-in image utility), and iconutil
# (ships with the Xcode CLT).
#
# qlmanage renders SVGs through WebKit so gradient stops, opacity, and complex
# stroke fills come out the way the source intends. ImageMagick's bundled
# rasterizer drops gradient references to flat black on Macs that lack the
# librsvg delegate.
#
# Output lands in crates/hypercolor-app/icons/. Re-run after editing the SVG.
# Artifacts are committed so contributors without Quick Look tooling can build.

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

SRC_SVG="packaging/icons/hypercolor.svg"
DEST_DIR="crates/hypercolor-app/icons"
WORK_DIR="$(mktemp -d -t hypercolor-icons.XXXXXX)"
ICONSET_DIR="${WORK_DIR}/icon.iconset"

trap 'rm -rf "${WORK_DIR}"' EXIT

info()  { printf '\033[38;2;128;255;234m→\033[0m %s\n' "$*"; }
ok()    { printf '\033[38;2;80;250;123m✅\033[0m %s\n' "$*"; }
die()   { printf '\033[38;2;255;99;99m✗\033[0m %s\n' "$*" >&2; exit 1; }

require() {
  command -v "$1" >/dev/null 2>&1 || die "missing '$1' on PATH${2:+; $2}"
}

[[ -f "${SRC_SVG}" ]] || die "source SVG not found at ${SRC_SVG}"
require qlmanage "ships with macOS; run on a Mac, not in Linux CI"
require sips "ships with macOS"
require iconutil "ships with the Xcode Command Line Tools"

mkdir -p "${ICONSET_DIR}" "${DEST_DIR}"

# The marketing SVG embeds a "HYPERCOLOR" wordmark that we deliberately drop
# from the rasterized icon: it is illegible below 128px and macOS HIG advises
# against text inside dock icons. The Finder/Dock label already names the app.
RENDER_SVG="${WORK_DIR}/hypercolor-no-text.svg"
sed -E '/<text[^>]*>.*<\/text>/d' "${SRC_SVG}" > "${RENDER_SVG}"

# Render once at the largest size, then downscale. qlmanage emits a single
# thumbnail per invocation, so doing it once amortizes WebKit startup cost.
info "rasterizing ${SRC_SVG} at 1024px via Quick Look"
qlmanage -t -s 1024 -o "${WORK_DIR}" "${RENDER_SVG}" >/dev/null 2>&1
MASTER_PNG="${WORK_DIR}/$(basename "${RENDER_SVG}").png"
[[ -f "${MASTER_PNG}" ]] || die "qlmanage did not produce ${MASTER_PNG}"

# Apple's iconset naming convention: icon_<point>x<point>[@2x].png. The 2x
# variant carries double the pixel count for retina displays.
declare -a APPLE_SIZES=(
  "16x16:16"
  "16x16@2x:32"
  "32x32:32"
  "32x32@2x:64"
  "128x128:128"
  "128x128@2x:256"
  "256x256:256"
  "256x256@2x:512"
  "512x512:512"
  "512x512@2x:1024"
)

info "downscaling to iconset members"
for entry in "${APPLE_SIZES[@]}"; do
  name="${entry%%:*}"
  px="${entry##*:}"
  out="${ICONSET_DIR}/icon_${name}.png"
  sips -z "${px}" "${px}" "${MASTER_PNG}" --out "${out}" >/dev/null
done

# Tauri's bundle config consumes individual PNGs plus the .icns directly. Mirror
# the conventional Tauri naming so the manifest stays declarative.
info "publishing icon files to ${DEST_DIR}"
cp "${ICONSET_DIR}/icon_32x32.png"     "${DEST_DIR}/32x32.png"
cp "${ICONSET_DIR}/icon_128x128.png"   "${DEST_DIR}/128x128.png"
cp "${ICONSET_DIR}/icon_128x128@2x.png" "${DEST_DIR}/128x128@2x.png"
cp "${MASTER_PNG}"                      "${DEST_DIR}/icon.png"

info "assembling icon.icns"
iconutil --convert icns --output "${DEST_DIR}/icon.icns" "${ICONSET_DIR}"

ok "wrote macOS icon ladder + icon.icns into ${DEST_DIR}"
