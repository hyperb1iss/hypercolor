# Spec 77: macOS Capture Security and Release Hardening

Status: Approved, Wave 1 landed (status corrected 2026-08-25). H3.5 shipped: CPU
publication is gated behind `#[cfg(feature = "macos-capture-fixtures")]`
(`crates/hypercolor-core/src/input/screen/macos.rs`) with the GPU-only guard enforced in
CI. H1.1 shipped as `ProtectedControl`
(`crates/hypercolor-daemon/src/api/security.rs`).
Author: Nova
Date: 2026-08-13
Depends on: spec 61, spec 73, spec 79 (macOS capture and host input)
Baseline: `854f36f9ece49794f7c069da8e31cfcc86c0f96d`
Scope: macOS screen and host-input capture, GPU publication, daemon API
authorization, desktop trust, daemon ownership, installers, signing, CI, and
physical release acceptance.

## Mission

Finish the macOS capture and host-input feature as one coherent, production-ready
system. Close every confirmed correctness, security, maintainability, portability,
and release-integrity finding without reducing frame rate, resolution, preview
cadence, device-output cadence, or supported functionality.

macOS production screen capture is GPU-only. ScreenCaptureKit acquisition,
IOSurface import, color conversion, tone mapping, composition, and LED reduction
remain on IOSurface, Metal, and wgpu. CPU implementations exist only as
fixture-gated parity oracles. A GPU failure invalidates stale output, rebuilds the
GPU route transactionally, and fails closed when recovery cannot complete. It
never selects a production CPU fallback.

This spec is also the progress ledger for the hardening work. Task identifiers in
Sibyl mirror the identifiers below.

## Non-negotiable invariants

Invariants 1 and 3 describe the target capture plane and become binding when
H3.5 lands. Until then one named temporary mitigation remains: production
capture still carries the legacy CPU publication fallback
(`hypercolor-core/src/input/screen/macos.rs`), kept only so capture degrades
instead of dying where the GPU path cannot run. H3.5 removes it; no new code
may depend on it.

1. Production macOS screen capture never materializes a full frame on the CPU for
   composition, transformation, reduction, recovery, or fallback.
2. Explicit egress may read completed GPU output for a transport payload, such as
   a WebSocket preview. The readback cannot feed another capture or compositor
   path.
3. Missing, incompatible, or failed Metal capability produces `native_pending` or
   `native_unavailable`. It never produces `cpu_fallback`.
4. Terminal capture or structural renderer failure clears retained screen output
   before recovery begins.
5. The canonical `MacosDaemonGuard` flock is the only ownership authority. PID,
   port health, tokens, diagnostics, and session artifacts cannot elect an owner.
6. Loopback is network locality, not user identity. TCC actions and sensitive
   screen/input streams require protected-control authorization.
7. The packaged Tauri app renders bundled UI only. Daemon-served HTML never gains
   Tauri command authority.
8. Managed owners stop through the topology that launched them. Handover never
   signals a bare PID.
9. Release acceptance proves the exact signed artifacts that are promoted. No
   accepted candidate is rebuilt before publication.
10. Existing performance ceilings remain product contracts. Hardening cannot
    lower FPS, resolution, preview cadence, device cadence, or queue capacity to
    hide a defect.

## Target architecture

### Capture data-plane ownership

`hypercolor-macos-capture` owns ScreenCaptureKit callbacks, lifecycle ordering,
cancellation, and retained native-frame ownership.

`hypercolor-core` owns publication authority, GPU-required demand, IOSurface
admission, tone-map transition state, freshness, and consumer accounting.

`hypercolor-macos-gpu-interop` owns IOSurface, Core Video, and Metal wrapper
caches, including every backing lifetime those wrappers require.

`hypercolor-daemon` owns Metal execution, target recovery, queue invalidation,
and compositor integration.

### Control-plane authority

Three authorities remain intentionally separate:

1. The canonical flock decides which daemon owns macOS integration.
2. A private daemon-session attestation proves which server instance corresponds
   to the flock winner.
3. A protected-control credential authorizes sensitive REST and WebSocket
   operations.

The session artifact and credential are evidence and authorization. Neither is a
second ownership lease.

### GPU failure semantics

The daemon owns a transactional native execution state machine:

```text
Ready(target N)
  -> Invalidating(error)
  -> Rebuilding
  -> Ready(target N+1)
  -> Unavailable(last error)
```

