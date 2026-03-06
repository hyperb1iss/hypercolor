#!/usr/bin/env bash
set -euo pipefail

# Backwards-compatible Servo entrypoint. The shared wrapper now handles
# workspace-wide Cargo caching and compiler acceleration.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ "$#" -eq 0 ]; then
  exec "$SCRIPT_DIR/cargo-cache-build.sh" \
    cargo test -p hypercolor-core --features servo --all-targets
fi

exec "$SCRIPT_DIR/cargo-cache-build.sh" "$@"
