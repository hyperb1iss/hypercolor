#!/usr/bin/env bash
# In-guest driver for the Linux user guest proofs. guest-proof.sh copies it
# to /usr/local/bin/hc-guest-driver and runs it as the ordinary user.
#
#   hc-guest-driver install VERSION [--kill-at ACTION] [--with-receipt]
#                           [--delay-ms N] [--kill-service] [-- ARGS...]
#       Run VERSION's own bin/hypercolor as the release installer. With
#       --kill-at, watch the install journal; once its next action is ACTION
#       (and, with --with-receipt, the candidate receipt is recorded), wait
#       --delay-ms and SIGKILL the installer, or with --kill-service the
#       service's main process instead. ARGS go to the installer. Only the
#       installer process is killed; a systemctl child it is waiting on may
#       still finish, as after any single-process crash.
#   hc-guest-driver state
#       Print the journal, active pointer, service properties and /health.
#   hc-guest-driver health
#       Print /health, or "unreachable".
set -euo pipefail

RELEASES=/releases
CANDIDATES="${HOME}/candidates"
STATE_ROOT="${HOME}/.local/state/hypercolor/update"
JOURNAL="${STATE_ROOT}/install-journal.json"
ACTIVE="${HOME}/.local/share/hypercolor/releases/active"

die() {
    printf 'hc-guest-driver: %s\n' "$*" >&2
    exit 2
}

extract() {
    local version="$1"
    local archive="${RELEASES}/hypercolor-${version}-linux-amd64.tar.gz"
    local tree="${CANDIDATES}/hypercolor-${version}-linux-amd64"
    [[ -f "${archive}" ]] || die "no archive for ${version}"
    if [[ ! -x "${tree}/bin/hypercolor" ]]; then
        mkdir -p "${CANDIDATES}"
        tar -xzf "${archive}" -C "${CANDIDATES}"
    fi
    printf '%s\n' "${tree}"
}

health() {
    curl -fsS --max-time 5 http://127.0.0.1:9420/health 2>/dev/null || printf 'unreachable'
    printf '\n'
}

state() {
    printf '== journal\n'
    if [[ -f "${JOURNAL}" ]]; then
        python3 - "${JOURNAL}" <<'PY'
import json
import sys

journal = json.load(open(sys.argv[1]))
receipt = journal.get("candidate_owner_receipt")
summary = {
    "revision": journal["revision"],
    "transaction_id": journal["transaction_id"],
    "disposition": journal["disposition"],
    "next_action": journal["next_action"],
    "candidate_unit": journal["candidate_unit"],
    "prior_active_unit": journal["prior_active_unit"],
    "receipt": receipt is not None,
    "abandoned": journal.get("abandoned", False),
    "failure": journal.get("failure"),
}
print(json.dumps(summary, indent=2))
PY
    else
        printf 'absent\n'
    fi
    printf '== active\n'
    readlink "${ACTIVE}" 2>/dev/null || printf 'absent\n'
    printf '== service\n'
    systemctl --user show hypercolor.service \
        -p LoadState -p ActiveState -p SubState -p UnitFileState \
        -p MainPID -p InvocationID -p NRestarts -p Job 2>&1 || true
    printf '== session bus\n'
    if [[ -e "${XDG_RUNTIME_DIR}/bus" ]]; then printf 'reachable\n'; else printf 'hidden\n'; fi
    printf '== health\n'
    health
}

install() {
    local version="$1"
    shift
    local kill_at="" with_receipt=0 delay_ms=0 kill_service=0
    while (($#)); do
        case "$1" in
            --kill-at) kill_at="$2"; shift 2 ;;
            --with-receipt) with_receipt=1; shift ;;
            --delay-ms) delay_ms="$2"; shift 2 ;;
            --kill-service) kill_service=1; shift ;;
            --) shift; break ;;
            *) die "unknown install option $1" ;;
        esac
    done
    local tree digest
    tree="$(extract "${version}")"
    digest="$(sha256sum "${tree}/manifest.json" | cut -d' ' -f1)"
    python3 - "${JOURNAL}" "${kill_at}" "${with_receipt}" "${delay_ms}" "${kill_service}" \
        "${tree}/bin/hypercolor" __install-release \
        --install-prefix "${HOME}/.local" \
        --install-dir "${HOME}/.local/bin" \
        --expected-manifest-sha256 "${digest}" "$@" <<'PY'
import json
import os
import signal
import subprocess
import sys
import time

journal_path, kill_at, with_receipt, delay_ms, kill_service = sys.argv[1:6]
command = sys.argv[6:]
with_receipt = with_receipt == "1"
delay = int(delay_ms) / 1000
kill_service = kill_service == "1"


def journal():
    try:
        with open(journal_path, "rb") as handle:
            return json.loads(handle.read())
    except (OSError, ValueError):
        return None


started = time.monotonic()
installer = subprocess.Popen(command)
observed = None
acted = None
last = None
while kill_at and installer.poll() is None and observed is None:
    try:
        stat = os.stat(journal_path)
        key = (stat.st_ino, stat.st_mtime_ns, stat.st_size)
    except OSError:
        key = None
    if key is not None and key != last:
        last = key
        current = journal()
        if (
            current is not None
            and current.get("next_action") == kill_at
            and (not with_receipt or current.get("candidate_owner_receipt") is not None)
        ):
            observed = time.monotonic()
            break
    time.sleep(0.0005)

if observed is not None:
    deadline = observed + delay
    while installer.poll() is None and time.monotonic() < deadline:
        time.sleep(0.005)
    if installer.poll() is None:
        current = journal() or {}
        acted = current.get("next_action")
        if kill_service:
            subprocess.run(
                ["systemctl", "--user", "kill", "--kill-whom=main",
                 "--signal=SIGKILL", "hypercolor.service"],
                check=False,
            )
        else:
            installer.send_signal(signal.SIGKILL)

status = installer.wait()
elapsed_ms = int((time.monotonic() - started) * 1000)
# The journal once the installer is gone: after a kill it must still name
# the watched action, or the installer got past it before the signal landed.
settled = (journal() or {}).get("next_action")
result = {
    "exit": status,
    "elapsed_ms": elapsed_ms,
    "watched": kill_at or None,
    "observed_ms": None if observed is None else int((observed - started) * 1000),
    "acted_at": acted,
    "settled_at": settled,
    "action": None if acted is None else ("kill_service" if kill_service else "kill_installer"),
}
print("DRIVER " + json.dumps(result), flush=True)
PY
}

command="${1:-}"
shift || true
case "${command}" in
    install) (($#)) || die "install needs a version"; install "$@" ;;
    state) state ;;
    health) health ;;
    *) die "usage: hc-guest-driver install|state|health" ;;
esac
