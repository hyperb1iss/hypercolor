# Spec 73: Input and Capture Hardening

Status: APPROVED (Claude cross-model review PASS, round 6; T11 amendment PASS,
round 5 plus final delta); arbitrary-resolution amendment PASS, round 2 plus
final delta
Author: Nova
Date: 2026-07-26
Depends on: spec 14, spec 71, spec 72
Acceptance: physical validation follows
[docs/development/LINUX_CAPTURE_ACCEPTANCE.md](../development/LINUX_CAPTURE_ACCEPTANCE.md)
Scope: the complete input data plane and the screen/video capture stack on Linux
and Windows, including daemon, renderer, SDK, UI, diagnostics, and CI consumers.

## Mission

Make Hypercolor's input and screen capture stack production-grade after Windows
support exposed lifecycle, correctness, performance, and contract gaps that were
previously hidden by Linux-only assumptions.

This spec is the execution plan for every confirmed finding from the 2026-07-26
thermonuclear review. It is intentionally broader than a Windows patch. Windows
made the gaps visible; the fixes establish one coherent cross-platform model.

## Baseline and evidence

The review was performed at `nova/windows-host-input` commit `16e6671b`; this
plan branch starts at `cb6ecc13`, whose intervening commits are unrelated effect
artwork changes. The relevant baseline receipts were:

- `just test-crate hypercolor-windows-input` -> 58 passed, 0 failed.
- `just test-crate hypercolor-windows-capture` -> 10 passed, 0 failed, including
  a live Desktop Duplication frame.
- `just test-crate hypercolor-core input` -> 67 passed, 0 failed.
- `just test-crate hypercolor-daemon input` -> 13 passed, 0 failed.
- `just test-crate hypercolor-daemon screen` -> 32 passed, 0 failed.
- `just ui-test` did not compile tests on this Windows host. The UI crate reached
  the MSVC linker and failed with `LNK4003` and `LNK1120`; the cause is not yet
  established. Sibyl task
  `3f5b59f1-2dfb-4a70-bed2-45689627afc8` tracks that harness defect.

Passing tests prove the current happy paths, not production readiness. The
review combined source tracing, Windows live capture, targeted tests, and primary
Microsoft documentation for Raw Input, DPI, and Desktop Duplication behavior.

## Non-negotiable invariants

1. Consent and demand are separate. `enabled` grants permission; capture starts
   only when an active consumer demands the source.
2. A source never reports `live` after its worker exits or its data expires.
3. Start, reconfigure, and stop are transactional. Failure preserves the last
   known-good configuration and releases partially acquired resources.
4. The render thread never performs blocking device I/O, image decoding, FFT,
   source discovery, or unbounded allocation.
5. High-frequency data uses latest-value snapshots or bounded rings. Discrete
   events preserve order, multiplicity, timestamp, sequence, and source identity.
6. Capture cadence controls analysis and publication without lowering product
   ceilings. Acquisition may remain native-rate when the backend requires it.
7. Pixel geometry is explicit: extent, rotation, crop, aspect policy, colorspace,
   cursor composition, and effective analysis grid never travel as assumptions.
8. Raw and processed capture surfaces are distinct types. A consumer cannot
   accidentally bypass or double-apply tuning, smoothing, crop, or letterboxing.
9. Physical device identity is stable across hotplug and native handle reuse.
10. Browser and host input never double-deliver implicitly. Routing is explicit
    per consumer, with deterministic arbitration and diagnostics.
11. Health, capability, recent activity, and data freshness are separate fields.
12. No implementation may trade correctness for a permanent FPS, resolution,
    queue, sampling, or concurrency nerf.
13. Resolution is negotiated runtime shape, never a compile-time or product
    ceiling. Every extent and byte count is checked before allocation; pressure
    produces typed resource diagnostics rather than silent downscaling.

## Target architecture

### Source control plane

Every source publishes a `SourceStatus` snapshot with:

- source identity and kind;
- configured, consented, and demanded flags;
- lifecycle state: `stopped`, `starting`, `live`, `degraded`, `unavailable`, or
  `failed`;
- monotonic source-graph generation and per-source session generation;
- last successful sample time and freshness deadline;
- resource count, backend name, and structured error/remediation data.

`InputManager` owns the source graph generation. Adding, replacing, removing, or
restarting any source bumps it monotonically; consumers never derive generation
by summing source-local counters.

### Source data plane

Sources publish immutable `Arc` snapshots into per-source latest-value slots and
bounded event rings. The render loop reads a lock-free graph snapshot, selects
sources according to an explicit route, and borrows or clones `Arc` handles only.
It does not scan mutable sources under an async mutex or allocate fresh aggregate
vectors every frame.

Lifecycle commands and diagnostics stay on the control plane. Samples never
carry worker handles, mutable backend state, or configuration locks.

### Capture frame contract

Screen backends publish a backend-neutral `CaptureFrame` envelope:

- stable source id and topology generation;
- session generation, frame sequence, capture timestamp, and freshness deadline;
- physical extent, rotation, crop, source scale, colorspace, and transfer
  function;
- cursor metadata and whether the cursor is already composed;
- storage kind: CPU pixels or a platform GPU surface;
- optional dirty and move regions;
- explicit raw/processed stage marker.

Acquisition, analysis, and publication are separate latest-value stages. Native
frames may arrive at display cadence; analysis runs at configured cadence against
the newest frame; publication replaces stale data rather than queueing latency.

### Resolution and resource contract

Source, analysis, compositor, effect, and preview extents are independent typed
shapes. The source preserves its native logical geometry; each consumer declares
the extent and aspect policy it actually needs. Producers reuse an existing
surface only when its descriptor satisfies that contract, and otherwise build a
new generation transactionally.

There is no fixed 640x480, 4K, 8K, or platform-specific maximum in shared code.
A finite request is accepted when checked width, height, stride, plane size,
encoded size, GPU allocation, and in-flight byte budgets fit the active backend.
Overflow, allocation failure, or an exhausted resource budget returns a typed
error containing the requested descriptor and limiting resource. It never
changes the requested resolution or cadence implicitly.

Native acquisition stays latest-value and may feed multiple derived resolutions.
Compatible consumers share immutable producer surfaces and reduction work;
incompatible descriptors remain independent. CPU work scales with the pixels a
consumer actually requests, GPU work uses reusable descriptor-keyed resources,
and interactive transport is keyed latest-value with encoded-byte accounting and
bounded chunk reassembly. Resolution changes advance generation and replace all
shape-dependent pools, smoother state, routes, and transport assemblies at one
transactional boundary.

Property and integration tests cover zero/overflow rejection, one-pixel and odd
extents, portrait, ultrawide, rotated, negative-origin, live resize, 1080p, 4K,
8K, and synthetic dimensions beyond current display hardware where allocations
remain practical. Performance gates report pixels and bytes processed so results
normalize across shapes rather than blessing one canonical resolution.

### Arbitrary-resolution descriptor amendment

The first exact-demand implementation represented the aggregate screen demand as
one cadence plus one component-wise maximum extent. That fold is invalid. An
ultrawide `5120x720` consumer and a portrait `1920x2160` consumer must not create
an unrequested `5120x2160` analysis surface. The following contract amends T12,
T14, T16, T18, T22, and T23. It is the only long-term screen-demand model.

#### Logical demand and output contracts

At steady state, each consumer registers one immutable screen publication request
plus its requested cadence. A descriptor change holds old and staged requests
simultaneously only inside the two-plan lease helper. The registry preserves
entries independently and builds a canonical, sorted plan. It merges cadence only
for descriptors that are exactly equal.

```rust
enum ScreenExtentRequest {
    Native,
    Bounded {
        max_width: Option<NonZeroU32>,
        max_height: Option<NonZeroU32>,
        upscale: ScreenUpscalePolicy,
    },
}

enum ScreenUpscalePolicy {
    Never,
    Allow,
}

enum ScreenPublicationKind {
    Surface,
    Zones { columns: NonZeroU32, rows: NonZeroU32 },
}

struct ScreenPublicationRequest {
    source: ScreenSourceSelector,
    publication: ScreenPublicationDescriptor,
}

struct ScreenPublicationDescriptor {
    source: CaptureSourceId,
    kind: ScreenPublicationKind,
    extent: ScreenExtentRequest,
    aspect: ScreenAspectPolicy,
    processing_profile: Arc<ScreenProcessingProfile>,
}

struct ScreenBranchDemand {
    descriptor: ScreenPublicationDescriptor,
    requested_hz: NonZeroU32,
}

struct ScreenCapturePlan {
    generation: ScreenPlanGeneration,
    branches: Arc<[ScreenBranchDemand]>,
}
```

`Native` requests the processed native logical surface. `Bounded` accepts either
axis independently, so WebSocket requests with one zero axis preserve their
current aspect-derived semantics. `None, None` canonicalizes to `Native`.
`Never` means a bound, not a requested raster size: derived analysis never
manufactures pixels beyond the processed source. `Allow` is explicit and is used
only when a consumer genuinely requires resampling. Final compositor, preview,
or encoder raster size remains a separate exact output descriptor.

`ScreenAspectPolicy` is the geometric fit rule used while resolving an extent.
The processing profile's letterbox policy is the fill treatment applied once
after that resolution in T14's canonical processing order.

The request's source selector resolves to the descriptor's stable
`CaptureSourceId` before a branch enters the canonical plan. `Primary` and other
policy selectors remain control-plane inputs, never publication identities. A
topology or source-resolution change produces a new resolved source epoch.

Surface and zone publications are independent kinds. Zone analysis has its own
extent, grid, cadence, and processing state. Attaching a preview or passive canvas
subscriber cannot change LED analysis quality. A temporary compatibility adapter
may combine one surface branch and one zone branch into legacy `ScreenData`, but
that adapter is a consumer of the plan and never its owner.

`ScreenProcessingProfile` is immutable and equality-complete. It includes every
consumer-selectable operation that can change derived bytes: letterbox policy,
smoothing time constant and scene-cut policy, tuning, cursor policy, grid policy,
reduction filter, target pixel format and colorspace, and algorithm revision.
Rotation, reflection, crop, physical origin, source scale, native pixel format,
source colorspace, and transfer function remain source-frame metadata and are
applied exactly once before or during resolution. They still participate in the
resolved sharing keys below.

#### Resolution and sharing proof

Logical branches never collapse into an envelope. Every branch resolves as if it
were the only consumer. Sharing occurs only after independent resolution produces
equal internal descriptors.

```text
Resolved source epoch
  = stable source id
  + topology generation
  + capture session generation
  + native/storage/logical geometry
  + rotation, reflection, crop, origin, and scale
  + source pixel format, colorspace, and transfer function

Physical reduction descriptor
  = resolved source epoch
  + selected region and cursor-composition policy
  + resolved reduction extent
  + reduction filter and algorithm revision
  + target storage format and colorspace
  + backend/device resource generation

Derived analysis descriptor
  = physical reduction descriptor
  + publication kind and logical output descriptor
  + complete processing profile
  + analysis algorithm revision
```

The key types have private constructors that consume the complete resolved
descriptors. Callers cannot hand-assemble partial hashes. Two branches share work
only when full descriptor equality proves byte-for-byte equivalence. Requested
cadence is scheduling state, not a byte-equivalence field; equal branches execute
at the maximum requested cadence and publish latest-value snapshots to all of
their leases.

Property tests compare grouped execution with independent execution over source
geometry, crop, all transforms, color metadata, cursor policy, filters, profiles,
odd dimensions, portrait, ultrawide, and mixed branch kinds. Equality of output
bytes and metadata is the sharing proof.

#### Prepared plan transition

