#!/usr/bin/env bash
set -euo pipefail

EXPECTED_MAJOR=15
EXPECTED_MINOR=2

die() {
  printf 'macOS deployment target check failed: %s\n' "$*" >&2
  exit 1
}

version_matches_floor() {
  local version="$1"
  local major minor patch remainder

  [[ "${version}" =~ ^[0-9]+[.][0-9]+([.][0-9]+)?$ ]] || return 1
  IFS=. read -r major minor patch remainder <<< "${version}"
  patch="${patch:-0}"

  [[ -z "${remainder}" ]] || return 1
  (( 10#${major} == EXPECTED_MAJOR && 10#${minor} == EXPECTED_MINOR && 10#${patch} == 0 ))
}

emit_candidates() {
  local target

  for target in "$@"; do
    if [[ -d "${target}" ]]; then
      find "${target}" -type f -print0
    else
      printf '%s\0' "${target}"
    fi
  done
}

[[ "$#" -gt 0 ]] || {
  printf 'usage: scripts/verify-macos-deployment-target.sh <file-or-directory> [...]\n' >&2
  exit 2
}

for target in "$@"; do
  [[ -e "${target}" ]] || die "path does not exist: ${target}"
done

for command in awk file find xcrun; do
  command -v "${command}" >/dev/null 2>&1 || die "missing required command: ${command}"
done

macho_count=0
while IFS= read -r -d '' candidate; do
  file_kind="$(file -b "${candidate}")"
  [[ "${file_kind}" == *Mach-O* ]] || continue

  build_versions="$(xcrun vtool -show-build "${candidate}" \
    | awk '$1 == "minos" { print $2 }')"
  [[ -n "${build_versions}" ]] || die "missing LC_BUILD_VERSION minos: ${candidate}"

  while IFS= read -r minimum; do
    version_matches_floor "${minimum}" \
      || die "${candidate} has minos ${minimum}; expected 15.2"
  done <<< "${build_versions}"

  macho_count=$((macho_count + 1))
  printf 'verified macOS 15.2 deployment target: %s\n' "${candidate}"
done < <(emit_candidates "$@")

[[ "${macho_count}" -gt 0 ]] || die "no Mach-O files found"
printf 'verified %s Mach-O file(s)\n' "${macho_count}"
