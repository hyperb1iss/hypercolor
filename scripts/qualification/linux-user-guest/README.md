# Linux user guest proofs

Proves the raw Linux release installer (`hypercolor __install-release`)
against a real systemd user manager, the way an ordinary user runs it.
Each scenario boots a fresh guest, installs qualification releases,
injects daemon faults, kills the installer at chosen journal actions, cuts
power, and checks where the installation ends up.

The unit and integration suites in `crates/hypercolor-cli/tests/` drive the
same coordinator against a simulated user manager. This harness is the
check that the simulation matches systemd itself: it has found bugs the
simulator could not (see "What only the guest catches").

## Running

Requirements: Linux, rootless podman with cgroup v2 delegation (systemd as
PID 1 inside a container), `curl`, `python3` and network access for the
first build. Containers run with SELinux labeling disabled, so the bind
mounts also work on enforcing hosts. Several checkouts can run scenarios at
once: guests carry a label naming their checkout, and `clean` only removes
this checkout's.

```bash
just linux-guest-proof list                 # scenario names
just linux-guest-proof run baseline         # one scenario
just linux-guest-proof run stop-first-failing slow-start-beyond
just linux-guest-proof all                  # every scenario
just linux-guest-proof build                # images, CLI and releases only
just linux-guest-proof clean                # remove leftover guests
```

`just linux-guest-proof <command>` runs
`scripts/qualification/linux-user-guest/guest-proof.sh <command>`, which
works the same without `just`. `run` and `all` build first; a rebuild with
nothing changed takes a few seconds.

| Variable | Effect |
| --- | --- |
| `HC_GUEST_KEEP=1` | Leave each guest running afterwards for inspection |
| `HC_GUEST_RECEIPTS=<dir>` | Write receipts somewhere else |
| `HC_GUEST_BASE_VERSION=<x.y.z>` | Start the archives from another published release (default `0.5.1`) |

## Receipts

Every scenario writes a directory under `receipts/` (ignored by git):

- `receipt.txt`: source revision (marked dirty when uncommitted), CLI
  SHA-256, guest image ID, guest systemd version, the unit digest of every
  qualification release, and the verdict.
- `steps.log`: every step and expectation with timestamps.
- `NN-service-journal-before-power-cut.txt`: the service journal of each
  boot a power cut ends.
- `NN-install-<label>.log`: installer output for one run, ending with a
  `DRIVER` line (exit status, elapsed time, where the installer or daemon
  was killed).
- `NN-state-<label>.txt`: the install journal summary, active pointer,
  `hypercolor.service` properties, whether the session bus was reachable,
  and `/health`, after each run and before each recovery.
- `service-journal.txt`: the user journal of `hypercolor.service`.

Quote receipts in PR descriptions; do not commit them.

## How it works

**Guest.** `guest.Containerfile` is Ubuntu 24.04 (the release runtime
baseline: glibc 2.39, systemd 255) booting systemd as PID 1 under rootless
podman with `--network=none`. User `qualification` (uid 1100) lingers, so
its user manager starts at boot and autostarts an enabled
`hypercolor.service` exactly as a login does. NSS lookups use local files
only, so the installer's ownership checks can enumerate every principal.

**Session bus hidden.** Before every installer run the harness renames
`$XDG_RUNTIME_DIR/bus` away, so only the manager's private socket
(`$XDG_RUNTIME_DIR/systemd/private`) is reachable, as in a sandboxed service
that denies the session bus.

**Builds.** `builder.Containerfile` installs the toolchain pinned by
`rust-toolchain.toml` on the same Ubuntu base. The harness builds
`hypercolor-cli` from this checkout inside it, so the CLI links against the
guest's glibc, and compiles `hc-qual-daemon.rs` with plain `rustc`. Build
output stays in `target/linux-user-guest/`.

**Releases.** `make_release.py` starts from the published release tarball
(downloaded once and checked against its `.sha256`) and replaces three
members: `bin/hypercolor` becomes the CLI under test,
`bin/hypercolor-daemon` becomes the qualification daemon, and
`manifest.json` names a qualification version with both new digests.
A base release from before the managed package contract also gains the
`managed_package` block `scripts/dist.sh` writes, with the store inventory
in `packaging/managed/durable-stores.json`, since the installer refuses a
Linux candidate without one. Everything else is the published release byte
for byte. Three versions are
built (`<base>-qual.1` to `.3`), so each is a distinct unit. The guest
mounts them read-only at `/releases` and the driver extracts and runs each
release's own `bin/hypercolor`, as `install.sh` does.

**Qualification daemon.** `hc-qual-daemon` stands in for the real daemon.
It sends `READY=1`, pings the watchdog, answers `/health` and
`/api/v1/system` with its unit's manifest version, and stops on SIGTERM.
Faults are per version, read at every start from
`~/hc-qual/faults/<version>` (`key=value` lines):

| Key | Fault |
| --- | --- |
| `ready_delay_ms` | Wait before serving HTTP and sending `READY=1` |
| `exit_before_ready` | Exit with this status instead of becoming ready |
| `health_delay_ms` | Wait before answering each `/health` |
| `stop_delay_ms` | Wait after SIGTERM before exiting |
| `crash_after_ready_ms` | Abort this long after `READY=1` |
| `hang_http_after_ready_ms` | Stop answering HTTP (the watchdog keeps pinging) |
| `report_version` | Answer HTTP with another version |
| `probe_writes` | At start, try to write into every directory the service sandbox allows or denies |