Screen-plan generation is separate from structural input-graph generation.
Descriptor churn cannot rebuild unrelated audio or interaction routes. The
input-publication coordinator owns plan transitions; the render thread only reads
committed immutable snapshots.

```text
Active(N)
  -> Preparing(N+1, base graph generation, base plan generation)
  -> AwaitingBackend(N+1) when platform negotiation is asynchronous
  -> Armed(N+1) after every affected source acknowledges exact resources
  -> Active(N+1) through one atomic committed-plan pointer swap
```

Preparation follows these rules:

- The coordinator validates checked geometry and aggregate byte arithmetic, but
  never constructs a throwaway analyzer or full-frame surface.
- Each source worker receives only its candidate branch delta. It prepares real
  resources once on the thread that owns them while generation N remains active.
- Unchanged branches retain analyzers, smoother history, pools, publications, and
  last-good state by identity. Removed branches are not destroyed during prepare.
- Windows D3D11 duplication, textures, views, queries, and GPU reduction resources
  prepare on the Windows capture worker. Per-source branch analyzers prepare on
  that source's analysis worker.
- Wayland portal, PipeWire main-loop, stream format parameters, and negotiated
  acquisition geometry prepare on the capture main-loop thread. Branch analyzers
  prepare on the analysis worker. A format change is armed only after the backend
  acknowledges the candidate negotiation; the old stream and plan continue while
  consent or negotiation is pending.
- One source owns one analysis executor that iterates due branches. A branch does
  not create an operating-system thread.
- A worker returns an opaque prepared token, exact resource ledger, and resolved
  branch metadata. Tokens cannot be reused across source or plan generations.
- After every source is prepared, the coordinator rechecks the base structural
  graph generation and plan generation. Any mismatch aborts the candidate.
- Arm installs prepared resources behind generation fences without changing the
  active publication catalog. Arm is allocation-free and non-destructive.
- Commit is one atomic swap of the immutable plan and branch catalog. Publication
  accepts a frame only when plan generation, branch key, source epoch, and worker
  token match the committed catalog.
- Any prepare, negotiation, arm, recheck, or commit conflict aborts every prepared
  token and leaves generation N byte-identical. Retirement runs on the owning
  worker after commit and after outstanding leases release their `Arc` handles.

A plan may become committed while a newly exposed branch is still `pending` its
first frame. Continuity-sensitive resize uses a two-plan lease helper: first add
the new branch while retaining the old branch, wait until the new branch is live,
atomically switch the consumer lease, then remove the old branch. Failure leaves
the old lease and plan untouched. Callers cannot express release-then-acquire.

#### Publication hub and epoch fencing

Core owns one keyed publication hub. Its immutable catalog is read through an RCU
or `ArcSwap` pointer. Each branch owns:

- an `ArcSwapOption`-shaped latest-value slot;
- requested and resolved descriptors;
- plan generation and source epoch;
- capture sequence, capture time, publication time, and freshness deadline;
- branch health and resource diagnostics;
- branch-local analyzer, smoother, reusable pools, and last-good publication.

Consumers receive `Arc` snapshots through typed `BranchLease` handles. Reads do
not take the mutable `InputManager` or a worker publication mutex. A lease cannot
return a publication from a branch key or source epoch for which it was not
issued. Publications of branches retained by identity remain continuously valid
across plan commits. Removed catalog entries reject new leases immediately; their
existing leases and storage retire after all readers release them. New branches
start pending and never inherit another descriptor's pixels.

Last-good retention and delivery are distinct. Storage may remain retained for
diagnostics or rollback while current delivery is fenced.

| Event | Current delivery and last-good behavior |
| --- | --- |
| Same-epoch resource or geometry failure | Retain the branch's last-good, publish typed degraded health, and serve it only until its freshness deadline. |
| Static desktop or acquisition timeout | Retain last-good, advance no sequence, and become stale/degraded at the deadline. |
| Access loss that advances capture session | Keep old storage only for diagnostics; do not deliver it under the new epoch. |
| Source, topology, or processing-profile change | Fence current delivery. The new branch starts pending and cannot inherit old pixels. |
| Resize admission or first-frame failure | Keep the old branch and lease live. Return the exact error only to the requesting transition. |
| Worker exit, explicit stop, source removal, or branch retirement | Fence delivery and publish the lifecycle transition before stale data can be sampled. |

Health, freshness, and continuity policy remain separate. Retained data never
masquerades as `live`, and old dimensions never publish under a new key.

#### Platform fan-out, cadence, and resource admission

Windows keeps one Desktop Duplication session and one clean-desktop update per
native frame. CPU fallback maps the native frame once. GPU reduction resources
are keyed by the complete physical descriptor, not one mutable last-used slot, so
alternating branch extents do not rebuild resources every frame. Pointer-only
updates preserve clean-desktop continuity for every active physical key.

Wayland keeps one portal/PipeWire session and one bounded callback copy into a
canonical plane. The negotiated acquisition envelope may use the maximum resolved
need across active branches, or true native acquisition when the backend provides
it. That envelope exists only for capture negotiation. It is never a derived
publication, analyzer allocation, or logical branch.

When a branch-set change would alter a sub-native acquisition envelope, the
backend must either negotiate true native acquisition or carry every unchanged
branch through the prepared transition without an epoch fence or publication gap.
Envelope selection may never perturb a branch whose descriptor did not change.

The shared cadence primitive lands before multi-branch fan-out becomes visible.
Native acquisition runs at the maximum cadence required by a due physical key.
Each logical branch owns its next analysis and publication deadline. A physical
reduction runs only when at least one dependent branch is due. Superseded native
frames replace older frames rather than queueing latency.

Admission accounts for checked dimensions, strides, planes, CPU pools, smoother
and policy storage, GPU resources, encoded transport, and old-plus-staged overlap.
The resource ledger is byte-based and supplied by explicit configuration or real
backend capacity, never an axis, resolution, cadence, or consumer-count cap.
Actual `try_reserve` and backend allocation still provide final admission.
`ResourceExhausted` identifies the requested descriptor and limiting resource.
Unchanged branches reuse resources, and candidate failure never evicts or
downscales healthy work.

#### Migration and deletion gates

The amendment lands in reversible waves:

1. Fix the committed passive WebSocket demand panic, current single-descriptor
   worker adoption, typed resource propagation, and same-descriptor last-good.
2. Add descriptors, branch aggregation, plan generation, keyed hub, and pure
   independent-resolution/sharing tests. Keep one compatibility mirror implemented
   as an ordinary branch in the new plan.
3. Convert Windows and Wayland to worker-owned multi-branch preparation and land
   shared cadence enforcement. Move the authoritative renderer and zone analysis
   to exact branch leases.
4. Move interactive previews and WebSocket screen canvas/zones to exact leases.
   Delete the compatibility mirror, component-wise screen-demand union, and the
   single screen schedule in the same wave.
5. Complete physical-key sharing, descriptor-keyed GPU resources, allocation
   gates, and 4K/8K mixed-shape performance certification.

The compatibility mirror is not a second registry, scheduler, or demand owner.
It is one mechanically deletable branch adapter in the new plan. Its deletion is
a completion gate for wave 4.

Required tests include:

- `5120x720` plus `1920x2160` creates two branches and never allocates or
  publishes `5120x2160`;
- duplicate descriptors create one branch at maximum cadence;
- native, one-axis bounded, two-axis bounded, and explicitly upscaled requests
  survive registration and resolution unchanged;
- bounded never-upscale resolution cannot exceed processed source geometry;
- a zone grid finer than its resolved raster uses area-weighted sampling over
  normalized pixel footprints without allocating an upscaled intermediate;
- surface and zone consumers remain independent under attach, detach, and resize;
- grouped planning and output equal independent execution for every tested key;
- prepare or worker-side allocation failure preserves the exact old plan,
  catalog, manager demand, publications, resource identities, and smoother state;
- concurrent source-graph or plan changes abort a stale prepared token;
- rapid resize has no publication gap and no wrong-generation or wrong-dimension
  frame;
- source, topology, session, and profile changes cannot masquerade retained pixels
  as current;
- one failing branch retains only its own last-good while other branches advance;
- one Windows acquisition and clean-desktop update occur per native frame;
- one CPU map occurs per native frame and GPU dispatch count equals unique due
  physical keys;
- one Wayland callback copy occurs per native frame and acquisition renegotiation
  cannot become a derived publication;
- zero steady-state allocation occurs after branch warmup;
- descriptor churn retires resources without rebuilding the structural input
  graph;
- a passive screen-canvas subscribe/unsubscribe cycle against a live plan cannot
  panic or strand demand; and
- 1080p, 4K, 8K, portrait, ultrawide, duplicate, mixed-profile, and mixed-cadence
  benchmarks report pixels, bytes, allocations, resource rebuilds, and deadline
  percentiles without lowering requested quality.

### Input event contract

`TimedInputEvent` remains canonical after capture. It retains `source_id`,
physical/logical code, state, timestamp, sequence, and repeat multiplicity through
the bus, Servo queue, WebSocket, SDK, and fixtures. Snapshot state is derived from
events but does not replace them.

### Capability and availability contract

Effect metadata is authoritative and typed end-to-end. `input_reactive` and other
source requirements originate in `EffectMetadata`, flow through shared REST
types, and drive daemon demand plus UI behavior. SDK availability reports
declared capability, route selection, source health, and freshness separately;
recent activity is never used as a proxy for availability.

## Confirmed findings ledger

Each finding has exactly one primary implementation task. Task verification may
cover additional findings transitively.

### Screen/video capture

- F01 idle Windows capture despite permission-only config -> T12.
- F02 Windows live capture config incorrectly restart-only -> T12.
- F03 invalid capture limits and unenforced Windows/Wayland analysis FPS -> T12
  for validated limits and T18 for backend enforcement.
- F04 common aspect ratios fall back to an 8x6 reconstructed surface -> T14.
- F05 Servo build replaces the native ScreenCast with an HTML effect -> T19.
- F06 monitor `auto` is index zero and indices are unstable -> T15.
- F07 Desktop Duplication rotation and cursor composition are missing -> T15.
- F08 raw and processed capture semantics diverge by consumer -> T14.
- F09 cropped grids retain configured dimensions and publish black padding -> T14.
- F10 smoothing behavior depends on grid size and frame rate -> T14.
- F11 Wayland workers can die while stale data remains `running` -> T17.
- F12 PipeWire decoding ignores chunk bounds, stride direction, crop, transform -> T17.
- F13 DXGI errors are misclassified and retry sleeps are not interruptible -> T15.
- F14 Windows performs full-frame readback and CPU reduction -> T16.
- F15 screen snapshots and zone identifiers allocate deeply every frame -> T22.
- F16 multiple screen sources use accidental last-writer-wins selection -> T23.
- F17 cached screen demand can strand a newly added source -> T02/T12.

### Windows host input

- F18 Pause, PrintScreen, and Shift+numpad records need stateful canonicalization
  -> T07.
- F19 cursor normalization depends on DPI-unaware metrics and successful thread
  DPI pinning rather than enumerated physical monitor rectangles -> T08.
- F20 Raw Input and wait failures can be mistaken for buffer growth and hot-spin
  -> T08.
- F21 null-device keyboard and mouse records collide as `windows:unknown` -> T09.
- F22 input may precede arrival and delayed arrival resets absolute baselines -> T09.
- F23 handle reuse lacks the promised generation-tagged identity -> T09.
- F24 registration can outlive failed verification -> T08.
- F25 host and browser sources can double-deliver physical actions -> T11.
- F26 summed interaction generations can hide source replacement -> T02.
- F27 unexpected worker exit can remain `running` -> T01/T08.
- F28 UI lacks separate keyboard and mouse consent -> T21.
- F29 Windows CI skips daemon integration -> T24.
- F30 device label resolution allocates per raw record -> T09.
- F31 concurrency and physical acceptance coverage is incomplete -> T08/T25.

