#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
actual="$(mktemp)"
trap 'rm -f "$actual"' EXIT

cd "$repo_root"
rg --count-matches 'inner_html\s*=' crates/hypercolor-ui/src --glob '*.rs' \
  | sort >"$actual" || true

if [[ -s "$actual" ]]; then
  cat "$actual" >&2
  echo "Raw HTML sinks are forbidden in the UI." >&2
  exit 1
fi
