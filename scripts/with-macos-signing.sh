#!/usr/bin/env bash
# Give one signing command a private, short-lived notarization key file.
set -euo pipefail
set +x

for name in APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY \
  APPLE_TEAM_ID APPLE_API_KEY_ID APPLE_API_ISSUER APPLE_API_KEY_CONTENT; do
  if [[ -z "${!name:-}" ]]; then
    printf 'macOS release requires %s\n' "$name" >&2
    exit 1
  fi
done
[[ "$#" -gt 0 ]] || { echo 'expected a signing command' >&2; exit 2; }

umask 077
key_dir="$(mktemp -d "${TMPDIR:-/tmp}/hypercolor-notary.XXXXXX")"
trap 'rm -rf "$key_dir"' EXIT
export APPLE_API_KEY_PATH="${key_dir}/AuthKey.p8"
printf '%s' "$APPLE_API_KEY_CONTENT" > "$APPLE_API_KEY_PATH"
unset APPLE_API_KEY_CONTENT
"$@"
