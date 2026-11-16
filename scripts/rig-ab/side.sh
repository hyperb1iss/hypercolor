#!/usr/bin/env bash
# One side of a rig A/B: an isolated daemon and the driver that builds its
# rig and samples it. Started by ab.sh under `unshare -Urn`, so the daemon
# has a network namespace to itself and never meets the daemon that drives
# real hardware on this machine.
#
# usage: side.sh <label> <daemon-binary> <fixture-config> <run-dir> <start-epoch> <seconds>
set -euo pipefail

label="$1"
binary="$2"
fixture="$3"
run_dir="$4"
start_epoch="$5"
seconds="$6"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ip link set lo up

rm -rf "$run_dir"
mkdir -p "$run_dir"/{config,data,state,cache,run,tmp}

led_devices="${RIG_AB_LED_DEVICES:-6}"
led_pixels="${RIG_AB_LED_PIXELS:-150}"

{
  echo 'schema_version = 7'
  echo
  echo '[daemon]'
  echo 'port = 9420'
  echo
  # Background discovery would walk the real USB and SMBus devices.
  echo '[discovery]'
  echo 'background_enabled = false'
  echo 'mdns_enabled = false'
  echo 'blocks_scan = false'
  echo
  echo '[effect_engine]'
  echo 'compositor_acceleration_mode = "gpu"'
  echo
  # Every driver the fixture names stays off except the pixel driver, whose
  # controllers are sockets on this namespace's loopback.
  awk '/^\[drivers\./ { name = $0; getline; if (name != "[drivers.pixel]") { print name; print $0; print "" } }' "$fixture"
  echo '[drivers.pixel]'
  echo 'enabled = true'
  for ((i = 0; i < led_devices; i++)); do
    echo
    echo '[[drivers.pixel.devices]]'
    echo "id = \"bench-strip-$i\""
    echo "name = \"Bench Strip $i\""
    echo 'host = "127.0.0.1"'
    echo 'protocol = "ddp"'
    echo "port = $((4048 + i))"
    echo "pixel_count = $led_pixels"
  done
} > "$run_dir/config.toml"

export XDG_CONFIG_HOME="$run_dir/config" XDG_DATA_HOME="$run_dir/data" \
  XDG_STATE_HOME="$run_dir/state" XDG_CACHE_HOME="$run_dir/cache" \
  XDG_RUNTIME_DIR="$run_dir/run"
unset DBUS_SESSION_BUS_ADDRESS WAYLAND_DISPLAY DISPLAY PIPEWIRE_REMOTE PULSE_SERVER

TMPDIR="$run_dir/tmp" "$binary" -c "$run_dir/config.toml" --log-level "${RIG_AB_LOG_LEVEL:-warn}" \
  > "$run_dir/daemon.log" 2>&1 &
daemon_pid=$!
trap 'kill "$daemon_pid" 2>/dev/null || true; wait "$daemon_pid" 2>/dev/null || true' EXIT

# shellcheck disable=SC2086
python3 "$here/${RIG_AB_DRIVER:-driver.py}" --label "$label" --out "$run_dir/samples.json" \
  --start-epoch "$start_epoch" --seconds "$seconds" \
  --led-devices "$led_devices" --led-pixels "$led_pixels" ${RIG_AB_DRIVER_ARGS:-}
