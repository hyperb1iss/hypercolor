#!/usr/bin/env bash
# Runs one row of the notarized requirement table in this job's fresh VM.
set -euo pipefail

row="${1:?usage: row.sh <row>}"
repo="$(pwd)"
dist="${repo}/dist"
work="${RUNNER_TEMP}/notary-spike"
probe_bin="${repo}/spike/notary/probe/target/release/notary-probe"
team="${APPLE_TEAM_ID:?}"
summary="${GITHUB_STEP_SUMMARY:-/dev/null}"
apple_url="https://api.apple-cloudkit.com/"
lookup_url="${apple_url}database/1/com.apple.gk.ticket-delivery/production/public/records/lookup"
watchdog=""
mismatches=0

mkdir -p "${work}"
cat > "${work}/block-all.conf" <<'PF'
set skip on lo0
block drop all
PF
trap 'sudo pfctl -d >/dev/null 2>&1 || true' EXIT

note() {
  printf '\n== %s\n' "$*"
}

unpack() {
  local archive="$1"
  local dest="${work}/$2"
  mkdir -p "${dest}"
  tar -xpf "${dist}/${archive}.tar" -C "${dest}"
  printf '%s/NotarySpike.app' "${dest}"
}

# check <label> <spec|relaxed> <pass|fail|record> <bundle> [scope]
check() {
  local label="$1"
  local mode="$2"
  local expect="$3"
  local bundle="$4"
  local scope="${5:-all}"
  local status=0
  local observed
  note "${label} [${mode}, ${scope}]"
  "${probe_bin}" "${mode}" "${team}" "${bundle}" "${scope}" || status=$?
  case "${status}" in
    0) observed=pass ;;
    1) observed=fail ;;
    *) observed="unexpected-${status}" ;;
  esac
  printf 'observed %s, expected %s\n' "${observed}" "${expect}"
  printf '| %s | %s | %s | %s |\n' "${label}" "${mode}" "${observed}" \
    "${expect}" >> "${summary}"
  if [[ "${expect}" == record ]]; then
    # A mixed result is an observation. A crash is not.
    case "${status}" in
      0 | 1 | 3) ;;
      *) mismatches=$((mismatches + 1)) ;;
    esac
  elif [[ "${observed}" != "${expect}" ]]; then
    mismatches=$((mismatches + 1))
  fi
}

net_off() {
  local status=0
  note "cutting the network"
  curl -sS -o /dev/null --max-time 15 -w 'before the cut: HTTP %{http_code}\n' \
    "${apple_url}"
  sudo pfctl -E -f "${work}/block-all.conf" > "${work}/pf-enable.log" 2>&1
  [[ "$(sudo pfctl -s info 2>/dev/null)" == *"Status: Enabled"* ]]
  (sleep 240; sudo pfctl -d) >/dev/null 2>&1 &
  watchdog=$!
  curl -sS -o /dev/null --max-time 5 "${apple_url}" || status=$?
  # Only name resolution, connect and timeout failures prove the cut.
  case "${status}" in
    6 | 7 | 28) printf 'during the cut: curl exit %s\n' "${status}" ;;
    *)
      printf 'the cut is not proven: curl exit %s\n' "${status}" >&2
      exit 1
      ;;
  esac
}

net_on() {
  note "restoring the network"
  sudo pfctl -vsr 2>&1 | grep -E 'block|Packets' || true
  if ! kill -0 "${watchdog}" 2>/dev/null; then
    printf 'the watchdog fired, so this window was not fully offline\n'
    mismatches=$((mismatches + 1))
  fi
  kill "${watchdog}" 2>/dev/null || true
  sudo pfctl -d 2>&1 | grep -v '^Token' || true
  sudo pfctl -F all >/dev/null 2>&1 || true
  sleep 2
  curl -sS -o /dev/null --max-time 30 -w 'after the cut: HTTP %{http_code}\n' \
    "${apple_url}"
}