A structural failure clears the compositor screen layer, releases every screen
GPU cache, fences the failed target generation, rebuilds the complete target,
and publishes the replacement only after construction succeeds. A specifically
typed transient not-ready condition may defer while retaining a still-fresh
current frame. Persistent and unclassified failures invalidate immediately.

## Wave 0: contract and deterministic baseline

### H0.1 Make GPU-only capture normative

**Files:** `docs/specs/79-macos-screen-capture-and-host-input.md`, this spec

**Depends on:** none

**Parallel:** No

#### Implementation

- Remove every production CPU-fallback promise from spec 76.
- Define fixture-only CPU reference implementations.
- Define fail-closed Metal recovery and terminal publication invalidation.
- Define protected local control and exact-artifact acceptance.
- Sweep the revised spec for stale fallback terminology.

#### Verify

- [ ] Every remaining macOS `cpu` reference is explicitly fixture-only or a
      rejected alternative.
- [ ] `just docs-build` passes.

### H0.2 Restore portable macOS-capture compilation

**Files:** `crates/hypercolor-macos-capture/src/lib.rs`,
`crates/hypercolor-macos-capture/src/frame.rs`,
`crates/hypercolor-macos-capture/src/diagnostics.rs`,
`crates/hypercolor-macos-capture/src/stream_contract.rs`,
`crates/hypercolor-macos-capture/src/worker.rs`

**Depends on:** none

**Parallel:** Yes, with H0.3 and H0.4

#### Implementation

- Gate native-only modules and enum variants consistently.
- Preserve portable public contracts without compiling unused macOS internals.
- Add non-macOS compile-contract coverage.

#### Verify

- [ ] Linux no-default-feature daemon compilation passes.
- [ ] Workspace Clippy passes on non-macOS targets.
- [ ] Portable tests execute a nonzero test count.

### H0.3 Complete the Python WebSocket generator

**Files:** `protocol/websocket-v1.json`,
`python/scripts/generate_ws_protocol.py`,
`python/src/hypercolor/ws_protocol.py`, `python/tests/test_websocket.py`

**Depends on:** none

**Parallel:** Yes, with H0.2 and H0.4

#### Implementation

- Generate every required and optional field schema.
- Preserve explicit defaults and distinguish no default from `null`.
- Compare generated event contracts directly with the protocol manifest.

#### Verify

- [ ] `just python-ws-protocol-check` passes.
- [ ] `just python-verify` passes.
- [ ] `just python-generate-check` passes.

### H0.4 Restore deterministic CI gates

**Files:** `.github/workflows/ci.yml`, `sdk/packages/core/src/input/data.ts`

**Depends on:** none

**Parallel:** Yes, with H0.2 and H0.3. One owner controls the workflow file.

#### Implementation

- Fix SDK Biome import ordering.
- Install NASM before Intel workspace compilation.
- Run the complete macOS capture fixture crate so inline lifecycle tests execute.
- Add a PR documentation build without enabling deployment.
- Add the GPU-only architecture check after H3.5 removes the old path.

#### Verify

- [ ] `just sdk-lint` passes.
- [ ] `just sdk-check` passes.
- [ ] `just sdk-build` passes.
- [ ] The macOS capture CI selector executes inline and integration tests.
- [ ] The docs job builds pull requests and deploys only from its existing release
      boundary.

## Wave 1: security and ownership boundaries

### H1.1 Introduce protected-control authorization

**Files:** `crates/hypercolor-daemon/src/api/security.rs`,
`crates/hypercolor-daemon/src/api/capture.rs`,
`crates/hypercolor-daemon/src/api/ws/protocol.rs`,
`crates/hypercolor-daemon/src/api/ws/session.rs`, daemon security and WebSocket
tests

**Depends on:** none

**Parallel:** Yes, with H1.4 and Wave 2

#### Implementation

- Add one named `ProtectedControl` authorization requirement.
- Require it for input authorization, screen authorization, picker operations,
  monitor enumeration, `screen_canvas`, `screen_zones`, and `input_events`.
- Ensure IP address, missing Origin, CORS, and Fetch Metadata cannot satisfy it.
- Reject unauthorized WebSocket subscriptions before creating capture demand.
- Preserve existing ordinary lighting-control loopback compatibility.

#### Verify

