#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

retired_routes='/api/v1/(server|status|devices/(bindings|rebind|debug/(queues|routing)|metrics)|diagnose/memory|effects/(active|stop|screenshots)|library/playlists/stop|capture/source/pick|attachments/(categories|vendors|templates/\{[^}]+\})|logical-devices|displays/[^ )`]+/preview\.jpg|system/sensors/[^ )`]+)|/(attachments/(categories|vendors|templates/\{[^}]+\})|effects/(active|stop)|scenes/(active|deactivate|[^ )`]+/zones))'
failed=0

while IFS= read -r -d '' document; do
  relative="${document#"$repo_root/"}"
  case "$relative" in
    docs/archive/* | docs/specs/78-api-resource-model.md)
      continue
      ;;
  esac

  if head -n 20 "$document" \
    | rg -qi 'API status:.*historical|Status:.*superseded|Historical design snapshot'; then
    continue
  fi

  matches="$(rg -n "$retired_routes" "$document" || true)"
  if [[ -n "$matches" ]]; then
    printf 'retired API route in current documentation: %s\n%s\n' \
      "$relative" "$matches" >&2
    failed=1
  fi
done < <(
  find \
    "$repo_root/AGENTS.md" \
    "$repo_root/.agents" \
    "$repo_root/crates" \
    "$repo_root/docs" \
    -type f -name '*.md' -print0
)

if ((failed)); then
  exit 1
fi

echo 'retired API documentation fence: PASS'
