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
EFFECT_ID="solid_color"

usage() {
  cat <<'EOF'
Usage: scripts/simulator-demo.sh [--name NAME] [--width PX] [--height PX] [--square|--circle] [--effect ID_OR_NAME]

Creates or updates a virtual display simulator through the local daemon API,
optionally applies an effect, and prints a ready-to-open browser preview URL.

Examples:
  scripts/simulator-demo.sh
  scripts/simulator-demo.sh --name "Square Preview" --square
  scripts/simulator-demo.sh --name "Wide Preview" --width 640 --height 240 --effect rainbow
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

if [[ -n "$EFFECT_ID" ]]; then
  log_step "Resolving effect ${EFFECT_ID}"
  effects_json="$(api_json GET "${BASE_URL}/api/v1/effects")"
  resolved_effect_id="$(resolve_effect_id "$EFFECT_ID" "$effects_json")"
  [[ -n "$resolved_effect_id" ]] || die "effect not found: ${EFFECT_ID}"

  log_step "Applying effect ${resolved_effect_id}"
  api_json POST "${BASE_URL}/api/v1/effects/${resolved_effect_id}/apply" '{}' >/dev/null
fi

preview_url="${BASE_URL}/preview?mode=simulator&display=$(urlencode "$simulator_id")"

log_success "Simulator ready: ${simulator_name} [${simulator_id}] ${simulator_width}x${simulator_height} circular=${simulator_circular}"
log_info "Browser preview: ${preview_url}"
log_info "Canvas preview: ${BASE_URL}/preview"
log_info "Frame endpoint: ${BASE_URL}/api/v1/simulators/displays/${simulator_id}/frame"