- [ ] Loopback without a credential receives 401 or 403.
- [ ] A read credential cannot satisfy protected control.
- [ ] A control credential succeeds.
- [ ] Rejected subscriptions create no screen or input demand.
- [ ] `cargo test -p hypercolor-daemon --test security_api_tests` passes.
- [ ] Focused WebSocket authorization tests pass.

### H1.2 Publish private daemon-session attestation

**Files:** `crates/hypercolor-macos-owner/src/lib.rs`,
`crates/hypercolor-daemon/src/main.rs`,
`crates/hypercolor-daemon/src/api/system.rs`, owner and daemon tests

**Depends on:** H1.1

**Parallel:** No

#### Implementation

- Atomically publish a 0600 artifact after the daemon wins the canonical flock.
- Bind owner epoch, full process identity, server-instance ID, and protected
  credential or verifier.
- Allow replacement or cleanup only for a matching identity and epoch.
- Keep ownership and journal schema v1 readable during upgrade and rollback.
- Ensure no code treats artifact presence as ownership authority.

#### Verify

- [ ] Wrong owner, UID, mode, identity, and epoch are rejected.
- [ ] A stale crash artifact is replaced by the next canonical winner.
- [ ] Attestation cannot create a second owner.
- [ ] `cargo test -p hypercolor-macos-owner` passes.

### H1.3 Isolate Tauri from daemon-served content

**Files:** `crates/hypercolor-app/src/main.rs`,
`crates/hypercolor-app/src/supervisor/mod.rs`,
`crates/hypercolor-app/tauri.conf.json`,
`crates/hypercolor-app/tauri.bundle.conf.json`,
`crates/hypercolor-app/capabilities/default.json`,
`crates/hypercolor-app/build.rs`, app tests, UI API and WebSocket connection
configuration

**Depends on:** H1.2

**Parallel:** No. Serialize with H1.5 where both touch the supervisor.

#### Implementation

- Bundle staged UI through `frontendDist` and open `WebviewUrl::App` only.
- Remove remote URL command authority and wildcard custom-command access.
- Enumerate commands for the bundled app origin.
- Verify canonical ownership plus private attestation before exposing the
  protected credential.
- Render a bundled offline/error shell on port preemption or identity mismatch.
- Keep screen pixels on the daemon GPU-backed stream. Do not add a Tauri proxy.

#### Verify

- [ ] A fixture pre-bound to port 9420 cannot supply the app document.
- [ ] Remote content cannot invoke Tauri ownership commands.
- [ ] Remote content never receives the protected credential.
- [ ] A matching child and a matching external owner open normally.
- [ ] `just ui-test` and app packaging tests pass.

### H1.4 Remove PID as stop authority

**Files:** `crates/hypercolor-macos-owner/src/lib.rs`, owner coordinator tests,
`crates/hypercolor-app/src/ownership.rs`,
`crates/hypercolor-app/src/supervisor/mod.rs`, app owner tests

**Depends on:** none

**Parallel:** Yes, with H1.1 and Wave 2. Serialize supervisor edits with H1.3.

#### Implementation

- Stop app sidecars through the retained child handle.
- Stop launchd and Homebrew owners through their exact service identities.
- Leave standalone owners user-directed.
- Remove production bare-PID signaling from restart and handover.
- Use flock release and a newer matching owner publication as progress proof.

#### Verify

- [ ] A stale record plus forced PID reuse signals no replacement process.
- [ ] Identity mismatch fails closed.
- [ ] Each managed topology targets only its selected launcher identity.
- [ ] Crash-phase replay remains idempotent.

### H1.5 Make launcher metadata version-neutral

**Files:** daemon launcher-resolution modules,
`crates/hypercolor-app/src/supervisor/mod.rs`, the `hypercolor` installer
transaction, launchd and Homebrew service files, `scripts/get-hypercolor.sh`,
`scripts/install-release.sh`, `scripts/dist.sh`,
`scripts/verify-release-artifact.sh`, packaging and supervisor tests

**Depends on:** H1.4

**Parallel:** No. Serialize supervisor edits with H1.3.

#### Implementation