### Shared input, audio, media, and consumers

- F32 failed source startup can leak workers or leave partial state -> T06.
- F33 failed audio reconfiguration commits broken state -> T10.
- F34 terminal audio failures preserve stale spectra indefinitely -> T10.
- F35 CPAL callback allocates, locks, and performs DSP -> T10.
- F36 REST drops authoritative `input_reactive` metadata -> T03.
- F37 album art reads and decodes are unbounded -> T20.
- F38 SDK `available` means activity rather than capability/health -> T03.
- F39 Servo frame coalescing loses repeated key multiplicity -> T19.
- F40 Linux and Windows media keys use different logical names -> T20.
- F41 evdev diagonal motion uses Manhattan magnitude -> T07.
- F42 browser injection accepts unbounded payloads and drains from vector fronts
  -> T07.
- F43 the event bus strips `TimedInputEvent` sequence and timestamp -> T04.
- F44 Servo deep-clones every input snapshot per renderer -> T22.
- F45 Linux media capture is one-shot and artwork blocks metadata -> T20.
- F46 uncached interaction demand takes the manager mutex every frame -> T02.
- F47 `InputManager` allocates and scans under an async mutex every frame -> T02.
- F48 UI health does not react to worker failure -> T05/T21.
- F49 native interactive effects lack consumer/conformance fixtures -> T19.

## Execution waves

Tasks are atomic commit candidates unless a task explicitly names smaller commit
boundaries. File lists identify ownership surfaces, not permission to absorb
unrelated concurrent changes. Before each task, re-run `git status`, inspect the
target files, and rebase the task onto any newly landed local work deliberately.

### Wave 0: freeze the contract

#### T00 - Approve and register this execution spec

Files:

- `docs/specs/73-input-capture-hardening.md`

Depends: none.
Parallel: no; all implementation tasks depend on the reviewed contract.

Implementation:

- Review this spec with Claude as a principal Rust, Windows systems, multimedia,
  and real-time architecture reviewer.
- Resolve every blocker and major finding; re-review until the verdict is PASS.
- Record the approved plan and its task tree in Sibyl.
- Commit the frozen spec before implementation begins.

Verify:

- Every F01-F49 identifier appears once in the ledger and at least once in a
  task's acceptance scope.
- Claude verdict is PASS against the exact committed spec contents.
- `git diff --check` is clean.

### Wave 1: shared contracts and observability

#### T01 - Add the source lifecycle and health model

Files:

- `crates/hypercolor-core/src/input/traits.rs`
- `crates/hypercolor-core/src/input/mod.rs`
- `crates/hypercolor-core/src/input/audio/mod.rs`
- `crates/hypercolor-core/src/input/browser.rs`
- `crates/hypercolor-core/src/input/evdev.rs`
- `crates/hypercolor-core/src/input/interaction/mod.rs`
- `crates/hypercolor-core/src/input/media.rs`
- `crates/hypercolor-core/src/input/net.rs`
- `crates/hypercolor-core/src/input/screen/mod.rs`
- `crates/hypercolor-core/src/input/screen/wayland.rs`
- `crates/hypercolor-core/src/input/screen/windows.rs`
- `crates/hypercolor-core/src/input/windows.rs`
- `crates/hypercolor-core/tests/input_tests.rs`
- `crates/hypercolor-daemon/src/api/system.rs`
- `crates/hypercolor-daemon/src/mcp/tools/system.rs`
- `crates/hypercolor-daemon/tests/api_tests.rs`
- `crates/hypercolor-daemon/tests/mcp_tests.rs`
- generated OpenAPI outputs under `python/src/hypercolor/_generated/`

Depends: T00.
Parallel: no; this defines contracts consumed by T02, T05, T06, T08, T10, T17,
and T21.

Implementation:

- Add `SourceState`, structured `SourceIssue`, and `SourceStatus` with consent,
  demand, session generation, freshness, resource count, and backend fields.
- Give every source a `Send + Sync` status handle created at construction and
  published independently of the `Send`-only, `&mut self` `InputSource` object.
  Use an `Arc<ArcSwap<SourceStatus>>`-shaped latest-value handle already supported
  by the workspace's `arc-swap` dependency; the immutable manager graph retains
  it after worker launch. Platform stubs report `unavailable`, not generic failure.
- Aggregate status into daemon-local tolerant system diagnostics and MCP types
  without exposing sensitive input contents. Do not add diagnostic telemetry to
  the shared REST-domain vocabulary in `hypercolor-types::api`.
- Define transition invariants and test `starting -> live`, degradation, terminal
  failure, freshness expiry, stop, and restart.
- Regenerate the OpenAPI client after the public status schema changes.

Verify:

- A status read completes without acquiring the `InputManager` mutex, and a
  worker-exit transition publishes even while that mutex is deliberately held.
- `just test-crate hypercolor-core --test input_tests`
- `just test-crate hypercolor-daemon --test api_tests`
- `just test-crate hypercolor-daemon --test mcp_tests`
- `just python-generate-check`

#### T02 - Split the manager control plane from immutable frame data

Files:

- `crates/hypercolor-core/src/input/mod.rs`
- `crates/hypercolor-core/src/input/traits.rs`
- `crates/hypercolor-core/src/input/interaction/mod.rs`
- new focused modules under `crates/hypercolor-core/src/input/`
- `crates/hypercolor-core/tests/input_tests.rs`
- `crates/hypercolor-core/tests/alloc_contract_tests.rs`
- `crates/hypercolor-daemon/src/render_thread/capture_demand.rs`
- `crates/hypercolor-daemon/src/render_thread/pipeline_runtime.rs`
- `crates/hypercolor-daemon/tests/render_thread_tests.rs`

Depends: T01, T02A.
Parallel: no; it is the shared hot-path foundation.

Implementation:

- Make `InputManager` own a monotonic source-graph generation and immutable graph
  snapshot.
- Publish per-source `Arc` latest-value snapshots plus bounded event rings.
- Replace per-frame mutable source scans, summed generations, interaction mutex
  acquisition, and fresh aggregate vectors with generation-keyed route caches and
  reusable scratch storage.
- Ensure a source added while demand is already true is discovered on the next
  graph generation and started exactly once.
- Preserve source-local generation for freshness while never using arithmetic
  aggregation as identity.

Verify:

- Tests replace one source with another at the same local generation and observe
  cache invalidation.
- Tests add a screen source while screen demand is already true and observe start.
- The allocation harness shows steady-state manager sampling performs no heap
  allocation after warmup.
- `just test-crate hypercolor-core --test input_tests`
- `just test-crate hypercolor-daemon --test render_thread_tests`

#### T02A - Add deterministic hot-path allocation instrumentation

Files:

- workspace and affected crate dev-dependency declarations
- `Cargo.lock`
- `crates/hypercolor-core/tests/alloc_contract_tests.rs`
- `crates/hypercolor-windows-input/tests/alloc_contract_tests.rs`

Depends: T00.
Parallel: yes, with T01; complete before T02, T09, T10, T17, or T22.

Implementation:

- Add the current compatible `stats_alloc` release as a test-only counting
  allocator implemented outside Hypercolor, so the repository's
  `unsafe_code = "forbid"` boundary remains intact.
- Run allocation assertions in dedicated serial test binaries so unrelated test
  threads cannot pollute counts.
- Put exactly one `#[test]` in each allocation-contract binary and execute all
  cases sequentially inside it. The gate also forces `--test-threads=1`; no
  implicit cargo-harness scheduling assumption is allowed.
- Provide warmup, scoped count reset, allocation/deallocation deltas, and a guard
  that fails if the allocator is not active.
- Keep the dependency dev-only and run the license/advisory gate before accepting
  it.

Verify:

- A positive-control allocation increments the counter and a preallocated
  negative-control operation does not.
- Repeated runs produce identical counts.
- `just test-crate hypercolor-core --test alloc_contract_tests -- --test-threads=1`
- `just test-crate hypercolor-windows-input --test alloc_contract_tests -- --test-threads=1`
- `just deny`

#### T03 - Carry typed effect capability and source availability end-to-end

Files:

- `crates/hypercolor-types/src/api/effects.rs`
- `crates/hypercolor-daemon/src/api/effects.rs`
- `crates/hypercolor-daemon/tests/api_tests.rs`
- `crates/hypercolor-ui/src/api/effects.rs`
- `crates/hypercolor-ui/src/components/canvas_preview.rs`
- `crates/hypercolor-ui/tests/input_inject_tests.rs`
- `sdk/packages/core/src/input/data.ts`
- `sdk/packages/core/src/input/types.ts`
- `sdk/packages/core/package.json`
- `CHANGELOG.md`
- generated OpenAPI outputs under `python/src/hypercolor/_generated/`
- SDK input tests

Depends: T01.
Parallel: no with T01; both regenerate the same OpenAPI client.

Implementation:

- Add authoritative `input_reactive` and a future-compatible typed capability
  set to shared `EffectSummary`.
- Remove category/tag reconstruction from UI demand and preview decisions.
- Replace SDK activity-derived availability with declared, routed, healthy, fresh,
  and degraded fields. Keep public `available` as a deprecated alias with the new
  healthy-and-routed semantics for one minor version; document the behavior change
  and migration in the changelog.
- Use serde defaults so older daemon payloads remain readable during development.
- Regenerate the OpenAPI client after `EffectSummary` changes.

Verify:

- An effect with `input_reactive=true` and no interactive category/tag is detected
  by daemon, REST client, UI preview, and SDK fixture.
- An idle but healthy input source remains available.
- A recently active but failed source is not healthy or fresh.
- `just test-crate hypercolor-daemon --test api_tests`
- `just python-generate-check`
- `just ui-test` on Linux or in the CI `UI` job
- `cd sdk && bun test`
- The CI `SDK` job runs `bun test`; T24 owns that durable gate.

#### T04 - Preserve canonical timed events through every transport

Files:

- `crates/hypercolor-types/src/event.rs` and
  `crates/hypercolor-core/src/input/traits.rs`
- `crates/hypercolor-core/src/bus/mod.rs`
- `crates/hypercolor-daemon/src/render_thread/pipeline_runtime.rs`
- `crates/hypercolor-daemon/src/api/ws/relays.rs`
- `crates/hypercolor-daemon/src/api/ws/protocol.rs`
- `crates/hypercolor-daemon/src/api/ws/tests.rs`
- `crates/hypercolor-daemon/tests/ws_protocol_tests.rs`
- `protocol/websocket-v1.json`
- `crates/hypercolor-leptos-ext/src/ws/`
- relevant TUI/UI WebSocket decoders and tests
- generated WebSocket constants at `python/src/hypercolor/ws_protocol.py`

Depends: T01, T02.
Parallel: yes, after T02; one owner controls the shared WebSocket schema.

Implementation:

- Make `TimedInputEvent` the bus and authorized WebSocket payload.
- Preserve capture time, sequence, source identity, physical code, logical key,
  state, and repeat count.
- Version the binary protocol through `hypercolor-leptos-ext::ws`; never hand-roll
  parallel frame layouts.
- Keep host event contents behind control-tier authorization.
- Update `protocol/websocket-v1.json` first when channel constants or binary tags
  change, then regenerate `python/src/hypercolor/ws_protocol.py`. A payload-only
  change may legitimately produce no generated diff; the drift check still proves
  consistency.

Verify:

