#!/usr/bin/env bash
set -euo pipefail

real_cargo="${HYPERCOLOR_REAL_CARGO:?missing real Cargo path}"
target_dir="${HYPERCOLOR_NESTED_CARGO_TARGET_DIR:?missing nested Cargo target directory}"
escaped_target="${target_dir//\\/\\\\}"
escaped_target="${escaped_target//\"/\\\"}"

exec "$real_cargo" --config "build.target-dir=\"$escaped_target\"" "$@"