- During the compatibility window, launchers publish
  `HYPERCOLOR_MACOS_OWNER` plus the equal deprecated `--macos-owner` argument.
  New daemons prefer the environment, accept an equal argument, and reject a
  conflict. Remove the argument only after the supported-version floor moves.
  Since 2026-08-22 (Design 72 L1) launchers also publish the neutral
  `HYPERCOLOR_SERVICE_IDENTITY` declaration beside both; it must name the
  same owner, a disagreement rejects startup the same way, and an old
  daemon ignores it.
- Treat launcher metadata as a claim. Corroborate direct and Homebrew owners
  through exact launchctl PID identity and app sidecars through their signed
  parent before guard acquisition or owner publication.
- Add bounded legacy inference when metadata is absent. Malformed, ambiguous,
  or failed identity inspection rejects startup.
- Harden artifact verification against path traversal, symlink, hardlink,
  special, and duplicate members. Bind every member's type, mode, and digest.
- Let the candidate `hypercolor` binary own a Rust install transaction. Shell
  wrappers only download, verify, and invoke it. Homebrew and app casks retain
  their native transaction ownership.
- For raw direct installs, stage and verify an immutable digest-named unit
  before stopping the current owner. Then preflight authority, unload, prove
  guard release, switch one `active` symlink, reload, and require a newer exact
  owner publication.
- Journal first-install conversion from an in-place layout into a complete
  synthetic legacy unit before mutation. Keep this install journal and lock
  separate from the canonical owner store, handover journal, and flock.
- Roll back the active unit, launcher metadata, and loaded state on failure.
  Rollback completes only after a newer exact prior-owner publication.

#### Verify

- [ ] New launcher plus old daemon works in legacy mode.
- [ ] Old launcher plus new daemon is classified through bounded inference.
- [ ] Conflicting or uncorroborated launcher claims fail before guard
      acquisition and owner publication.
- [ ] Unsafe archive members and manifest mismatches fail before install
      mutation.
- [ ] Unsafe app and daemon skew fails before stopping the working owner.
- [ ] First conversion from an in-place install is crash-replay safe.
- [ ] Failure injection after every installer stage restores the prior unit.
- [ ] Rollback preserves the canonical flock, owner store, journal, and TCC
      identity.

## Wave 2: exact capture lifecycle and resource ownership

### H2.1 Add coherent publication invalidation observations

**Files:** `crates/hypercolor-core/src/input/screen/hub.rs`,
`crates/hypercolor-core/src/input/screen/macos.rs`,
`crates/hypercolor-daemon/src/render_thread/pipeline_runtime.rs`,
`crates/hypercolor-daemon/src/render_thread/frame_composer.rs`

**Depends on:** none

**Parallel:** Yes, with Wave 1 outside shared files

#### Implementation

- Add worker-authorized invalidation across every branch owned by a binding.
- Record a monotonic invalidation epoch.
- Expose publication, lifecycle, health, freshness, and epoch as one coherent
  observation.
- Clear the compositor queue when the epoch advances before latching a newer
  publication.

#### Verify

- [ ] Terminal invalidation clears all branches for exactly one binding.
- [ ] Pressure and recoverable health failure retain last-good output.
- [ ] A stale publisher cannot invalidate current authority.
- [ ] Invalidation followed by a fresh publication clears old output first.

### H2.2 Separate lifecycle control from frame coalescing

**Files:** `crates/hypercolor-macos-capture/src/worker.rs`,
`crates/hypercolor-macos-capture/src/mailbox.rs`,
`crates/hypercolor-macos-capture/src/native.rs`,
`crates/hypercolor-core/src/input/screen/macos.rs`

**Depends on:** H2.1

**Parallel:** No

#### Implementation

- Admit only complete frames to the latest-value slot.
- Give lifecycle events an ordered control snapshot and invalidation generation.
- Stamp asynchronous decode work with the active generation.
- Reject completed decode work after a later terminal generation.
- Keep recoverable errors diagnostic without overwriting control or frame state.

#### Verify

- [ ] A blocked pre-suspend decode cannot publish after restart.
- [ ] Lifecycle events cannot be superseded by frame pressure.
- [ ] Frame traffic remains constant-memory and latest-value.
- [ ] Fatal errors invalidate exactly once.

### H2.3 Bound and cancel native stream transactions

**Files:** `crates/hypercolor-macos-capture/src/native.rs`, new internal
`native/transactions.rs` and `native/lifecycle.rs`, capture lifecycle tests

**Depends on:** H2.2

**Parallel:** No

#### Implementation

