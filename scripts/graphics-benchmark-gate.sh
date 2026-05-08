#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE_ROOT="${HYPERCOLOR_CACHE_DIR:-$HOME/.cache/hypercolor}"
if [[ -n "${CRITERION_DIR:-}" ]]; then
  CRITERION_DIR="$CRITERION_DIR"
elif [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  CRITERION_DIR="$CARGO_TARGET_DIR/criterion"
elif [[ -d "$CACHE_ROOT/target/criterion" ]]; then
  CRITERION_DIR="$CACHE_ROOT/target/criterion"
else
  CRITERION_DIR="$ROOT_DIR/target/criterion"
fi
STRICT=0

if [[ "${1:-}" == "--strict" ]]; then
  STRICT=1
fi

GATES=(
  "daemon_sparkleflinger/cpu_single_replace_surface_and_zone_sample_640x480|8.00|16.67"
  "daemon_sparkleflinger/cpu_compose_and_zone_sample_640x480|16.67|16.67"
  "daemon_sparkleflinger/cpu_compose_and_zone_sample_640x480_fresh|16.67|16.67"
  "daemon_sparkleflinger/gpu_compose_and_zone_sample_640x480|16.67|16.67"
  "daemon_publish_handoff/slot_backed_surface|0.25|0.50"
  "core_canvas_handoff/canvas_frame_from_owned_shared|0.50|1.00"
)

PURPLE=$'\033[38;2;225;53;255m'
CYAN=$'\033[38;2;128;255;234m'
CORAL=$'\033[38;2;255;106;193m'
YELLOW=$'\033[38;2;241;250;140m'
GREEN=$'\033[38;2;80;250;123m'
RED=$'\033[38;2;255;99;99m'
RESET=$'\033[0m'

gate_for() {
  local id="$1"
  local gate
  for gate in "${GATES[@]}"; do
    IFS='|' read -r name p95 p99 <<<"$gate"
    if [[ "$id" == "$name" ]]; then
      printf '%s|%s\n' "$p95" "$p99"
      return 0
    fi
  done
  return 1
}

percentiles_for_sample() {
  local sample="$1"
  local iters times
  iters="$(sed -E 's/.*"iters":\[([^]]+)\].*/\1/' "$sample")"
  times="$(sed -E 's/.*"times":\[([^]]+)\].*/\1/' "$sample")"

  awk -v iters="$iters" -v times="$times" '
    BEGIN {
      n = split(iters, i, ",")
      split(times, t, ",")
      for (idx = 1; idx <= n; idx++) {
        if (i[idx] > 0) {
          printf "%.9f\n", (t[idx] / i[idx]) / 1000000.0
        }
      }
    }
  ' | sort -n | awk '
    { values[++n] = $1; sum += $1 }
    END {
      if (n == 0) {
        exit 1
      }
      p95_idx = int((n * 95 + 99) / 100)
      p99_idx = int((n * 99 + 99) / 100)
      if (p95_idx < 1) p95_idx = 1
      if (p99_idx < 1) p99_idx = 1
      if (p95_idx > n) p95_idx = n
      if (p99_idx > n) p99_idx = n
      printf "%.6f %.6f %.6f\n", sum / n, values[p95_idx], values[p99_idx]
    }
  '
}

if [[ ! -d "$CRITERION_DIR" ]]; then
  printf '%sNo Criterion output found at %s%s\n' "$RED" "$CRITERION_DIR" "$RESET" >&2
  printf 'Run %sjust bench-daemon%s or %sjust bench%s first.\n' "$CYAN" "$RESET" "$CYAN" "$RESET" >&2
  printf 'Set %sCRITERION_DIR%s when using a non-default target location.\n' "$CYAN" "$RESET" >&2
  exit 2
fi

printf '%sGraphics benchmark gate%s\n' "$PURPLE" "$RESET"
printf '%s%s%s\n' "$CYAN" "$CRITERION_DIR" "$RESET"
printf '────────────────────────────────────────────────────────────\n'

checked=0
failed=0

while IFS= read -r sample; do
  rel="${sample#"$CRITERION_DIR"/}"
  id="${rel%/new/sample.json}"
  if ! gate="$(gate_for "$id")"; then
    continue
  fi

  IFS='|' read -r max_p95 max_p99 <<<"$gate"
  read -r mean p95 p99 < <(percentiles_for_sample "$sample")
  checked=$((checked + 1))

  if awk -v p95="$p95" -v p99="$p99" -v max_p95="$max_p95" -v max_p99="$max_p99" \
    'BEGIN { exit !((p95 <= max_p95) && (p99 <= max_p99)) }'
  then
    printf '%sPASS%s %-72s mean=%sms p95=%sms p99=%sms\n' \
      "$GREEN" "$RESET" "$id" "$mean" "$p95" "$p99"
  else
    failed=$((failed + 1))
    printf '%sWARN%s %-72s mean=%sms p95=%sms/%sms p99=%sms/%sms\n' \
      "$YELLOW" "$RESET" "$id" "$mean" "$p95" "$max_p95" "$p99" "$max_p99"
  fi
done < <(find "$CRITERION_DIR" -path '*/new/sample.json' -type f | sort)

printf '────────────────────────────────────────────────────────────\n'
if [[ "$checked" -eq 0 ]]; then
  printf '%sNo configured benchmark samples found.%s\n' "$YELLOW" "$RESET"
  exit 2
fi

if [[ "$failed" -gt 0 ]]; then
  printf '%s%d benchmark gate(s) over warning budget.%s\n' "$YELLOW" "$failed" "$RESET"
  if [[ "$STRICT" -eq 1 ]]; then
    exit 1
  fi
else
  printf '%sAll %d configured benchmark gate(s) passed.%s\n' "$GREEN" "$checked" "$RESET"
fi
