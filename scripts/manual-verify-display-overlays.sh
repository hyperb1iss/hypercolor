#!/usr/bin/env bash
set -euo pipefail

HOST="${HYPERCOLOR_HOST:-127.0.0.1}"
PORT="${HYPERCOLOR_PORT:-9420}"
BASE_URL="http://${HOST}:${PORT}"

RESET=$'\033[0m'
CYAN=$'\033[38;2;128;255;234m'
PINK=$'\033[38;2;255;106;193m'
YELLOW=$'\033[38;2;241;250;140m'
GREEN=$'\033[38;2;80;250;123m'
RED=$'\033[38;2;255;99;99m'

usage() {
  cat <<'EOF'
Usage: scripts/manual-verify-display-overlays.sh [display-id-or-name] [effect-id]

Applies a checkerboard background effect, then creates or updates two native
display overlays named "Wave 2 Clock" and "Wave 2 Sensor" on the selected
display so Wave 2.10 can be verified on real hardware.

If no display is provided and exactly one display-capable device is connected,
that display is selected automatically.

Examples:
  scripts/manual-verify-display-overlays.sh
  scripts/manual-verify-display-overlays.sh "Pump LCD"
  scripts/manual-verify-display-overlays.sh 3d2f... solid_color
EOF
}

need_cmd() {
  local name="$1"
  if ! command -v "$name" >/dev/null 2>&1; then
    printf '%serror:%s missing required command: %s\n' "$RED" "$RESET" "$name" >&2
    exit 1
  fi
}

log_step() {
  printf '%s->%s %s\n' "$PINK" "$RESET" "$1"
}

log_info() {
  printf '%s%s%s\n' "$CYAN" "$1" "$RESET"
}

log_warn() {
  printf '%swarning:%s %s\n' "$YELLOW" "$RESET" "$1" >&2
}

log_success() {
  printf '%s%s%s\n' "$GREEN" "$1" "$RESET"
}

die() {
  printf '%serror:%s %s\n' "$RED" "$RESET" "$1" >&2
  exit 1
}

urlencode() {
  jq -nr --arg value "$1" '$value|@uri'
}

api_json() {
  local method="$1"
  local url="$2"
  local body="${3-}"
  local tmp
  local status
  tmp="$(mktemp)"
  if [[ -n "$body" ]]; then
    status="$(
      curl --silent --show-error \
        --output "$tmp" \
        --write-out '%{http_code}' \
        --request "$method" \
        --header 'content-type: application/json' \
        --data "$body" \
        "$url"
    )"
  else
    status="$(
      curl --silent --show-error \
        --output "$tmp" \
        --write-out '%{http_code}' \
        --request "$method" \
        "$url"
    )"
  fi

  if [[ "${status}" != 2* ]]; then
    printf '%srequest failed:%s %s %s (HTTP %s)\n' "$RED" "$RESET" "$method" "$url" "$status" >&2
    cat "$tmp" >&2
    rm -f "$tmp"
    exit 1
  fi

  cat "$tmp"
  rm -f "$tmp"
}

print_displays() {
  local displays_json="$1"
  jq -r '
    .data[]
    | "  - \(.name) [\(.id)] \(.width)x\(.height) circular=\(.circular)"
  ' <<<"$displays_json"
}

upsert_overlay() {
  local display_id="$1"
  local name="$2"
  local payload="$3"
  local current_json="$4"
  local existing_id
  local response

  existing_id="$(
    jq -r --arg name "$name" '
      .data.overlays[]? | select(.name == $name) | .id
    ' <<<"$current_json" | head -n1
  )"

  if [[ -n "$existing_id" ]]; then
    log_step "Updating ${name} (${existing_id})"
    response="$(
      api_json PATCH \
        "${BASE_URL}/api/v1/displays/${display_id}/overlays/${existing_id}" \
        "$payload"
    )"
    jq -r '.data.id' <<<"$response"
    return 0
  fi

  log_step "Creating ${name}"
  response="$(
    api_json POST \
      "${BASE_URL}/api/v1/displays/${display_id}/overlays" \
      "$payload"
  )"
  jq -r '.data.id' <<<"$response"
}

build_clock_payload() {
  local width="$1"
  local height="$2"
  local margin="$3"
  jq -cn \
    --argjson width "$width" \
    --argjson height "$height" \
    --argjson margin "$margin" \
    '{
      name: "Wave 2 Clock",
      source: {
        type: "clock",
        style: "digital",
        hour_format: "twenty_four",
        show_seconds: true,
        show_date: true,
        date_format: "%Y-%m-%d",
        color: "#80ffea",
        secondary_color: "#ff6ac1",
        template: "clocks/digital-default.svg"
      },
      position: {
        anchored: {
          anchor: "top_center",
          offset_x: 0,
          offset_y: $margin,
          width: (($width * 70) / 100 | floor),
          height: (($height * 24) / 100 | floor)
        }
      },
      blend_mode: "screen",
      opacity: 0.92,
      enabled: true
    }'
}

build_sensor_payload() {
  local sensor="$1"
  local range_min="$2"
  local range_max="$3"
  local size="$4"
  local margin="$5"
  jq -cn \
    --arg sensor "$sensor" \
    --argjson range_min "$range_min" \
    --argjson range_max "$range_max" \
    --argjson size "$size" \
    --argjson margin "$margin" \
    '{
      name: "Wave 2 Sensor",
      source: {
        type: "sensor",
        sensor: $sensor,
        style: "gauge",
        range_min: $range_min,
        range_max: $range_max,
        color_min: "#80ffea",
        color_max: "#ff6363",
        template: "gauges/radial-default.svg"
      },
      position: {
        anchored: {
          anchor: "bottom_center",
          offset_x: 0,
          offset_y: (-1 * $margin),
          width: $size,
          height: $size
        }
      },
      blend_mode: "screen",
      opacity: 0.95,
      enabled: true
    }'
}