- Round-trip tests cover repeat events, equal timestamps, sequence gaps, and
  tolerant decode of the prior payload where compatibility is required.
- Read-tier sockets cannot subscribe to host input content.
- `just test-crate hypercolor-daemon --lib`
- `just test-crate hypercolor-daemon --test ws_protocol_tests`
- `just test-crate hypercolor-leptos-ext`
- relevant `hypercolor-tui` and UI decoder tests.
- `just python-ws-protocol-check`

#### T05 - Publish status changes as events and reconnect-safe hints

Files:

- `crates/hypercolor-core/src/bus/mod.rs`
- `crates/hypercolor-daemon/src/api/ws/relays.rs`
- `crates/hypercolor-daemon/src/api/ws/protocol.rs`
- `crates/hypercolor-daemon/src/api/ws/tests.rs`
- `protocol/websocket-v1.json`
- `crates/hypercolor-ui/src/ws/messages.rs`
- `crates/hypercolor-ui/src/ws/mod.rs`
- `crates/hypercolor-ui/src/api/system.rs`
- tests beside each surface
- generated WebSocket constants at `python/src/hypercolor/ws_protocol.py`

Depends: T01, T04.
Parallel: yes, with platform correctness tasks after T04.

Implementation:

- Publish source-status transitions as discrete bus events without embedding
  captured input data.
- Add a UI hint signal and fold connection generation into status resource epochs
  so gaps trigger a refetch rather than polling.
- Coalesce duplicate health transitions while preserving session-generation
  changes.
- Update the WebSocket manifest first and regenerate the Python constants when the
  event vocabulary, channel, or binary tag changes.

Verify:

- Worker failure invalidates the UI status resource immediately.
- A reconnect refetches status even when the failure event was missed.
- No timer-driven browser polling is introduced.
- `just test-crate hypercolor-daemon --lib`
- `just python-ws-protocol-check`

### Wave 2: lifecycle and host-input correctness

#### T06 - Make source start, stop, and replacement transactional

Files:

- `crates/hypercolor-core/src/input/mod.rs`
- `crates/hypercolor-core/src/input/evdev.rs`
- `crates/hypercolor-core/src/input/interaction/mod.rs`
- `crates/hypercolor-core/src/input/windows.rs`
- `crates/hypercolor-core/src/input/net.rs`
- `crates/hypercolor-core/tests/input_tests.rs`
- `crates/hypercolor-core/tests/lifecycle_tests.rs`

Depends: T01, T02.
Parallel: no with T07, T08, or T10; worker rollback semantics freeze before
platform ingestion and audio lifecycle build on them.

Implementation:

- Require each source to roll itself back when readiness fails and retain its join
  handle until termination is observed.
- Make manager graph startup all-or-clean: stop every source that entered
  `starting` or `live` when any required startup fails.
- Detect unexpected worker exit and transition status before stale data can be
  sampled.
- Make stop idempotent, bounded, and interrupt every retry wait.

Verify:

- Fault-injection tests cover readiness timeout, late readiness, panic/exit after
  readiness, partial graph startup, repeated stop, and replacement.
- No test can observe a worker publishing after its source was removed.
- Every production `InputSource` implementation (audio, evdev, interaction,
  media, net, generic screen, Wayland screen, Windows screen, and Windows host)
  publishes a T01 status handle and cannot report live after its worker exits.
- Browser child health is registry-owned and exposed only through exact child
  route diagnostics; browser children do not enter the sampled manager graph.

#### T07 - Canonicalize host events and bound browser/evdev ingestion

Files:

- `crates/hypercolor-windows-input/src/decode.rs`
- `crates/hypercolor-windows-input/tests/decode_tests.rs`
- `crates/hypercolor-core/src/input/evdev.rs`
- `crates/hypercolor-core/src/input/browser.rs`
- `crates/hypercolor-daemon/src/api/ws/protocol.rs`
- `crates/hypercolor-daemon/src/api/ws/session.rs`
- relevant core tests

Depends: T04, T05, T06.
Parallel: yes; file-disjoint from T08-T10 except Windows test coordination.

Implementation:

- Add a stateful Windows canonicalizer for Pause, PrintScreen, fake Shift around
  numpad navigation, E0/E1 prefixes, repeats, and releases.
- Aggregate evdev relative motion until `SYN_REPORT` and compute Euclidean
  magnitude with `hypot`.
- Validate browser batch count, string length, finite coordinates, source id, and
  total encoded size at the protocol boundary.
- Replace vector-front draining with bounded rings or `VecDeque`; overflow drops
  oldest and increments a visible counter.

Verify:

- Golden Windows sequences produce one canonical logical action each.
- Diagonal evdev motion has the expected Euclidean magnitude.
- Oversized, non-finite, and adversarial browser payloads are rejected before
  allocation growth; bounded overflow preserves newest ordered events.

#### T08 - Harden Windows Raw Input metrics, errors, and session ownership

Files:

- `crates/hypercolor-windows-input/src/metrics.rs`
- `crates/hypercolor-windows-input/src/pump.rs`
- `crates/hypercolor-windows-input/src/claim.rs`
- `crates/hypercolor-windows-input/src/session.rs`
- `crates/hypercolor-core/src/input/windows.rs`
- Windows input tests and live probe example

Depends: T01, T06.
Parallel: yes; serialize only tests that intentionally contend for process-global
Raw Input registration.

Implementation:

- Enumerate physical monitor rectangles with DPI-coherent APIs and normalize
  physical cursor coordinates against the selected virtual desktop.
- Classify `GetRawInputBuffer`'s `UINT(-1)` using `GetLastError`; never treat an
  arbitrary failure as a resize request.
- Classify `WAIT_FAILED`, surface terminal errors, and use interruptible waits with
  bounded backoff only for proven transient conditions.
- Make claim acquisition transactional through registration, verification, and
  publication. A cancelled or failed claimant cannot register late or deregister
  a successor.
- Expose worker exit through T01 status.

Verify:

- Fault injection covers access errors, invalid handles, resize races,
  `WAIT_FAILED`, cancellation at each claim stage, and successor ownership.
- Concurrent session tests prove one owner and deterministic loser diagnostics.
- Windows live probe reports physical cursor extents matching monitor topology.
- `just test-crate hypercolor-windows-input`

#### T09 - Give Windows devices stable, kind-safe identities

Files:

- `crates/hypercolor-windows-input/src/devices.rs`
- `crates/hypercolor-windows-input/src/pump.rs`
- `crates/hypercolor-windows-input/src/shared.rs`
- `crates/hypercolor-windows-input/tests/pending_tests.rs`
- `crates/hypercolor-windows-input/tests/session_tests.rs`
- `crates/hypercolor-windows-input/tests/alloc_contract_tests.rs`

Depends: T02A, T07, T08.
Parallel: no with T08; it follows the session ownership commit.

Implementation:

- Partition null-device identities by kind and session; synthesize arrival before
  first data for both keyboard and mouse.
- Keep pending absolute baselines across delayed metadata enrichment.
- Tag native handles with monotonically increasing device generations so reuse
  cannot inherit the departed device's identity or state.
- Intern immutable device descriptors so raw records borrow labels and paths
  rather than cloning strings.
- Synthesize releases per device generation on departure or session loss.

Verify:

- Tests cover first-data-before-arrival, delayed arrival, null keyboard plus null
  mouse, handle reuse, removal with held state, and metadata refresh.
- The allocation-contract test observes zero device-label allocations in
  steady-state record decode after warmup.

#### T10 - Make audio reconfiguration and analysis real-time safe

Files:

- `crates/hypercolor-core/src/input/audio/mod.rs`
- `crates/hypercolor-core/src/input/audio/linux.rs`
- `crates/hypercolor-core/src/input/audio/fft.rs`
- `crates/hypercolor-core/tests/alloc_contract_tests.rs`
- `crates/hypercolor-daemon/src/api/config.rs`
- audio tests in core and daemon

Depends: T01, T02A, T06.
Parallel: no with T12; `api/config.rs` ownership passes from T10 to T12.

Implementation:

- Stage a replacement stream and analysis worker fully before swapping active
  config, name, health, or running state. Failure leaves the old stream intact and
  a retry remains actionable.
- Reduce platform callbacks to format conversion into a preallocated lock-free
  single-producer/single-consumer ring. Perform mixing, FFT, features, and snapshot
  publication on an analysis worker.
- Extract the callback body into a synchronous
  `push_frames(&[f32], &Ring) -> PushStats` seam and add its case to
  `alloc_contract_tests.rs`; measure only while platform and analysis threads are
  stopped.
- Detect terminal CPAL/Pulse failures, expire stale spectrum data, and reconnect
  with a state machine that distinguishes device loss from permission/backend
  absence.
- Use bounded buffers and report dropped samples without blocking callbacks.

Verify:

- A->B failure preserves live A; a later B retry succeeds.
- The extracted synchronous callback seam records zero allocations and locks after
  warmup. Live-stream tests assert only ring/drop/health counters, never global
  allocation counts while platform and analysis threads are active.
- Device loss clears freshness, updates health, and reconnects without daemon
  restart.
- `just test-crate hypercolor-core --test audio_pipeline_tests`
- `just test-crate hypercolor-daemon --lib`

#### T11 - Add explicit input routes and deterministic arbitration

Files:

- `crates/hypercolor-types/src/config.rs` and config tests
- `crates/hypercolor-core/src/config/mod.rs`
- new `crates/hypercolor-core/src/input/routing.rs`
- `crates/hypercolor-core/src/input/{mod,graph,traits,browser}.rs`
- `crates/hypercolor-daemon/src/render_thread/pipeline_runtime.rs`
- `crates/hypercolor-daemon/src/render_thread/capture_demand.rs`
- new interactive-preview executor/render-lane module plus render-group input
  ownership
- new input-publication pump owned outside the authoritative frame executor
- `crates/hypercolor-daemon/src/render_thread/{frame_executor,frame_policy,frame_io,scene_dependency}.rs`
- `crates/hypercolor-daemon/src/render_thread/render_groups/model.rs`
- `crates/hypercolor-daemon/src/render_thread/gpu_device.rs`
- `crates/hypercolor-daemon/src/preview_runtime.rs`
- `crates/hypercolor-daemon/src/api/ws/{session,protocol,relays,cache,tests}.rs`
- `crates/hypercolor-daemon/src/api/mod.rs`
- `crates/hypercolor-daemon/src/api/config.rs`
- `crates/hypercolor-daemon/src/startup/{mod,services}.rs`
- daemon status, diagnose, and MCP status surfaces
- `crates/hypercolor-leptos-ext/src/ws/preview.rs` and WebSocket schema tests
- `crates/hypercolor-ui/src/components/canvas_preview.rs`
- `crates/hypercolor-ui/src/ws/{connection,input,messages,preview}.rs`
- generated OpenAPI outputs under `python/src/hypercolor/_generated/`
- generated WebSocket constants at `python/src/hypercolor/ws_protocol.py`
- stock interactive effect conformance fixtures
- user-facing input migration documentation
- `CHANGELOG.md`

Depends: T02, T03, T04, T07, T10.
Parallel: no with T10 or T12; `api/config.rs` ownership passes T10 -> T11 -> T12.

Implementation:

- Add `InteractionRoutePolicy::{Host, Browser, Merge}`, serialized as `host`,
  `browser`, and `merge`. Persist separate daemon-effect and interactive-preview
  policies, defaulting to `host` and `browser` respectively for new configs.
  Bump the config schema and migrate an older config with no route fields to
  daemon `merge` plus preview `browser` for one minor version; a freshly created
  config uses the new defaults. All three variants remain valid for both consumer
  classes.
