#!/usr/bin/env bash
# Builds the spike's bundles and leaves them in <dist>:
#   A and B are signed, notarized and stapled, under separate submissions.
#   C is signed and never submitted.
# Run under with-macos-signing.sh, which supplies the notary key file.
set -euo pipefail

repo="$(pwd)"
dist="${repo}/${1:?usage: build-bundles.sh <dist>}"
identifier="tech.hyperbliss.hypercolor.notary-spike"
salt="${GITHUB_RUN_ID:-local}"

# Reuse the release lane's helpers: everything above the command dispatch.
helpers="$(mktemp)"
sed '/^validate_manifest$/,$d' scripts/sign-macos-artifacts.sh > "${helpers}"
# shellcheck source=/dev/null
source "${helpers}"
rm -f "${helpers}"
ROOT_DIR="${repo}"

compile() {
  local stamp="$1"
  local output="$2"
  local source="${SIGNING_TMP}/main.c"
  cat > "${source}" <<C
const char build_stamp[] __attribute__((used)) = "${stamp}";
int main(void) { return 0; }
C
  xcrun --sdk macosx clang -mmacosx-version-min=15.2 -arch arm64 \
    -o "${output}" "${source}"
}

build_bundle() {
  local variant="$1"
  local app="${SIGNING_TMP}/${variant}/NotarySpike.app"
  local macos="${app}/Contents/MacOS"
  mkdir -p "${macos}"
  compile "${salt}-${variant}-app" "${macos}/notary-spike"
  compile "${salt}-${variant}-sidecar" "${macos}/notary-spike-sidecar"
  compile "${salt}-${variant}-cli" "${macos}/notary-spike-cli"
  cat > "${app}/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>notary-spike</string>
  <key>CFBundleIdentifier</key><string>${identifier}</string>
  <key>CFBundleName</key><string>NotarySpike</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.0.1</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>15.2</string>
</dict>
</plist>
PLIST
  plutil -lint "${app}/Contents/Info.plist"

  codesign_object "${macos}/notary-spike-sidecar" "${identifier}.sidecar" none
  verify_signature "${macos}/notary-spike-sidecar" "${identifier}.sidecar" none
  codesign_object "${macos}/notary-spike-cli" "${identifier}.cli" none
  verify_signature "${macos}/notary-spike-cli" "${identifier}.cli" none
  codesign_object "${app}" "${identifier}" none
  verify_signature "${app}" "${identifier}" none
}

archive() {
  local variant="$1"
  local name="$2"
  COPYFILE_DISABLE=1 tar --no-xattrs -cf "${dist}/${name}.tar" \
    -C "${SIGNING_TMP}/${variant}" NotarySpike.app
}

notarize_and_staple() {
  local variant="$1"
  local app="${SIGNING_TMP}/${variant}/NotarySpike.app"
  local zip="${SIGNING_TMP}/NotarySpike-${variant}.zip"
  ditto -c -k --keepParent "${app}" "${zip}"
  notarize "${zip}" "${dist}/${variant}-notarization.json"
  xcrun stapler staple "${app}"
  xcrun stapler validate "${app}"
}

record_cdhashes() {
  local variant="$1"
  local macos="${SIGNING_TMP}/${variant}/NotarySpike.app/Contents/MacOS"
  local path
  for path in "${macos}/notary-spike" "${macos}/notary-spike-sidecar" \
    "${macos}/notary-spike-cli"; do
    printf '%s %s %s\n' "${variant}" "$(basename "${path}")" \
      "$(codesign -dvvv "${path}" 2>&1 | sed -n 's/^CDHash=//p')"
  done
}

for command in ditto jq plutil tar xcrun; do
  require "${command}"
done
prepare_signing_identity
validate_notary_credentials
ensure_signing_tmp
mkdir -p "${dist}"

for variant in A B C; do
  build_bundle "${variant}"
done
archive A A-unstapled
archive B B-unstapled
archive C C-signed

for variant in A B; do
  ditto "${SIGNING_TMP}/${variant}" "${SIGNING_TMP}/${variant}-before"
  notarize_and_staple "${variant}"
  # Stapling must add the ticket file and touch nothing else.
  diff -r "${SIGNING_TMP}/${variant}-before" "${SIGNING_TMP}/${variant}" \
    | tee "${dist}/${variant}-staple-diff.txt" || true
  [[ "$(wc -l < "${dist}/${variant}-staple-diff.txt")" -eq 1 ]] \
    || die "stapling changed more than the ticket file"
done
archive A A-stapled
archive B B-stapled
cp "${SIGNING_TMP}/B/NotarySpike.app/Contents/CodeResources" "${dist}/B.ticket"

for variant in A B C; do
  record_cdhashes "${variant}"
done | tee "${dist}/cdhashes.txt"
jq '{id, status, name, createdDate}' "${dist}/A-notarization.json" \
  "${dist}/B-notarization.json"
wc -c "${dist}/B.ticket"