`GET /qual/launch` reports how the daemon was started: its resolved
executable, its arguments, the XDG variables the launcher set, and the
write probes (`hc-guest-driver launch-report` prints it).

The harness writes fault files with `podman exec -i ... tee` (without `-i`
the file comes out empty).

**Killing the installer.** `guest-driver.sh` runs inside the guest as uid
1100. With `--kill-at <action>` it watches the install journal and, once
its `next_action` is that action (optionally only after the candidate's
owner receipt is recorded, and after `--delay-ms`), sends `SIGKILL` to the
installer, or with `--kill-service` to the daemon's main process instead.
The `DRIVER` line records the watched action, the action the journal named
when the driver acted, and the one it names once the installer is gone. A
scenario fails unless the driver acted at the watched action and, when it
killed the installer, the journal still names that action afterwards. Only the installer process is
killed, so a `systemctl` child it was waiting on can still finish, as after
any single-process crash.

**Power cut.** `podman kill --signal KILL` takes down the whole guest; the
harness then starts it again from its disk. The user manager comes back and
autostarts `hypercolor.service` from the on-disk pointer and fragment
before the installer inspects anything. The guest's journal is volatile, so
the service journal of the boot that ends is saved to the receipts first.
Processes die but the page cache survives, since the guest shares the host
kernel; page-cache loss needs a virtual machine.

## Scenarios

| Scenario | What happens | Expected end |
| --- | --- | --- |
| `baseline` | Fresh install, then an upgrade | Both commit; the upgrade serves the new unit |
| `restart-after-receipt` | Installer killed during the candidate's proof after its receipt; candidate restarted; installer rerun | Rolls back; prior runs |
| `crash-in-proof` | Candidate killed while the proof waits on a slow `/health` | Same run rolls back |
| `abandon-after-power-cut` | Prior stops slowly; installer killed at `UnloadPrior`; power cut; prior autostarts | Abandoned without effect; next run commits |
| `stop-first-failing` | Candidate never becomes ready; installer killed during its start; power cut; candidate autostarts and crash-loops | Recovery stops it first and rolls back |
| `stop-first-healthy` | Installer killed at `SwitchToCandidate`; power cut | Recovery stops the autostarted service and commits |
| `disabled-running` | Running prior with autostart disabled; failing then healthy candidate | Rollback restarts the prior; upgrade commits; autostart stays disabled |
| `slow-start-within` | Candidate ready after 20 s | Commits within the default `TimeoutStartSec` |
| `slow-start-beyond` | `TimeoutStartSec=8s` drop-in; candidate needs 30 s | systemd fails the start; rolls back well before 30 s |
| `probation-healthy` | A healthy candidate under the default 90 s window | Commits after the whole window, under one invocation |
| `probation-crash-late` | The candidate crashes 89 s after readiness | The same run rolls back and names the prior that runs again |
| `probation-installer-killed` | Installer killed 30 s into the window; installer rerun | The candidate keeps its invocation; the rerun watches a whole window again and commits |
| `probation-power-cut` | Power cut 20 s into the window | The candidate autostarts under a new invocation; recovery rolls back and names the prior |
| `units-collected` | Three installs, with a leftover staging tree planted before the third | Only the active release and the one it replaced remain; the leftover is gone |
| `hardened-unit` | Fresh install with write probes, then an upgrade | The unit starts the launcher with its sandbox; the daemon, its UI and effects come from one release (`/proc` agrees); it can write exactly config, data, daemon state, coordinator, cache and a private `/tmp` |
| `launcher-swap` | `active` flips between two releases continuously while the service restarts 24 times | Every start runs one whole release, and both are selected |
| `recovery-role` | Installer killed during a wrong candidate's proof; recovery run through the launcher's update-executor role | The prior release's CLI recovers and rolls back; a second run finds nothing to recover |

Every scenario whose rollback restarts the prior also checks the
installer's report of the release that runs again against the service's
`MainPID` and `InvocationID`. An abandonment restarts nothing, so it has no
such report.
The probation and collection scenarios install their first releases with
`--probation-seconds 0` to stay short. Every other install uses the
installer's default window, so the recovery scenarios run with probation on.

Scenarios live in `scenarios.sh` as `scenario_<name>` functions built from
the helpers in `guest-proof.sh` (`install_run`, `set_faults`, `power_cut`,
`expect_*`). A new function is a new scenario.

## Checking the harness itself

```bash
shellcheck -x scripts/qualification/linux-user-guest/*.sh
python3 -m py_compile scripts/qualification/linux-user-guest/make_release.py
rustfmt --edition 2024 --check scripts/qualification/linux-user-guest/hc-qual-daemon.rs
rustc --edition 2024 --test -o target/linux-user-guest/lint/hc-qual-daemon-tests \
    scripts/qualification/linux-user-guest/hc-qual-daemon.rs && target/linux-user-guest/lint/hc-qual-daemon-tests
clippy-driver --edition 2024 -W clippy::pedantic -D warnings --emit=metadata \
    --out-dir target/linux-user-guest/lint scripts/qualification/linux-user-guest/hc-qual-daemon.rs
```

## What only the guest catches

Found here and fixed in the installer:

- zbus panics on the manager's private socket, because systemd stamps a
  well-known sender on direct connections.
- systemd 255 never answers a call that arrives in the same read as the
  D-Bus `BEGIN`.
- `ResetFailedUnit` returns `NoSuchUnit` for a unit systemd garbage
  collected between two calls, which broke every fresh install.
- After `daemon-reload` while the service runs, `systemctl show` reports the
  exec pid as 0 beside the real `MainPID`.