- Keep one immutable, lock-free browser connection registry outside the sampled
  manager graph. An interactive WebSocket attach creates a unique browser child
  slot addressed by structured `(server_connection_incarnation,
  client_preview_id)` identity. Its opaque source incarnation is distinct from
  manager-local slot ids and from its diagnostic string. Attach/detach never
  mutates the manager's source graph or puts `InputManager` on the WebSocket
  injection path.
- Publish only addressable child slots. Each child carries coherent held state,
  motion, an independent bounded event history, and the registry's always-live
  health handle. There is no browser union, aggregate fallback, or sampled
  browser owner.
- Resolve source sets exactly: `host` selects every eligible non-browser
  interaction slot; preview `browser` selects only that preview's child;
  preview `merge` selects host plus that child. Daemon `browser` selects only an
  explicitly claimed authoritative browser child; daemon `merge` selects host plus
  that claimed child. No route implicitly selects every browser connection.
- Model the authoritative browser child as a control-authorized single-owner lease
  on `(server_connection_incarnation, client_preview_id)`. Claim and release are
  explicit and idempotent; a conflicting claim returns an error rather than
  stealing ownership. Disconnect or close synthesizes releases and leaves daemon
  `browser` empty or daemon `merge` host-only until a new owner claims it. The
  migrated UI claims from its main interactive preview so legacy browser-driven
  hardware output has a deterministic successor instead of silently disappearing.
- Add a real render-consumer boundary. The authoritative scene keeps its existing
  render lane and resolves the daemon-effect route. Every active interactive
  preview owns isolated interaction-consuming effect instances, route state,
  event cursors, retained frames, compositor, composition planner, output
  artifacts, effect delta clock, and any mutable transition or deferred-work state
  used by its path. State within one lane may share that lane's immutable GPU device
  handle, but preview lanes never share the authoritative device/queue,
  `SparkleFlinger`, stateful interaction renderer, clock, retained-frame cache, or
  route cursor. Filtering the current global `FrameInputs.interaction` after merge
  is not sufficient.
- Split interaction-invariant producers from per-consumer rendering. Media decode,
  static assets, screen capture, and native/HTML layers whose typed metadata does
  not require interaction run once at their own cadence and publish immutable
  surfaces for every lane to latch. Only interaction-consuming effect state is
  instantiated per consumer. Preview composition pools are demand-sized and reuse
  full-resolution surfaces instead of copying the authoritative lane's eager
  8-to-64-slot pool allocation into every preview.
- Run interactive lanes on a preview executor outside the authoritative render
  thread and hardware frame deadline. Each lane has its own monotonic clock and
  sequential state ownership, while independent lanes execute concurrently.
- The preview executor owns shared worker and device pools; opening a preview does
  not create one OS thread or one logical GPU device per lane. Account admitted
  surface, renderer, encoder, and transport bytes explicitly against configurable
  pool capacity. Capacity errors reject only the new open with exact diagnostics;
  they never lower requested FPS or resolution, serialize healthy lanes, or evict
  existing work. Closing a lane joins or returns every task and device allocation
  before its publication lifetime is retired.
- Give preview rendering an independent `wgpu::Device`/`Queue` from the same
  adapter when GPU composition is available; otherwise use the full-resolution CPU
  compositor on the preview executor. A preview may never submit to or call
  device-wide `poll(Wait)` on the authoritative device. Interaction-invariant
  producers publish device-neutral immutable surfaces; a GPU-only producer may
  cache one immutable device-specific view per device, never one decode/render per
  lane. Capacity must scale to the supported concurrent preview load; do not lower
  preview FPS, canvas resolution, or authoritative cadence to hide contention.
- Move `InputManager::sample_sources` into a dedicated input-publication pump.
  The pump runs whenever any authoritative, preview, passive-stream, or diagnostic
  consumer resolves a live source, at the maximum requested/source cadence across
  those consumers. It publishes slots independently of authoritative frame skip,
  reuse, output sleep, or idle decisions. Render and preview lanes only read the
  immutable graph; neither owns sampling nor drains backend queues.
- Key interactive preview consumers by
  `(server_connection_incarnation, client_preview_id)`, not
  canvas subscription or canvas dimensions. Multiple previews on one connection
  and multiple connections viewing the same canvas remain independent. Share only
  immutable registry, scene, asset, and non-interaction input snapshots; never
  share a stateful renderer or route cursor between consumers.
- Extend the shared WebSocket codec with additive interactive-preview open, close,
  input, authoritative-claim, acknowledgment, and addressed-frame contracts.
  Preserve the existing passive `canvas` channel and frame layout for older
  clients; the new binary tag/header carries the preview id. `input_inject` names a
  preview id and is rejected unless that preview is active on the same connection.
  Client ids are opaque within one connection and cannot address another session.
  Disconnect, explicit close, authorization loss, or future cancellation drops a
  guard that releases the route, lease, and render lane exactly once.
- Interactive preview transport is latest-value by preview publication, not a
  count-bounded queue of fully encoded images. Replace a stale unsent frame before
  encoding where possible and before enqueue otherwise; retain at most one current
  encoded publication per preview plus the frame actively writing. Enforce both a
  per-publication and per-connection encoded-byte budget, with visible replacement,
  rejection, and send-latency counters.
- Frames larger than the one-message WebSocket ceiling use an additive chunk
  envelope carrying preview id, publication identity, frame number, total encoded
  bytes, chunk offset/index/count, and format metadata. Reassembly is bounded by
  the advertised frame limit and discards incomplete or superseded generations.
  Raw 4096-square RGB/RGBA and worst-case valid JPEG requests remain representable;
  the implementation may not hide the mismatch by lowering dimensions, FPS, or
  supported formats.
- Clients activate a publication only after its addressed open acknowledgment.
  Ordered open/close/error acknowledgments fence rapid close/reopen sequences, and
  reconnect clears pending, opened, reassembly, and rendered state. A binary frame
  from an unconfirmed or superseded publication is dropped before presentation.
- Default browser preview to its connection-scoped source and daemon effects to
  host input; never merge them implicitly.
- Preserve a configurable legacy `merge` route for one minor version and document
  the migration. Stock interactive effects must remain behaviorally equivalent
  under the new preview default before that compatibility route can be removed.
- If merge is requested, deduplicate only when physical identity and sequence
  provenance prove duplication. Do not heuristic-dedupe repeated keys.
- Store one event cursor per `(consumer_id, source_incarnation)`. A new consumer,
  newly selected source, or replacement slot starts at that slot's current tail; graph
  rebuilds preserve cursors for unchanged slot ids. Each source retains a fixed
  bounded history independently, reports overwritten events, and never waits for
  all consumers to advance before reclaiming entries.
- When a read reports overwritten events, synthesize releases for previously
  observed controls from that source, clear its observed provenance, quarantine
  controls present in its current held snapshot until their first release, advance
  to the current tail, and add the loss to that consumer's drop total. Overflow can
  never leave an effect logically stuck or invent a press.
- Track observed press provenance per `(consumer_id, source_incarnation, control)`.
  Suppress a release whose press that consumer never observed. Before removing a
  source from a route, synthesize releases only for controls whose last routed,
  observed provenance is disappearing; a control still held by another selected
  source remains held. Newly selected sources quarantine preexisting key/button
  holds: they do not expose them as down or replay presses, and suppress the first
  matching release. Absolute pointer position may initialize immediately. A fresh
  press after release establishes provenance. Route snapshots, event selection,
  held provenance, and availability all use the same resolved source set.
- Detach first marks the child nonaccepting and removes it from the registry, then
  bumps affected routes. Consumers synthesize from their own provenance; teardown
  never relies on a final release event being drained before the child retires.
- Split route-only live config from keyboard/mouse capture consent changes. Route
  updates never restart host hardware.
- Add a prepared `ConfigManager` transaction under its existing single writer lock
  and a dedicated input-control writer shared by every `InputManager` graph
  mutation.
  For a request that changes consent and routes together, capture the expected
  config pointer, host-source incarnation, and routing snapshot; validate the
  candidate; then stage the replacement host runtime and serialized temporary
  config file off-lock. Lock order is fixed: acquire the async input-control writer,
  then the async `InputManager` guard, then the synchronous `ConfigManager` writer
  lock. Recheck those identities and compare the live config pointer with the
  expected pointer; abort on mismatch. The synchronous commit section contains no
  `.await`: atomically replace the config file as the last fallible step, move the
  prepared host source into the graph, enqueue route-transition releases, install
  the live config with a compare-and-swap, and publish the routing snapshot as the
  visibility fence using only infallible moves after persistence. Every path that
  needs more than one of these locks uses the same order.
- Backend callbacks and restore-token sinks never mutate config while an
  `InputManager` guard is held; they return deferred persistence work for the
  caller to execute after releasing the guard. Source start, stop, restart, and
  retirement also happen outside the guard, so its ordered commit section contains
  only the staged file rename and in-memory graph moves. Every config mutation uses
  the one `ConfigManager` writer lock. `replace_source` returns retirement
  ownership rather than calling `stop()` inside the manager mutation. A route-only
  request skips hardware preparation and never restarts host capture.
- Give every consumer an independent monotonic route generation. Include that
  generation, selected source incarnations and interaction generations, plus
  selected availability revisions in its reuse key so same-boolean policy,
  health, or source-identity changes invalidate only affected work. Use the global
  graph generation only as a preparation fence and diagnostic field; unrelated
  audio or screen graph changes do not invalidate interaction consumers. Route
  changes do not manufacture capture demand. The sole demand model is the union,
  across authoritative effects, interactive preview lanes, passive preview-stream
  leases, and diagnostic leases, of typed audio, screen, and interaction
  requirements. Preview demand is independent of output sleep. Screen-canvas and
  screen-zone subscriptions become typed screen-consumer leases rather than a
  separate subscriber-count rule; host-routed preview interaction and
  audio-reactive preview scenes likewise keep their pipelines publishing while
  hardware output is asleep.
- Publish one generation-tagged immutable diagnostics snapshot containing each
  consumer's policy, selected and suppressed stable source descriptors,
  cursor-drop totals, availability handles/revisions, route generation, config
  generation, source graph generation, and browser registry generation. Status,
  diagnose, and MCP load it once, evaluate retained status handles at one `Instant`,
  and never acquire `InputManager`; they cannot combine fields from different
  route generations.
- Make the routing publisher the sole author of that snapshot. Each lane owns a
  lock-free metrics handle tagged with its current route generation; cursor
  overflow and invalid-event drops update separate counters, and the publisher
  folds only matching-generation values into a replacement snapshot.
- Resize or retarget an existing preview lane in place. Preserve its identity,
  route generation, cursors, held provenance, and effect state while rebuilding
  only dimension-dependent composition resources.

Verify:

- The same physical action injected through host and preview is delivered once
  under each default route and twice only under explicit merge.
- Route changes synthesize releases from the old route and invalidate demand.
- Browser connections remain independently addressable through publication,
  routing, reconnect, and disconnect; one connection cannot leak held state or
  events into another preview.
- Daemon effects continue to receive host input while one or more previews receive
  their own browser sources in the same frame. The non-vacuous fixture injects a
  host action and distinct browser actions for two simultaneous previews, asserts
  three isolated snapshots and outputs, then closes one preview while the other two
  continue unchanged.
- Cursor tests cover tail-start attach, unchanged graph rebuild, slot replacement,
  bounded-ring overflow, reconnect, and two consumers advancing at different rates.
- Provenance tests cover route removal, a key held by two merged sources, release
  without an observed press, detach while held, and add-source-while-held.
- Mixed config tests prove prepare failure changes neither consent nor routes, a
  successful combined update exposes one coherent post-commit snapshot, route-only
  updates never restart hardware, and old-source retirement runs outside the lock.
