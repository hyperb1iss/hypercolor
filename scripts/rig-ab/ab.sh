#!/usr/bin/env bash
# Concurrent A/B of daemon builds on a simulated rig: one zone, an animated
# native effect, DDP strips that end in sockets on loopback, and simulated
# LCD panels with no face.
#
# usage: ab.sh <runs-dir> <pairs> <seconds> <label>=<daemon-binary>[=<fixture-config>] ...
#
#   scripts/rig-ab/ab.sh target/rig-ab/run1 8 40 \
#     base=/path/to/base/target/preview/hypercolor-daemon=/path/to/base/e2e/harness/browser-config.toml \
#     head=target/preview/hypercolor-daemon
#   scripts/rig-ab/report.py target/rig-ab/run1 base head
#
# Each pair starts one daemon per label at the same moment, each in its own
# user and network namespace, and samples them over the same wall-clock
# window, so whatever else the machine is doing lands on every side alike.
# A sequential before and after on a busy machine measures the machine.
#
# The fixture is the e2e config of the checkout the binary was built from:
# it names every driver that build knows, and all of them stay off except
# the pixel driver. Build the binaries without Servo and with the preview
# profile:
#
#   ./scripts/cargo-cache-build.sh cargo build -p hypercolor-daemon \
#     --bin hypercolor-daemon --profile preview \
#     --no-default-features --features builtin-drivers,wgpu
#
# Needs unshare, ip, and python3 with the websockets package.
#
# Environment: RIG_AB_WARMUP (seconds before the window, default 25),
# RIG_AB_LED_DEVICES and RIG_AB_LED_PIXELS (default 6 strips of 150),
# RIG_AB_LOG_LEVEL (default warn; info logs every poll), RIG_AB_DRIVER
# (default driver.py; parity.py captures panel frames instead), and
# RIG_AB_DRIVER_ARGS (extra arguments for the driver, such as
# "--effect Gradient").
set -euo pipefail

if [ "$#" -lt 4 ]; then
  sed -n '2,31p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
  exit 2
fi

runs="$1"
pairs="$2"
seconds="$3"
shift 3
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
warmup="${RIG_AB_WARMUP:-25}"

mkdir -p "$runs"
runs="$(cd "$runs" && pwd)"
for ((pair = 1; pair <= pairs; pair++)); do
  dir="$runs/pair-$(printf '%02d' "$pair")"
  mkdir -p "$dir"
  start="$(python3 -c "import time; print(time.time() + $warmup)")"
  pids=()
  for side in "$@"; do
    IFS='=' read -r label binary fixture <<< "$side"
    binary="$(realpath "$binary")"
    fixture="$(realpath "${fixture:-$repo/e2e/harness/browser-config.toml}")"
    unshare -Urn "$here/side.sh" "$label" "$binary" "$fixture" "$dir/$label" "$start" "$seconds" \
      > "$dir/$label.out" 2>&1 &
    pids+=("$!")
  done
  for pid in "${pids[@]}"; do
    wait "$pid" || echo "pair $pair: a side failed, see $dir" >&2
  done
  cat "$dir"/*.out
  # The namespace hides the network, not the hardware. A daemon that
  # probed it was not running the config this harness wrote.
  if grep -lE "attempting USB connect|smbus_probe" "$dir"/*/daemon.log; then
    echo "a bench daemon probed real hardware; stopping" >&2
    exit 1
  fi
done
echo "done: $here/report.py $runs <label> [<label> ...]"
