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

NAME="Preview Simulator"
WIDTH=480
HEIGHT=480
CIRCULAR=true
EFFECT_ID=""
WAIT_FRAME=false
FRAME_OUT=""
TIMEOUT_SECONDS=10

usage() {
  cat <<'EOF'
Usage: scripts/simulator-demo.sh [--name NAME] [--width PX] [--height PX] [--square|--circle] [--effect ID_OR_NAME] [--wait-frame] [--frame-out PATH] [--timeout SECONDS]

Creates or updates a virtual display simulator through the local daemon API,
optionally applies an effect, and prints a ready-to-open browser preview URL.
When requested, it can also wait for the frame endpoint to produce image bytes
and save that frame for CI or visual inspection. If `--effect` is omitted, the
helper auto-selects the first native effect, falling back to the first listed
effect.

Examples:
  scripts/simulator-demo.sh
  scripts/simulator-demo.sh --name "Square Preview" --square
  scripts/simulator-demo.sh --name "Wide Preview" --width 640 --height 240 --effect rainbow
  scripts/simulator-demo.sh --wait-frame --frame-out /tmp/simulator.jpg
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

resolve_effect_id() {
  local selector="$1"
  local effects_json="$2"
  jq -r --arg selector "$selector" '
    .data.items[]
    | select(.id == $selector or .name == $selector)
    | .id
  ' <<<"$effects_json" | head -n1
}

resolve_default_effect_id() {
  local effects_json="$1"
  jq -r '
    (
      [.data.items[] | select(.source == "native") | .id][0]
      // .data.items[0].id
    ) // empty
  ' <<<"$effects_json"
}

wait_for_frame() {
  local frame_url="$1"
  local timeout_seconds="$2"
  local frame_out="${3-}"
  local deadline=$((SECONDS + timeout_seconds))
  local tmp
  local status

  while (( SECONDS <= deadline )); do
    tmp="$(mktemp)"
    status="$(
      curl --silent --show-error \
        --output "$tmp" \
        --write-out '%{http_code}' \
        "$frame_url"
    )"

    if [[ "$status" == "200" && -s "$tmp" ]]; then
      if [[ -n "$frame_out" ]]; then
        mkdir -p "$(dirname "$frame_out")"
        cp "$tmp" "$frame_out"
        rm -f "$tmp"
        log_success "Saved simulator frame to ${frame_out}"
      else
        rm -f "$tmp"
        log_success "Simulator frame is available"
      fi
      return 0
    fi

    rm -f "$tmp"
    sleep 0.25
  done

  return 1
}

ensure_active_layout_target() {
  local simulator_id="$1"
  local simulator_name="$2"
  local active_layout_json
  local existing_zone
  local layout_id
  local layout_device_id
  local zone_id
  local display_order
  local updated_zones
  local update_payload

  active_layout_json="$(api_json GET "${BASE_URL}/api/v1/layouts/active")"
  layout_id="$(jq -r '.data.id' <<<"$active_layout_json")"
  [[ -n "$layout_id" && "$layout_id" != "null" ]] || die "active layout is missing an id"

  layout_device_id="simulator:${simulator_id}"
  existing_zone="$(
    jq -c --arg device_id "$layout_device_id" '
      first(.data.zones[]? | select(.device_id == $device_id)) // empty
    ' <<<"$active_layout_json"
  )"
  if [[ -n "$existing_zone" ]]; then
    log_info "Active layout already targets ${simulator_name}; preserving existing zone"
    log_step "Re-applying active layout ${layout_id}"
    api_json POST "${BASE_URL}/api/v1/layouts/${layout_id}/apply" >/dev/null
    return 0
  fi

  zone_id="$(
    jq -r --arg device_id "$layout_device_id" '
      first(.data.zones[]? | select(.device_id == $device_id) | .id) // empty
    ' <<<"$active_layout_json"
  )"
  if [[ -z "$zone_id" ]]; then
    zone_id="zone_simulator_${simulator_id}"
  fi

  display_order="$(
    jq -r --arg device_id "$layout_device_id" '
      first(.data.zones[]? | select(.device_id == $device_id) | .display_order)
      // (((.data.zones // []) | map(.display_order // 0) | max) // -1) + 1
    ' <<<"$active_layout_json"
  )"

  updated_zones="$(
    jq -c \
      --arg device_id "$layout_device_id" \
      --arg zone_id "$zone_id" \
      --arg zone_name "${simulator_name} Display" \
      --argjson display_order "$display_order" '
      ((.data.zones // []) | map(select(.device_id != $device_id))) + [
        {
          id: $zone_id,
          name: $zone_name,
          device_id: $device_id,
          zone_name: null,
          position: { x: 0.5, y: 0.5 },
          size: { x: 1.0, y: 1.0 },
          rotation: 0.0,
          scale: 1.0,
          display_order: $display_order,
          orientation: null,
          topology: { type: "point" },
          led_mapping: null,
          sampling_mode: null,
          edge_behavior: null,
          shape: null,
          shape_preset: null,
          attachment: null
        }
      ]
    ' <<<"$active_layout_json"
  )"

  update_payload="$(jq -cn --argjson zones "$updated_zones" '{ zones: $zones }')"

  log_step "Updating active layout ${layout_id} with simulator display zone"
  api_json PUT "${BASE_URL}/api/v1/layouts/${layout_id}" "$update_payload" >/dev/null

  log_step "Re-applying active layout ${layout_id}"
  api_json POST "${BASE_URL}/api/v1/layouts/${layout_id}/apply" >/dev/null
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --name)
      NAME="${2-}"
      shift 2
      ;;
    --width)
      WIDTH="${2-}"
      shift 2
      ;;
    --height)
      HEIGHT="${2-}"
      shift 2
      ;;
    --square)
      CIRCULAR=false
      shift
      ;;
    --circle|--circular)
      CIRCULAR=true
      shift
      ;;
    --effect)
      EFFECT_ID="${2-}"
      shift 2
      ;;
    --wait-frame)
      WAIT_FRAME=true
      shift
      ;;
    --frame-out)
      FRAME_OUT="${2-}"
      WAIT_FRAME=true
      shift 2
      ;;
    --timeout)
      TIMEOUT_SECONDS="${2-}"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