- A concurrent source-picker/restore-token update and combined consent/route update
  complete without deadlock or lost writes while the publication pump continues to
  advance. Lock instrumentation proves no backend callback, worker wait, or `.await`
  occurs inside the synchronous ordered commit section.
- Migration tests distinguish old-schema missing fields from new-config defaults,
  and authoritative-lease tests cover claim, conflict, idempotence, disconnect,
  browser-without-owner, merge-without-owner, and clean handoff.
- Under the accepted concurrent-preview load, authoritative frame interval, output
  latency, and drop counters remain inside the existing zero-preview performance
  thresholds while every preview meets its requested cadence and resolution.
  Instrumentation proves previews never submit or wait on the authoritative wgpu
  device. Producer counters prove one media, screen, static, or non-interaction
  render is fanned out rather than duplicated per preview.
- Slow-client tests prove encoded memory remains inside the byte budget at 640x480
  and 4096x4096, stale frames are replaced rather than queued, chunks reassemble
  exactly once, superseded partial frames are reclaimed, and one preview cannot
  starve another. Shutdown tests prove all preview workers are joined and Servo
  teardown completes before executor ownership is released.
- Sampling-pump tests prove host events and held snapshots stay live during
  authoritative reuse, idle, output sleep, and preview-only operation, without a
  delayed event burst when hardware rendering resumes. Demand tests cover audio,
  screen, and interaction preview consumers independently.
- Status and MCP route diagnostics remain responsive while `InputManager` is held.
- Conformance fixtures run every stock interactive effect through legacy merge and
  the new default preview route and compare canonical inputs and visible output.
- Config tests cover omitted fields, every route spelling, invalid values, and
  round trips. Generated clients and migration docs match the final schema.

### Wave 3: capture correctness and semantics

#### T12 - Correct capture demand, live configuration, and cadence

Files:

- `crates/hypercolor-daemon/src/render_thread/scene_snapshot.rs`
- `crates/hypercolor-daemon/src/render_thread/capture_demand.rs`
- `crates/hypercolor-daemon/src/render_thread/pipeline_runtime.rs`
- `crates/hypercolor-daemon/src/api/config.rs`
- `crates/hypercolor-daemon/src/startup/services.rs`
- `crates/hypercolor-types/src/config.rs`
- daemon and config tests
- generated OpenAPI outputs under `python/src/hypercolor/_generated/`

Depends: T02, T03, T10, T11.
Parallel: yes, with T13 after route/cache contracts settle.

Implementation:

- Stop treating an idle scene as capture demand. Reuse T11's typed consumer union:
  audio, screen, and interaction demand comes only from an authoritative effect,
  interactive preview lane, passive preview-stream lease, or explicit diagnostic
  lease that declares that domain.
- For screen demand, implement the arbitrary-resolution descriptor amendment:
  preserve every consumer descriptor independently, resolve it against its source,
  and merge cadence only after complete descriptor equality. The compatibility
  mirror is an ordinary branch; no demand path may construct a component-wise
  maximum analysis extent.
- Remove the Linux-only live-apply gate for capture settings where Windows has a
  real implementation. Reopen/reconfigure backends transactionally.
- A demanded replacement reaches `Live` or a usable `Degraded` state before config
  persistence, graph publication, or retirement of the known-good source. A
  bounded observation timeout is an error, not permission to commit `Starting`;
  delayed portal consent remains an asynchronous prepared transaction.
- Reserve a non-serialized capture-persistence epoch under the `ConfigManager`
  writer lock for each prepared source lifetime. Restore-token/source callbacks
  may persist only while their exact epoch, config pointer, and graph/source
  identity remain current; rollback restores the prior authority before any staged
  source becomes externally visible.
- Every config writer, including reload, uses the same writer lock. Unchanged
  capture values compare lifecycle and an applied-config fingerprint, repairing
  missing, stopped, failed, or divergent sources and removing disabled extras.
- Replace the silent global `1..=240` FPS clamp with validated platform limits
  based on real backend capability and an explicit error for unsupported values.
- Invalidate cached demand on source-graph and capability generation changes.
- Regenerate the OpenAPI client if live-config/status schemas change.
- Land demand/live-apply as one isolated commit. Its rollback boundary is that
  commit alone; persisted config remains backward compatible so reverting restores
  prior runtime behavior without rewriting user configuration.

Verify:

- Enabled plus idle performs no capture; starting a screen effect begins capture;
  removing the last demand stops it.
- Every advertised live setting changes behavior without daemon restart or stale
  state on failure.
- Validated limits reject unsupported values and accept every rate the backends
  advertise; cadence enforcement is proven in T18.
- Mixed ultrawide, portrait, native, and one-axis-bounded demand produces the
  exact independently resolved branches required by the amendment and never a
  synthetic envelope publication.
- `just python-generate-check`

#### T13 - Introduce the backend-neutral capture frame envelope

Files:

- new focused files under `crates/hypercolor-core/src/input/screen/`
- `crates/hypercolor-core/src/input/screen/mod.rs`
- Windows and Wayland screen source adapters
- screen tests

Depends: T01.
Parallel: yes, with T12; merge before T14-T18.

Implementation:

- Add the `CaptureFrame` envelope defined above with stable source identity,
  topology/session generations, time/sequence, geometry, color, cursor, storage,
  freshness, and damage metadata.
- Preserve CPU and GPU storage without leaking platform API types into core.
- Reject inconsistent extents, crop, strides, and stage transitions at adapters.
- Keep legacy `ScreenData` construction behind one compatibility conversion until
  all consumers migrate.
- Keep the envelope and compatibility conversion entirely under `input/screen/`
  and import it through that module; T13 does not change the shared `InputSource`
  trait or `input/traits.rs`.
- Backends emit native scanout-oriented pixels and set `rotation` to the transform
  still to be applied. `RawCaptureSurface` carries that pending transform; T14
  applies it exactly once and stamps `ProcessedCaptureSurface` with identity
  rotation plus the rotation-applied logical extent.

Verify:

- Property tests cover rotations, negative origins, crop bounds, CPU stride, GPU
  handle lifetime, stale sessions, and invalid metadata.
- Non-Windows builds compile without Windows types or feature leakage.

#### T14 - Unify raw/processed surface and grid semantics

Files:

- `crates/hypercolor-core/src/input/screen/mod.rs`
- `crates/hypercolor-core/src/input/traits.rs`
- `crates/hypercolor-core/src/input/screen/sector.rs`
- `crates/hypercolor-core/src/input/screen/smooth.rs`
- `crates/hypercolor-core/src/input/screen/tune.rs`
- `crates/hypercolor-daemon/src/render_thread/screen_canvas.rs`
- `crates/hypercolor-daemon/src/render_thread/pipeline_runtime.rs`
- core and daemon screen tests

Depends: T11, T12, T13.
Parallel: no with T19; this defines its consumer contract.

Implementation:

- Create explicit `RawCaptureSurface` and `ProcessedCaptureSurface` stages.
- Apply rotation/crop, letterbox policy, tuning, and time-based smoothing once in
  the documented canonical order before any consumer sees processed pixels.
- Implement `Surface` and `Zones` as independent descriptor-keyed branches with
  complete immutable processing profiles. Each branch resolves as if it were the
  only consumer; sharing follows only from equality of complete resolved keys.
- Preserve the actual surface at its native aspect and contain/cover it into the
  compositor target. Use sectors only when no surface storage exists.
- Treat one-axis and two-axis `Bounded` extents as analysis bounds. Unless
  upscaling is explicit, derived analysis cannot exceed processed source geometry;
  exact compositor and encoder rasters remain separate output descriptors.
- Publish effective cropped grid dimensions separately from requested analysis
  dimensions; direct grid indexing replaces serialized zone-id parsing.
- Normalize scene-cut distance by sample count and express EMA as a time constant
  using capture timestamp deltas.
- Land the geometry conversion separately from consumer migration. Before T19,
  rollback is a single T14 revert through the compatibility adapter; after T19,
  rollback order is T19 then T14.

Verify:

- 16:9, 16:10, 21:9, portrait, and rotated sources retain correct geometry on a
  4:3 canvas without 8x6 fallback.
- A rotated Windows source and transformed Wayland source produce identical
  processed geometry with rotation applied exactly once.
- Raw and processed consumers cannot be accidentally interchanged at compile time.
- Attaching or resizing a surface consumer cannot change zone analysis geometry,
  cadence, smoother state, or publication identity, and the inverse also holds.
- Smoothing response is equivalent across grid sizes and 30/60/120 FPS within
  tolerance; scene cuts reset identically.
- Cropping publishes no synthetic black sectors outside effective dimensions.

#### T15 - Make Desktop Duplication topology and pixels correct

Files:

- `crates/hypercolor-windows-capture/src/duplication.rs`
- `crates/hypercolor-windows-capture/src/shared.rs`
- `crates/hypercolor-windows-capture/tests/duplication_tests.rs`
- `crates/hypercolor-core/src/input/screen/windows.rs`
- Windows capture examples/tests

Depends: T13.
Parallel: yes, with T17; no overlap with T16 until the envelope contract freezes.

Implementation:

- Enumerate adapters/outputs into stable source descriptors including device
  name, desktop rectangle, primary flag, rotation, and topology generation.
- Resolve `auto` to the primary output; resolve configured sources by stable id,
  not transient enumeration index. Reopen on source or topology generation change.
- Report the pending display rotation in the frame envelope without pre-rotating
  pixels. Compose pointer shape and position according to Desktop Duplication
  metadata in the same native scanout coordinate space.
- Retain the last desktop image and republish when `LastMouseUpdateTime`, pointer
  position, visibility, or shape changes even if `LastPresentTime == 0`; T18
  cadence limits pointer-only publication.
- Map `E_ACCESSDENIED`, session/desktop switches, device removal/reset, timeout,
  and duplication concurrency exhaustion to distinct structured issues.
- Replace fixed two-second sleeps with stop-aware retry deadlines.

Verify:

- Synthetic topology tests cover primary-not-first, hotplug reorder, negative
  origins, stable id selection, all rotations, and source disappearance.
- Cursor tests cover color, monochrome, masked-color, hidden, and already-composed
  cases, plus cursor-only motion over a completely static desktop.
- Live acceptance covers monitor switch, lock/unlock, UAC/secure desktop recovery,
  display rotation, cursor motion, and daemon stop during retry.

#### T16 - Move Windows reduction onto the GPU with pipelined readback

Files:

- `crates/hypercolor-windows-capture/src/duplication.rs`
- an embedded HLSL compute shader and reduction modules in
  `crates/hypercolor-windows-capture/src/`
- `crates/hypercolor-windows-capture/Cargo.toml`
- `Cargo.lock`
- `justfile`
- `crates/hypercolor-windows-capture/benches/capture_reduction.rs`
- focused tests in `crates/hypercolor-windows-capture/tests/`

Depends: T13, T15.
Parallel: no with T15; it follows correctness.

Implementation:

- Copy the non-SRV duplication texture into a reusable app-owned clean desktop
  texture created with `D3D11_BIND_SHADER_RESOURCE`. Never write cursor pixels
  into it; it remains the source of truth for pointer-only updates.
- Compile the embedded HLSL once with the system `D3DCompile` entry point exposed
  by the `Win32_Graphics_Direct3D_Fxc` windows-rs feature. Feed the intermediate
  SRV to a compute shader and write the configured region reduction into a small
  `D3D11_BIND_UNORDERED_ACCESS` texture before CPU readback. Compiler absence or
  shader creation failure, unsupported composite-format UAV support, or view
  creation failure selects the explicit CPU fallback and a degraded issue.