- Replace raw receivers with typed transactions carrying cancellation and
  deadlines.
- Bound native start completion, first complete frame, and stop completion.
- Preserve the previous committed stream when a candidate fails or times out.
- Fence late callbacks from cancelled, timed-out, or superseded epochs.
- Retire authority synchronously but wait and join off the main thread.
- Quarantine timed-out native objects until late completion or destruction.

#### Verify

- [ ] Missing start callback and missing first frame time out deterministically.
- [ ] Timeout racing with a valid frame commits exactly one result.
- [ ] Stop returns to the main-thread caller without waiting.
- [ ] Repeated activate and deactivate leaves no pending transaction.

### H2.4 Track live IOSurface identities

**Files:** `crates/hypercolor-core/src/input/screen/macos.rs`, new internal
`macos/surface_pool.rs`, admission tests

**Depends on:** none

**Parallel:** Yes, with H2.1

#### Implementation

- Replace historical identity accumulation with one token per live IOSurface.
- Share tokens for repeated observation of the same identity.
- Reconcile exact bytes when the final token drops.
- Treat queue depth as an initial reserve, not an identity cap.

#### Verify

- [ ] A ninth historical IOSurface succeeds after an earlier identity drops.
- [ ] More than eight simultaneous identities depend only on real byte capacity.
- [ ] Repeated observation shares one token.
- [ ] Allocation mismatch for one identity is rejected.

### H2.5 Retain capture owners in GPU wrapper caches

**Files:** `crates/hypercolor-macos-gpu-interop/src/macos.rs`,
`crates/hypercolor-macos-gpu-interop/src/screen_capture.rs`,
`crates/hypercolor-daemon/src/render_thread/sparkleflinger/gpu.rs`

**Depends on:** H2.4

**Parallel:** No

#### Implementation

- Retain the capture owner in direct wgpu, Core Video, and native Metal cache
  entries.
- Add one `clear_capture_caches()` operation for all screen-specific caches.
- Clear caches on route retirement and before Metal recovery.

#### Verify

- [ ] Every cache keeps admission alive after the current frame drops.
- [ ] Eviction and explicit clearing release admission ownership.
- [ ] Re-import does not lose live ownership.

## Wave 3: GPU-only execution and recovery

### H3.1 Make native execution a typed macOS requirement

**Files:** `crates/hypercolor-core/src/input/screen/publication.rs`,
`crates/hypercolor-core/src/input/screen/macos.rs`, daemon demand and publication
binding modules

**Depends on:** H2.1

**Parallel:** No within the GPU lane

#### Implementation

- Let demand describe output kind, extent, processing profile, and cadence.
- Bind the executor at render commit against the current native target.
- Give production macOS a native-required constructor without a CPU option.
- Preserve intentional generic policies on other platforms.

#### Verify

- [ ] Missing and incompatible macOS targets resolve no CPU branch.
- [ ] Windows and generic fallback behavior remains unchanged where intentional.
- [ ] macOS telemetry reports only native states.

### H3.2 Complete native Metal publication capabilities

**Files:** `crates/hypercolor-macos-gpu-interop/src/screen_capture.rs`,
`crates/hypercolor-macos-gpu-interop/src/native_reduction.rs`, Metal shader
sources, daemon macOS GPU execution and tests

**Depends on:** H3.1

**Parallel:** No

#### Implementation

- Implement every production operation still unsupported by Metal, including
  edge-extend letterboxing.
- Validate target identity, generation, device, format, extent, colorimetry, and
  descriptors before publication.
- Keep preview consumers on renderer-bound GPU publications.

#### Verify

- [ ] Native operations cover every production processing profile.
- [ ] Target and descriptor mismatches fail as typed native capability errors.
- [ ] No consumer creates a private macOS CPU branch.

### H3.3 Rebuild failed Metal execution transactionally

**Files:** daemon `sparkleflinger/gpu.rs`, new internal
`sparkleflinger/gpu/macos_screen.rs`, GPU interop cache APIs, renderer tests

**Depends on:** H2.5, H3.1, H3.2

**Parallel:** No

#### Implementation

- Introduce `Ready`, `Invalidating`, `Rebuilding`, and `Unavailable` states.
- Clear old compositor output and every screen GPU cache on structural failure.
- Fence failed target generations and publish replacements transactionally.
- Couple queue behavior and recovery through a typed copy outcome.
- Retry native reconstruction without opening a CPU path.

