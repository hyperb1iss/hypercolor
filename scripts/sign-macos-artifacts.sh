#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
MANIFEST="${ROOT_DIR}/packaging/macos/signing-manifest.tsv"
APP_ENTITLEMENTS="crates/hypercolor-app/entitlements.plist"
DAEMON_ENTITLEMENTS="packaging/macos/daemon.entitlements.plist"
SIGNING_TMP=""
SIGNING_KEYCHAIN=""
KEYCHAIN_LIST_CHANGED=0
ORIGINAL_KEYCHAINS=()

die() {
  printf 'macOS signing failed: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: scripts/sign-macos-artifacts.sh <command> [options]

Commands:
  validate-manifest
  app --target <triple> --version <version> --arch <arm64|x86_64> [--ci]
  standalone --directory <distribution> --target <triple>

The app command pre-signs the staged daemon sidecar, builds only the Tauri
app bundle, reapplies every manifest signature, notarizes and staples the app,
then creates, signs, notarizes, and staples a separate DMG.

Signing requires APPLE_SIGNING_IDENTITY and APPLE_TEAM_ID. The identity may
already be installed, or APPLE_CERTIFICATE and APPLE_CERTIFICATE_PASSWORD may
provide a base64-encoded PKCS#12 certificate. Notarization accepts either the
APPLE_ID, APPLE_TEAM_ID, APPLE_APP_SPECIFIC_PASSWORD trio or the
APPLE_API_KEY_ID, APPLE_API_ISSUER, APPLE_API_KEY_PATH trio.
EOF
}

cleanup() {
  if [[ "${KEYCHAIN_LIST_CHANGED}" -eq 1 ]]; then
    security list-keychains -d user -s "${ORIGINAL_KEYCHAINS[@]}" >/dev/null
  fi
  if [[ -n "${SIGNING_KEYCHAIN}" && -f "${SIGNING_KEYCHAIN}" ]]; then
    security delete-keychain "${SIGNING_KEYCHAIN}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${SIGNING_TMP}" && -d "${SIGNING_TMP}" ]]; then
    rm -rf "${SIGNING_TMP}"
  fi
}
trap cleanup EXIT

require() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

