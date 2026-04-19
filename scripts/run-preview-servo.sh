#!/usr/bin/env bash
set -euo pipefail

# Run hypercolor-daemon with Servo-enabled HTML effect rendering.
# Uses the shared cache wrapper to avoid repeating expensive servo/mozjs builds.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"

if [ "$#" -eq 0 ]; then
  exec "$SCRIPT_DIR/servo-cache-build.sh" \
    cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview --features servo -- --bind 127.0.0.1:9420
fi

exec "$SCRIPT_DIR/servo-cache-build.sh" \
  cargo run -p hypercolor-daemon --bin hypercolor-daemon --profile preview --features servo -- "$@"
