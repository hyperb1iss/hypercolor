#!/usr/bin/env bash
# Resolve the code-signing identity for local macOS bundle builds.
#
# Ad-hoc signatures carry a per-build cdhash designated requirement, so
# macOS TCC treats every rebuild as a brand-new app and drops Screen
# Recording / Input Monitoring grants. Signing dev bundles with a stable
# local certificate keeps those grants alive across rebuilds. See
# docs/development/DEV_SETUP.md for the one-time certificate setup.
#
# Resolution order:
#   1. APPLE_SIGNING_IDENTITY, when the caller already exported one.
#   2. The local "Hypercolor Dev" identity, when the keychain holds a
#      valid one.
#   3. "-" (explicit ad-hoc), with a warning on stderr.
set -euo pipefail

DEV_IDENTITY="Hypercolor Dev"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "-"
  exit 0
fi

if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  echo "${APPLE_SIGNING_IDENTITY}"
  exit 0
fi

# Sign by certificate hash rather than name: a duplicate certificate with
# the same label (easy to create by running Certificate Assistant twice)
# makes codesign reject the name as ambiguous, while the hash of the one
# valid identity stays unique.
identity_hash="$(security find-identity -v -p codesigning 2>/dev/null \
  | awk -v name="\"${DEV_IDENTITY}\"" '$0 ~ name { print $2; exit }')"
if [[ -n "${identity_hash}" ]]; then
  echo "${identity_hash}"
  exit 0
fi

echo "warning: no '${DEV_IDENTITY}' signing identity found; bundle will be ad-hoc signed and macOS permission grants will not survive rebuilds. See docs/development/DEV_SETUP.md." >&2
echo "-"
