#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Match the sink identifier itself so intervening Rust comments cannot hide
# an assignment. Method-based DOM sinks follow the same prohibition.
if rg -n '\b(inner_html|set_inner_html|set_outer_html|insert_adjacent_html)\b' \
  crates/hypercolor-ui/src --glob '*.rs'; then
  echo "Raw HTML sinks are forbidden in the UI." >&2
  exit 1
else
  search_status=$?
  if [[ $search_status -ne 1 ]]; then
    exit "$search_status"
  fi
fi