#### Verify

- [ ] One injected import failure clears old output and creates a new target.
- [ ] Repeated failures never retain a permanently stale image.
- [ ] Rebuild failure becomes unavailable without CPU demand.
- [ ] Publications for failed target generations are rejected.
- [ ] A valid replacement target restores output.

### H3.4 Share SDR and HDR tone transitions

**Files:** `crates/hypercolor-core/src/input/screen/tone_map.rs`,
`crates/hypercolor-core/src/input/screen/hub.rs`, core macOS publication,
daemon Metal screen execution, parity tests

**Depends on:** H3.3

**Parallel:** No

#### Implementation

- Give each managed native route one shared 250 ms tone-map transition.
- Sample the transition at the capture timestamp.
- Carry immutable sampled constants in the native work payload.
- Preserve the transition across compatible runtime replacement.

#### Verify

- [ ] GPU output matches the fixture CPU oracle at 0, 125, and 250 ms.
- [ ] SDR-to-HDR and HDR-to-SDR transitions match.
- [ ] Mid-transition retarget begins from the current interpolated curve.
- [ ] Readback differs by at most one 8-bit code value.

### H3.5 Remove production macOS CPU publication

**Files:** core macOS screen runtime, daemon publication and telemetry, fixture
modules, new `scripts/check-macos-gpu-only.sh`, `.github/workflows/ci.yml`

**Depends on:** H3.2, H3.3, H3.4

**Parallel:** No

#### Implementation

- Delete CPU executor storage, CPU fanout, scalar production publication, and
  full-frame production mapping.
- Move scalar reducers and reference frames behind fixture-only compilation.
- Remove `cpu_fallback` from production macOS telemetry.
- Add and wire a GPU-only architecture check.
- Prove the new guard fires against a deliberate forbidden fixture and remains
  quiet on production code.

#### Verify

- [ ] Production macOS cannot request or construct a CPU capture executor.
- [ ] `scripts/check-macos-gpu-only.sh` passes normally and fails against an
      injected forbidden symbol.
- [ ] macOS fixture parity tests remain available.

## Wave 4: state ownership and generated contracts

### H4.1 Add an authoritative interaction consumer registry

**Files:** core input routing, new `interaction_consumers.rs`, input status,
daemon authoritative and preview routing, focused tests

**Depends on:** H3.1 before shared daemon routing edits

**Parallel:** No within shared input routing files

#### Implementation

- Key registrations by consumer and selected-source incarnation.
- Replace routes atomically.
- Derive active consumer count from registrations.
- Use generation-fenced teardown for previews and reconnects.

#### Verify

- [ ] One authoritative route plus two previews reports three consumers.
- [ ] Reroute, duplicate commit, stale generation, and teardown remain exact.
- [ ] Failed previews leak no registration.

### H4.2 Add actionable capture and recovery telemetry

**Files:** core input status, daemon status and diagnostics, shared API types and
consumer tests where public

**Depends on:** H2.1, H2.4, H3.3, H4.1

**Parallel:** No

#### Implementation

- Report publication invalidation epoch and transaction state.
- Report live IOSurface count, admitted bytes, and cache-retained bytes.
- Report native target generation and recovery state.
- Report authoritative and preview consumer counts.
- Report protected-control rejection without exposing credentials.

#### Verify

- [ ] Status values move with injected lifecycle, cache, and recovery changes.
- [ ] Sensitive credential material never appears in logs or diagnostics.
- [ ] Public consumers tolerate future enum additions.

## Wave 5: signing, rollout, and release provenance

### H5.1 Remove signing secrets from process arguments

**Files:** `scripts/sign-macos-artifacts.sh`, audited Security.framework helper,
signing-script tests, public CI validation only

**Depends on:** none

**Parallel:** Yes, with Waves 2 through 4 outside the workflow file

#### Implementation

- Import PKCS#12 and ephemeral-keychain credentials through stdin or private file
  descriptors and Security.framework.
- Keep all release credentials and signed-release orchestration in the
  proprietary build system. Public CI never receives release credentials.
- Keep an explicit file-based App Store Connect API-key interface for the
  proprietary build system.
- Keep Apple-ID mode interactive through a stored notarytool keychain profile.
- Remove password-bearing argv paths.

#### Verify

