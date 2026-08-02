# Spec 74: PipeWire Metadata, D4 Transforms, and Topology Truth

Status: draft, revision 2 after skeptic review (REWORK verdict addressed).
Executes the remaining core of spec 73 T17 on the foundation the
2026-08-02 first-light session proved live: compositor-real format
negotiation, native-extent adoption, and session-stamped restore tokens
(commits `ba15626f..6fe3254c`).

## Mission

Make Wayland capture geometrically truthful. Today every live frame claims
full-frame crop, identity transform, and a topology frozen at session
establishment. On a rotated output the pixels are wrong; on a cropped or
padded stream the sampled region is wrong; on a mid-stream mode change the
published geometry is stale. Each of these is invisible to the test suite
because the live PipeWire boundary has no fixture seam.

Field evidence from the first-light session (hyperia, COSMIC): DP-1 runs
`rotate90` at 150% scale and cannot be captured correctly by the current
identity-transform path. The acceptance matrix rows for rotation,
reflection, crop, and same-stream renegotiation are all blocked on this
spec.

## Non-negotiable constraints

- The spec 73 invariants hold throughout: exact extents, byte and compute
  admission before allocation, transactional plan transitions, one bounded
  callback copy, no product-baseline lowering.
- The four-state `CaptureRotation` is replaced, not wrapped. No adapter
  enum, no lossy mapping at the backend boundary.
- Present-but-malformed metadata drops the frame with a typed, per-reason
  counter. Absent metadata means full frame and identity transform.
- The PipeWire callback stays exact and minimal: validate, copy once,
  requeue before returning. Metadata is copied in the callback, applied in
  canonical processing.
- Unsafe code lives only in the new audited interop crate, which carries
  `#![deny(unsafe_op_in_unsafe_fn)]` and documented invariants per block,
  matching the existing platform-interop crates.

## Why an interop crate is mandatory

pipewire-rs 0.9 exposes `Buffer` with a private `NonNull<pw_buffer>` and
no meta accessor; the raw dequeue (`StreamRef::dequeue_raw_buffer`) is
`pub unsafe`. Reading `spa_meta_region` and `spa_meta_videotransform`
requires walking the raw `spa_buffer` metas array through the sys types.
That is FFI pointer arithmetic against a C ABI, and the workspace forbids
unsafe outside the audited interop crates, so `hypercolor-core` can
neither dequeue raw nor inspect metas itself.

## Wave 1: `hypercolor-pipewire-interop`

New crate, Linux-only, modeled on `hypercolor-linux-gpu-interop`'s audit
posture. Depends on the `pipewire` crate (for its `sys` re-exports; the
bare sys crates are not imported directly) and `thiserror`. No dependency
on hypercolor-core; core depends on interop, mirroring the
windows-capture direction so core's `unsafe_code = "forbid"` stands.

The boundary is a visitor over a guard-owned dequeue, because core can
never hold a raw buffer pointer: the process callback receives
`&StreamRef`, whose `as_raw_ptr()` is safe and public, and hands that
pointer to interop, which performs the unsafe dequeue internally, lends a
view, and requeues on drop.

Surface:

- `with_dequeued_buffer(stream: *mut pw_sys::pw_stream, visit: impl FnOnce(SpaBufferView<'_>) -> V) -> DequeueOutcome<V>`
  dequeues inside the crate, validates buffer, data, chunk, and meta
  pointers plus sizes before the visitor runs, and requeues exactly once
  on every path: visitor return, validation fault, and visitor panic
  (`catch_unwind` inside, panic reported as a typed outcome, never
  unwound into the C trampoline; since Rust 1.81 such an unwind aborts
  the daemon, which one bad frame must not do).
- `SpaBufferView<'_>`: non-`Send`, lifetime-bound to the guard, exposing
  the validated plane bytes plus:
  - `crop() -> Option<Result<PixelRect, SpaMetaFault>>` for
    `SPA_META_VideoCrop`; absent is `None`, present-but-malformed
    (undersized meta, negative or overflowing rect) is `Some(Err(..))`.
  - `transform() -> Option<Result<SpaD4Transform, SpaMetaFault>>` for
    `SPA_META_VideoTransform` with the full eight-value vocabulary and
    out-of-range ids as faults.