manifest_has() {
  local wanted_scope="$1"
  local wanted_path="$2"
  local wanted_identifier="$3"
  local scope relative_path identifier entitlements

  while IFS=$'\t' read -r scope relative_path identifier entitlements; do
    [[ -n "${scope}" && "${scope}" != \#* ]] || continue
    if [[ "${scope}" == "${wanted_scope}" && "${relative_path}" == "${wanted_path}" && "${identifier}" == "${wanted_identifier}" ]]; then
      return 0
    fi
  done < "${MANIFEST}"
  return 1
}

validate_manifest() {
  [[ -s "${MANIFEST}" ]] || die "missing signing manifest: ${MANIFEST}"

  local seen
  seen="$(mktemp)"
  local count=0
  local scope relative_path identifier entitlements extra
  while IFS=$'\t' read -r scope relative_path identifier entitlements extra; do
    [[ -n "${scope}" && "${scope}" != \#* ]] || continue
    [[ -z "${extra:-}" ]] || die "manifest entry has more than four fields: ${scope}/${relative_path}"
    case "${scope}" in
      app|standalone) ;;
      *) die "invalid manifest scope: ${scope}" ;;
    esac
    [[ -n "${relative_path}" && "${relative_path}" != /* && "${relative_path}" != *..* ]] \
      || die "invalid manifest path: ${relative_path}"
    [[ "${identifier}" == tech.hyperbliss.hypercolor* ]] \
      || die "invalid signing identifier: ${identifier}"
    if [[ "${entitlements}" != "none" ]]; then
      [[ -s "${ROOT_DIR}/${entitlements}" ]] \
        || die "missing entitlements file: ${entitlements}"
    fi
    if grep -Fqx "${scope}"$'\t'"${relative_path}" "${seen}"; then
      die "duplicate manifest path: ${scope}/${relative_path}"
    fi
    printf '%s\t%s\n' "${scope}" "${relative_path}" >> "${seen}"
    count=$((count + 1))
  done < "${MANIFEST}"

  [[ "${count}" -eq 7 ]] || die "expected 7 signing manifest entries, found ${count}"
  manifest_has app 'Contents/MacOS/Hypercolor' 'tech.hyperbliss.hypercolor' \
    || die "manifest is missing the app identity"
  manifest_has app 'Contents/MacOS/hypercolor-daemon-{target}' 'tech.hyperbliss.hypercolor.sidecar' \
    || die "manifest is missing the daemon sidecar identity"
  manifest_has standalone 'bin/hypercolor-daemon' 'tech.hyperbliss.hypercolor.daemon' \
    || die "manifest is missing the standalone daemon identity"
  manifest_has standalone 'bin/hypercolor' 'tech.hyperbliss.hypercolor.cli' \
    || die "manifest is missing the standalone CLI identity"
  manifest_has standalone 'bin/hypercolor-app' 'tech.hyperbliss.hypercolor.app-host' \
    || die "manifest is missing the standalone app host identity"
  manifest_has standalone 'bin/hypercolor-tray' 'tech.hyperbliss.hypercolor.tray' \
    || die "manifest is missing the standalone tray identity"
  cmp -s "${ROOT_DIR}/${APP_ENTITLEMENTS}" "${ROOT_DIR}/${DAEMON_ENTITLEMENTS}" \
    || die "daemon entitlements diverge from the app profile"
}

ensure_signing_tmp() {
  if [[ -z "${SIGNING_TMP}" ]]; then
    SIGNING_TMP="$(mktemp -d)"
  fi
}

decode_certificate() {
  local output="$1"
  if printf '%s' "${APPLE_CERTIFICATE}" | base64 -D > "${output}" 2>/dev/null; then
    return
  fi
  printf '%s' "${APPLE_CERTIFICATE}" | base64 --decode > "${output}" 2>/dev/null \
    || die "APPLE_CERTIFICATE is not valid base64"
}

prepare_signing_identity() {
  require codesign
  require security
  [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]] \
    || die "APPLE_SIGNING_IDENTITY is required"
  [[ "${APPLE_SIGNING_IDENTITY}" != "-" ]] \
    || die "ad-hoc signing identities are forbidden"
  [[ -n "${APPLE_TEAM_ID:-}" ]] || die "APPLE_TEAM_ID is required"

  if security find-identity -v -p codesigning \
    | grep -F "${APPLE_SIGNING_IDENTITY}" >/dev/null; then
    return
  fi

  [[ -n "${APPLE_CERTIFICATE:-}" ]] \
    || die "signing identity is not installed and APPLE_CERTIFICATE is missing"
  [[ -n "${APPLE_CERTIFICATE_PASSWORD:-}" ]] \
    || die "APPLE_CERTIFICATE_PASSWORD is required"

  ensure_signing_tmp
  local certificate="${SIGNING_TMP}/certificate.p12"
  local keychain_password
  keychain_password="$(uuidgen)"
  SIGNING_KEYCHAIN="${SIGNING_TMP}/hypercolor-signing.keychain-db"
  decode_certificate "${certificate}"

  security create-keychain -p "${keychain_password}" "${SIGNING_KEYCHAIN}" >/dev/null
  security set-keychain-settings -lut 21600 "${SIGNING_KEYCHAIN}"
  security unlock-keychain -p "${keychain_password}" "${SIGNING_KEYCHAIN}"
  security import "${certificate}" -k "${SIGNING_KEYCHAIN}" \
    -P "${APPLE_CERTIFICATE_PASSWORD}" -T /usr/bin/codesign -T /usr/bin/security >/dev/null
  security set-key-partition-list -S apple-tool:,apple: -s \
    -k "${keychain_password}" "${SIGNING_KEYCHAIN}" >/dev/null

  local keychain
  while IFS= read -r keychain; do
    keychain="${keychain#*\"}"
    keychain="${keychain%\"*}"
    [[ -n "${keychain}" ]] && ORIGINAL_KEYCHAINS+=("${keychain}")
  done < <(security list-keychains -d user)
  security list-keychains -d user -s "${SIGNING_KEYCHAIN}" \
    "${ORIGINAL_KEYCHAINS[@]}" >/dev/null
  KEYCHAIN_LIST_CHANGED=1

  security find-identity -v -p codesigning "${SIGNING_KEYCHAIN}" \
    | grep -F "${APPLE_SIGNING_IDENTITY}" >/dev/null \
    || die "imported certificate does not provide APPLE_SIGNING_IDENTITY"
}

validate_notary_credentials() {
  if [[ -n "${APPLE_API_KEY_ID:-}" || -n "${APPLE_API_ISSUER:-}" || -n "${APPLE_API_KEY_PATH:-}" ]]; then
    [[ -n "${APPLE_API_KEY_ID:-}" && -n "${APPLE_API_ISSUER:-}" && -s "${APPLE_API_KEY_PATH:-}" ]] \
      || die "notarization requires the complete App Store Connect API key trio"
    return
  fi
  [[ -n "${APPLE_ID:-}" && -n "${APPLE_TEAM_ID:-}" && -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" ]] \
    || die "notarization requires Apple ID credentials or an App Store Connect API key"
}

resolve_rule() {
  local wanted_scope="$1"
  local wanted_path="$2"
  local target="$3"
  local scope relative_path identifier entitlements expanded_path
  local matches=0

  RULE_IDENTIFIER=""
  RULE_ENTITLEMENTS=""
  while IFS=$'\t' read -r scope relative_path identifier entitlements; do
    [[ -n "${scope}" && "${scope}" != \#* ]] || continue
    expanded_path="${relative_path//\{target\}/${target}}"
    if [[ "${scope}" == "${wanted_scope}" && "${expanded_path}" == "${wanted_path}" ]]; then
      RULE_IDENTIFIER="${identifier}"
      RULE_ENTITLEMENTS="${entitlements}"
      matches=$((matches + 1))
    fi
  done < "${MANIFEST}"
  [[ "${matches}" -eq 1 ]] \
    || die "${wanted_scope}/${wanted_path} matched ${matches} signing manifest entries"
}

codesign_object() {
  local path="$1"
  local identifier="$2"
  local entitlements="$3"
  local args=(
    --force
    --sign "${APPLE_SIGNING_IDENTITY}"
    --identifier "${identifier}"
    --options runtime
    --timestamp
  )
  if [[ "${entitlements}" != "none" ]]; then
    args+=(--entitlements "${ROOT_DIR}/${entitlements}")
  fi
  if [[ -n "${SIGNING_KEYCHAIN}" ]]; then
    args+=(--keychain "${SIGNING_KEYCHAIN}")
  fi
  codesign "${args[@]}" "${path}"
}

signature_metadata() {
  codesign -d --verbose=4 "$1" 2>&1
}

signature_requirement() {
  codesign -d -r- "$1" 2>&1 | sed -n 's/^designated => /designated => /p'
}

normalize_entitlements() {
  plutil -convert json -o - "$1" | jq -S .
}

verify_signature() {
  local path="$1"
  local identifier="$2"
  local entitlements="$3"
  local metadata requirement actual_entitlements expected_normalized actual_normalized

  codesign --verify --strict --verbose=2 "${path}"
  metadata="$(signature_metadata "${path}")"
  grep -F "Identifier=${identifier}" <<< "${metadata}" >/dev/null \
    || die "identifier mismatch for ${path}"
  grep -F "TeamIdentifier=${APPLE_TEAM_ID}" <<< "${metadata}" >/dev/null \
    || die "team identifier mismatch for ${path}"
  grep -F 'flags=0x10000(runtime)' <<< "${metadata}" >/dev/null \
    || die "hardened runtime is missing for ${path}"
  grep -F 'Timestamp=' <<< "${metadata}" >/dev/null \
    || die "secure timestamp is missing for ${path}"

  requirement="$(signature_requirement "${path}")"
  grep -F "identifier \"${identifier}\"" <<< "${requirement}" >/dev/null \
    || die "designated requirement identifier mismatch for ${path}"
  grep -F 'anchor apple generic' <<< "${requirement}" >/dev/null \
    || die "designated requirement anchor mismatch for ${path}"
  grep -F "certificate leaf[subject.OU] = \"${APPLE_TEAM_ID}\"" <<< "${requirement}" >/dev/null \
    || die "designated requirement team mismatch for ${path}"

  ensure_signing_tmp
  actual_entitlements="${SIGNING_TMP}/actual-entitlements.plist"
  : > "${actual_entitlements}"
  codesign -d --entitlements :- "${path}" > "${actual_entitlements}" 2>/dev/null || true
  if [[ "${entitlements}" == "none" ]]; then
    [[ ! -s "${actual_entitlements}" ]] \
      || die "unexpected entitlements on ${path}"
  else
    expected_normalized="$(normalize_entitlements "${ROOT_DIR}/${entitlements}")"
    actual_normalized="$(normalize_entitlements "${actual_entitlements}")"
    [[ "${actual_normalized}" == "${expected_normalized}" ]] \
      || die "entitlements mismatch for ${path}"
  fi
}

is_macho() {
  file -b "$1" | grep -F 'Mach-O' >/dev/null
}

assert_scope_files() {
  local scope_root="$1"
  local wanted_scope="$2"
  local target="$3"
  local scope relative_path identifier entitlements expanded_path
  while IFS=$'\t' read -r scope relative_path identifier entitlements; do
    [[ "${scope}" == "${wanted_scope}" ]] || continue
    expanded_path="${relative_path//\{target\}/${target}}"
    [[ -f "${scope_root}/${expanded_path}" ]] \
      || die "manifest object is missing: ${wanted_scope}/${expanded_path}"
    is_macho "${scope_root}/${expanded_path}" \
      || die "manifest object is not Mach-O: ${wanted_scope}/${expanded_path}"
  done < "${MANIFEST}"
}

sign_scope() {
  local scope_root="$1"
  local scope="$2"
  local target="$3"
  local app_main="${scope_root}/Contents/MacOS/Hypercolor"
  local macho_count=0
  local path relative_path

  assert_scope_files "${scope_root}" "${scope}" "${target}"
  while IFS= read -r -d '' path; do
    is_macho "${path}" || continue
    relative_path="${path#"${scope_root}/"}"
    resolve_rule "${scope}" "${relative_path}" "${target}"
    macho_count=$((macho_count + 1))
    if [[ "${scope}" == "app" && "${path}" == "${app_main}" ]]; then
      continue
    fi
    codesign_object "${path}" "${RULE_IDENTIFIER}" "${RULE_ENTITLEMENTS}"
    verify_signature "${path}" "${RULE_IDENTIFIER}" "${RULE_ENTITLEMENTS}"
  done < <(find "${scope_root}" -type f -print0)
  [[ "${macho_count}" -gt 0 ]] || die "no Mach-O objects found in ${scope_root}"

  if [[ "${scope}" == "app" ]]; then
    resolve_rule app 'Contents/MacOS/Hypercolor' "${target}"
    codesign_object "${scope_root}" "${RULE_IDENTIFIER}" "${RULE_ENTITLEMENTS}"
    verify_signature "${scope_root}" "${RULE_IDENTIFIER}" "${RULE_ENTITLEMENTS}"
  fi

  while IFS= read -r -d '' path; do
    is_macho "${path}" || continue
    relative_path="${path#"${scope_root}/"}"
    resolve_rule "${scope}" "${relative_path}" "${target}"
    verify_signature "${path}" "${RULE_IDENTIFIER}" "${RULE_ENTITLEMENTS}"
  done < <(find "${scope_root}" -type f -print0)
}

verify_scope() {
  local scope_root="$1"
  local scope="$2"
  local target="$3"
  local path relative_path

  assert_scope_files "${scope_root}" "${scope}" "${target}"
  if [[ "${scope}" == "app" ]]; then
    resolve_rule app 'Contents/MacOS/Hypercolor' "${target}"
    verify_signature "${scope_root}" "${RULE_IDENTIFIER}" "${RULE_ENTITLEMENTS}"
  fi
  while IFS= read -r -d '' path; do
    is_macho "${path}" || continue
    relative_path="${path#"${scope_root}/"}"
    resolve_rule "${scope}" "${relative_path}" "${target}"
    verify_signature "${path}" "${RULE_IDENTIFIER}" "${RULE_ENTITLEMENTS}"
  done < <(find "${scope_root}" -type f -print0)
}

notarize() {
  local submission="$1"
  local receipt="$2"
  if [[ -n "${APPLE_API_KEY_ID:-}" ]]; then
    xcrun notarytool submit "${submission}" --wait --output-format json \
      --key "${APPLE_API_KEY_PATH}" --key-id "${APPLE_API_KEY_ID}" \
      --issuer "${APPLE_API_ISSUER}" > "${receipt}"
  else
    xcrun notarytool submit "${submission}" --wait --output-format json \
      --apple-id "${APPLE_ID}" --team-id "${APPLE_TEAM_ID}" \
      --password "${APPLE_APP_SPECIFIC_PASSWORD}" > "${receipt}"
  fi
  jq -e '.status == "Accepted"' "${receipt}" >/dev/null \
    || die "Apple notarization did not accept ${submission}"
}

write_object_inventory() {
  local scope_root="$1"
  local scope="$2"
  local target="$3"
  local output="$4"
  local records
  records="$(mktemp)"
  local path relative_path requirement
  while IFS= read -r -d '' path; do
    is_macho "${path}" || continue
    relative_path="${path#"${scope_root}/"}"
    resolve_rule "${scope}" "${relative_path}" "${target}"
    requirement="$(signature_requirement "${path}")"
    jq -n \
      --arg path "${relative_path}" \
      --arg identifier "${RULE_IDENTIFIER}" \
      --arg requirement "${requirement}" \
      '{path: $path, identifier: $identifier, designated_requirement: $requirement}' \
      >> "${records}"
  done < <(find "${scope_root}" -type f -print0)
  jq -s . "${records}" > "${output}"
}

sign_dmg() {
  local dmg="$1"
  local args=(--force --sign "${APPLE_SIGNING_IDENTITY}" --timestamp)
  if [[ -n "${SIGNING_KEYCHAIN}" ]]; then
    args+=(--keychain "${SIGNING_KEYCHAIN}")
  fi
  codesign "${args[@]}" "${dmg}"
  codesign --verify --strict --verbose=2 "${dmg}"
  local metadata
  metadata="$(signature_metadata "${dmg}")"
  grep -F "TeamIdentifier=${APPLE_TEAM_ID}" <<< "${metadata}" >/dev/null \
    || die "team identifier mismatch for ${dmg}"
  grep -F 'Timestamp=' <<< "${metadata}" >/dev/null \
    || die "secure timestamp is missing for ${dmg}"
}

build_app_artifacts() {
  local target="$1"
  local version="$2"
  local arch="$3"
  local ci="$4"

  prepare_signing_identity
  validate_notary_credentials
  for command in cargo ditto file find hdiutil jq plutil sed xcrun; do
    require "${command}"
  done

  local staged_sidecar="${ROOT_DIR}/target/bundle-stage/binaries/hypercolor-daemon-${target}"
  resolve_rule app "Contents/MacOS/hypercolor-daemon-${target}" "${target}"
  [[ -f "${staged_sidecar}" ]] || die "staged daemon sidecar is missing: ${staged_sidecar}"
  codesign_object "${staged_sidecar}" "${RULE_IDENTIFIER}" "${RULE_ENTITLEMENTS}"
  verify_signature "${staged_sidecar}" "${RULE_IDENTIFIER}" "${RULE_ENTITLEMENTS}"

  local tauri_args=(tauri build --bundles app --config tauri.bundle.conf.json --target "${target}")
  [[ "${ci}" -eq 1 ]] && tauri_args+=(--ci)
  (
    cd "${ROOT_DIR}/crates/hypercolor-app"
    cargo "${tauri_args[@]}"
  )

  local target_dir profile_dir app dmg_dir dmg app_zip app_receipt dmg_receipt inventory
  target_dir="$(
    cd "${ROOT_DIR}/crates/hypercolor-app"
    cargo metadata --format-version 1 --no-deps | jq -r '.target_directory'
  )"
  profile_dir="${target_dir}/${target}/release"
  app="${profile_dir}/bundle/macos/Hypercolor.app"
  dmg_dir="${profile_dir}/bundle/dmg"
  dmg="${dmg_dir}/Hypercolor-${version}-${arch}.dmg"
  [[ -d "${app}" ]] || die "Tauri app bundle is missing: ${app}"

  sign_scope "${app}" app "${target}"
  ensure_signing_tmp
  app_zip="${SIGNING_TMP}/Hypercolor-app.zip"
  app_receipt="${SIGNING_TMP}/app-notarization.json"
  dmg_receipt="${SIGNING_TMP}/dmg-notarization.json"
  inventory="${SIGNING_TMP}/app-signing-inventory.json"
  ditto -c -k --keepParent "${app}" "${app_zip}"
  notarize "${app_zip}" "${app_receipt}"
  xcrun stapler staple "${app}"
  xcrun stapler validate "${app}"
  verify_scope "${app}" app "${target}"
  write_object_inventory "${app}" app "${target}" "${inventory}"

  local dmg_stage="${SIGNING_TMP}/dmg-stage"
  mkdir -p "${dmg_stage}"
  ditto "${app}" "${dmg_stage}/Hypercolor.app"
  ln -s /Applications "${dmg_stage}/Applications"
  mkdir -p "${dmg_dir}"
  rm -f "${dmg}"
  hdiutil create -volname Hypercolor -srcfolder "${dmg_stage}" \
    -ov -format UDZO "${dmg}" >/dev/null
  sign_dmg "${dmg}"
  notarize "${dmg}" "${dmg_receipt}"
  xcrun stapler staple "${dmg}"
  xcrun stapler validate "${dmg}"

  jq -n \
    --arg team_id "${APPLE_TEAM_ID}" \
    --arg target "${target}" \
    --slurpfile objects "${inventory}" \
    --slurpfile app_notarization "${app_receipt}" \
    --slurpfile dmg_notarization "${dmg_receipt}" \
    '{team_id: $team_id, target: $target, objects: $objects[0], app_notarization: $app_notarization[0], dmg_notarization: $dmg_notarization[0]}' \
    > "${dmg}.notarization.json"

  printf 'signed app: %s\n' "${app}"
  printf 'signed DMG: %s\n' "${dmg}"
}

sign_standalone_artifacts() {
  local directory="$1"
  local target="$2"
  [[ -d "${directory}" ]] || die "standalone distribution is missing: ${directory}"
  prepare_signing_identity
  validate_notary_credentials
  for command in ditto file find jq plutil xcrun; do
    require "${command}"
  done

  sign_scope "${directory}" standalone "${target}"
  ensure_signing_tmp
  local archive="${SIGNING_TMP}/standalone.zip"
  local receipt="${SIGNING_TMP}/standalone-notarization.json"
  local inventory="${SIGNING_TMP}/standalone-signing-inventory.json"
  local provenance="${directory}/share/hypercolor/macos-notarization.json"
  write_object_inventory "${directory}" standalone "${target}" "${inventory}"
  ditto -c -k --keepParent "${directory}" "${archive}"
  notarize "${archive}" "${receipt}"
  mkdir -p "$(dirname -- "${provenance}")"
  jq -n \
    --arg team_id "${APPLE_TEAM_ID}" \
    --arg target "${target}" \
    --slurpfile objects "${inventory}" \
    --slurpfile notarization "${receipt}" \
    '{team_id: $team_id, target: $target, objects: $objects[0], notarization: $notarization[0]}' \
    > "${provenance}"
  printf 'signed standalone distribution: %s\n' "${directory}"
}

validate_manifest

command_name="${1:-}"
[[ -n "${command_name}" ]] || {
  usage >&2
  exit 2
}
shift

case "${command_name}" in
  validate-manifest)
    [[ "$#" -eq 0 ]] || die "validate-manifest takes no arguments"
    printf 'validated macOS signing manifest\n'
    ;;
  app)
    target=""
    version=""
    arch=""
    ci=0
    while [[ "$#" -gt 0 ]]; do
      case "$1" in
        --target) target="$2"; shift 2 ;;
        --version) version="$2"; shift 2 ;;
        --arch) arch="$2"; shift 2 ;;
        --ci) ci=1; shift ;;
        *) die "unknown app option: $1" ;;
      esac
    done
    [[ "${target}" == *-apple-darwin ]] || die "app target must be an Apple Darwin triple"
    [[ "${version}" =~ ^[0-9]+[.][0-9]+[.][0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$ ]] \
      || die "app version must be semver"
    case "${arch}" in
      arm64|x86_64) ;;
      *) die "app architecture must be arm64 or x86_64" ;;
    esac
    case "${target}:${arch}" in
      aarch64-apple-darwin:arm64|x86_64-apple-darwin:x86_64) ;;
      *) die "app architecture does not match target ${target}" ;;
    esac
    build_app_artifacts "${target}" "${version}" "${arch}" "${ci}"
    ;;
  standalone)
    directory=""
    target=""
    while [[ "$#" -gt 0 ]]; do
      case "$1" in
        --directory) directory="$2"; shift 2 ;;
        --target) target="$2"; shift 2 ;;
        *) die "unknown standalone option: $1" ;;
      esac
    done
    [[ "${target}" == *-apple-darwin ]] \
      || die "standalone target must be an Apple Darwin triple"
    [[ -n "${directory}" ]] || die "standalone directory is required"
    sign_standalone_artifacts "${directory}" "${target}"
    ;;
  -h|--help|help)
    usage
    ;;
  *)
    usage >&2
    die "unknown command: ${command_name}"
    ;;
esac
