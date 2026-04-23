#!/usr/bin/env bash
# Generate the numbers cited in Cinder RFCs 36-42.
# Output checked in at docs/design/cinder-audit-snapshot.txt.
# CI enforces that the snapshot matches the script output via `just verify`.
#
# Usage:
#   scripts/cinder-audit.sh              # Print to stdout
#   scripts/cinder-audit.sh > docs/design/cinder-audit-snapshot.txt
#
# Source files audited live under crates/hypercolor-ui/ and crates/hypercolor-daemon/.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

echo "=== Cinder audit snapshot ==="
echo "Generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "Commit: $(git rev-parse HEAD)"
echo

echo "--- UI total LOC ---"
find crates/hypercolor-ui/src -name '*.rs' | xargs wc -l | tail -1
echo

echo "--- Per-file LOC (RFC-cited) ---"
for f in \
    crates/hypercolor-ui/src/components/color_wheel.rs \
    crates/hypercolor-ui/src/components/canvas_preview.rs \
    crates/hypercolor-ui/src/ws/connection.rs \
    crates/hypercolor-ui/src/ws/messages.rs \
    crates/hypercolor-ui/src/components/preview_runtime/webgl.rs \
    crates/hypercolor-ui/src/components/preview_runtime/canvas2d.rs; do
    if [ -f "$f" ]; then
        wc -l "$f"
    else
        echo "MISSING: $f"
    fi
done
echo

echo "--- Pattern counts in hypercolor-ui ---"
printf 'dyn_into total:                %d\n' \
    "$(grep -rn 'dyn_into' crates/hypercolor-ui/src | wc -l | tr -d ' ')"
printf 'dyn_into::<HtmlInputElement>:  %d\n' \
    "$(grep -rE 'dyn_into::<(web_sys::)?HtmlInputElement>' crates/hypercolor-ui/src | wc -l | tr -d ' ')"
printf 'Closure::*:                    %d\n' \
    "$(grep -rn 'Closure::' crates/hypercolor-ui/src | wc -l | tr -d ' ')"
printf 'Rc<RefCell<Option<Closure:     %d\n' \
    "$(grep -rn 'Rc<RefCell<Option<Closure' crates/hypercolor-ui/src | wc -l | tr -d ' ')"
printf 'tex_image_2d_with_i32_and_:    %d\n' \
    "$(grep -rn 'tex_image_2d_with_i32_and' crates/hypercolor-ui/src | wc -l | tr -d ' ')"
printf 'new_with_u8_clamped_array:     %d\n' \
    "$(grep -rn 'new_with_u8_clamped_array' crates/hypercolor-ui/src | wc -l | tr -d ' ')"
printf 'Uint8Array::get_index:         %d\n' \
    "$(grep -rn 'Uint8Array.*get_index\|\.get_index(' crates/hypercolor-ui/src | wc -l | tr -d ' ')"
echo

echo "--- WebSocket layer LOC ---"
echo "UI src/ws/:"
find crates/hypercolor-ui/src/ws -name '*.rs' | xargs wc -l | tail -1
echo "Daemon api/ws/ (excluding tests):"
find crates/hypercolor-daemon/src/api/ws -name '*.rs' ! -name '*tests.rs' 2>/dev/null | xargs wc -l | tail -1 || echo "  (no matching files found)"
echo "Daemon api/ws/ (including tests):"
find crates/hypercolor-daemon/src/api/ws -name '*.rs' 2>/dev/null | xargs wc -l | tail -1 || echo "  (no matching files found)"
echo

echo "--- Bundle size ---"
if ls crates/hypercolor-ui/dist/*.wasm 1>/dev/null 2>&1; then
    for wasm in crates/hypercolor-ui/dist/*.wasm; do
        sz_raw=$(wc -c < "$wasm" | tr -d ' ')
        sz_gz=$(gzip -c "$wasm" | wc -c | tr -d ' ')
        echo "$wasm"
        printf '    raw:     %10d bytes\n' "$sz_raw"
        printf '    gzipped: %10d bytes\n' "$sz_gz"
    done
else
    echo "No built wasm in dist/ (run 'just ui-build' first)"
fi
echo

echo "--- Binary frame types in daemon protocol ---"
grep -rn 'const.*_FRAME_HEADER\|const.*FRAME_HEADER' crates/hypercolor-ui/src/ws/ 2>/dev/null | head -20
echo

echo "=== END snapshot ==="
