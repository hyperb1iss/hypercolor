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

  if rg "${rg_args[@]}" --regexp "$pattern" "$repo_root"; then
    echo "forbidden OSS boundary marker found: $label" >&2
    fail=1
  fi
}

if [[ "$strict" == false ]]; then
  echo "OSS boundary strict mode is staged but not enforced yet."
  echo "Run scripts/check-oss-boundary.sh --strict after cloud extraction."
  exit 0
fi

check_absent "Hypercolor Cloud prose" 'Hypercolor Cloud'
check_absent "cloud API path" '/api/v1/cloud'
check_absent "cloud crate names" 'hypercolor-(cloud-api|cloud-client|daemon-link)'
check_absent "official cloud feature" 'official-cloud'
check_absent "cloud runtime state" 'cloud_(login_sessions|connection|socket)'
check_absent "cloud endpoints" '(api|app|auth)?\.?hypercolor\.lighting'
check_absent "cloud entitlement feature key" 'hc\.cloud_sync'

exit "$fail"
