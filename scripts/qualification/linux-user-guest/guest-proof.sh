#!/usr/bin/env bash
# Ordinary-user systemd guest proofs for the Linux release installer.
#
# Each scenario boots a fresh rootless podman Ubuntu 24.04 guest with systemd
# as PID 1, installs qualification releases as uid 1100 through the release
# installer built from this checkout, injects faults, kills the installer at
# journal actions, cuts power, and checks where the installation ends up.
# See README.md in this directory.
set -Eeuo pipefail
trap 'printf "guest-proof: command failed at line %s: %s\n" "${LINENO}" "${BASH_COMMAND}" >&2' ERR

HARNESS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${HARNESS_DIR}/../../.." && pwd)"
WORK="${REPO_ROOT}/target/linux-user-guest"
RECEIPT_ROOT="${HC_GUEST_RECEIPTS:-${HARNESS_DIR}/receipts}"
BASE_VERSION="${HC_GUEST_BASE_VERSION:-0.5.1}"
GUEST_UID=1100
GUEST_HOME=/home/qualification
GUEST_RUNTIME="/run/user/${GUEST_UID}"
GUEST_UNITS="${GUEST_HOME}/.local/share/hypercolor/releases/units"

V_A="${BASE_VERSION}-qual.1"
V_B="${BASE_VERSION}-qual.2"
V_C="${BASE_VERSION}-qual.3"
# V_D ships a companion unit template (companions/); V_E ships a different
# one under the same name (companions-alt/), which must never replace it.
V_D="${BASE_VERSION}-qual.4"
V_E="${BASE_VERSION}-qual.5"

GUEST=""
RECEIPT=""
STEP=0
FAILED=()
# Guests carry the checkout they belong to, so `clean` in one worktree never
# removes a guest another worktree is running.
CHECKOUT_LABEL="hypercolor.qualification.checkout=$(printf '%s' "${REPO_ROOT}" | sha256sum | cut -c1-16)"

# shellcheck source-path=SCRIPTDIR source=scenarios.sh
source "${HARNESS_DIR}/scenarios.sh"

# ─── Output ──────────────────────────────────────────────────────────────────

log() {
    local line
    line="$(date -u +%H:%M:%S.%3N) $*"
    printf '%s\n' "${line}"
    if [[ -n "${RECEIPT}" ]]; then
        printf '%s\n' "${line}" >>"${RECEIPT}/steps.log"
    fi
}

fail() {
    log "FAIL: $*"
    exit 1
}

usage() {
    cat <<USAGE
Usage: $(basename "$0") <command>

  list                 List scenarios
  build                Build the guest images, the installer CLI and releases
  run <scenario>...    Run scenarios, each in a fresh guest
  all                  Run every scenario
  clean                Remove guests this harness left behind

Environment:
  HC_GUEST_BASE_VERSION  Published release the archives start from ($BASE_VERSION)
  HC_GUEST_KEEP=1        Keep each guest running after its scenario
  HC_GUEST_RECEIPTS      Receipt directory (${HARNESS_DIR}/receipts)
USAGE
}

# ─── Build ───────────────────────────────────────────────────────────────────

toolchain() {
    sed -n 's/^channel *= *"\(.*\)"/\1/p' "${REPO_ROOT}/rust-toolchain.toml"
}

content_tag() {
    cat "$@" | sha256sum | cut -c1-16
}

guest_image() {
    printf 'localhost/hypercolor-linux-user-guest:%s' \
        "$(content_tag "${HARNESS_DIR}/guest.Containerfile")"
}

builder_image() {
    printf 'localhost/hypercolor-linux-user-builder:%s' \
        "$(printf '%s\n' "$(toolchain)" | content_tag "${HARNESS_DIR}/builder.Containerfile" -)"
}

ensure_image() {
    local tag="$1" file="$2"
    shift 2
    if ! podman image exists "${tag}"; then
        log "building image ${tag}"
        podman build --pull=missing -f "${file}" -t "${tag}" "$@" "${HARNESS_DIR}"
    fi
}