store_state() {
  note "ticket store, $1"
  sudo find /var/db/SystemPolicyConfiguration -iname '*ticket*' \
    -exec shasum -a 256 {} + 2>&1 || true
}

system_log() {
  note "system log, ticket activity"
  log show --last 10m --info --debug --style compact \
    --predicate 'eventMessage CONTAINS[c] "ticket" OR eventMessage CONTAINS[c] "notariz"' \
    2>&1 | tail -n 80 || true
}

# Asks Apple's ticket service for a cdhash directly, over the network.
apple_lookup() {
  local bundle="$1"
  local cdhash
  cdhash="$(codesign -dvvv "${bundle}" 2>&1 | sed -n 's/^CDHash=//p')"
  note "Apple's ticket service, cdhash ${cdhash}"
  curl -sS --max-time 30 -H 'Content-Type: application/json' \
    -d "{\"records\":[{\"recordName\":\"2/2/${cdhash}\"}]}" "${lookup_url}" \
    | jq -c '.records[0] | {recordName, serverErrorCode, reason, ticket: ((.fields // {}) | keys)}' \
    || true
}

gatekeeper() {
  note "Gatekeeper assessment"
  spctl -a -vvv -t exec "$1" 2>&1 || true
}

# Runs a command while recording DNS queries and outbound connection attempts.
capture() {
  local name="$1"
  shift
  local file="${work}/capture-${name}.txt"
  local pidfile="${work}/capture-${name}.pid"
  local filter='udp port 53 or (tcp[tcpflags] & (tcp-syn|tcp-ack) == tcp-syn)'
  local interface pid
  interface="$(route -n get default | awk '/interface:/ { print $2 }')"
  # sudo drops signals sent from its own process group, so tcpdump's real pid
  # is the one to kill.
  sudo sh -c 'echo $$ > "$1"; exec tcpdump -l -n -i "$2" "$3" > "$4" 2>/dev/null' \
    capture "${pidfile}" "${interface}" "${filter}" "${file}" &
  pid=$!
  sleep 2
  "$@"
  sleep 3
  sudo kill "$(cat "${pidfile}")" 2>/dev/null || true
  wait "${pid}" 2>/dev/null || true
  note "capture ${name}: $(grep -c . "${file}" || true) packets"
  grep -i -E 'apple|cloudkit|icloud' "${file}" || printf 'no Apple hostnames\n'
}

flip_byte() {
  python3 - "$1" "$2" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
offset = int(sys.argv[2])
data = bytearray(path.read_bytes())
data[offset] ^= 0xFF
path.write_bytes(data)
print(f"flipped byte {offset} of {len(data)}")
PY
}

row_1() {
  local unstapled
  unstapled="$(unpack A-unstapled a)"
  net_off
  check "row 1: unstapled, store never saw it" spec fail "${unstapled}"
  net_on
}

row_2() {
  local signed
  signed="$(unpack C-signed c)"
  net_off
  check "row 2: never submitted, offline" spec fail "${signed}"
  net_on
  check "row 2: never submitted, online" spec fail "${signed}"
  check "row 2: never submitted, online, network allowed" relaxed fail "${signed}"
  gatekeeper "${signed}"
  apple_lookup "${signed}"
}

row_3_4() {
  local stapled unstapled
  stapled="$(unpack A-stapled stapled)"
  unstapled="$(unpack A-unstapled unstapled)"
  store_state "before"
  net_off
  check "row 3 precondition: executables before the bundle" spec fail "${stapled}" nested
  check "row 3: stapled" spec pass "${stapled}"
  store_state "after row 3"
  check "row 4: unstapled, after row 3" spec pass "${unstapled}"
  sudo killall syspolicyd 2>&1 || true
  sleep 3
  check "row 4: unstapled, after a syspolicyd restart" spec pass "${unstapled}"
  net_on
}

