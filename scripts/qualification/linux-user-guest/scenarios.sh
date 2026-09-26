# shellcheck shell=bash
# Guest proof scenarios, sourced by guest-proof.sh. Each scenario_<name>
# runs in a fresh guest and fails through the expect_* helpers.
#
# Releases: V_A is the installed prior, V_B the candidate under test and V_C
# a second, healthy candidate. Faults are per version and read at each
# start of that version's daemon.

# A fresh install, then an ordinary upgrade.
scenario_baseline() {
    install_run fresh "${V_A}"
    expect_exit 0
    expect_journal committed
    expect_active "${V_A}"
    expect_health "${V_A}"
    expect_prop UnitFileState enabled

    install_run upgrade "${V_B}"
    expect_exit 0
    expect_journal committed
    expect_active "${V_B}"
    expect_health "${V_B}"
}

# The installer dies during the candidate's proof after its receipt, the
# candidate restarts under a new invocation, and a rerun rolls back.
scenario_restart_after_receipt() {
    install_run fresh "${V_A}"
    expect_exit 0
    set_faults "${V_B}" health_delay_ms=3000

    install_run interrupted "${V_B}" --kill-at prove_candidate --with-receipt --delay-ms 500
    expect_acted kill_installer
    log "restarting the candidate"
    gx systemctl --user restart hypercolor.service
    wait_active
    snapshot before-recovery

    install_run recovery "${V_B}"
    expect_exit nonzero
    expect_journal rolled_back
    expect_output "drift before ProveCandidate"
    expect_active "${V_A}"
    expect_health "${V_A}"
    expect_restored_receipt "${V_A}"
}

# The candidate is killed while the proof waits on /health; the same run
# rolls back.
scenario_crash_in_proof() {
    install_run fresh "${V_A}"
    expect_exit 0
    set_faults "${V_B}" health_delay_ms=4000

    install_run crash "${V_B}" --kill-at prove_candidate --with-receipt --delay-ms 1000 --kill-service
    expect_acted kill_service
    expect_exit nonzero
    expect_journal rolled_back
    expect_output "failed during ProveCandidate"
    expect_active "${V_A}"
    expect_health "${V_A}"
    expect_restored_receipt "${V_A}"
}

# The prior stops slowly, the installer dies at UnloadPrior, power is cut,
# and the prior autostarts under a new invocation. The baseline is lost, so
# the transaction is abandoned without effect; the next run commits.
scenario_abandon_after_power_cut() {
    set_faults "${V_A}" stop_delay_ms=15000
    install_run fresh "${V_A}"
    expect_exit 0

    install_run interrupted "${V_B}" --kill-at unload_prior --delay-ms 1500
    expect_acted kill_installer
    power_cut
    wait_active
    expect_health "${V_A}"
    snapshot before-recovery

    install_run recovery "${V_B}"
    expect_exit nonzero
    expect_journal rolled_back true
    expect_output "stopped before changing anything"
    expect_active "${V_A}"
    expect_health "${V_A}"

    install_run retry "${V_B}"
    expect_exit 0
    expect_journal committed
    expect_active "${V_B}"
    expect_health "${V_B}"
}

# A candidate that never becomes ready is starting when the installer dies
# and power is cut. It autostarts from the switched pointer and crash-loops;
# recovery settles its start job, stops it first and rolls back.
scenario_stop_first_failing() {
    install_run fresh "${V_A}"
    expect_exit 0
    set_faults "${V_B}" ready_delay_ms=2000 exit_before_ready=1

    install_run interrupted "${V_B}" --kill-at restore_candidate_runtime --delay-ms 1000
    expect_acted kill_installer
    power_cut
    sleep 3
    snapshot before-recovery

    install_run recovery "${V_B}"
    expect_exit nonzero
    expect_journal rolled_back
    # Recovery stopped the autostarted candidate, then resumed its start,
    # which failed again.
    expect_output "failed during RestoreCandidateRuntime"
    expect_active "${V_A}"
    expect_health "${V_A}"
    expect_restored_receipt "${V_A}"
    expect_prop ActiveState active
}