build() {
    mkdir -p "${WORK}/downloads" "${WORK}/releases" "${WORK}/bin" "${WORK}/cargo-home"
    ensure_image "$(guest_image)" "${HARNESS_DIR}/guest.Containerfile"
    ensure_image "$(builder_image)" "${HARNESS_DIR}/builder.Containerfile" \
        --build-arg "RUST_TOOLCHAIN=$(toolchain)"

    local tarball="hypercolor-${BASE_VERSION}-linux-amd64.tar.gz"
    if [[ ! -f "${WORK}/downloads/${tarball}" ]]; then
        log "downloading ${tarball}"
        local url="https://github.com/hyperb1iss/hypercolor/releases/download/v${BASE_VERSION}/${tarball}"
        curl -fsSL -o "${WORK}/downloads/${tarball}.partial" "${url}"
        curl -fsSL -o "${WORK}/downloads/${tarball}.sha256" "${url}.sha256"
        mv "${WORK}/downloads/${tarball}.partial" "${WORK}/downloads/${tarball}"
    fi
    (cd "${WORK}/downloads" && sha256sum --quiet -c "${tarball}.sha256")

    log "building the installer CLI and qualification daemon in $(builder_image)"
    podman run --rm --userns=keep-id --security-opt label=disable \
        -v "${REPO_ROOT}:/src:ro" -v "${WORK}:/work" \
        -e CARGO_HOME=/work/cargo-home -e "RUSTUP_TOOLCHAIN=$(toolchain)" \
        -e CARGO_TERM_COLOR=never -w /src "$(builder_image)" bash -c '
            set -euo pipefail
            cargo build --locked -p hypercolor-cli --bin hypercolor \
                --target-dir /work/cargo-target
            install -m 0755 /work/cargo-target/debug/hypercolor /work/bin/hypercolor
            strip /work/bin/hypercolor
            rustc --edition 2024 -O -o /work/bin/hc-qual-daemon \
                /src/scripts/qualification/linux-user-guest/hc-qual-daemon.rs
            strip /work/bin/hc-qual-daemon
            ldd --version | sed -n 1p'

    local inputs
    inputs="$(cat "${WORK}/bin/hypercolor" "${WORK}/bin/hc-qual-daemon" \
        "${HARNESS_DIR}/make_release.py" "${REPO_ROOT}/packaging/managed/durable-stores.json" \
        "${HARNESS_DIR}/companions/"* "${HARNESS_DIR}/companions-alt/"* \
        "${WORK}/downloads/${tarball}" | sha256sum | cut -c1-64)"
    if [[ "$(cat "${WORK}/releases/inputs" 2>/dev/null)" != "${inputs}" ]]; then
        rm -f "${WORK}/releases/"*
        local version
        for version in "${V_A}" "${V_B}" "${V_C}" "${V_D}" "${V_E}"; do
            log "packing qualification release ${version}"
            local companions=()
            if [[ "${version}" == "${V_D}" ]]; then
                companions=(--companions "${HARNESS_DIR}/companions/companions.json")
            elif [[ "${version}" == "${V_E}" ]]; then
                companions=(--companions "${HARNESS_DIR}/companions-alt/companions.json")
            fi
            python3 "${HARNESS_DIR}/make_release.py" \
                --base "${WORK}/downloads/${tarball}" \
                --cli "${WORK}/bin/hypercolor" \
                --daemon "${WORK}/bin/hc-qual-daemon" \
                --version "${version}" "${companions[@]}" \
                --out "${WORK}/releases/hypercolor-${version}-linux-amd64.tar.gz" >/dev/null
        done
        printf '%s\n' "${inputs}" >"${WORK}/releases/inputs"
    fi
    local version
    for version in "${V_A}" "${V_B}" "${V_C}" "${V_D}" "${V_E}"; do
        log "release ${version} unit $(unit_of "${version}")"
    done
}

unit_of() {
    cat "${WORK}/releases/hypercolor-$1-linux-amd64.tar.gz.manifest-sha256"
}

# ─── Guest ───────────────────────────────────────────────────────────────────

groot() {
    podman exec "${GUEST}" "$@"
}

gx() {
    podman exec --user "${GUEST_UID}" -w "${GUEST_HOME}" \
        -e "HOME=${GUEST_HOME}" -e "XDG_RUNTIME_DIR=${GUEST_RUNTIME}" \
        "${GUEST}" "$@"
}

wait_boot() {
    local deadline=$((SECONDS + 90)) state=""
    while ((SECONDS < deadline)); do
        state="$(groot systemctl is-system-running 2>/dev/null || true)"
        if [[ "${state}" == running || "${state}" == degraded ]] &&
            groot test -S "${GUEST_RUNTIME}/systemd/private" 2>/dev/null; then
            hide_bus
            return 0
        fi
        sleep 0.5
    done
    fail "guest did not boot (system state ${state:-unknown})"
}