row_5() {
  local unstapled
  unstapled="$(unpack A-unstapled a)"
  store_state "before"
  capture spec check "row 5: unstapled, online" spec record "${unstapled}"
  capture relaxed check "row 5: unstapled, online, network allowed" relaxed record "${unstapled}"
  capture gatekeeper gatekeeper "${unstapled}"
  check "row 5: unstapled, after the Gatekeeper assessment" spec record "${unstapled}"
  store_state "after"
  apple_lookup "${unstapled}"
}

row_6() {
  local carrier a_unstapled b_unstapled
  carrier="$(unpack A-unstapled carrier)"
  a_unstapled="$(unpack A-unstapled a)"
  b_unstapled="$(unpack B-unstapled b)"
  cp "${dist}/B.ticket" "${carrier}/Contents/CodeResources"
  store_state "before"
  net_off
  check "row 6 precondition: A unstapled" spec fail "${a_unstapled}"
  check "row 6 precondition: B unstapled" spec fail "${b_unstapled}"
  check "row 6: A carrying B's ticket" spec fail "${carrier}"
  check "row 6: A unstapled, afterwards" spec fail "${a_unstapled}"
  check "row 6 control: B unstapled, B's ticket now registered" spec record "${b_unstapled}"
  store_state "after"
  net_on
}

row_7() {
  local stapled damaged unstapled ticket pristine size index offset status
  stapled="$(unpack A-stapled stapled)"
  damaged="$(unpack A-stapled damaged)"
  unstapled="$(unpack A-unstapled unstapled)"
  ticket="${damaged}/Contents/CodeResources"
  pristine="${work}/ticket.pristine"
  cp "${ticket}" "${pristine}"
  size="$(wc -c < "${pristine}" | tr -d ' ')"
  store_state "before"
  net_off
  check "row 7 precondition: unstapled" spec fail "${unstapled}"

  flip_byte "${ticket}" "$((size / 2))"
  check "row 7: one ticket byte changed" spec fail "${damaged}" bundle
  check "row 7: unstapled, afterwards" spec fail "${unstapled}"
  store_state "after row 7"

  note "row 7 sweep: 64 offsets across ${size} bytes"
  for ((index = 0; index < 64; index++)); do
    offset=$((index * (size - 1) / 63))
    cp "${pristine}" "${ticket}"
    flip_byte "${ticket}" "${offset}" >/dev/null
    status=0
    "${probe_bin}" spec "${team}" "${damaged}" bundle >/dev/null || status=$?
    if [[ "${status}" -ne 1 ]]; then
      printf 'offset %s gave status %s and stopped the sweep\n' "${offset}" "${status}"
      mismatches=$((mismatches + 1))
      break
    fi
  done
  printf 'sweep rejected %s of 64 offsets\n' "${index}"
  printf '| row 7 sweep: rejected offsets | spec | %s of 64 | 64 of 64 |\n' \
    "${index}" >> "${summary}"

  check "row 7 control: the intact staple" spec record "${stapled}"
  store_state "after the control"
  net_on
}

note "fresh VM"
printf 'runner=%s image=%s/%s boot_session=%s\n' "${RUNNER_NAME:-}" \
  "${ImageOS:-}" "${ImageVersion:-}" "$(sysctl -n kern.bootsessionuuid)"
sw_vers
uname -m
cat "${dist}/cdhashes.txt"
{
  printf '### Row %s on %s\n\n' "${row}" "${ImageOS:-unknown}"
  printf '| Check | Flags | Observed | Expected |\n| --- | --- | --- | --- |\n'
} >> "${summary}"

case "${row}" in
  1) row_1 ;;
  2) row_2 ;;
  3-4) row_3_4 ;;
  5) row_5 ;;
  6) row_6 ;;
  7) row_7 ;;
  *) printf 'unknown row %s\n' "${row}" >&2; exit 2 ;;
esac

system_log
note "mismatches: ${mismatches}"
[[ "${mismatches}" -eq 0 ]]