- `DequeueOutcome::Empty | Faulted(SpaBufferFault) | Visited(V) | VisitorPanicked`
  so the caller's drop accounting stays typed. The fault taxonomy absorbs
  the first-line checks core performs today (missing buffer, missing
  plane, unmapped plane) so the callback's pre-decode classification has
  one home.
- ABI assertions: compile-time size/offset checks for the meta structs on
  x86_64 and aarch64, per T17.

Verification: unit tests build synthetic `spa_buffer` layouts in safe Rust
(byte arrays with hand-placed metas) covering valid, absent, undersized,
misaligned, and out-of-range cases; requeue-exactly-once on return, fault,
and panic paths via a counting stub. T17's copy-linearity clause (time
and bytes scale linearly with the validated chunk size and remain within
the negotiated buffer deadline) is measured at the decode/processing
fixture seam in Wave 5, where the bounded copy actually lives, not here.

## Wave 2: the eight-state transform vocabulary

Replace `CaptureRotation { Identity, Cw90, Cw180, Cw270 }` with the SPA D4
group: the four rotations plus `Flipped`, `Flipped90`, `Flipped180`,
`Flipped270`. One canonical type in `screen/frame.rs`, serde snake_case,
with exhaustive `apply_to_extent`, `apply_to_point`, and `invert` methods
so axis-swap logic stops being open-coded at call sites.