done

need_cmd curl
need_cmd jq
need_cmd mktemp

[[ -n "$NAME" ]] || die "simulator name must not be empty"
[[ "$WIDTH" =~ ^[0-9]+$ ]] || die "width must be a positive integer"
[[ "$HEIGHT" =~ ^[0-9]+$ ]] || die "height must be a positive integer"
[[ "$TIMEOUT_SECONDS" =~ ^[0-9]+$ ]] || die "timeout must be a positive integer"

log_step "Checking daemon health at ${BASE_URL}"
curl --silent --show-error --fail "${BASE_URL}/health" >/dev/null \
  || die "daemon is not reachable at ${BASE_URL}"

log_step "Loading simulator inventory"
simulators_json="$(api_json GET "${BASE_URL}/api/v1/simulators/displays")"
existing_id="$(
  jq -r --arg name "$NAME" '
    .data[]
    | select(.name == $name)
    | .id
  ' <<<"$simulators_json" | head -n1
)"

payload="$(
  jq -cn \
    --arg name "$NAME" \
    --argjson width "$WIDTH" \
    --argjson height "$HEIGHT" \
    --argjson circular "$CIRCULAR" \
    '{
      name: $name,
      width: $width,
      height: $height,
      circular: $circular,
      enabled: true
    }'
)"

if [[ -n "$existing_id" ]]; then
  log_step "Updating simulator ${NAME} (${existing_id})"
  simulator_json="$(api_json PATCH "${BASE_URL}/api/v1/simulators/displays/${existing_id}" "$payload")"
else
  log_step "Creating simulator ${NAME}"
  simulator_json="$(api_json POST "${BASE_URL}/api/v1/simulators/displays" "$payload")"
fi

simulator_id="$(jq -r '.data.id' <<<"$simulator_json")"
simulator_name="$(jq -r '.data.name' <<<"$simulator_json")"
simulator_width="$(jq -r '.data.width' <<<"$simulator_json")"
simulator_height="$(jq -r '.data.height' <<<"$simulator_json")"
simulator_circular="$(jq -r '.data.circular' <<<"$simulator_json")"

ensure_active_layout_target "$simulator_id" "$simulator_name"

if [[ -n "$EFFECT_ID" || "$WAIT_FRAME" == true ]]; then
  log_step "Loading effect inventory"
  effects_json="$(api_json GET "${BASE_URL}/api/v1/effects")"
  if [[ -n "$EFFECT_ID" ]]; then
    log_step "Resolving effect ${EFFECT_ID}"
    resolved_effect_id="$(resolve_effect_id "$EFFECT_ID" "$effects_json")"
    [[ -n "$resolved_effect_id" ]] || die "effect not found: ${EFFECT_ID}"
  else
    resolved_effect_id="$(resolve_default_effect_id "$effects_json")"
    [[ -n "$resolved_effect_id" ]] || die "no effects are available to render the simulator"
    log_step "Auto-selected effect ${resolved_effect_id}"
  fi

  log_step "Applying effect ${resolved_effect_id}"
  api_json POST "${BASE_URL}/api/v1/effects/${resolved_effect_id}/apply" '{}' >/dev/null
fi

preview_url="${BASE_URL}/preview?mode=simulator&display=$(urlencode "$simulator_id")"
frame_url="${BASE_URL}/api/v1/simulators/displays/${simulator_id}/frame"

log_success "Simulator ready: ${simulator_name} [${simulator_id}] ${simulator_width}x${simulator_height} circular=${simulator_circular}"
log_info "Browser preview: ${preview_url}"
log_info "Canvas preview: ${BASE_URL}/preview"
log_info "Frame endpoint: ${frame_url}"

if [[ "$WAIT_FRAME" == true ]]; then
  log_step "Waiting up to ${TIMEOUT_SECONDS}s for a simulator frame"
  wait_for_frame "$frame_url" "$TIMEOUT_SECONDS" "$FRAME_OUT" \
    || die "timed out waiting for simulator frame at ${frame_url}"
fi