# The installer dies at SwitchToCandidate and power is cut; whichever
# release autostarts is stopped first and the healthy candidate commits.
scenario_stop_first_healthy() {
    install_run fresh "${V_A}"
    expect_exit 0

    install_run interrupted "${V_B}" --kill-at switch_to_candidate
    expect_acted kill_installer
    power_cut
    wait_active
    log "autostarted: $(gx hc-guest-driver health)"
    snapshot before-recovery

    install_run recovery "${V_B}"
    expect_exit 0
    expect_journal committed
    expect_active "${V_B}"
    expect_health "${V_B}"
}

# A running prior with autostart disabled: a failing candidate rolls back
# and restarts the prior, a healthy one commits, and autostart stays off.
scenario_disabled_running() {
    install_run fresh "${V_A}"
    expect_exit 0
    gx systemctl --user disable hypercolor.service
    expect_prop UnitFileState disabled
    set_faults "${V_B}" exit_before_ready=1

    install_run failing "${V_B}"
    expect_exit nonzero
    expect_journal rolled_back
    expect_output "failed during RestoreCandidateRuntime"
    expect_active "${V_A}"
    expect_health "${V_A}"
    expect_restored_receipt "${V_A}"
    expect_prop UnitFileState disabled

    install_run healthy "${V_C}"
    expect_exit 0
    expect_journal committed
    expect_active "${V_C}"
    expect_health "${V_C}"
    expect_prop UnitFileState disabled
}

# A candidate that needs 20 s to become ready commits within the unit's
# default TimeoutStartSec.
scenario_slow_start_within() {
    install_run fresh "${V_A}"
    expect_exit 0
    set_faults "${V_B}" ready_delay_ms=20000

    install_run slow "${V_B}"
    expect_exit 0
    expect_journal committed
    expect_active "${V_B}"
    expect_health "${V_B}"
    ((DRIVER_ELAPSED_MS >= 20000)) || fail "commit took ${DRIVER_ELAPSED_MS} ms, under the ready delay"
    log "  ok: committed after ${DRIVER_ELAPSED_MS} ms"
}

# A drop-in shortens TimeoutStartSec to 8 s and the candidate needs 30 s:
# systemd fails the start and the run rolls back well before 30 s.
scenario_slow_start_beyond() {
    install_run fresh "${V_A}"
    expect_exit 0
    gx mkdir -p "${GUEST_HOME}/.config/systemd/user/hypercolor.service.d"
    printf '[Service]\nTimeoutStartSec=8s\n' | podman exec -i --user "${GUEST_UID}" "${GUEST}" \
        tee "${GUEST_HOME}/.config/systemd/user/hypercolor.service.d/timeout.conf" >/dev/null
    gx systemctl --user daemon-reload
    set_faults "${V_B}" ready_delay_ms=30000

    install_run slow "${V_B}"
    expect_exit nonzero
    expect_journal rolled_back
    expect_output "failed during RestoreCandidateRuntime"
    expect_active "${V_A}"
    expect_health "${V_A}"
    expect_restored_receipt "${V_A}"
    ((DRIVER_ELAPSED_MS < 30000)) || fail "rollback took ${DRIVER_ELAPSED_MS} ms, past the ready delay"
    log "  ok: rolled back after ${DRIVER_ELAPSED_MS} ms"
}

# ─── Probation, restored-release receipts and unit collection ───────────
#
# These need an installer with a probation window (90 s by default) and
# rollback reports that name the restored release. Fresh installs of the
# prior skip the window to keep the scenarios short.

