# Linux Capture Acceptance Handoff

Status: ready for physical validation on Linux as of 2026-08-01.

Baseline: `main` at `5ce4298e`.

This handoff covers the first real Wayland and PipeWire validation pass after
the input and capture consolidation. The production tree is cohesive and the
remaining work is physical evidence, failure reproduction, and the explicit
follow-ups below. Do not remove or bypass the Wayland stack while diagnosing a
failure.

## What landed

The relevant commits are:

- `46e1a21f` admits reusable CPU capture planes before allocation.
- `4e2c9e5e` keeps Windows-only admission helpers out of Linux builds.
- `0fafb3a9` separates committed-plan, per-source runtime, and branch worker
  authority while preserving retained branches atomically.
- `2c3c86dd` publishes exact Wayland CPU Surface and Zones branches with shared
  byte and compute admission, cancellation, and retained-runtime reaping.
- `5ce4298e` applies Linux capture capacity through daemon startup and live
  configuration transactions.

Cross-model review converged to `PASS`. Independent verification covered the
Windows capture stack and a WSL Wayland suite. The WSL Wayland filter reported
50 passed and 0 failed. Full workspace tests and allocation contracts passed on
Windows after isolating Windows linker resource issues.

## Architectural truth

- Wayland capture is part of the supported architecture. The temporary
  integration boundary that once excluded it is obsolete.
- Resolution is not capped by a fixed axis or pixel count. Checked memory and
  compute admission decide whether an exact demand fits.
- The Wayland portal currently negotiates native acquisition. Keep this for
  correctness until branch-aware PipeWire renegotiation can preserve every
  exact branch transactionally.
- One PipeWire callback copy enters a preallocated CPU double buffer. Exact
  Surface and Zones branches fan out from that owned CPU frame.
- Windows DXGI capture retains its native GPU publication path. Wayland's CPU
  capture path does not disable or downgrade the GPU compositor used by the
  renderer.
- A source's committed-plan generation, runtime-owner identity, and branch
  worker generation are distinct. An unrelated source commit must not fence an
  unchanged source.
- Retained and newly prepared branches for one source share a finalization gate.
  Mixed-generation publication must remain atomic.

## Known gaps, not permission to nerf

1. Live PipeWire portal and compositor behavior has not been exercised yet.
2. Spec 73 T17 remains the authority for chunk offset and size, signed stride,
   crop metadata, all eight SPA D4 transforms, exact-once buffer requeue, stream
   death, and reconnect acceptance.
3. Resolved: `/api/v1/status` now reports `screen_capture_capacity` on Linux
   (field `admission_enforced`, no longer `windows_admission_enforced`).
   Record the capacity object in every acceptance row.
4. Branch-aware Wayland acquisition envelopes remain Sibyl task
   `c292d05e-2aef-410b-b3c9-526a52a14549`. Native acquisition is the correct
   fallback until that transaction is designed and verified.
5. The broader Wayland admission task
   `dc55c2cd-23f0-451d-ab83-c4ce069c5628` still carries unverified 16K,
   repeated-restart, and shrink-rollback acceptance. Do not mark it complete
   from the WSL fixture suite alone.

Do not make a failing case pass by lowering capture FPS, monitor resolution,
canvas size, preview cadence, or memory ceilings. Record the contended resource
and fix the underlying path.

## Linux baseline

Start with a clean current tree:

```bash
git status --short --branch
git fetch origin main
git switch main
git merge --ff-only origin/main
git rev-parse HEAD
```

The expected baseline commit is `5ce4298e` or a descendant containing all five
commits above.

Record the host before testing:

```bash
mkdir -p /tmp/hypercolor-linux-acceptance
{
  date --iso-8601=seconds
  uname -a
  printf 'session=%s desktop=%s\n' "$XDG_SESSION_TYPE" "$XDG_CURRENT_DESKTOP"
  rustc --version
  cargo --version
  pkg-config --modversion libpipewire-0.3
  wpctl status
} | tee /tmp/hypercolor-linux-acceptance/host.txt
```