select_display() {
  local displays_json="$1"
  local selector="$2"
  if [[ -z "$selector" ]]; then
    local count
    count="$(jq '.data | length' <<<"$displays_json")"
    if [[ "$count" -eq 0 ]]; then
      printf '%sNo display-capable devices are connected.%s\n' "$RED" "$RESET" >&2
      exit 1
    fi
    if [[ "$count" -eq 1 ]]; then
      jq -c '.data[0]' <<<"$displays_json"
      return 0
    fi

    printf '%sMultiple display-capable devices are connected.%s\n' "$YELLOW" "$RESET" >&2
    printf 'Choose one by id or exact name:\n' >&2
    print_displays "$displays_json" >&2
    exit 1
  fi

  local match
  match="$(
    jq -c --arg selector "$selector" '
      .data[]
      | select(.id == $selector or .name == $selector)
    ' <<<"$displays_json" | head -n1
  )"
  [[ -n "$match" ]] || {
    printf '%sNo display matched "%s".%s\n' "$RED" "$selector" "$RESET" >&2
    print_displays "$displays_json" >&2
    exit 1
  }
  printf '%s\n' "$match"
}

select_sensor() {
  local sensors_json="$1"
  jq -r '
    if .data.cpu_temp_celsius != null then
      "cpu_temp"
    elif .data.gpu_temp_celsius != null then
      "gpu_temp"
    else
      "ram_used"
    end
  ' <<<"$sensors_json"
}

if [[ "${1-}" == "--help" || "${1-}" == "-h" ]]; then
  usage
  exit 0
fi

need_cmd curl
need_cmd jq
need_cmd mktemp

display_selector="${1-}"
effect_id="${2-solid_color}"

log_step "Checking daemon health at ${BASE_URL}"
curl --silent --show-error --fail "${BASE_URL}/health" >/dev/null \
  || die "daemon is not reachable at ${BASE_URL}"

log_step "Resolving display"
displays_json="$(api_json GET "${BASE_URL}/api/v1/displays")"
display_json="$(select_display "$displays_json" "$display_selector")"
display_id="$(jq -r '.id' <<<"$display_json")"
display_name="$(jq -r '.name' <<<"$display_json")"
display_width="$(jq -r '.width' <<<"$display_json")"
display_height="$(jq -r '.height' <<<"$display_json")"

log_info "Using display: ${display_name} [${display_id}] ${display_width}x${display_height}"

log_step "Choosing live sensor source"
sensors_json="$(api_json GET "${BASE_URL}/api/v1/system/sensors")"
sensor_label="$(select_sensor "$sensors_json")"
range_min=0
range_max=100
if [[ "$sensor_label" == "cpu_temp" || "$sensor_label" == "gpu_temp" ]]; then
  range_min=20
  range_max=100
fi
log_info "Using sensor overlay label: ${sensor_label}"

log_step "Applying effect ${effect_id}"
effect_payload="$(
  jq -cn '
    {
      controls: {
        pattern: "Checker",
        color: [0.05, 0.07, 0.16, 1.0],
        secondary_color: [0.00, 0.16, 0.26, 1.0],
        scale: 8.0,
        brightness: 0.9
      }
    }'
)"
api_json \
  POST \
  "${BASE_URL}/api/v1/effects/$(urlencode "$effect_id")/apply" \
  "$effect_payload" >/dev/null

min_side="$display_width"
if (( display_height < min_side )); then
  min_side="$display_height"
fi
margin=$(( min_side * 4 / 100 ))
sensor_size=$(( min_side * 42 / 100 ))

clock_payload="$(build_clock_payload "$display_width" "$display_height" "$margin")"
sensor_payload="$(build_sensor_payload "$sensor_label" "$range_min" "$range_max" "$sensor_size" "$margin")"

log_step "Fetching current overlay config"
current_overlays="$(api_json GET "${BASE_URL}/api/v1/displays/${display_id}/overlays")"

clock_slot_id="$(upsert_overlay "$display_id" "Wave 2 Clock" "$clock_payload" "$current_overlays")"
current_overlays="$(api_json GET "${BASE_URL}/api/v1/displays/${display_id}/overlays")"
sensor_slot_id="$(upsert_overlay "$display_id" "Wave 2 Sensor" "$sensor_payload" "$current_overlays")"

log_step "Reading overlay runtime"
clock_runtime_json="$(
  api_json GET "${BASE_URL}/api/v1/displays/${display_id}/overlays/${clock_slot_id}"
)"
sensor_runtime_json="$(
  api_json GET "${BASE_URL}/api/v1/displays/${display_id}/overlays/${sensor_slot_id}"
)"

log_success "Wave 2 overlay demo is ready."
printf '  display: %s [%s]\n' "$display_name" "$display_id"
printf '  effect:  %s\n' "$effect_id"
printf '  sensor:  %s\n' "$sensor_label"
printf '  clock:   %s\n' "$clock_slot_id"
printf '  gauge:   %s\n' "$sensor_slot_id"
printf '  runtime:\n'
jq -r '
  "    - \(.data.slot.name): status=\(.data.runtime.status) failures=\(.data.runtime.consecutive_failures)"
' <<<"$clock_runtime_json"
jq -r '
  "    - \(.data.slot.name): status=\(.data.runtime.status) failures=\(.data.runtime.consecutive_failures)"
' <<<"$sensor_runtime_json"
