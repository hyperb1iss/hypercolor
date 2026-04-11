#!/usr/bin/env bash
set -euo pipefail

HOST="${HYPERCOLOR_HOST:-127.0.0.1}"
PORT="${HYPERCOLOR_PORT:-9420}"
BASE_URL="http://${HOST}:${PORT}"

RESET=$'\033[0m'
CYAN=$'\033[38;2;128;255;234m'
PINK=$'\033[38;2;225;53;255m'
YELLOW=$'\033[38;2;241;250;140m'
GREEN=$'\033[38;2;80;250;123m'
RED=$'\033[38;2;255;99;99m'

usage() {
  cat <<'EOF'
Usage: scripts/check-display-overlay-matrix.sh

Reads /api/v1/displays from the local daemon and reports whether the connected
display-capable devices satisfy Spec 40's Wave 2 hardware verification matrix:

  - one 480x480 circular display
  - one 480x480 square display
  - one non-square display

Exit status is 0 when the matrix is satisfied, 1 when one or more categories are
still missing, and non-zero on command or daemon errors.
EOF
}

need_cmd() {
  local name="$1"
  if ! command -v "$name" >/dev/null 2>&1; then
    printf '%serror:%s missing required command: %s\n' "$RED" "$RESET" "$name" >&2
    exit 2
  fi
}

log_step() {
  printf '%s->%s %s\n' "$PINK" "$RESET" "$1"
}

log_info() {
  printf '%s%s%s\n' "$CYAN" "$1" "$RESET"
}

log_warn() {
  printf '%swarning:%s %s\n' "$YELLOW" "$RESET" "$1"
}

log_success() {
  printf '%s%s%s\n' "$GREEN" "$1" "$RESET"
}

die() {
  printf '%serror:%s %s\n' "$RED" "$RESET" "$1" >&2
  exit 2
}

api_json() {
  local url="$1"
  local tmp
  local status
  tmp="$(mktemp)"
  status="$(
    curl --silent --show-error \
      --output "$tmp" \
      --write-out '%{http_code}' \
      "$url"
  )"

  if [[ "${status}" != 2* ]]; then
    printf '%srequest failed:%s GET %s (HTTP %s)\n' "$RED" "$RESET" "$url" "$status" >&2
    cat "$tmp" >&2
    rm -f "$tmp"
    exit 2
  fi

  cat "$tmp"
  rm -f "$tmp"
}

print_display_inventory() {
  local displays_json="$1"
  jq -r '
    .data[]
    | .category = (
        if .circular and .width == 480 and .height == 480 then
          "circular_480x480"
        elif (not .circular) and .width == 480 and .height == 480 then
          "square_480x480"
        elif .width != .height then
          "non_square"
        elif .circular then
          "circular_other"
        else
          "square_other"
        end
      )
    | "  - \(.name) [\(.id)] \(.width)x\(.height) circular=\(.circular) overlays=\(.enabled_overlay_count)/\(.overlay_count) category=\(.category)"
  ' <<<"$displays_json"
}

first_match() {
  local displays_json="$1"
  local filter="$2"
  jq -r "
    [ .data[] | select(${filter}) | \"\\(.name) [\\(.id)] \\(.width)x\\(.height)\" ]
    | first
    | . // empty
  " <<<"$displays_json"
}

report_requirement() {
  local label="$1"
  local match="$2"
  if [[ -n "$match" ]]; then
    log_success "${label}: ${match}"
    return 0
  fi

  log_warn "${label}: missing"
  return 1
}

if [[ "${1-}" == "--help" || "${1-}" == "-h" ]]; then
  usage
  exit 0
fi

need_cmd curl
need_cmd jq
need_cmd mktemp

log_step "Checking daemon health at ${BASE_URL}"
curl --silent --show-error --fail "${BASE_URL}/health" >/dev/null \
  || die "daemon is not reachable at ${BASE_URL}"

log_step "Loading display inventory"
displays_json="$(api_json "${BASE_URL}/api/v1/displays")"
display_count="$(jq '.data | length' <<<"$displays_json")"

if [[ "$display_count" -eq 0 ]]; then
  log_warn "No display-capable devices are currently connected."
else
  log_info "Connected displays:"
  print_display_inventory "$displays_json"
fi

missing=0

if ! report_requirement \
  "Wave 2 circular 480x480" \
  "$(first_match "$displays_json" '.circular and .width == 480 and .height == 480')"; then
  missing=$((missing + 1))
fi

if ! report_requirement \
  "Wave 2 square 480x480" \
  "$(first_match "$displays_json" '(.circular | not) and .width == 480 and .height == 480')"; then
  missing=$((missing + 1))
fi

if ! report_requirement \
  "Wave 2 non-square" \
  "$(first_match "$displays_json" '.width != .height')"; then
  missing=$((missing + 1))
fi

if [[ "$missing" -eq 0 ]]; then
  log_success "Spec 40 Wave 2 hardware matrix is ready for manual overlay verification."
  exit 0
fi

log_warn "Spec 40 Wave 2 hardware matrix is incomplete (${missing} requirement(s) missing)."
log_info "Connect the missing displays, then run this check again before using just overlay-demo."
exit 1
