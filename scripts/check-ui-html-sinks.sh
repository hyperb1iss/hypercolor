#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
expected="$(mktemp)"
actual="$(mktemp)"
trap 'rm -f "$expected" "$actual"' EXIT

cat >"$expected" <<'EOF'
crates/hypercolor-ui/src/components/attachment_panel.rs:1
crates/hypercolor-ui/src/components/component_picker.rs:1
crates/hypercolor-ui/src/components/device_card.rs:1
crates/hypercolor-ui/src/pages/studio/device_card.rs:1
crates/hypercolor-ui/src/vendors.rs:1
EOF

cd "$repo_root"
rg --count-matches 'inner_html\s*=' crates/hypercolor-ui/src --glob '*.rs' \
  | sort >"$actual" || true

if ! diff -u "$expected" "$actual"; then
  echo "Raw HTML sinks changed. Replace the new sink or review the allowlist." >&2
  exit 1
fi