# A healthy candidate commits only after it stayed up, under one
# invocation, for the whole default window.
scenario_probation_healthy() {
    install_run fresh "${V_A}" -- --probation-seconds 0
    expect_exit 0

    install_run upgrade "${V_B}"
    expect_exit 0
    expect_journal committed
    expect_active "${V_B}"
    expect_health "${V_B}"
    expect_prop NRestarts 0
    ((DRIVER_ELAPSED_MS >= 90000)) || fail "committed after ${DRIVER_ELAPSED_MS} ms, inside the window"
    log "  ok: committed after ${DRIVER_ELAPSED_MS} ms"
}

# The candidate crashes 89 s after it became ready, a second before the
# window ends. The same run rolls back and reports the release that runs
# again, which is exactly the process systemd shows.
scenario_probation_crash_late() {
    install_run fresh "${V_A}" -- --probation-seconds 0
    expect_exit 0
    set_faults "${V_B}" crash_after_ready_ms=89000

    install_run upgrade "${V_B}"
    expect_exit nonzero
    expect_journal rolled_back
    expect_output "probation"
    expect_active "${V_A}"
    expect_health "${V_A}"
    expect_restored_receipt "${V_A}"
    ((DRIVER_ELAPSED_MS >= 89000 && DRIVER_ELAPSED_MS < 120000)) ||
        fail "rolled back after ${DRIVER_ELAPSED_MS} ms, not at the crash"
    log "  ok: rolled back after ${DRIVER_ELAPSED_MS} ms"
}

# The installer dies 30 s into the window. The candidate keeps running
# under the same invocation, and the rerun watches it for a whole window
# again before it commits.
scenario_probation_installer_killed() {
    install_run fresh "${V_A}" -- --probation-seconds 0
    expect_exit 0

    install_run interrupted "${V_B}" --kill-at prove_candidate --with-receipt --delay-ms 30000
    expect_acted kill_installer
    local invocation
    invocation="$(service_prop InvocationID)"
    snapshot before-recovery

    install_run recovery "${V_B}"
    expect_exit 0
    expect_journal committed
    expect_active "${V_B}"
    expect_health "${V_B}"
    [[ "$(service_prop InvocationID)" == "${invocation}" ]] ||
        fail "the candidate restarted during recovery"
    ((DRIVER_ELAPSED_MS >= 90000)) ||
        fail "recovery committed after ${DRIVER_ELAPSED_MS} ms, without a whole window"
    log "  ok: same invocation ${invocation}; recovery committed after ${DRIVER_ELAPSED_MS} ms"
}

# Power is cut 20 s into the window. The candidate autostarts under a new
# invocation, not the one on probation, so recovery rolls back and reports
# the prior.
scenario_probation_power_cut() {
    install_run fresh "${V_A}" -- --probation-seconds 0
    expect_exit 0

    install_run interrupted "${V_B}" --kill-at prove_candidate --with-receipt --delay-ms 20000
    expect_acted kill_installer
    power_cut
    wait_active
    expect_health "${V_B}"
    snapshot before-recovery

    install_run recovery "${V_B}"
    expect_exit nonzero
    expect_journal rolled_back
    expect_active "${V_A}"
    expect_health "${V_A}"
    expect_restored_receipt "${V_A}"
}

# After three installs only the active release and the one it replaced
# remain, and what an interrupted staging left behind is gone.
scenario_units_collected() {
    install_run first "${V_A}" -- --probation-seconds 0
    expect_exit 0
    install_run second "${V_B}" -- --probation-seconds 0
    expect_exit 0
    gx mkdir -p "${GUEST_UNITS}/.hypercolor-stage-payload-4242-0/bin"
    gx touch "${GUEST_UNITS}/.hypercolor-stage-payload-4242-0/bin/hypercolor-daemon"

    install_run third "${V_C}" -- --probation-seconds 0
    expect_exit 0
    expect_journal committed
    expect_active "${V_C}"
    expect_units "${V_B}" "${V_C}"
}

