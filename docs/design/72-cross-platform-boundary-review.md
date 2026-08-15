# Cross-Platform Boundary Review: The Platform Layer

**Reviewed:** 2026-08-15, main `28edd518` plus the PR #158 worktree
`nova/macos-capture-input` `b881c795`.
**Method:** six parallel read-only boundary reviews (screen capture, host
input, GPU interop, audio/media/sensor sources, process supervision and
privilege, driver/HAL), each comparing code on both trees against the
governing specs, synthesized here. Companion to
`docs/design/71-macos-capture-input-pr158-review.md`, which is the bug-level
review of PR #158; this document is the abstraction-level review of every
platform boundary.

**Verdict: the boundaries are half-built, and the same half everywhere.**
The platform-neutral edges of each pipeline (vocabulary types, delivery
machinery, trait seams facing the engine) are genuinely shared, mostly
excellent, and platform-free. The middle layer between "OS API fires" and
"neutral type published" is hand-mirrored per platform: written once per OS,
kept in sync by convention, doc comments, and review rather than by the
compiler. macOS is the third mirror, and PR #158 shows exactly what that
costs: the branch could not add a platform without editing the other
platforms' files, and it grew several new sibling copies of code that
already existed twice.

The fix is the platform layer this document specifies: one engine per
capability owned once in neutral code, one small backend trait per
capability that platform crates implement, one shared vocabulary that
platform code produces rather than mirrors.

---

## 1. The pattern, stated once 🔮

Every boundary reviewed shows the same three-layer anatomy:

1. **Neutral vocabulary and delivery (shared, good).** `CaptureFrame`,
   `InteractionData`, the keymap table, the publication graph, routing,
   `HypercolorBus`, the hub's epoch fencing, the spatial sampler. Zero
   platform names, zero cfg, singly implemented.
2. **The engine middle (triplicated, the problem).** The state machines
   that adapt a platform backend to the neutral vocabulary: capture worker
   choreography, host-input fold, GPU frame vocabulary and import
   choreography, worker retention, supervision-adjacent ownership logic.
   These exist per-OS as sibling modules with the platform name substituted.