# Only the manager's private socket stays reachable, as in the activator
# units, so an installer that reached for the session bus fails here.
hide_bus() {
    if gx test -e "${GUEST_RUNTIME}/bus"; then
        gx mv "${GUEST_RUNTIME}/bus" "${GUEST_RUNTIME}/bus.hidden"
    fi
    if gx test -e "${GUEST_RUNTIME}/bus"; then
        fail "the session bus is still reachable"
    fi
}

guest_up() {
    local name
    name="hc-guest-proof-$1-$(date -u +%Y%m%d%H%M%S)-${RANDOM}"
    podman run -d --name "${name}" --label hypercolor.qualification=linux-user-guest \
        --label "${CHECKOUT_LABEL}" --security-opt label=disable \
        --systemd=always --cgroupns=private --pid=private --network=none \
        -v "${WORK}/releases:/releases:ro" "$(guest_image)" >/dev/null
    GUEST="${name}"
    podman cp "${HARNESS_DIR}/guest-driver.sh" "${GUEST}:/usr/local/bin/hc-guest-driver"
    groot chmod 0755 /usr/local/bin/hc-guest-driver
    wait_boot
    gx mkdir -p "${GUEST_HOME}/hc-qual/faults"
    log "guest ${GUEST} up: $(groot systemctl --version | head -n 1)"
}

guest_down() {
    [[ -n "${GUEST}" ]] || return 0
    if [[ "${HC_GUEST_KEEP:-0}" == 1 ]]; then
        log "keeping guest ${GUEST}"
    else
        podman rm -f -t 0 "${GUEST}" >/dev/null 2>&1 || true
    fi
    GUEST=""
}

# Power cut: kill the whole guest, then boot it again from its disk. The
# user manager autostarts an enabled hypercolor.service from the on-disk
# pointer and fragment before anything inspects it.
power_cut() {
    log "power cut"
    STEP=$((STEP + 1))
    gx journalctl --user -u hypercolor.service --no-pager -o short-monotonic \
        >"${RECEIPT}/$(printf '%02d' "${STEP}")-service-journal-before-power-cut.txt" 2>&1 || true
    podman kill --signal KILL "${GUEST}" >/dev/null
    podman wait "${GUEST}" >/dev/null 2>&1 || true
    podman start "${GUEST}" >/dev/null
    wait_boot
    log "guest booted again"
}

set_faults() {
    local version="$1"
    shift
    log "faults ${version}: $*"
    printf '%s\n' "$@" | podman exec -i --user "${GUEST_UID}" "${GUEST}" \
        tee "${GUEST_HOME}/hc-qual/faults/${version}" >/dev/null
}

clear_faults() {
    gx rm -f "${GUEST_HOME}/hc-qual/faults/$1"
}

# ─── Steps and expectations ──────────────────────────────────────────────────

DRIVER_EXIT=""
DRIVER_ELAPSED_MS=""
DRIVER_WATCHED=""
DRIVER_ACTED_AT=""
DRIVER_SETTLED_AT=""
DRIVER_ACTION=""
LAST_INSTALL_OUTPUT=""

install_run() {
    local label="$1" version="$2"
    shift 2
    STEP=$((STEP + 1))
    local output
    output="${RECEIPT}/$(printf '%02d' "${STEP}")-install-${label}.log"
    hide_bus
    log "install ${label}: ${version} $*"
    gx hc-guest-driver install "${version}" "$@" >"${output}" 2>&1 || true
    local driver
    driver="$(sed -n 's/^DRIVER //p' "${output}" | tail -n 1)"
    [[ -n "${driver}" ]] || fail "driver produced no result for ${label} (see ${output})"
    DRIVER_EXIT="$(json_field "${driver}" exit)"
    DRIVER_ELAPSED_MS="$(json_field "${driver}" elapsed_ms)"
    DRIVER_WATCHED="$(json_field "${driver}" watched)"
    DRIVER_ACTED_AT="$(json_field "${driver}" acted_at)"
    DRIVER_SETTLED_AT="$(json_field "${driver}" settled_at)"
    DRIVER_ACTION="$(json_field "${driver}" action)"
    LAST_INSTALL_OUTPUT="${output}"
    log "  exit=${DRIVER_EXIT} elapsed_ms=${DRIVER_ELAPSED_MS} action=${DRIVER_ACTION} at=${DRIVER_ACTED_AT}"
    snapshot "after-${label}"
}