`XDG_SESSION_TYPE` must be `wayland` for this pass. Confirm that PipeWire,
WirePlumber, and the desktop portal are healthy using the service tools native
to the distribution. Do not restart desktop media services casually; that can
interrupt audio, video, and the current graphical session.

Run the static gate before live testing:

```bash
just verify
```

If a dependency build fails before Hypercolor compiles, preserve the first
external error separately from product test results. Do not report an unrun test
as failed or passed.

## Live bring-up

Run the daemon manually in terminal A so the portal interaction and logs remain
visible:

```bash
RUST_LOG=hypercolor_core::input::screen=trace,hypercolor_daemon=debug \
  just daemon 2>&1 | tee /tmp/hypercolor-linux-acceptance/daemon.log
```

In terminal B, confirm health and enable capture transactionally:

```bash
curl -fsS http://127.0.0.1:9420/health
just cli config get capture.enabled
just cli config set capture.enabled true --live
curl -fsS -X POST http://127.0.0.1:9420/api/v1/capture/source/pick
```

Choose a display in the portal. Use `source = "auto"`; the portal restore token
owns the persistent selection on Linux.

Capture baseline telemetry after frames begin moving:

```bash
just cli status --json \
  | tee /tmp/hypercolor-linux-acceptance/status-live.json
just cli diagnose --system --json \
  --report /tmp/hypercolor-linux-acceptance/diagnose-live.json
```

In the status payload, find the source with `kind = "screen"`. A healthy active
capture should be configured and consented, have `source_id =
"wayland_screen_capture"` with `backend = "pipewire"`, report `state = "live"`
or an explained `degraded`, and remain `fresh` while demanded.
Record its source-graph generation, session generation, resource count, last
sample age, and any issue object.

For visual inspection, attach a screen preview through the web UI in a separate
terminal:

```bash
just ui-dev
```

Open `http://127.0.0.1:9430`, subscribe to the screen preview, and use a desktop
test image with unique colors and labels in every corner. This makes rotation,
reflection, crop, row-order, and channel-order defects immediately visible.

## Acceptance matrix

Record host topology, exact command or desktop action, expected result, actual
result, status JSON, and relevant daemon log lines for every row.

### 1. Basic publication

- Select the primary display and verify continuous preview publication.
- Verify the preview dimensions and orientation match the selected output.
- Verify channel order with pure red, green, blue, white, and black regions.
- Confirm no stale-runtime, wrong-generation, authority, or allocation error is
  emitted after warmup.

### 2. Demand and live configuration

- Detach the screen preview and stop every screen-consuming effect. The source
  may remain configured, but expensive analysis and publication must become
  idle when demand is off.
- Reattach preview and confirm publication resumes without restarting the
  daemon or reopening the portal.
- Apply representative live changes and verify each transaction preserves the
  last-good publication until the replacement is committed:

```bash
just cli config set capture.capture_fps 60 --live
just cli config set capture.grid_cols 32 --live
just cli config set capture.grid_rows 18 --live
just cli config set capture.letterbox true --live
just cli config set capture.letterbox false --live
```

- Known gate interaction: with demand active, the transaction requires the
  replacement source to become usable within 500 ms, but a Wayland replacement
  only reaches `live` after a portal round trip and first sample. An HTTP 422
  "did not become usable" with the last-good publication intact is the current
  expected shape, not a stop condition; record it as evidence for Sibyl task
  `534c978b-3ef4-4dba-9214-16a3242748c5` (gate redesign) rather than a new
  failure. A 409 during the window means capture demand flapped mid-prepare;
  re-run the row.

- Disable and re-enable capture live. The old worker must stop, admitted
  resources must retire, and the replacement must not publish under the old
  runtime identity.

### 3. Portal transitions

- Reopen the picker and cancel. Cancellation must not corrupt or partially
  replace the committed source. Record whether an existing last-good stream is
  retained and whether an initial cancelled selection becomes unavailable.