3. **Platform backends (correctly quarantined).** The unsafe FFI crates
   (`windows-capture`, `windows-input`, the three gpu-interop crates, the
   branch's `macos-capture`/`macos-input`) follow one layout convention:
   unsafe quarantined, `undocumented_unsafe_blocks` denied, stubs so every
   platform compiles everywhere. This layer is healthy.

Layer 2 is where "do we have the right abstractions" fails today. Receipts,
one per boundary:

- **Screen capture:** `ExactPublicationShared` exists three times with
  identical method sets (`wayland.rs:533-645`, `windows.rs:390-505`, branch
  `macos.rs:611-692`); the `begin_screen_publication_preparation` and
  retirement bodies are verbatim modulo the platform name in error strings.
  Each platform module is 4,400 to 5,800 lines, of which roughly 1.5k to 2k
  is cloned protocol-client scaffolding. Drift has begun: macOS uses `Vec`
  where the others use `ExactBoxList`; only Wayland has session-scoped
  clearing.
- **Host input:** spec 71 D1 promised "per-platform `HostInputBackend`
  implementations that are pure event producers; all state folding lives in
  one shared, platform-independent module"
  (`docs/specs/71-interactive-input-pipeline.md:78-79`). `HostInputBackend`
  appears in zero Rust files. Specs 72 and 76 quietly re-planned the fold as
  a hand-mirrored "sibling of evdev.rs", and today the ~300-line fold state
  machine (SharedState latch, snapshot/generation, release synthesis, epoch
  rotation, even an identical explanatory comment at `evdev.rs:495` and
  `windows.rs:623`) exists three times. No test asserts the three folds
  produce identical snapshots for equivalent event streams.
- **GPU interop:** three structurally identical `ImportedEffectFrame`
  definitions (`linux.rs:181`, `macos.rs:202`, `windows.rs:233`) unified by
  cfg-selected re-export in `core/src/effect/traits.rs:22-33`. Spec 58
  §5.1 says "mirror the macOS crate exactly", making the mirror a review
  obligation rather than a compiler guarantee, and it has already drifted:
  `ImportedFrameTimings` fields differ per platform, and `storage_id` means
  distinct GPU storage on Linux but content version on macOS/Windows.
- **Supervision:** main's supervisor is one neutral state machine with two
  platform seams (launcher plan, lifetime guard). The macOS branch built a
  second philosophy beside it (ownership recovery wrapping `start()`,
  external-owner mode, a 1 Hz verify poll) instead of becoming the third
  tenant of the launcher-plan seam that `SystemdUserServicePlan` already
  models.
- **Sources:** four platform-plug idioms for four source families: audio
  does in-file cfg enum fallthrough with ~390 lines of inline Pulse code,
  media has a clean provider trait plus neutral session state machine,
  sensors bypass `InputSource` entirely (no health reporting; a dead sensor
  thread goes silently stale), interaction does file-per-platform selected
  at composition.
- **Driver/HAL:** the healthy counterexample. `driver-api` is platform-free,
  the daemon registers backends by advertised transport kind with zero
  `cfg(target_os)`, and one SMBus protocol drives i2cdev and PawnIO without
  knowing either exists. This boundary is the in-house proof that the
  pattern works. Its one structural debt: per-driver descriptor tables
  encode OS transport strategy in cfg'd const fns, where
  `cfg(not(windows))` silently means Linux, so ASUS Aura binds
  `UsbHidRaw` on macOS and fails every connect by construction
  (`hal/src/drivers/asus/devices.rs:98-107`,
  `core/src/device/usb_backend.rs:701-705`).

Two aggravating factors cut across all six:

- **Platform nouns are leaking into neutral vocabulary.** The branch adds
  `MacosProtectedSourceState`, `MacosTahoeCapabilities`,
  `set_macos_daemon_ownership`, and `set_macos_metal4_capability` to shared
  `status.rs`, `InputManager`, and `InputSource` itself;
  `hypercolor-macos-owner` is an unconditional dependency of app, daemon,
  and CLI; the shared event vocabulary goes from zero platform nouns on main
  to 14 `Macos` mentions. Main already contains the correct precedent in
  both directions: `InteractionDegradation` keeps failure modes neutral by
  design (`traits.rs:188-206`), while `CaptureConfigPersistenceUpdate`
  shows the anti-pattern (platform-cfg'd enum variants,
  `daemon/src/startup/services.rs:1082-1099`, so the type's shape changes
  per compile target and every consumer inherits the branching).
- **The specs ratified the drift.** Spec 71's shared-fold contract was
  walked back by specs 72/76 without a decision record. Spec 76 (rev 26,
  reviewed and passed) prescribes the four-artifact macOS ownership
  machinery that spec 77 invariant 5 forbids ("the flock is the only
  ownership authority"); the branch faithfully implemented both sides of
  the contradiction. Boundary erosion here is happening at spec-writing
  time, not just at code-writing time.

---

## 2. The platform layer 💎

"One clear abstraction that platform crates just implement" is the right
target, and the evidence says it is not one mega-trait. `driver-api` works
precisely because it is a per-capability contract. The platform layer is
one *rule set* plus one *contract per capability*:

### Rules

1. **Vocabulary is owned once and produced, never mirrored.** Shared types
   live in neutral crates (`hypercolor-types` or a small seam crate per
   capability). Platform crates construct them; they never define
   look-alikes that core unifies by cfg re-export.
2. **No platform nouns in shared code.** No `Macos*`/`Windows*` type names,
   methods, or enum variants in neutral crates, the event bus, `status.rs`,
   or any shared trait. Platform payloads cross through one neutral verb
   with a typed envelope (the `SourcePlatformStatus` shape). Facts that
   vary per platform but are knowable per frame (orientation, residency,
   lease lifetime) travel as data on the frame, not as cfg tables in
   consumers.
3. **No cfg'd shape changes in shared types.** No cfg'd enum variants, no
   cfg'd struct fields, no cfg'd function parameters in shared code. Where
   a platform has no value, the neutral type carries a unit/empty variant
   (the branch's `NativeScreenCacheLease` generalization is the model).
4. **cfg lives in exactly two places:** inside platform crates (which
   compile everywhere via stubs, the established interop convention), and
   at composition roots (`startup/services.rs`, `build_interaction_source`,
   supervisor construction). A cfg anywhere else is a defect.
5. **Per capability: one engine, one backend trait.** Core owns the state
   machine once; the platform implements a small trait answering only the
   questions that genuinely differ per OS. New platform = new backend impl
   plus composition-root arm. If adding a platform requires editing another
   platform's file, the seam is wrong.

### The capability contracts

| Capability | Engine (owned once) | Platform backend contract | Today |
|---|---|---|---|
| Screen capture | `ScreenCaptureAdapter<B>` in `core/input/screen`: owned-source ledger, exact-runtime reap/bind, worker command envelope, preparation/retirement choreography, publication slots, settings versioning | `CaptureBackend`: session open/close, frame pump, branch resolution, native route preparation, wake hook | three ~5k-line sibling modules |
| Host input | `host_fold` module: SharedState, snapshot/generation, event caps, release synthesis, epoch rotation, parameterized by a fold policy (held-state keying, repeat derivation, pointer model) | `HostInputBackend` (spec 71 D1 as written): decode to canonical events, session lifecycle, platform status | three ~300-line mirrored folds |
| GPU frame import | one vocabulary crate: `ImportedEffectFrame`, `ImportedFrameFormat`, uniform `ImportedFrameTimings`, `FrameOrigin`, a fallback-reason trait on errors | interop crates produce the shared frame type; error enums implement the reason trait beside their definitions | three mirrored vocabularies, ~220 lines of downcast mapping in core, cfg flip tables in the compositor |
| Media | `MediaProviderSession` (already exists and is the best plug in the tree) | `MediaMetadataProvider` (already exists): connect, poll, disconnect | idiom proven, unused for macOS |
| Audio capture | the existing engine: RT ring, analysis worker, recovery worker, prepared reconfiguration | `AudioCaptureBackend` behind the media-style shape; Pulse moves out of the 390-line inline block | in-file cfg enum fallthrough |
| Supervision | the existing supervisor state machine: probe, plan, spawn, watchdog, backoff, circuit breaker | `LauncherPlan { Reuse, Start, SpawnChild }` generalized from `SystemdUserServicePlan`, plus a stop authority and a filled lifetime guard per OS | Linux-only plan seam, no-op unix guard, macOS philosophy fork |
| HID transport | one hal-owned `resolve_transport(intent, os)` | descriptors declare HID *intent* once, platform-free; the resolver picks hidraw/hidapi/interrupt-claim | cfg'd const fns in nine device tables |

### The enabling trait split

`InputSource` (`core/src/input/traits.rs:470-830`) is a role union: six
universal data-plane methods plus ~29 role-specific defaulted ones (the
exact screen-publication protocol alone is 12 methods that exactly one type
per platform implements), with `is_audio_source`/`is_screen_source`/
`is_interaction_source` flags re-deriving what the type system already
knew, and a wrong-by-default `unwrap_or(SourceKind::Interaction)` at
`input/mod.rs:350`. Two independent lanes named the same first move: shrink
`InputSource` to data plane plus status seam, move each role's control
plane to its own trait, and make registration typed so kind is declared,
not inferred. This is what lets the capture adapter and host fold have a
single implementor each, lets sensors join as a plain source, and gives the
manager one generic generation-fenced swap instead of three parallel
plan/commit/retire vocabularies (audio, screen, and the branch's third host
lane, `mod.rs:92-245` and branch `mod.rs:1905-1945`).

---

## 3. Findings by boundary, condensed 🎯

Full per-lane detail lives with the receipts above; this is the ranked
digest. CONFIRMED means read in the code by the reviewing lane; the
handful of hypotheses are marked.

### Screen capture

1. Adapter scaffolding triplicated (CONFIRMED, receipts §1). The next
   protocol amendment, spec 74's event-driven re-arm, pays the tripled
   cost.
2. Two publication paths live on all three platforms (CONFIRMED): the
   legacy CPU+sRGB `ScreenData` path and the exact branch-lease path.
   Spec 73's own wave-4 gate schedules the legacy mirror for deletion;
   PR #158 minted a third copy of it (`macos.rs:2482`). The legacy path's
   CPU+sRGB requirement is what forces GPU-native backends to keep a
   readback lane alive.
3. The 228-name (248 on the branch) flat `screen/mod.rs` namespace mixes
   consumer, implementor, and planner vocabularies; three curated facades
   would fix the presentation without touching the machinery.
4. Platform code placement differs per platform (Linux fully in core,
   Windows crate-heavy, macOS crate plus core pooling); spec 74 wave 1's
   `hypercolor-pipewire-interop` aligns Linux and should state the
   placement rule.

### Host input

1. The fold is the F1 above; the branch's scroll work is the cost proof
   (one shared projector took edits in evdev.rs +251, windows.rs +92,
   browser.rs +244).
2. `worker_retention` has a verbatim twin in `hypercolor-windows-input`
   (two reaper singleton threads run on Windows) and the macOS input crate
   mints a third policy (plain bounded join). One dependency-free crate
   fixes the direction problem.
3. The interaction router's consumer catalog is built twice
   (`pipeline_runtime.rs:460-497` vs `interactive_preview.rs:1274-1305`)
   and the copies have already diverged on revision tracking.
4. Branch adds `set_macos_*` verbs to `InputSource` itself; route them
   through the existing neutral platform-payload verb instead.
5. Entry-point nits: stale `sample_all` module doc, two enums named
   `CaptureDomain`, test-only samplers un-gated.

### GPU interop

1. Shared vocabulary crate (the table above) deletes the mirrored types,
   the stub copies, the cfg re-export, the per-OS timings mapping, and
   most of the 220-line error classifier.
2. Frame orientation as data: native frames already carry `origin`; the
   importer drops it and the compositor re-derives it from two cfg truth
   tables (`gpu/source.rs:240-250, 278-288`).
3. Both screen-bridge impls live inline in shared `sparkleflinger/gpu.rs`
   (main: 45 platform markers; branch: 122). Move to
   `gpu/screen_native/{windows,macos}.rs` with the shared scaffolding
   (storage-id interning, byte quoting, manifest validation) factored once.
4. `NativeScreenCacheLease` on all platforms kills the cfg'd
   fields/params and the branch's three-clause cfg union.
5. The GL ring/fence choreography exists three times inside the interop
   crates; tolerable (inside the quarantine), track for a shared skeleton.
6. HYPOTHESIS: the `OnceLock` one-shot Servo device install has no
   device-loss renegotiation path.

### Audio, media, sensors

1. Promote media's provider idiom to the named pattern; macOS media is a
   self-contained provider plus one factory arm, not the dead-code cfg
   patches the branch currently carries.
2. Sensors become a normal `InputSource` facade over the existing poller,
   plus `SourceKind::Sensors`; today a panicked sensor thread leaves a
   permanently stale snapshot with no health surface.
3. Audio's Pulse capture moves behind the backend seam;
   `AudioCaptureBackend` already exists as an identity enum that never
   grew the trait.
4. CONFIRMED path, impact hypothesis: on stock macOS the SystemMonitor
   device selection falls through to `default_input_device()`, the
   microphone, and reports healthy (`audio/mod.rs:1684-1687`). PR #158
   ships this as behavior since no macOS audio backend exists on the
   branch.
5. `NetSource` samples inline under the manager lock; a second exception
   to "workers publish, sample reads". Don't copy it.

### Supervision and privilege

1. Amend spec 76's ownership section to match spec 77 invariant 5, then
   land the doc-71 collapse as the macOS instance of the launcher seam:
   the flock-held file carries `{owner, incarnation, server_session_id,
   credential}`; probe is one locked read; handover is launchd
   bootout/bootstrap through the existing stop-authority mapping.
2. Fill the unix lifetime guard (pdeathsig on Linux, kqueue
   `EVFILT_PROC` on macOS) so sidecar lifetime is a kernel fact; this is
   design 46 §5.3's stated intent and removes the reason most of the
   stale-sidecar recovery machinery exists. The hole is latent on Linux
   too.
3. De-macOS the shared vocabulary (finding F5 of that lane):
   `hypercolor-macos-owner` becomes a macOS-only dependency; seam-crossing
   types become `VerifiedConnection { session_id, credential }`.
4. Relocate the TCC canary (3,529 lines inside `hypercolor-daemon`) to a
   standalone harness crate; keep it as the release gate spec 76 demands.
5. The Windows privilege pattern (request file, verb allowlist, UAC,
   standalone signed helper, broker service, unprivileged daemon) and the
   Linux front-loading (udev + systemd) are the models; macOS needs
   neither elevation path, only identity-bound TCC.

### Driver/HAL

1. Transport intent resolver in hal (the table above); ASUS's macOS hole
   is the proof case, Nollie's three-way split is the per-driver
   workaround that shouldn't have to exist.
2. Delete the four dead `hypercolor-core` deps from hue/wled/govee/
   nanoleaf manifests (verified zero source usage; contradicts the stated
   layering rule).
3. Add an `UnsupportedPlatform` transport error variant and
   platform-aware transports in `DriverModuleDescriptor`, so a macOS UI
   can say "SMBus: not available on macOS" instead of an eternally empty
   scan.
4. HYPOTHESIS (needs hardware): nusb plain `claim_interface` likely fails
   on macOS for HID-class interfaces Apple's driver holds; the fix is the
   same hidapi routing the resolver provides.
5. Doc rot: CLAUDE.md's mermaid draws `CORE --> DAPI` backwards;
   driver-api depends on types only.

---

## 4. Keep 🌈

These are the load-bearing good decisions the platform layer builds on,
not incidental praise:

- The keymap: one physical table, per-platform identifier columns,
  provenance-typed resolution, totality tests; the branch's compile-time
  `panic!` on missing macOS rows is exactly how three platforms stay
  honest.
- The interop-crate convention: unsafe quarantine, stubs everywhere,
  documented sync modes; PR #158 followed it verbatim for input and
  converged the gpu crate layouts.
- `CaptureFrame`, the hub's epoch fencing, `ScreenNativeTargetPreparer`
  (core never links wgpu), byte-admission-before-allocation.
- The supervisor core with its pure, testable plan enums; stop-through-
  launcher; the Windows Job Object.
- `worker_retention` as a mechanism (it just needs to exist once).
- Media's provider seam; `audio/realtime.rs` as the RT-safety boundary
  (the July findings about FFT/locking in the callback are fixed).
- driver-api's discipline, SMBus operation framing, the identity story
  (fingerprints, `PortableIdentityClaim` refusal-by-default).
- Demand-driven capture, degraded-not-failed starts, synthesize-releases,
  typed `InteractionDegradation`.
- The delivery layer's total platform-freedom: graph, routing,
  gpu_sampling, the CPU compositor: zero platform names.
- `gpu_device.rs`'s runtime `cfg!()` inside data-driven, unit-testable
  checks: the model the rest of the seam should copy.

---

## 5. Sequencing 🦋

Ordered to avoid fighting PR #158, which is in flight and owned by its own
lane. The branch made several fold/capture surfaces line up across
platforms, so the extractions get *cheaper* after it lands; doing them on
main first would hand the branch a gratuitous conflict across its largest
files.

**Wave 0, on PR #158 itself (in addition to doc 71's blockers):**
spec 76 ownership amendment plus the doc-71 collapse; fill the unix
lifetime guard; strip `Macos*` from shared vocabulary (status, events,
`InputSource` verbs, unconditional deps); no macOS media/audio cfg patches
in shared files (implement a provider or leave the seam alone); relocate
the canary.

**Wave 1, immediately after merge (the platform layer proper):**
the `InputSource` role split and typed registration; `host_fold`
extraction with cross-backend parity tests (same canonical event stream
through all three policies, identical snapshots asserted);
`ScreenCaptureAdapter<B>` extraction; the GPU vocabulary crate with
`origin` as data; screen bridges out of `gpu.rs`; shared
`worker-retention` crate.

**Wave 2, scheduled, independent:**
spec 73 wave 4 (delete the legacy screen mirror path); hal transport
resolver plus `UnsupportedPlatform`; sensors as a source; audio backend
seam; media provider for macOS; router catalog unification; spec 74 wave 1
(`hypercolor-pipewire-interop`).

**Anytime, trivial:** four dead manifest deps; mermaid edge; stale module
docs; duplicate `CaptureDomain` name; test-only sampler gating.

The test of success is mechanical: after wave 1, adding a platform to any
capability means one backend impl, one composition-root arm, zero edits to
any other platform's files, and `grep -r 'cfg(target_os' crates/hypercolor-core/src
crates/hypercolor-daemon/src` returns only composition roots and platform
crates. That grep is currently 43 files; it should be under ten.
