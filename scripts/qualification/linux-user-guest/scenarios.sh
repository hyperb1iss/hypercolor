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
    ((DRIVER_ELAPSED_MS < 30000)) || fail "rollback took ${DRIVER_ELAPSED_MS} ms, past the ready delay"
    log "  ok: rolled back after ${DRIVER_ELAPSED_MS} ms"
}