- Reselect the same display. A replacement session may advance the session
  generation without duplicating publications.
- Select a different display. Source geometry and topology must change together,
  with no wrong-sized or wrong-generation frame between them.
- Restart the daemon and verify the restore token reconnects or requests consent
  cleanly according to the desktop portal policy.

### 4. Resolution and topology

- Exercise every available native mode, prioritizing 1080p, 1440p, 4K,
  ultrawide, high-refresh, and mixed-DPI multi-monitor layouts.
- Exercise 8K and 16K through real or virtual outputs when available. Admission
  may reject an impossible configuration with a typed capacity error; it must
  never overflow, panic, silently clamp, or partially commit.
- Change display resolution while capture is live. The topology generation must
  advance and publication must transition without a stale-size frame.
- Rotate a display through every compositor-supported orientation. Quarter turns
  must swap logical axes exactly once.
- Exercise reflected transforms where the compositor exposes them. Unavailable
  transform cases remain explicit unexecuted gates, not presumed passes.

### 5. Crop, stride, and format

- Prefer a compositor or synthetic PipeWire source that exposes non-zero chunk
  offsets, row padding, negative stride, crop metadata, and format changes.
- Use the unique-corner image to prove raw-plane crop happens before the SPA D4
  transform and that cursor geometry follows the same transform.
- Malformed or truncated buffers must drop with a typed reason. They must not
  read out of bounds, poison the last-good frame, or crash the callback thread.

If the live portal cannot produce one of these cases, leave it unexecuted and use
the T17 synthetic fixture seam later. Do not infer coverage from a normal
full-frame desktop stream.

### 6. Failure and recovery

- Revoke portal permission or terminate only a disposable synthetic stream. Do
  not kill the desktop PipeWire service without explicitly accepting the wider
  session blast radius.
- Stream death must make the source unavailable before stale pixels can be
  mistaken for current data.
- Replacing the stream while demand remains active must recover without a daemon
  restart, duplicated worker, or leaked reservation.
- Repeat enable, select, cancel, disable, and re-enable transitions while a
  preview subscriber attaches and detaches.

### 7. Soak and resource behavior

- Run at native resolution and the intended FPS ceiling. Do not lower either to
  manufacture stable telemetry.
- After warmup, observe process resident memory during at least ten minutes of
  steady capture and repeated preview attach/detach cycles.
- Memory may move within admitted pools but must converge instead of growing per
  frame or per transition.
- Record render FPS, budget misses, sample age, publication counts, callback
  drops, and any typed admission failure.

## Stop conditions

Stop the current case and preserve evidence if any of these occur:

- panic, abort, callback-thread death, or daemon exit;
- stale pixels remain reported as fresh after stream loss;
- wrong dimensions, orientation, crop, channel order, or source identity;
- a retained branch is fenced by an unrelated-source configuration change;
- memory grows without converging after warmup;
- more than one callback copy occurs for one native frame;
- a rejected transition changes the committed plan or last-good publication;
- performance stabilizes only after lowering a product baseline.

For a failure, save the full daemon log, status and diagnosis JSON before and
after, the selected display topology, exact reproduction steps, and the first
error. Diagnose before patching. Production changes belong in a focused task
after the root cause is confirmed.

## Completion bar

The Linux pass is complete only when every executed row has evidence, every
unavailable hardware case is named as unexecuted, and reproduced failures have a
separate task. A clean basic portal run is valuable evidence, but it does not by
itself close T17 or T25.

Primary references:

- `docs/specs/73-input-capture-hardening.md`, especially the arbitrary-resolution
  contract, T17, and T25.
- `crates/hypercolor-core/src/input/screen/wayland.rs` and its tests.
- `crates/hypercolor-daemon/src/startup/services.rs` for Linux capacity wiring.
- `crates/hypercolor-daemon/src/api/system.rs` for current status telemetry.
