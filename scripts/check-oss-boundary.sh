#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mode="${1:-baseline}"

if [[ "$mode" != "baseline" && "$mode" != "--strict" && "$mode" != "strict" ]]; then
  echo "usage: scripts/check-oss-boundary.sh [--strict]" >&2
  exit 2
fi

strict=false
if [[ "$mode" == "--strict" || "$mode" == "strict" ]]; then
  strict=true
fi

rg_args=(
  --hidden
  --line-number
  --glob '!.git/**'
  --glob '!target/**'
  --glob '!effects/hypercolor/**'
  --glob '!crates/hypercolor-driver-govee/**'
  --glob '!docs/specs/49-govee-driver.md'
  --glob '!docs/design/69-oss-internal-boundary.md'
  --glob '!scripts/check-oss-boundary.sh'
)

fail=0

check_absent() {
  local label="$1"
  local pattern="$2"

  # Search the repo from inside its root with a relative path (`.`) so the
  # relative `--glob '!...'` exclusions below anchor correctly. ripgrep only
  # anchors leading-path globs against the search root when that root is
  # relative; passing an absolute `$repo_root` leaves `scripts/...`, `.git/**`,
  # and the doc excludes unanchored, which produced false positives against the
  # script's own pattern table, `.git/COMMIT_EDITMSG`, and the boundary doc.
  if (cd "$repo_root" && rg "${rg_args[@]}" --regexp "$pattern" .); then
    echo "forbidden OSS boundary marker found: $label" >&2
    fail=1
  fi
}

if [[ "$strict" == false ]]; then
  echo "OSS boundary baseline mode is advisory."
  echo "Run scripts/check-oss-boundary.sh --strict or just verify for enforcement."
  exit 0
fi

check_absent "Hypercolor Cloud prose" 'Hypercolor Cloud'
check_absent "cloud sync roadmap" 'Cloud Sync'
check_absent "cloud API path" '/api/v1/cloud'
check_absent "cloud crate names" 'hypercolor-(cloud-api|cloud-client|cloud-ui|daemon-link)'
check_absent "unprefixed cloud crate names" '\b(cloud-api|cloud-client|cloud-ui|daemon-link)\b'
check_absent "official cloud feature" 'official-cloud'
check_absent "cloud CLI command docs" 'hypercolor cloud|`cloud`[[:space:]]*\|[[:space:]]*Cloud login'
check_absent "cloud runtime state" 'cloud_(login_sessions|connection|socket)'
check_absent "cloud endpoints" 'api\.hypercolor\.lighting/v1/daemon|hypercolor\.lighting/(activate|api/auth/device)|app\.hypercolor\.lighting'
check_absent "cloud entitlement feature key" 'hc\.cloud_sync'
check_absent "commercial updater crate" 'hypercolor-updater'
check_absent "commercial update endpoints" '/v1/updates|updates\.hypercolor\.lighting'
check_absent "commercial update docs" 'entitlement-gated|proprietary cloud server|docs/design/(50-update-pipeline|52-updater-client)'

exit "$fail"
