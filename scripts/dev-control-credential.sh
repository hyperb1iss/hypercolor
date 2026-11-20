#!/usr/bin/env bash
# Print the protected-control credential a dev stack hands its daemon.
#
# Audio, input, and screen capture settings only change for a session that
# holds this credential, loopback included. A dev stack has no app shell
# to mint one, so the recipe passes this value to the daemon and prints a
# UI link that carries it as the URL fragment.
#
# A credential already in HYPERCOLOR_PROTECTED_CONTROL_CREDENTIAL is kept,
# so one exported value gives a link that survives restarts.
set -euo pipefail

credential="${HYPERCOLOR_PROTECTED_CONTROL_CREDENTIAL:-}"
if [[ -z "${credential}" ]]; then
  credential="hc_pc_$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
fi
if [[ ! "${credential}" =~ ^hc_pc_[0-9a-f]{64}$ ]]; then
  echo "HYPERCOLOR_PROTECTED_CONTROL_CREDENTIAL must be a canonical hc_pc_ token." >&2
  exit 1
fi
printf '%s\n' "${credential}"