- Use a staging ring with query/fence readiness so acquisition never blocks on the
  texture consumed by analysis.
- Read back only reduced pixels needed by current CPU consumers. GPU compositor
  import is a separate future task, not part of F14.
- Composite the retained pointer shape into a second reusable texture created with
  `D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_UNORDERED_ACCESS`. Each reduction copies
  the clean desktop texture into that target, blends the current color,
  monochrome, or masked-color pointer, then reduces from the composite SRV. On
  pointer-only updates with `LastPresentTime == 0`, repeat the clean-copy and
  composition sequence so the old cursor leaves no residue. The CPU fallback
  performs the identical clean-copy-then-composite sequence.
- Reuse textures, views, buffers, and query objects across stable extents; rebuild
  transactionally on topology or grid changes.
- Key reusable reduction resources by the complete physical reduction descriptor,
  not a mutable last-used extent. One native acquisition and clean-desktop update
  fan out to all due physical keys; CPU fallback maps the native frame once.
- Prepare replacement GPU resources and their old-plus-staged byte ledger on the
  Windows capture worker. Failed admission keeps every active key and last-good
  publication intact.
- Keep a tested CPU fallback for unsupported hardware, with health and telemetry
  identifying the active path. The fallback retains configured cadence and
  quality rather than silently reducing them.
- Add a Windows-capable `just bench-windows-capture` recipe that invokes this
  crate's Criterion target through `scripts/cargo-cache-build.ps1`; do not rely on
  the Unix-only workspace benchmark recipes.

Verify:

- Pixel parity against CPU reference for odd sizes, crop, rotation, and edge
  regions within declared rounding tolerance, plus every T15 pointer shape and
  cursor-only updates over a static desktop.
- Benchmarks report acquisition, GPU reduction, wait, map, CPU analysis, bytes
  read, and missed-deadline percentiles at 1080p, 1440p, and 4K.
- At 4K/120 Hz acquisition with 60 Hz analysis, readback bandwidth scales with the
  analysis surface rather than source pixels and no unbounded backlog forms.
- Alternating 4K, 8K, portrait, and ultrawide branches rebuild only when a complete
  physical key changes; dispatch count equals unique due physical keys.
- `just deny`

#### T17 - Make PipeWire decoding and worker lifetime correct

Files:

- `crates/hypercolor-core/src/input/screen/wayland.rs`
- `crates/hypercolor-core/tests/alloc_contract_tests.rs`
- Linux screen tests and PipeWire fixtures

Depends: T01, T02A, T10, T13.
Parallel: yes, with T15.

Implementation:

- Honor SPA chunk offset/size, signed stride, video crop, buffer bounds, and
  negotiated format. Report the stream transform in the envelope without applying
  it in the backend.
- Add a narrow `hypercolor-pipewire-interop` crate as the only audited unsafe SPA
  buffer boundary. It uses generated `pipewire::sys`/`spa::sys` ABI types, owns raw
  dequeue behind a non-`Send` exact-once RAII requeue guard, validates all pointers,
  meta sizes, and mapped bounds, and lends bytes to a higher-ranked visitor that
  cannot retain the PipeWire buffer.
- Explicitly negotiate `SPA_PARAM_Meta` for `SPA_META_VideoCrop` and
  `SPA_META_VideoTransform`. Copy metadata immediately in the callback; absent
  crop means full frame and absent transform means identity, while present but
  malformed metadata drops the frame with a typed counter.
- Replace the four-state capture rotation contract with the complete eight-state
  SPA D4 transform vocabulary, including all reflected variants. Crop in raw-plane
  coordinates, then apply the transform exactly once in canonical processing;
  cursor geometry follows the same exhaustive mapping.
- Keep the PipeWire callback exact and minimal: validate metadata, perform one
  bounded memcpy into a preallocated double buffer, requeue the SPA buffer before
  returning, and wake analysis. Never retain an SPA buffer past the callback.
- Extract that body as
  `decode_chunk(&SpaChunkView, &mut DoubleBuffer) -> CopyStats` and add its case
  to `alloc_contract_tests.rs`; measure it synchronously with platform and analysis
  threads stopped.
- Move downscale, letterbox detection, smoothing, tuning, and publication to an
  analysis worker governed by T12 cadence.
- Detect stream error/termination, expire the latest snapshot, publish degraded or
  failed status, and reconnect while demand remains active.
- Make all waits stop-aware and clear session data on teardown.
- Negotiated native extent or transform changes advance topology even when the
  portal stream identity is stable. Source scale compares logical dimensions with
  the post-transform extent, swapping axes for quarter turns.

Verify:

- Fixtures cover truncated chunks, non-zero offsets, negative stride, crop,
  transform, row padding, format changes, malformed metadata, and worker exit.
- ABI fixtures assert metadata constants/layouts on x86_64 and aarch64 Linux,
  exact-once requeue on success/error/panic, and all eight transforms over a
  unique-corner image. Same-portal extent/transform changes rebuild topology.
- The extracted synchronous `decode_chunk` seam asserts zero allocations, zero
  downscale/letterbox/smoothing work, and no lock held across the copy. Live
  PipeWire tests assert counters and drop metrics only. Copy time and bytes scale
  linearly with the validated chunk size and remain within the negotiated buffer
  deadline.
- Killing the synthetic PipeWire stream makes data unavailable before freshness
  expiry and recovers on replacement.

#### T18 - Validate cross-platform capture cadence and freshness

Files:

- `crates/hypercolor-core/src/input/screen/mod.rs`
- new shared capture cadence module under core screen input
- Windows and Wayland adapters
- core/daemon capture tests and benchmarks

Depends: T12, T14, T15, T17.
Parallel: no; it is the cross-platform integration checkpoint.

Implementation:

- Land one monotonic cadence primitive before multi-branch publication becomes
  visible. Windows and Wayland use it for per-physical-key work and per-logical-
  branch analysis/publication deadlines.
- Native acquisition runs only as fast as the maximum due physical-key cadence.
  Expensive reduction runs only when a dependent branch is due, and each logical
  branch independently honors its requested cadence.
- Always analyze the newest eligible source frame and count superseded acquisition
  frames rather than queueing them.
- Expire publication by timestamp/session generation, not by repeated identical
  pixels.
- Surface acquisition FPS, analysis FPS, publish FPS, superseded frames, stale
  frames, and deadline misses separately.

Verify:

- Deterministic clock tests cover jitter, clock jumps, burst acquisition, analysis
  overruns, stop/restart, live FPS changes, mixed cadences, and branches that share
  one physical reduction key.
- Neither backend exceeds any branch cadence beyond one scheduling quantum,
  performs duplicate native acquisition for branch fan-out, or accumulates
  latency.

### Wave 4: consumers, media, and source selection

#### T19 - Make every renderer consume the canonical contracts

Files:

- `crates/hypercolor-core/src/effect/servo/renderer/frame_queue.rs`
- `crates/hypercolor-core/src/effect/lightscript/payload.rs`
- `crates/hypercolor-core/src/effect/builtin/screen_cast.rs`
- `crates/hypercolor-core/src/effect/builtin/mod.rs`
- `crates/hypercolor-core/src/effect/loader.rs`
- `crates/hypercolor-core/src/effect/meta_parser.rs`
- `sdk/src/effects/screen-cast/main.ts`
- SDK frame adapter and conformance fixtures
- renderer and builtin tests

Depends: T03, T04, T11, T14.
Parallel: yes, with T20; it owns renderer files.

Implementation:

- Preserve repeated input events and multiplicity when Servo frames coalesce.
- Feed all renderers `Arc` canonical snapshots and the same processed capture
  surface semantics.
- Prevent an HTML effect from replacing the native ScreenCast under the same
  builtin id. Establish one authoritative registration and explicit variants if
  both implementations are kept.
- Give HTML/LightScript consumers the real processed surface or a documented grid
  projection derived from it, not an accidental 8x6 substitute.
- Add native, Canvas2D, WebGL, browser injection, screen aspect, and repeat-key
  conformance fixtures.

Verify:

- Two identical key presses remain two ordered events after queue coalescing.
- Default Servo and non-Servo builds resolve ScreenCast to the same semantic
  consumer and output equivalent samples.
- Native interactive fixture exercises state plus timed batch.

Status on 2026-08-22: landed with design 72 C4. Renderers receive the
exact publication as `Option<&Arc<ScreenBranchPublication>>`; Servo keeps
the Arc across coalesced frames without copying pixels; LightScript reads
zone publications cell for cell and projects surface publications onto
its grid as a documented box average, so the 8x6 substitute is gone.
Repeat-key multiplicity and the ScreenCast registration identity landed
earlier. The macOS production path still publishes GPU-resident work
only, so CPU renderers on macOS read an absent screen until a GPU
readback or a CPU branch exists there.

#### T20 - Bound media enrichment and make providers resilient

Files:

- `crates/hypercolor-core/src/input/media.rs`
- `crates/hypercolor-core/src/input/keymap.rs`
- media tests
- SDK media adapter/types if health fields are shared there

Depends: T01, T02.
Parallel: yes, with T19.

Implementation:

- Add a provider abstraction that separates player metadata from artwork
  enrichment and supports reconnectable Linux and Windows implementations.
- Publish metadata immediately; fetch/decode artwork asynchronously with bounded
  streaming bytes, time, redirect policy, dimensions, decoded pixels, and output
  data-url size.
- Configure decoder allocation limits before full decode to prevent image bombs.
- Reconnect Linux session-bus/player discovery after loss while preserving honest
  health and freshness.
- Define one shared logical media-key inventory and map platform physical codes
  into it.

Verify:

- Unknown-length and oversized local/HTTP artwork abort before unbounded growth.
- Huge-dimension compressed images are rejected before full pixel allocation.
- Slow/broken artwork never delays title/artist/playback publication.
- Linux bus loss and player replacement recover without daemon restart.
- Linux and Windows media keys produce identical logical names.

#### T21 - Finish consent and reactive health UX

Files:

- `crates/hypercolor-ui/src/components/settings_sections.rs`
- `crates/hypercolor-ui/src/components/input_access_banner.rs`
- `crates/hypercolor-ui/src/input_access.rs`
- `crates/hypercolor-ui/src/api/system.rs`
- UI tests
- user-facing input/capture docs
- `crates/hypercolor-daemon/src/api/ws/tests.rs` when channels change
- `protocol/websocket-v1.json` when channels or binary tags change
- generated OpenAPI outputs under `python/src/hypercolor/_generated/` when
  consumed REST schemas change
- generated WebSocket constants at `python/src/hypercolor/ws_protocol.py` when
  channel constants change

Depends: T03, T05, T11, T12.
Parallel: yes, after API contracts stabilize.

Implementation:

- Add separate host keyboard and mouse consent controls plus route selection.
- Show configured, demanded, live, degraded, unavailable, freshness, backend, and
  remediation without exposing captured content.
- React to source-status WebSocket hints and reconnect generation; never poll.
- Distinguish permission granted but idle from capture active.
- Keep Windows and Linux remediation platform-specific and actionable.
- Regenerate affected Python client artifacts if T21 changes a shared schema;
  otherwise both generated checks must still prove no drift.

Verify:

- UI tests cover independent keyboard/mouse toggles, failed live apply rollback,
  worker death, recovery, route change, reconnect, and stale data.
- `just python-generate-check`
- `just python-ws-protocol-check`
- `just ui-test` on Linux or in the CI `UI` job
- `just ui-build` on Linux or in the CI `UI` job

#### T22 - Remove per-frame deep clones and serialized identifiers

Files:

- `crates/hypercolor-core/src/input/traits.rs`
- `crates/hypercolor-core/src/effect/pool.rs`
- `crates/hypercolor-core/src/effect/servo/renderer/frame_queue.rs`
- `crates/hypercolor-daemon/src/render_thread/pipeline_runtime.rs`
- `crates/hypercolor-daemon/src/render_thread/screen_canvas.rs`
- `crates/hypercolor-core/tests/alloc_contract_tests.rs`
- benchmarks/tests

Depends: T02, T02A, T14, T19, T23.
Parallel: no with T19; it is a focused performance follow-up.

Implementation:

- Store descriptor-keyed branch publications behind `Arc` and clone only typed
  lease/snapshot handles across renderer pools. Reads come from the committed hub
  catalog and never take the mutable manager or worker publication locks.
- Replace nested per-frame `Vec<String>` and repeated zone-id serialization/parsing
  with indexed grid storage and interned immutable source/device descriptors.
- Reuse branch-local analyzers, smoother storage, pools, and route/consumer scratch
  after warmup. Resource identity survives unrelated descriptor churn.
- Keep ownership explicit enough that source replacement cannot mutate a frame
  already being rendered.

Verify:

- Allocation benchmarks cover one and many renderer groups for audio, input,
  screen, and media snapshots.
- Snapshot fan-out allocation is constant with renderer count after warmup.
- Zero steady-state allocation holds for warmed 1080p, 4K, 8K, portrait,
  ultrawide, duplicate-descriptor, and mixed-profile branch plans.
- Functional parity tests prove immutable older frames survive source updates.

#### T23 - Support multiple screen sources without accidental overwrite

Files:

- source routing module from T11
- `crates/hypercolor-core/src/input/screen/mod.rs`
- `crates/hypercolor-daemon/src/render_thread/pipeline_runtime.rs`
- capture config/API types
- tests

Depends: T02, T11, T13, T14, T18.
Parallel: yes, after shared route contracts settle.

Implementation:

- Represent screen samples by stable source id instead of one optional slot.
- Resolve policy selectors to stable source ids before constructing publication
  descriptors. Select an exact descriptor branch per effect group, preview
  subscription, or capture consumer through typed leases.
- Own one descriptor-keyed publication hub whose immutable catalog commits through
  the amendment's prepare/arm/commit protocol. Structural graph and screen-plan
  generations remain independent.
- Preserve independent cadence, health, freshness, session generation, last-good,
  and resource diagnostics per source and branch. Epoch fences prevent old pixels
  from masquerading as current after source, topology, session, or profile change.
- Reject missing configured sources with diagnostics instead of silently using the
  last enumerated sample.

Verify:

- Two synthetic monitors can feed separate effect groups simultaneously.
- One source can feed incompatible exact descriptors simultaneously without
  synthetic union, cross-branch state mutation, or duplicate OS acquisition.
- Reordering samples or enumeration does not change selection.
- Removing one source degrades only its consumers and does not overwrite the other.

### Wave 5: CI, performance contracts, and acceptance

#### T24 - Exercise Windows integration in CI

Files:

- `.github/workflows/ci.yml`
- Cargo feature/build configuration only where required
- Windows-specific daemon integration tests

Depends: T08, T09, T12, T15, T16, T19.
Parallel: yes, once platform contracts compile.

Implementation:

- Add Windows jobs that compile and run daemon input/capture integration against
  deterministic stub backends, plus native crate tests.
- Run daemon integration as
  `cargo nextest run --locked -p hypercolor-daemon --no-default-features
--features builtin-drivers --test-threads=1`, matching the existing Linux daemon
  suite and avoiding Servo/mozangle on a bare Windows runner.
- Seed `effects/screenshots/curated/rainbow/default.webp` before the daemon suite,
  matching the fixture prerequisite in the existing Linux daemon job.
- Add `bun test` to the existing CI `SDK` job so published input-contract behavior
  is a pull-request gate rather than a local-only receipt.
- Compile all target-specific code paths and ensure non-Windows stubs remain API
  compatible.
- Keep the Servo feature matrix on the existing Linux `rust-test-servo` job.
  Windows gates host-input/capture daemon integration in the non-Servo job; the
  platform-independent ScreenCast registration collision is exercised with Servo
  enabled on Linux instead of forcing mozangle onto the bare Windows test job.
- Keep hardware-only tests explicitly ignored with named environment prerequisites;
  do not pretend stubs prove physical acceptance.

Verify:

- CI-equivalent commands pass on Windows.
- Linux workspace and target-specific stub tests pass.
- The Windows non-Servo daemon integration and Linux Servo registration suites
  both pass.
- The CI `SDK` job runs `bun test` in addition to check, typecheck, build, and
  effect build.

#### T25 - Run physical, failure, and concurrency acceptance

Files:

- test fixtures, ignored hardware tests, examples, and acceptance documentation
- no production behavior changes unless a reproduced failure requires a new task

Depends: T06-T24.
Parallel: platform acceptance can run concurrently on separate hosts; final
verdict waits for all mandatory gates.

Implementation:

- Execute spec 72's W4 physical coverage
  (`docs/specs/72-windows-host-input.md`): multiple keyboards/mice,
  hotplug, handle reuse simulation, Pause/PrintScreen/numpad, high polling rate,
  session lock/unlock, stop/restart, browser route isolation, and ownership
  contention.
- Execute Windows capture coverage: primary/non-primary monitors, rotation,
  cursor shapes, hotplug/reorder, secure desktop, sleep/resume, 4K/high-refresh,
  live config, and demand-off idle.
- Execute Wayland coverage across representative PipeWire formats, transforms,
  stride/crop cases, portal cancellation, stream death, and reconnect.
- Execute audio device switch/loss and media bus/artwork failure coverage.

Verify:

- Every physical case records host, display/input topology, command, result, and
  relevant health/performance telemetry.
- Any unexecuted hardware case remains an explicit release gate, not a presumed
  pass.

#### T26 - Lock performance and quality into durable gates

Files:

- benchmark modules in affected crates
- CI benchmark smoke configuration
- operator/developer docs
- this spec's completion ledger

Depends: T16, T18, T22, T25.
Parallel: no; final certification task.

Implementation:

- Record before/after p50/p95/p99 acquisition, analysis, publication, callback,
  render sampling, allocation, memory bandwidth, and deadline telemetry.
- Add regression thresholds that detect architectural cliffs without lowering
  supported FPS, resolution, or device rates.
- Adjudicate T16's retained 4K synthetic baseline explicitly: the readback ring
  stayed bounded (`ring_busy=0`, all 60 reductions drained, 3,686,400 readback
  bytes versus 33,177,600 source bytes), but analysis reported 29/60 deadline
  misses and 17.0536 ms p99 against a 16.67 ms 60 Hz budget. T26 must profile and
  recover headroom or establish a separately justified hardware-class contract;
  bounded backlog alone is not a fully green performance verdict.
- Run an independent Claude correctness, concurrency, security/privacy, and
  performance review over the exact final commit range.
- Run a separate read-only verification agent against the original ask and all
  F01-F49 acceptance claims.
- Fix every blocker/major and re-run the affected gates before final certification.

Verify:

- `just verify`
- `just python-generate-check`
- `just python-ws-protocol-check`
- `just ui-test` on Linux or in the CI `UI` job
- `just ui-build` on Linux or in the CI `UI` job
- `cd sdk && bun test`
- `just sdk-build`
- `just deny`
- platform feature-matrix checks from T24
- focused benchmark and hardware receipts from T25
- Claude verdict PASS
- independent verifier verdict PASS
- `git diff --check`

## Dependency and collision map

- T00 gates all work; T02A may run beside T01.
- T01 -> T02, T03, T04, T05, T06, T08, T10, T13, T17, T20.
- T02A -> T02, T09, T10, T17, T22.
- T02 -> T04, T06, T11, T12, T20, T22, T23.
- T03 -> T11, T12, T19, T21.
- T04 -> T05, T07, T11, T19.
- T05 -> T07, T21.
- T06 -> T07, T08, T10, T25.
- T10 -> T17 serializes the shared core allocation-contract binary.
- T07 + T08 -> T09.
- T10 -> T11 -> T12 serializes `api/config.rs`.
- T02 -> T04 -> T11 -> T12 -> T14 -> T23 -> T22 serializes
  `pipeline_runtime.rs`; T02 -> T12 also serializes `capture_demand.rs`.
- T01 -> T02 -> T04 -> T14 -> T22 serializes `input/traits.rs`.
- T01 -> T02 -> T06 serializes `input/mod.rs`.
- T04 -> T05 -> T07 -> T21 serializes `api/ws/*`, the WebSocket manifest, and its
  generated Python constants.
- T19 -> T22 serializes Servo `frame_queue.rs`.
- T02A -> T02 -> T10 -> T17 -> T22 serializes
  `hypercolor-core/tests/alloc_contract_tests.rs` (with transitive edges through
  T06 and T23 where applicable).
- T11 + T12 + T13 -> T14.
- T13 -> T15 + T17; T15 -> T16.
- T12 + T14 + T15 + T17 -> T18.
- T14 -> T19; T19 + T23 -> T22.
- T11 + T13 + T14 + T18 -> T23.
- T08 + T09 + T12 + T15 + T16 + T19 -> T24.
- T06-T24 -> T25; T16 + T18 + T22 + T25 -> T26.

The main collision surfaces are `input/traits.rs`, `input/mod.rs`,
`input/screen/mod.rs`, `pipeline_runtime.rs`, `api/config.rs`, `api/ws/*`, and
Servo `frame_queue.rs`.
Those files have one task owner at a time. Platform crates, media, audio, UI, and
CI can run in parallel after their shared contracts freeze.

`CHANGELOG.md` is append-only in T03 and T11. Each task adds its own scoped entry
without rewriting or reformatting the other task's section; T03 -> T11 is a direct
dependency.

## Commit and review cadence

- T00 is one spec commit.
- T01-T05 and T02A land as separate contract/instrumentation commits in
  dependency order.
- T06-T11 are separate lifecycle/input commits; T08 and T09 are deliberately
  sequential.
- T12-T18 are separate capture commits; T15 precedes T16.
- T19-T24 are separate consumer/integration commits; dependency order places T23
  before T22 despite their numeric labels.
- T25 contains test and acceptance artifacts only. Production fixes discovered
  there become focused commits with their own re-verification.
- T26 contains durable benchmarks/docs and the completion ledger.

After every two or three edits, run the tightest relevant test. Before each commit,
run the task's full focused gate, inspect the exact staged diff, and use a wrapped
Conventional Commit body. After each wave, run `just check` plus affected crate
tests and an adversarial Claude delta review. The implementer never assigns the
final PASS.

## Completion definition

This epic is complete only when:

1. Every F01-F49 finding has a committed fix and a named passing proof.
2. No source can remain healthy/live with a dead worker or stale session.
3. Windows and Wayland capture share envelope, cadence, freshness, processing,
   routing, and diagnostics semantics.
4. Raw Input is correct for special keys, DPI, device lifecycle, native error
   paths, concurrency, and physical acceptance.
5. Audio callbacks and capture callbacks are bounded and real-time safe.
6. Media enrichment is bounded and independently failure-tolerant.
7. All consumers use typed capabilities and canonical timed data.
8. The UI presents real consent, routing, health, freshness, and remediation.
9. Performance evidence shows the fixes scale without lowering product baselines.
10. Workspace, UI, SDK, Windows feature matrix, Claude review, and independent
    verification all pass with exact receipts.

No service is automatically started or restarted by this plan. Hardware acceptance
commands that require the daemon are proposed immediately before execution with
their exact impact and fallback.