- [x] Sentinel credentials never appear in argv, logs, receipts, xtrace, or
      cleanup output while another process polls the process table.
- [ ] Existing signature, entitlement, notarization, and stapling verification
      remains green.
- [x] Public workflows contain no Apple release-secret references and never
      publish unsigned macOS artifacts as releases.

### H5.2 Bind physical acceptance to immutable artifacts

**Files:** public promotion contract, proprietary release workflow, TCC canary
runner and receipt validator, artifact verification scripts, canary tests

**Depends on:** H1.3, H1.5, H5.1

**Parallel:** No

#### Implementation

- Build, sign, notarize, staple, checksum, and upload each macOS candidate once
  in the proprietary build system.
- Drive the production app and daemon with a separate signed canary harness.
- Bind receipts to artifact and inner Mach-O digests, signing identity, commit,
  version, architecture, OS, topology, and every required TCC row.
- Make proprietary macOS release and Homebrew publication consume only artifact
  IDs and receipt bundles produced by the accepted build. Export public
  provenance, never signing credentials.
- Forbid post-canary rebuilds.

#### Verify

- [ ] Missing, failed, duplicate, stale, wrong-architecture, wrong-identity, and
      digest-mismatched receipts block publication.
- [ ] A one-byte artifact mutation blocks release.
- [ ] Receipt replay against another commit or version blocks release.
- [ ] The promoted artifact hashes equal the physically accepted hashes.

## Wave 6: module decomposition

### H6.1 Decompose capture and GPU modules behind stable facades

**Files:** `hypercolor-macos-capture/src/native.rs`, core macOS screen runtime,
daemon macOS GPU runtime and their new internal modules

**Depends on:** Waves 2 and 3 green

**Parallel:** One exclusive owner per module split

#### Implementation

- Keep `MacosScreenCaptureSession`, `MacosScreenCaptureInput`, and
  `MacosScreenBridge` as narrow facades.
- Split capture into stream, mailbox, lifecycle, transactions, picker, frame
  decode, reference, capabilities, and tests.
- Split core publication into control, admission, publication, status, worker,
  fixtures, and tests.
- Split daemon Metal execution into preparation, import, reduction, cache,
  color, recovery, and tests.
- Keep fixture modules unavailable to production builds.

#### Verify

- [ ] Focused capture, core, interop, and daemon suites pass after each move.
- [ ] Public API shape remains unchanged unless an earlier task intentionally
      changed it.
- [ ] Production binaries contain no fixture-only symbols.

### H6.2 Decompose ownership, app commands, and canary modules

**Files:** `hypercolor-macos-owner/src/lib.rs`,
`hypercolor-app/src/ownership.rs`, `hypercolor-daemon/src/macos_tcc_canary.rs`

**Depends on:** H1.5 and H5.2

**Parallel:** One exclusive owner per crate

#### Implementation

- Split owner model, store, guard, journal, coordinator, executor, process
  identity, and tests.
- Split app commands, planning, executor, launchd, Homebrew, remediation, and
  tests.
- Split canary identity, artifacts, rows, receipts, validation, and harness
  protocol.
- Preserve narrow stable facades.

#### Verify

- [ ] Owner crash and replay matrix passes after the split.
- [ ] App command and packaging tests pass.
- [ ] Canary receipt validation remains byte-for-byte equivalent.

## Wave 7: final verification and acceptance

### H7.1 Run repository and platform gates

**Files:** no planned product edits

**Depends on:** Waves 0 through 6

**Parallel:** Independent verification agents may run non-mutating gates in
parallel.

#### Verify

- [ ] `just fmt-check` passes.
- [ ] `just check` passes.
- [ ] `just lint` passes.
- [ ] `just test` passes.
- [ ] `just verify` passes.
- [ ] `just ui-test` passes.
- [ ] `just ui-build` passes.
- [ ] `just sdk-lint` passes.
- [ ] `just sdk-check` passes.
- [ ] `just sdk-build` passes.
- [ ] `just python-ws-protocol-check` passes.
- [ ] `just python-verify` passes.
- [ ] `just python-generate-check` passes.
- [ ] `just docs-build` passes.
- [ ] Linux, Windows, Apple Silicon, and Intel CI lanes pass with nonzero test
      counts.

### H7.2 Prove the same-machine account boundary

**Files:** physical acceptance receipts only

**Depends on:** H1.3, H1.5, H3.5