# A managed install runs its daemon through the installation's launcher
# inside the sandbox: healthy, from its own release only, able to write
# exactly the recorded configuration, data and daemon state roots and the
# coordinator's directory, with a private /tmp.
scenario_hardened_unit() {
    set_faults "${V_A}" probe_writes=1
    install_run fresh "${V_A}" -- --probation-seconds 0
    expect_exit 0
    expect_journal committed
    expect_active "${V_A}"
    expect_health "${V_A}"

    local fragment="${GUEST_HOME}/.config/systemd/user/hypercolor.service"
    local launcher="${GUEST_RELEASES}/launcher/hypercolor"
    gx cat "${fragment}" >"${RECEIPT}/unit.service"
    grep -qxF "ExecStart=${launcher} __launch --role daemon" "${RECEIPT}/unit.service" ||
        fail "the unit does not start the launcher"
    grep -qxF "ExecStartPre=+${launcher} __launch --role prepare-roots" "${RECEIPT}/unit.service" ||
        fail "the unit does not prepare its roots before the sandbox"
    local directive
    for directive in ProtectSystem=strict ProtectHome=read-only PrivateTmp=true \
        NoNewPrivileges=true Type=notify WatchdogSec=30 \
        Environment=HYPERCOLOR_SERVICE_IDENTITY=user_service:systemd:hypercolor.service; do
        grep -qxF "${directive}" "${RECEIPT}/unit.service" || fail "the unit lacks ${directive}"
    done
    log "  ok: the unit starts the launcher with its sandbox"
    gx systemctl --user show hypercolor.service -p ProtectSystem -p ProtectHome \
        -p PrivateTmp -p NoNewPrivileges -p ReadWritePaths -p ReadOnlyPaths \
        >"${RECEIPT}/sandbox-properties.txt"
    expect_prop ProtectSystem strict
    expect_prop ProtectHome read-only
    expect_prop PrivateTmp yes
    expect_prop NoNewPrivileges yes

    expect_launch "${V_A}"
    expect_writes
    local pid
    pid="$(service_prop MainPID)"
    if gx test -e "/tmp/hc-qual-private-${pid}"; then
        fail "the daemon's /tmp is not private"
    fi
    log "  ok: the daemon's /tmp is private"
    local name
    for name in coordinator activator; do
        [[ "$(gx stat -c %a "${GUEST_HOME}/.local/state/hypercolor/update/${name}")" == 700 ]] ||
            fail "${name}/ is not 0700"
    done
    log "  ok: coordinator/ and activator/ exist, 0700"
    [[ "$(gx stat -c %a "${launcher}")" == 555 ]] || fail "the launcher is not 0555"
    log "  ok: the launcher is read-only"

    install_run upgrade "${V_B}" -- --probation-seconds 0
    expect_exit 0
    expect_active "${V_B}"
    expect_launch "${V_B}"
}

# Deleting the configuration directory to reset it, as the uninstall guide
# suggests, must not keep the sandboxed service from starting: its
# prepare-roots step recreates the recorded root before systemd builds the
# sandbox around it.
scenario_config_reset() {
    install_run fresh "${V_A}" -- --probation-seconds 0
    expect_exit 0
    expect_health "${V_A}"
    local config="${GUEST_HOME}/.config/hypercolor"
    gx systemctl --user stop hypercolor.service
    gx rm -rf "${config}"
    if gx test -e "${config}"; then
        fail "the configuration root is still there"
    fi
    log "deleted ${config}; starting the service"
    gx systemctl --user start hypercolor.service || fail "the service did not start after the reset"
    wait_active
    expect_health "${V_A}"
    [[ "$(gx stat -c %a "${config}")" == 700 ]] || fail "the configuration root was not recreated 0700"
    gx journalctl --user -u hypercolor.service --no-pager -o cat \
        >"${RECEIPT}/service-journal.txt" 2>&1 || true
    grep -qF "hypercolor launcher: created ${config}" "${RECEIPT}/service-journal.txt" ||
        fail "the launcher did not report recreating the root"
    log "  ok: the service recreated the deleted configuration root and started"
}

