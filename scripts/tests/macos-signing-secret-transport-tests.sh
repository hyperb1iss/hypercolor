#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
TEST_TMP="$(mktemp -d "${TMPDIR:-/tmp}/hypercolor-signing-transport.XXXXXX")"
HELPER="${TEST_TMP}/macos-signing-keychain"
PROCESS_LOG="${TEST_TMP}/processes.log"
STOP_FILE="${TEST_TMP}/stop"
POLL_PID=""
KEYCHAINS=()

cleanup() {
  if [[ -n "${POLL_PID}" ]]; then
    kill "${POLL_PID}" >/dev/null 2>&1 || true
    wait "${POLL_PID}" >/dev/null 2>&1 || true
  fi
  local keychain
  for keychain in "${KEYCHAINS[@]}"; do
    security delete-keychain "${keychain}" >/dev/null 2>&1 || true
  done
  case "${TEST_TMP}" in
    "${TMPDIR:-/tmp}"/hypercolor-signing-transport.*) rm -rf "${TEST_TMP}" ;;
    *) printf 'refusing to remove unexpected test directory: %s\n' "${TEST_TMP}" >&2 ;;
  esac
}
trap cleanup EXIT

xcrun --sdk macosx clang \
  -std=c17 -Wall -Wextra -Werror -Wno-deprecated-declarations \
  -mmacosx-version-min=15.2 \
  -framework Security -framework CoreFoundation \
  "${ROOT_DIR}/scripts/macos-signing-keychain.c" \
  -o "${HELPER}"

certificate_password="$(openssl rand -hex 32)"
keychain_password="$(openssl rand -hex 32)"
openssl req -x509 -newkey rsa:2048 \
  -keyout "${TEST_TMP}/key.pem" \
  -out "${TEST_TMP}/certificate.pem" \
  -nodes -days 1 \
  -subj '/CN=Hypercolor Signing Transport Test' \
  -addext 'keyUsage=digitalSignature' \
  -addext 'extendedKeyUsage=codeSigning' >/dev/null 2>&1
printf '%s\n' "${certificate_password}" \
  | openssl pkcs12 -export \
      -inkey "${TEST_TMP}/key.pem" \
      -in "${TEST_TMP}/certificate.pem" \
      -out "${TEST_TMP}/identity.p12" \
      -keypbe PBE-SHA1-3DES \
      -certpbe PBE-SHA1-3DES \
      -macalg sha1 \
      -passout stdin >/dev/null 2>&1

: > "${PROCESS_LOG}"
poll_processes() {
  while [[ ! -e "${STOP_FILE}" ]]; do
    ps -A -o command= >> "${PROCESS_LOG}"
  done
}
poll_processes &
POLL_PID=$!

for index in {1..16}; do
  keychain="${TEST_TMP}/test-${index}.keychain-db"
  KEYCHAINS+=("${keychain}")
  printf '%s\0%s\0' "${keychain_password}" "${certificate_password}" \
    | "${HELPER}" "${keychain}" "${TEST_TMP}/identity.p12"
  security find-key -s -t private "${keychain}" >/dev/null
done

: > "${STOP_FILE}"
wait "${POLL_PID}"
POLL_PID=""

while IFS= read -r command; do
  if [[ "${command}" == *"${certificate_password}"* ]]; then
    printf 'certificate password appeared in process arguments: %s\n' "${command}" >&2
    exit 1
  fi
  if [[ "${command}" == *"${keychain_password}"* ]]; then
    printf 'keychain password appeared in process arguments: %s\n' "${command}" >&2
    exit 1
  fi
done < "${PROCESS_LOG}"

export APPLE_CERTIFICATE_PASSWORD="${certificate_password}"
export APPLE_APP_SPECIFIC_PASSWORD="${keychain_password}"
trace_output="$(bash -x "${ROOT_DIR}/scripts/sign-macos-artifacts.sh" validate-manifest 2>&1)"
unset APPLE_CERTIFICATE_PASSWORD APPLE_APP_SPECIFIC_PASSWORD
if [[ "${trace_output}" == *"${certificate_password}"* ]]; then
  printf 'certificate password appeared in xtrace output\n' >&2
  exit 1
fi
if [[ "${trace_output}" == *"${keychain_password}"* ]]; then
  printf 'keychain password appeared in xtrace output\n' >&2
  exit 1
fi

certificate_password=""
keychain_password=""
printf 'macOS signing secret transport: PASS\n'