json_field() {
    python3 -c 'import json,sys; v=json.loads(sys.argv[1]).get(sys.argv[2]); print("" if v is None else v)' "$1" "$2"
}

snapshot() {
    STEP=$((STEP + 1))
    gx hc-guest-driver state >"${RECEIPT}/$(printf '%02d' "${STEP}")-state-$1.txt" 2>&1 || true
}

journal_field() {
    local path="${GUEST_HOME}/.local/state/hypercolor/update/install-journal.json"
    gx python3 -c 'import json,sys; j=json.load(open(sys.argv[1])); v=j.get(sys.argv[2]); print("" if v is None else ("true" if v is True else ("false" if v is False else v)))' \
        "${path}" "$1"
}

service_prop() {
    gx systemctl --user show hypercolor.service -p "$1" --value
}

expect_exit() {
    case "$1" in
        0) [[ "${DRIVER_EXIT}" == 0 ]] || fail "installer exited ${DRIVER_EXIT}, expected success" ;;
        nonzero) [[ "${DRIVER_EXIT}" != 0 ]] || fail "installer succeeded, expected a failure exit" ;;
    esac
    log "  ok: installer exit ${DRIVER_EXIT}"
}

# The driver acted, and the journal named the watched action both when it
# acted and, for an installer kill, after the installer was gone. Anything
# else proves a different interruption than the scenario names.
expect_acted() {
    [[ "${DRIVER_ACTION}" == "$1" ]] ||
        fail "driver did not ${1//_/ } at its kill point (action '${DRIVER_ACTION}'); rerun"
    [[ "${DRIVER_ACTED_AT}" == "${DRIVER_WATCHED}" ]] ||
        fail "driver acted at ${DRIVER_ACTED_AT}, past ${DRIVER_WATCHED}; rerun"
    if [[ "$1" == kill_installer && "${DRIVER_SETTLED_AT}" != "${DRIVER_WATCHED}" ]]; then
        fail "the killed installer left the journal at ${DRIVER_SETTLED_AT}, past ${DRIVER_WATCHED}; rerun"
    fi
    log "  ok: ${1//_/ } at ${DRIVER_ACTED_AT}"
}

expect_journal() {
    local disposition="$1" abandoned="${2:-false}"
    local actual
    actual="$(journal_field disposition)"
    [[ "${actual}" == "${disposition}" ]] || fail "journal disposition ${actual}, expected ${disposition}"
    actual="$(journal_field abandoned)"
    [[ "${actual:-false}" == "${abandoned}" ]] || fail "journal abandoned=${actual:-false}, expected ${abandoned}"
    log "  ok: journal ${disposition} abandoned=${abandoned}"
}

expect_active() {
    local actual
    actual="$(gx readlink "${GUEST_HOME}/.local/share/hypercolor/releases/active" || true)"
    [[ "${actual}" == "units/$(unit_of "$1")" ]] || fail "active pointer ${actual}, expected $1"
    log "  ok: active pointer names $1"
}

expect_health() {
    local version="$1" deadline=$((SECONDS + 30)) body=""
    while ((SECONDS < deadline)); do
        body="$(gx hc-guest-driver health)"
        if [[ "${body}" == *"\"version\":\"${version}\""* ]]; then
            log "  ok: /health answers ${version}"
            return 0
        fi
        sleep 0.5
    done
    fail "/health answered ${body}, expected ${version}"
}

expect_prop() {
    local actual
    actual="$(service_prop "$1")"
    [[ "${actual}" == "$2" ]] || fail "hypercolor.service $1=${actual}, expected $2"
    log "  ok: $1=$2"
}

expect_output() {
    grep -qF -- "$1" "${LAST_INSTALL_OUTPUT}" || fail "installer output lacks: $1"
    log "  ok: installer said: $1"
}