# The active pointer swaps between two releases as fast as the guest can
# while the service restarts again and again; every start runs one whole
# release, and both releases get selected.
scenario_launcher_swap() {
    install_run fresh "${V_A}" -- --probation-seconds 0
    expect_exit 0
    install_run upgrade "${V_B}" -- --probation-seconds 0
    expect_exit 0
    expect_active "${V_B}"

    local unit_a unit_b
    unit_a="$(unit_of "${V_A}")"
    unit_b="$(unit_of "${V_B}")"
    log "swapping active between ${V_A} and ${V_B} while restarting"
    podman exec -d --user "${GUEST_UID}" -w "${GUEST_RELEASES}" "${GUEST}" bash -c "
        touch /tmp/hc-qual-swapping
        while [ -e /tmp/hc-qual-swapping ]; do
            ln -s units/${unit_a} active.swap && mv -T active.swap active
            ln -s units/${unit_b} active.swap && mv -T active.swap active
        done"
    local round seen_a=0 seen_b=0 report unit
    for round in $(seq 1 24); do
        # Reset the start rate limit, which counts manual restarts too.
        gx systemctl --user reset-failed hypercolor.service
        gx systemctl --user restart hypercolor.service
        wait_active
        report="$(launch_report "round-${round}")"
        unit="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["exe"])' "${report}")"
        python3 - "${report}" <<'PY' || fail "round ${round} mixed two releases"
import json
import sys

launch = json.loads(sys.argv[1])
argv = launch["argv"]
release = launch["exe"].rsplit("/bin/", 1)[0]
expected = [
    f"{release}/bin/hypercolor-daemon",
    "--ui-dir", f"{release}/share/hypercolor/ui",
    "--effects-dir", f"{release}/share/hypercolor/effects/bundled",
]
if argv != expected or "/active/" in " ".join(argv):
    sys.exit(f"argv {argv} for {launch['exe']}")
PY
        case "${unit}" in
            */units/${unit_a}/*) seen_a=$((seen_a + 1)) ;;
            */units/${unit_b}/*) seen_b=$((seen_b + 1)) ;;
            *) fail "round ${round} ran ${unit}" ;;
        esac
    done
    groot rm -f /tmp/hc-qual-swapping
    sleep 1
    gx bash -c "cd ${GUEST_RELEASES} && ln -s units/${unit_b} active.swap && mv -T active.swap active"
    log "  starts: ${seen_a} from ${V_A}, ${seen_b} from ${V_B}"
    ((seen_a > 0 && seen_b > 0)) || fail "the swaps never reached both releases"
    log "  ok: every start ran one whole release, and both were selected"
}

# The installer dies during the candidate's proof. A recovery run through
# the launcher's update-executor role executes the prior release's CLI,
# which finds the candidate wrong and rolls back.
scenario_recovery_role() {
    install_run fresh "${V_A}" -- --probation-seconds 0
    expect_exit 0
    set_faults "${V_B}" health_delay_ms=3000 report_version=0.0.0-wrong

    install_run interrupted "${V_B}" --kill-at prove_candidate --with-receipt --delay-ms 500 -- --probation-seconds 0
    expect_acted kill_installer
    expect_journal forward
    expect_active "${V_B}"

    recover_run recovery --probation-seconds 0
    expect_exit 0
    local unit_a
    unit_a="$(unit_of "${V_A}")"
    expect_output "update-executor from release ${unit_a}, the prior of the unsettled install"
    expect_output "Recovering with ${GUEST_RELEASES}/units/${unit_a}/bin/hypercolor"
    expect_journal rolled_back
    expect_active "${V_A}"
    expect_health "${V_A}"
    expect_restored_receipt "${V_A}"

    recover_run settled
    expect_exit 0
    expect_output "update-executor from release ${unit_a}"
    expect_output "No unsettled install to recover."
}
