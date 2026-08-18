#!/usr/bin/env bash
# Generate the Tauri app icon ladder from the canonical checked-in brand mark.

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

exec uv run assets/brand/build.py app-icon