# A rollback report names the release that runs again, and it is exactly
# the process systemd shows for the service.
expect_restored_receipt() {
    local version="$1" line pid invocation
    line="$(grep -m1 -oE 'rolled back: [^ ]+ \(unit [0-9a-f]{64}\) runs again as pid [0-9]+, invocation [0-9a-f]+' \
        "${LAST_INSTALL_OUTPUT}" || true)"
    [[ -n "${line}" ]] || fail "installer output names no restored release"
    [[ "${line}" == "rolled back: ${version} (unit $(unit_of "${version}")) runs again as pid "* ]] ||
        fail "the restored release is not ${version}: ${line}"
    pid="$(sed -E 's/.* pid ([0-9]+),.*/\1/' <<<"${line}")"
    invocation="$(sed -E 's/.*invocation ([0-9a-f]+)$/\1/' <<<"${line}")"
    [[ "$(service_prop MainPID)" == "${pid}" ]] ||
        fail "restored pid ${pid} is not the service's MainPID $(service_prop MainPID)"
    [[ "$(service_prop InvocationID)" == "${invocation}" ]] ||
        fail "restored invocation ${invocation} is not the service's $(service_prop InvocationID)"
    log "  ok: the rollback names ${version} as pid ${pid}, invocation ${invocation}"
}

# The releases directory holds exactly the units of these versions.
expect_units() {
    local expected actual version
    expected="$(for version in "$@"; do unit_of "${version}"; done | sort)"
    actual="$(gx ls -A "${GUEST_UNITS}" | sort)"
    [[ "${actual}" == "${expected}" ]] ||
        fail "units are $(tr '\n' ' ' <<<"${actual}"), expected those of $*"
    log "  ok: units are exactly those of $*"
}

GUEST_RELEASES="${GUEST_HOME}/.local/share/hypercolor/releases"

# Run the installation's launcher as a recovery unit would: update-executor
# role, __recover-release. Sets DRIVER_EXIT and LAST_INSTALL_OUTPUT.
recover_run() {
    local label="$1"
    shift
    STEP=$((STEP + 1))
    local output
    output="${RECEIPT}/$(printf '%02d' "${STEP}")-recover-${label}.log"
    hide_bus
    log "recover ${label}: $*"
    gx hc-guest-driver recover "$@" >"${output}" 2>&1 || true
    DRIVER_EXIT="$(sed -n 's/^DRIVER //p' "${output}" | tail -n 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["exit"])')"
    LAST_INSTALL_OUTPUT="${output}"
    log "  exit=${DRIVER_EXIT}"
    snapshot "after-${label}"
}

# Save and print the qualification daemon's launch report.
launch_report() {
    STEP=$((STEP + 1))
    local output
    output="${RECEIPT}/$(printf '%02d' "${STEP}")-launch-$1.json"
    gx hc-guest-driver launch-report >"${output}"
    cat "${output}"
}

# The running daemon, its UI and its effects all come from VERSION's own
# release directory, never through the active pointer, both as the daemon
# reports itself and as /proc shows its main process.
expect_launch() {
    local version="$1" unit report pid
    unit="${GUEST_RELEASES}/units/$(unit_of "${version}")"
    report="$(launch_report "${version}")"
    pid="$(service_prop MainPID)"
    local exe cmdline
    exe="$(gx readlink "/proc/${pid}/exe")"
    cmdline="$(gx cat "/proc/${pid}/cmdline" | tr '\0' ' ')"
    python3 - "${report}" "${unit}" "${exe}" "${cmdline}" <<'PY' || fail "the running daemon is not ${version}'s whole release"
import json
import sys

report, unit, exe, cmdline = sys.argv[1:5]
launch = json.loads(report)
argv = launch["argv"]
expected = [
    f"{unit}/bin/hypercolor-daemon",
    "--ui-dir", f"{unit}/share/hypercolor/ui",
    "--effects-dir", f"{unit}/share/hypercolor/effects/bundled",
]
problems = []
if launch["exe"] != expected[0]:
    problems.append(f"reported exe {launch['exe']}")
if exe != expected[0]:
    problems.append(f"/proc exe {exe}")
if argv != expected:
    problems.append(f"argv {argv}")
if cmdline.split() != expected:
    problems.append(f"/proc cmdline {cmdline}")
if problems:
    sys.exit("; ".join(problems))
PY
    log "  ok: ${version}'s daemon runs with its own UI and effects (/proc agrees)"
}

# Every directory the sandbox must let the daemon write is writable, and
# every other one is not.
expect_writes() {
    local report
    report="$(launch_report writes)"
    python3 - "${report}" <<'PY' || fail "the sandbox's writable set is wrong"
import json
import sys

writes = json.loads(sys.argv[1])["writes"]
writable = {"config", "data", "daemon_state", "coordinator", "cache", "tmp", "var_tmp"}
denied = {"releases", "update_state", "activator", "local_bin", "legacy_lib", "user_units", "home"}
problems = [f"{label}={writes.get(label)}" for label in sorted(writable) if writes.get(label) != "ok"]
problems += [
    f"{label}={writes.get(label)}"
    for label in sorted(denied)
    if not str(writes.get(label, "")).startswith("denied")
]
if set(writes) != writable | denied:
    problems.append(f"probed {sorted(writes)}")
print(json.dumps(writes, indent=2))
if problems:
    sys.exit("; ".join(problems))
PY
    log "  ok: writable exactly config, data, daemon state, coordinator, cache and private tmp"
}