The frame-level D4 transform is the single carrier of orientation and
reflection for Wayland. The existing descriptor-level
`ScreenSourceReflection` channel remains for its current purpose
(source-normalization requests) and Wayland continues to assert `None`
there; the two must not compose. Canonical processing drops a frame that
carries both a reflected D4 transform and a non-`None` descriptor
reflection, with a typed per-reason counter, consistent with the
malformed-metadata policy (a debug assertion would compile out of the
daemon's preview profile). Windows keeps mapping DXGI rotation into the
rotation subset, unchanged.

Known ripple (receipts current at `5ce4298e`): `process.rs:170-176, 315,
407-428` (plane transform and cursor mapping), `sampling.rs:164-166,
1105-1112` (rotation-reflection composition), `publication.rs:3148`,
`windows.rs:1996, 4079`, plus every four-arm `match` the compiler flags
once the variants land.

Verification: the T17 unique-corner image test, all eight transforms, at
asymmetric extents (portrait, ultrawide, odd dimensions), asserting exact
pixel placement after canonical processing; cursor geometry through the
same exhaustive mapping; the both-channels-reflected drop has a negative
test asserting the typed counter.

## Wave 3: topology advances with the stream

Ordered before metadata honoring: applying real transforms against a
topology frozen at session establishment mixes axes in `source_scale`
(logical width over pre-transform native width) and rescales crops
against stale extents, so topology truth must land first.

- `WaylandTopologySignature` gains the negotiated native extent and the
  stream transform. A same-portal-stream change in either advances
  `topology_generation` and re-resolves the source epoch; the current
  cached-topology behavior is deleted, and
  `physical_topology_persists_across_storage_resize_and_session_restart`
  is rewritten to assert the advance.
- `source_scale` compares logical dimensions against the post-transform
  extent, swapping axes for quarter turns.
- Frames whose storage extent disagrees with the resolved topology drop
  with a typed counter instead of sampling through a stale geometry.
- The replan actor is named and built here, because no current actor
  replans committed exact branches on a topology change: committed plans
  pin the full capture epoch, and the daemon pump
  (`hypercolor-daemon/src/render_thread/input_publication.rs`, in this
  wave's blast radius) wakes only on demand and graph changes; the
  source's `resolution_revision` is today a commit fence read inside an
  already-running transition, never a trigger. This wave folds the
  resolution revision into the pump's applied-state key so a source
  resolution change triggers a replan transition on the pump's next
  scheduled iteration; there is no wake primitive, the bound is one pump
  scheduling quantum, and that suffices because mid-stream changes only
  occur while demand keeps the pump cycling. The pump needs cheap
  per-iteration access to the revision (a mirrored atomic or the
  existing reader; the value currently sits behind the manager lock).
  The transition retires the stale-epoch branches and re-prepares
  against the new topology through the existing transactional path. The worker-internal
  `RequiresNativeExtent` restart from `ba15626f` remains the
  pre-publication path; post-publication changes go through the pump
  replan.

Verification: the `PipeWireFormatState` machine fixtures from the Wave 5
list pull forward into this wave's budget (they drive a plain struct and
need no stream facade), joined by daemon tests for the pump: a
mid-stream extent change retires and re-prepares exact branches and
publishes correctly-sized frames on all branches within a bounded number
of frames; a transform change does the same; the pump picks up the
revision fold within one scheduling quantum without a demand or graph
change; no stale-size or stale-orientation frame is ever published
between epochs.

## Wave 4: negotiate and honor the metadata

- Meta negotiation happens where compositors expect it: the
  `param_changed(Format)` callback calls `pw_stream_update_params` after
  fixation with `SPA_PARAM_Meta` pods advertising `VideoCrop` and
  `VideoTransform` (and `SPA_PARAM_Buffers`, decided here since it rides
  the same call). Every renegotiation path that re-enters format
  fixation, including adoption and restoration through
  `update_pipewire_format`, re-issues the same meta advertisement, so a
  format change never silently drops meta support.
- The process callback moves onto Wave 1's visitor; the current inline
  drop checks collapse into `DequeueOutcome`, and crop plus transform
  are copied into `SpaChunkView`, whose plumbing through `decode_chunk`
  into analysis already carries both. The hardcoded `None`/`Identity`
  construction site disappears.
- Malformed metadata increments a per-reason `ChunkDropReason` counter
  and drops the frame. The collapsed single `dropped_frames` counter
  becomes a per-reason map surfaced through source status (daemon-local
  telemetry; no `hypercolor-types::api` contract change) so acceptance
  runs distinguish `InvalidCrop` from `BufferUnavailable` without daemon
  logs.
- Crop applies in raw-plane coordinates before the transform, exactly
  once, in canonical processing, feeding the machinery Windows input
  already ordered this way.

Verification: fixture-driven meta cases (valid, absent, malformed for
both metas), the crop-before-transform ordering against the
unique-corner image, per-reason counters visible in a status snapshot
test, and a renegotiation test proving meta advertisement survives a
format adoption round trip.

## Wave 5: the fixture seams, named honestly

There is no single seam; there are four, and the stream-event seam
requires a refactor this wave budgets explicitly:

- Interop unit fixtures (Wave 1's own tests): synthetic buffer layouts,
  meta faults, requeue accounting. Live in the interop crate.
- Pure decode and processing fixtures: `SpaChunkView`/`decode_chunk` and
  the canonical crop/transform path, extended with row padding and the
  eight-transform matrix. Possible today, extended here.
- `PipeWireFormatState` machine fixtures: format sequences, adoption,
  restoration, rejection ordering. The state machine is already a plain
  struct; tests drive it directly.
- Stream-event injection: `param_changed`, `state_changed`, and process
  events against the worker's loop logic. This requires extracting the
  three inline closure bodies in `run_pipewire_loop` into named
  event-handler functions over a narrow stream-facade trait (the real
  stream and a fixture stream implement it). The extraction itself is
  unconditional production restructuring and this wave's budgeted
  refactor; only the fixture facade implementation sits behind a
  `wayland-capture-fixtures` feature mirroring
  `windows-capture-fixtures`. Stream death, recovery on replacement
  (T17's kill-and-replace clause), analysis-worker exit, and the
  copy-linearity measurement from Wave 1's deferral run at these seams
  in CI without a compositor.

## Acceptance

- All eight transforms verified against the unique-corner image in CI.
- Live: DP-1 (rotate90, 150% scale) captures with correct orientation
  and pointer mapping; a compositor-side resolution change mid-stream
  retires and re-prepares exact branches and publishes correctly-sized
  frames on all branches within a bounded number of frames; crop
  metadata from a synthetic PipeWire source samples the exact region.
- Per-reason drop counters visible in `/api/v1/status`.
- `just verify` green including the new crate; cargo-deny clean.

## Out of scope

DMA-BUF GPU import (tracked separately), SPA colorimetry negotiation (the
sRGB stamp from `ba15626f` stands until then), portal pending-state
telemetry, and demand-side acquisition-envelope negotiation (the
branch-aware task covers demand-driven changes; this spec covers
compositor-driven ones).
