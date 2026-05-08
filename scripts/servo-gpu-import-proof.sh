#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

printf 'Servo GPU import proof\n'
printf '  fixture: deterministic_servo_gpu_import_matches_cpu_readback\n'
printf '  gate: HYPERCOLOR_RUN_SERVO_GPU_PARITY=1\n'

HYPERCOLOR_RUN_SERVO_GPU_PARITY=1 \
  ./scripts/cargo-cache-build.sh \
  cargo test --locked -p hypercolor-core --features servo-gpu-import \
  deterministic_servo_gpu_import_matches_cpu_readback -- --nocapture