wait_active() {
    local deadline=$((SECONDS + ${1:-60}))
    while ((SECONDS < deadline)); do
        if [[ "$(service_prop ActiveState)" == active ]]; then
            return 0
        fi
        sleep 0.5
    done
    fail "hypercolor.service never became active"
}

# ─── Scenario runner ─────────────────────────────────────────────────────────

list_scenarios() {
    declare -F | sed -n 's/^declare -f scenario_//p' | tr '_' '-'
}

# Runs one scenario and records a failure in FAILED. It must be called as a
# plain command, never under `if`, `||` or `&&`: bash ignores `set -e` for
# everything beneath those, including the scenario's own subshell.
run_one() {
    local name="$1" fn="scenario_${1//-/_}"
    if ! declare -F "${fn}" >/dev/null; then
        printf 'unknown scenario %s\n' "${name}" >&2
        FAILED+=("${name}")
        return 0
    fi
    RECEIPT="${RECEIPT_ROOT}/$(date -u +%Y%m%dT%H%M%SZ)-${name}"
    STEP=0
    mkdir -p "${RECEIPT}"
    {
        printf 'scenario: %s\n' "${name}"
        printf 'source: %s%s\n' "$(git -C "${REPO_ROOT}" rev-parse HEAD)" \
            "$(git -C "${REPO_ROOT}" diff --quiet HEAD -- || printf ' (dirty)')"
        printf 'cli_sha256: %s\n' "$(sha256sum "${WORK}/bin/hypercolor" | cut -c1-64)"
        printf 'guest_image: %s %s\n' "$(guest_image)" "$(podman image inspect "$(guest_image)" --format '{{.Id}}')"
        printf 'releases: %s=%s %s=%s %s=%s %s=%s %s=%s\n' "${V_A}" "$(unit_of "${V_A}")" \
            "${V_B}" "$(unit_of "${V_B}")" "${V_C}" "$(unit_of "${V_C}")" \
            "${V_D}" "$(unit_of "${V_D}")" "${V_E}" "$(unit_of "${V_E}")"
    } >"${RECEIPT}/receipt.txt"
    log "scenario ${name}"
    local status
    set +e
    (
        set -e
        trap 'guest_down' EXIT
        guest_up "${name}"
        printf 'guest_systemd: %s\n' "$(groot systemctl --version | head -n 1)" >>"${RECEIPT}/receipt.txt"
        "${fn}"
        gx journalctl --user -u hypercolor.service --no-pager -o short-monotonic \
            >"${RECEIPT}/service-journal.txt" 2>&1 || true
    )
    status=$?
    set -e
    if ((status == 0)); then
        printf 'verdict: PASS\n' >>"${RECEIPT}/receipt.txt"
        log "PASS ${name} (receipts: ${RECEIPT})"
    else
        printf 'verdict: FAIL\n' >>"${RECEIPT}/receipt.txt"
        log "FAIL ${name} (receipts: ${RECEIPT})"
        FAILED+=("${name}")
    fi
    RECEIPT=""
}

run_many() {
    build
    local name
    for name in "$@"; do
        run_one "${name}"
    done
    if ((${#FAILED[@]})); then
        log "failed: ${FAILED[*]}"
        return 1
    fi
    log "all passed: $*"
}

main() {
    command -v podman >/dev/null || { echo "podman is required" >&2; exit 2; }
    local command="${1:-}"
    shift || true
    case "${command}" in
        list) list_scenarios ;;
        build) build ;;
        run) (($#)) || { usage; exit 2; }; run_many "$@" ;;
        all) mapfile -t names < <(list_scenarios); run_many "${names[@]}" ;;
        clean) podman ps -a --filter label=hypercolor.qualification=linux-user-guest \
            --filter "label=${CHECKOUT_LABEL}" -q | xargs -r podman rm -f -t 0 ;;
        *) usage; exit 2 ;;
    esac
}

main "$@"