**Parallel:** No

#### Verify

- [ ] User B cannot call protected REST routes against user A.
- [ ] User B cannot subscribe to user A's screen or input streams.
- [ ] No unauthorized prompt, picker, mutation, pixel payload, or key payload
      occurs.
- [ ] A pre-bound server on port 9420 cannot become the app document.
- [ ] User A's attested app session succeeds.
- [ ] Fast user switching, crash/restart, lock/unlock, and configured API keys
      preserve the boundary.

### H7.3 Prove GPU correctness and lifecycle recovery

**Files:** physical acceptance receipts only

**Depends on:** H2.5, H3.5, H4.2

**Parallel:** Can run beside H7.2 on separate hosts

#### Verify

- [ ] Apple Silicon and Intel pass SDR and HDR capture.
- [ ] Sleep/wake, source repick, resolution changes, and native-plane changes
      recover without daemon restart.
- [ ] Rapid activate/deactivate keeps the main thread responsive.
- [ ] Injected Metal failure clears old output and restores a new target
      generation.
- [ ] More than eight historical IOSurfaces rotate without false exhaustion.
- [ ] A 30-minute 4K60 run preserves cadence and zero full-frame CPU copies.
- [ ] 4K120 passes where supported hardware is available.
- [ ] A four-hour lifecycle soak leaks no transaction, surface, cache, or
      consumer registration.
- [ ] Instruments shows no production CPU reducer or full-frame CPU fallback.

### H7.4 Prove upgrade, rollback, and artifact promotion

**Files:** physical and workflow acceptance receipts only

**Depends on:** H5.2

**Parallel:** Can run beside H7.2 and H7.3 on separate hosts

#### Verify

- [ ] App sidecar, direct launchd, and Homebrew upgrade and rollback succeed.
- [ ] Every old/new launcher and daemon combination has defined behavior.
- [ ] Failure injection restores the prior complete installation.
- [ ] TCC revoke/regrant and topology transitions preserve ownership integrity.
- [ ] Release promotion uses exactly the accepted signed artifacts.

## Parallel execution and collision control

Four implementation lanes may run concurrently:

1. API security and bundled app origin.
2. Ownership, launcher, signing, and release provenance.
3. Capture lifecycle, IOSurface ownership, and Metal recovery.
4. Generated contracts, consumer accounting, CI, and documentation.

Shared surfaces are serialized:

- One owner controls `.github/workflows/ci.yml`.
- App attestation lands before launcher work touches the supervisor.
- Consumer accounting lands before GPU demand changes touch daemon routing.
- The capture and renderer lane exclusively owns macOS screen publication files.
- Broad decomposition begins only after functional remediation is green.
- Broad formatting waits for integration.
- Every atomic commit runs its focused gate before handoff.

Each non-trivial wave receives independent adversarial verification from an agent
that did not implement it. Security and GPU recovery always receive focused
passes. A failed review returns to implementation and repeats verification before
the next wave.

## Rejected alternatives and non-goals

- Do not add a second ownership lock or make attestation an ownership lease.
- Do not treat loopback, CORS, Host, Origin, or Fetch Metadata as user identity.
- Do not narrow the PID race and keep bare-PID signaling.
- Do not proxy capture frames through Tauri.
- Do not disable protected previews or sensitive input features.
- Do not lower frame rate, resolution, cadence, or performance baselines.
- Do not add retries as a substitute for exact invalidation and recovery.
- Do not keep launcher topology only in argv.
- Do not expose signing credentials through environment variables as a substitute
  for deliberate secret transport.
- Do not physically test one binary and rebuild another for release.
- Do not pursue the disproven `codesign -d -r-` stdout finding.

## Progress ledger

- [x] Deep review completed at baseline `854f36f`.
- [x] GPU-only production requirement approved.
- [x] Follow-up architecture and implementation plan approved.
- [x] Wave 0 completed in commits `29598732`, `165dc483`, `423af8a`, and
      `3f4a4a5` with independent verification.
- [ ] Wave 1 in progress.
- [ ] Wave 2 pending.
- [ ] Wave 3 pending.
- [ ] Wave 4 pending.
- [ ] Wave 5 pending.
- [ ] Wave 6 pending.
- [ ] Wave 7 pending.

Active tasks: H1.1 protected-control authorization and H1.4 identity-safe owner
handover.
