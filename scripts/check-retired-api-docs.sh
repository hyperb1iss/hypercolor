#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

# Single quotes keep Markdown backticks and regex metacharacters literal.
# shellcheck disable=SC2016
retired_routes='/api/v1/(server|status|devices/(bindings|rebind|debug/(queues|routing)|metrics)|diagnose/memory|effects/(active|stop|screenshots)|library/playlists/stop|capture/source/pick|attachments/(categories|vendors|templates/\{[^}]+\})|logical-devices|displays/[^ )`]+/preview\.jpg|system/sensors/[^ )`]+)|/(attachments/(categories|vendors|templates/\{[^}]+\})|effects/(active|stop)|scenes/(active|deactivate|[^ )`]+/zones))'
failed=0

target_manifest="$repo_root/crates/hypercolor-daemon/tests/fixtures/rest_v1/spec78-target-manifest.json"
rest_reference="$repo_root/docs/content/api/rest.md"

expected_operations="$(
  jq -r '.paths[] | .path as $path | .methods[] | "\(.) \($path)"' \
    "$target_manifest" \
    | grep -Ev '^GET /api/v1/ws$' \
    | sort -u
)"
documented_operations="$(
  grep -Eo '<api_endpoint method="[A-Z]+" path="[^"]+"' "$rest_reference" \
    | sed 's/<api_endpoint method="//; s/" path="/ /; s/"$//' \
    | sort -u
)"

missing_operations="$(
  comm -23 \
    <(printf '%s\n' "$expected_operations") \
    <(printf '%s\n' "$documented_operations")
)"
unexpected_operations="$(
  comm -13 \
    <(printf '%s\n' "$expected_operations") \
    <(printf '%s\n' "$documented_operations")
)"

if [[ -n "$missing_operations" ]]; then
  printf 'current REST operations missing from docs/content/api/rest.md:\n%s\n' \
    "$missing_operations" >&2
  failed=1
fi

if [[ -n "$unexpected_operations" ]]; then
  printf 'non-canonical REST operations in docs/content/api/rest.md:\n%s\n' \
    "$unexpected_operations" >&2
  failed=1
fi

while IFS= read -r -d '' document; do
  relative="${document#"$repo_root/"}"
  case "$relative" in
    docs/archive/* | docs/specs/78-api-resource-model.md)
      continue
      ;;
  esac

  if head -n 20 "$document" \
    | grep -Eqi 'API status:.*historical|Status:.*superseded|Historical design snapshot'; then
    continue
  fi

  matches="$(grep -En "$retired_routes" "$document" || true)"
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

echo 'REST API documentation fences: PASS'
