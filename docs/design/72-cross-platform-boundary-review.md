# Cross-Platform Boundary Review: The Platform Layer

**Reviewed:** 2026-08-18, current tree `91b4134b`, including PR #158 at
merge commit `60463f6d` and origin main `5be1bf4f`.
**Method:** seven read-only boundary reviews (screen capture, host input,
GPU interop, audio/media/sensor sources, process supervision and
privilege, driver/HAL, desktop scaffolding), hardened by one cross-model
review round, then revalidated after the macOS merge and the recent spec 76
internal-API work. Companion to
`docs/design/71-macos-capture-input-pr158-review.md`, the bug-level review
of PR #158; this document is the abstraction-level review of every
platform boundary.

**Verdict: the boundaries are half-built, and the same half everywhere.**
The platform-neutral edges of each pipeline (vocabulary types, delivery
machinery, trait seams facing the engine) are genuinely shared, mostly
excellent, and platform-free. The middle layer between "OS API fires" and
"neutral type published" is hand-mirrored per platform: written once per
OS, kept in sync by convention, doc comments, and review rather than by
the compiler. macOS is the third mirror, and the merged PR #158 shows
exactly what that costs: adding a platform required edits to the other
platforms' files and added sibling copies of code that already existed
twice.

The fix is the platform layer this document specifies: one engine per
capability owned once in neutral code, one capability seam per domain
that platform crates implement, one shared vocabulary that platform code
produces rather than mirrors.

---

## 1. The pattern, stated once 🔮

Every boundary reviewed shows the same three-layer anatomy:

1. **Neutral vocabulary and delivery (shared, good).** `CaptureFrame`,
   `InteractionData`, the keymap table, the publication graph, routing,
   `HypercolorBus`, the hub's epoch fencing, the spatial sampler, the
   session-event vocabulary. Zero platform names, zero cfg, singly
   implemented.
2. **The engine middle (triplicated, the problem).** The state machines
   that adapt a platform backend to the neutral vocabulary: capture worker
   choreography, host-input fold, GPU frame vocabulary and import
   choreography, worker retention, daemon-ownership and service-mode
   logic. These exist per-OS as sibling modules with the platform name
   substituted.
3. **Platform backends (correctly quarantined).** The unsafe FFI crates
   (`windows-capture`, `windows-input`, the three gpu-interop crates, the
   `macos-capture`/`macos-input`) follow one layout convention:
   unsafe quarantined, `undocumented_unsafe_blocks` denied, stubs so every
   platform compiles everywhere. This layer is healthy.

Layer 2 is where "do we have the right abstractions" fails today.
Receipts, one per boundary:

- **Screen capture:** `ExactPublicationShared` exists three times with a
  shared core of functionally identical methods plus per-platform extras
  already accreting (`wayland.rs:533-645` adds session-scoped clearing,
  `windows.rs:390-505` adds descriptor allocation,
  `macos.rs:621-706` adds compute-policy helpers); the
  `begin_screen_publication_preparation` and retirement bodies are
  verbatim modulo the platform name in error strings. Each platform
  module is 4,382 to 5,935 lines, of which roughly 1.5k to 2k is cloned
  protocol-client scaffolding. Storage drift has begun too: macOS uses
  `Vec` where the others use `ExactBoxList`.
- **Host input:** spec 71 D1 promised "per-platform `HostInputBackend`
  implementations that are pure event producers; all state folding lives
  in one shared, platform-independent module"
  (`docs/specs/71-interactive-input-pipeline.md:78-79`).
  `HostInputBackend` appears in zero Rust files. Specs 72 and 76 quietly
  re-planned the fold as a hand-mirrored "sibling of evdev.rs", and today
  the ~300-line fold state machine (SharedState latch,
  snapshot/generation, release synthesis, and epoch rotation) exists
  three times. No test asserts the three folds produce identical
  snapshots for equivalent event streams.
- **GPU interop:** three structurally identical `ImportedEffectFrame`
  definitions (`linux.rs:181`, `macos.rs:507`, `windows.rs:233`) unified
  by cfg-selected re-export in `core/src/effect/traits.rs:22-33`. Spec 58
  §5.1 says "mirror the macOS crate exactly", making the mirror a review
  obligation rather than a compiler guarantee, and it has already
  drifted: `ImportedFrameTimings` fields differ per platform (forcing
  per-OS telemetry mapping in core), and `storage_id` is a content
  generation wearing a storage-identity name: Linux mints a fresh value
  per issued import even when the slot texture is reused
  (`slot_pool.rs:388,408`), and the daemon conflates identity with
  generation at `sparkleflinger/gpu/source.rs:292-293`.
- **Supervision:** the supervisor is one neutral state machine with two
  platform seams (launcher plan, lifetime guard). The macOS work built a
  second philosophy beside it (ownership recovery that can
  suppress the supervisor entirely, external-owner mode, a 1 Hz verify
  poll) instead of becoming the third tenant of the launcher-plan seam
  that `SystemdUserServicePlan` already models. Specs 76 and 77 agree on
  authority (the flock/single-instance guard is the sole owner-arbiter;
  the owner record is "diagnostic only", spec 76:390); the problem is
  size, naming, and placement of the machinery around that authority,
  not a spec contradiction.
- **Desktop scaffolding:** service mode ("which launcher owns the
  daemon") is scattered knowledge across at least seven hardcoded sites
  (CLI constants, supervisor constant, `support.rs` SCM names, the
  PowerShell installer, systemd units, the launchd plist template, the
  Homebrew service block), with two live user-visible bugs: on a
  Homebrew macOS install the CLI reports "not loaded" while the daemon
  runs under `homebrew.mxcl.hypercolor`, and on Windows the supervisor
  has no Start arm for an installed-but-stopped SCM service so it
  silently shadows the user's chosen topology with a spawned child. The
  merged macOS work first-classed mode awareness for one platform only,
  minting several parallel `Macos*Owner` enums plus a string vocabulary for one
  concept. Meanwhile two trays ship: design 46 retired
  `hypercolor-tray` into the app releases ago (`46:125-127`), specs 61
  and 76 still sign and package it, and the fork has diverged on
  security behavior (the app's daemon client pins server identity across
  reconnects and binds API keys to host+port; the tray still matches by
  `instance_id` alone).
- **Sources:** four platform-plug idioms for four source families: audio
  does in-file cfg enum fallthrough with ~390 lines of inline Pulse code,
  media has a clean provider trait plus neutral session state machine,
  sensors bypass `InputSource` entirely (no health reporting; a dead
  sensor thread goes silently stale), interaction does file-per-platform
  selected at composition.
- **Session/power events:** the one domain where the neutral abstraction
  predates this review: `SessionEvent` (`types/src/session.rs:14-31`),
  the `SessionMonitor` backend trait (`core/src/session/mod.rs:32-42`),
  and a full sleep-policy engine (`daemon/src/session.rs`) exist with
  zero platform nouns, but only Linux implements a monitor. The Windows
  power work went around the engine into the app as an HTTP
  discover-nudge (`app/src/power_events/windows_impl.rs:173-183`), so
  suspend fade/release policy is dead on Windows, dead in headless SCM
  mode (whose handler declines `PowerEvent`,
  `daemon/src/windows_service.rs:54-63`), and absent on macOS on both
  trees, despite spec 77 H7.3 requiring sleep/wake recovery.
- **Filesystem and paths:** `hypercolor-platform-fs` is a crate for one
  function whose unix arm hands its own job back to the caller ("Unix
  callers remain responsible for syncing the parent directory",
  `lib.rs:16-21`), and no caller does. Atomic-write/symlink/permission
  hygiene is now hand-rolled in three places (platform-fs, driver-api's
  `#[cfg(unix)]` 0600 credential store, and
  `hypercolor-macos-owner`). Path resolution has a canonical module
  (`core/src/config/paths.rs`). The CLI now consumes that resolver, but
  app first-run, diagnostics, the Servo cache, and service installation
  still bypass the authority with direct home or `dirs::` lookups.
- **Driver/HAL:** the healthy counterexample. `driver-api` is
  platform-free, the daemon registers backends by advertised transport
  kind with zero `cfg(target_os)`, and one SMBus protocol drives i2cdev
  and PawnIO without knowing either exists. This boundary is the
  in-house proof that the pattern works. Its one structural debt:
  per-driver descriptor tables encode OS transport strategy in cfg'd
  const fns, where `cfg(not(windows))` silently means Linux, so ASUS
  Aura binds `UsbHidRaw` on macOS and fails every connect by
  construction (`hal/src/drivers/asus/devices.rs:86-107`,
  `core/src/device/usb_backend.rs:701-705`).

Two aggravating factors cut across all of these:

- **Platform nouns are leaking into neutral vocabulary.** PR #158 added
  `MacosProtectedSourceState`, `MacosTahoeCapabilities`,
  `set_macos_daemon_ownership`, and `set_macos_metal4_capability` to
  shared `status.rs`, `InputManager`, and `InputSource` itself;
  `hypercolor-macos-owner` is an unconditional dependency of app,
  daemon, and CLI; the shared event vocabulary now has 14 `Macos`
  mentions. The pre-existing tree is not clean either:
  `CaptureConfigPersistenceUpdate` has platform-cfg'd enum variants
  (`daemon/src/startup/services.rs:980-1002`), and media's
  `ArtworkSource::WindowsSession` carries a cfg-shaped native variant in
  a shared enum (`media.rs:117-135`). The correct precedent exists in
  the same files: `InteractionDegradation` keeps failure modes neutral
  by design (`traits.rs:324-337`).
- **The specs ratified the drift.** Spec 71's shared-fold contract was
  walked back by specs 72 and 76 without a decision record. Design 46
  retired the standalone tray; specs 61 and 76 still require and sign
  it. Boundary erosion here is happening at spec-writing time, not just
  at code-writing time; the platform layer needs the specs updated with
  it or the next implementer faithfully re-derives the divergence.

---

## 2. The platform layer 💎

"One clear abstraction that platform crates just implement" is the right
target, and the evidence says it is not one mega-trait. `driver-api`
works because it is a per-capability contract, and the scaffolding lane
independently reached the same verdict for the app shell (first-run,
diagnostics, and single-instance are healthy as-is; the shell's concerns
share a composition root, not a lifecycle, so no `ShellPlatform` role
union). The platform layer is one *rule set* plus one *contract per
capability*:

### Rules

1. **Vocabulary is owned once and produced, never mirrored.** Shared
   types live in neutral crates (`hypercolor-types` or a small seam crate
   per capability). Platform crates construct them; they never define
   look-alikes that core unifies by cfg re-export.
2. **No platform nouns in shared code.** No `Macos*`/`Windows*` type
   names, methods, or enum variants in neutral crates, the event bus,
   `status.rs`, or any shared trait. The status boundary is: neutral
   lifecycle fields (state, freshness, issue code, remediation text)
   plus at most one versioned platform-diagnostics envelope whose
   payload the platform crate serializes; shared code treats the payload
   as opaque data with neutral display fields. Facts that vary per
   platform but are knowable per frame (orientation, residency, lease
   lifetime, run mode) travel as data, not as cfg in consumers.
3. **No cfg'd shape changes in shared types.** No cfg'd enum variants,
   no cfg'd struct fields, no cfg'd function parameters in shared code.
   Where a platform has no value, the neutral type carries a unit/empty
   variant (PR #158's `NativeScreenCacheLease` generalization is the
   model). `CaptureConfigPersistenceUpdate` and
   `ArtworkSource::WindowsSession` are the named counterexamples to
   convert.
4. **`cfg(target_os)` lives in exactly two places:** inside platform
   crates (which compile everywhere via stubs, the established interop
   convention), and at composition roots (`startup/services.rs`,
   `build_interaction_source`, supervisor construction). A
   `cfg(target_os)` anywhere else is a defect. `cfg(test)` and cargo
   feature cfgs are out of scope for this rule and remain legitimate
   everywhere.
5. **Per capability: one engine, one capability seam.** Core owns the
   state machine once; the platform implements the seam, which is
   whatever small shape the capability genuinely needs: a backend trait,
   a produced vocabulary, a plan enum, or a resolver function. New
   platform = new seam impl plus a composition-root arm. If adding a
   platform requires editing another platform's file, the seam is wrong.

### The capability contracts

| Capability | Engine (owned once) | Platform seam | Today |
|---|---|---|---|
| Screen capture | `ScreenCaptureAdapter<B>` in `core/input/screen`: owned-source ledger, exact-runtime reap/bind, worker command envelope, preparation/retirement choreography, publication slots, settings versioning | `CaptureBackend`: session open/close, frame pump, branch resolution, native route preparation, wake hook | three ~5k-line sibling modules |
| Host input | `host_fold` module owning ALL fold semantics: held state, repeat classification, release synthesis, epochs, snapshots. No mutable platform policy hooks | `HostInputBackend` (spec 71 D1 as written): a pure producer of ordered neutral pre-fold events carrying device identity (optional), raw edge plus repeat evidence, relative or absolute motion with topology generation, scroll units, and state gaps | three ~300-line mirrored folds |
| GPU frame import | one vocabulary crate: `ImportedEffectFrame`, `ImportedFrameFormat`, uniform `ImportedFrameTimings` with explicitly optional phases, `FrameOrigin`, `content_generation` (not `storage_id`), a fallback-reason trait on errors | interop crates produce the shared frame type; error enums implement the reason trait beside their definitions | three mirrored vocabularies, ~220 lines of downcast mapping in core, cfg flip tables in the compositor |
| Media | `MediaProviderSession` (exists; the artwork enum needs the rule-3 conversion) | `MediaMetadataProvider` (exists): connect, poll, disconnect | idiom proven, unused for macOS |
| Audio capture | the existing engine: RT ring, analysis worker, recovery worker, prepared reconfiguration | `AudioCaptureBackend` as a push-producer session (see contract sketches; NOT media's poll shape): open a stream against the RT ring, report device identity and loopback capability, close | in-file cfg enum fallthrough |
| Supervision / launch | the existing supervisor state machine: probe, plan, spawn, watchdog, backoff, circuit breaker | `LauncherPlan` with payloads plus an owner-preference policy input, `StopAuthority`, a filled lifetime guard (see sketches) | Linux-only plan seam, no-op unix guard, macOS philosophy fork |
| Service identity | `DaemonRunMode`/`ServiceIdentity` in `hypercolor-types`, declared by the launcher (generalize spec 77 H1.5's env channel to all platforms), self-reported by the daemon in its status API | each service definition (systemd unit, SCM registration, launchd plist, brew block) declares its identity on its own ExecStart line | mode knowledge scattered across seven sites and several parallel `Macos*Owner` vocabularies |
| Session/power | `SessionEvent` + dedup forwarder + `SleepPolicy` + controller (all exist, all neutral) | `SessionMonitor` (exists), implemented by platform crates and installed at the daemon composition root: Linux logind/screensaver shipped; Windows and macOS remain | engine live on one platform; Windows built as an app-side HTTP nudge instead; macOS absent |
| HID transport | one hal-owned `resolve_transport(intent, os)` | descriptors declare HID *intent* once, platform-free; the resolver picks hidraw/hidapi/interrupt-claim | cfg'd const fns in nine device tables |
| Filesystem + paths | `hypercolor-platform-fs` grown into the audited hygiene home: `durable_replace` that syncs the parent itself, secret-write with the 0600/owner discipline, symlink-refusing open; `core/config/paths.rs` as the single path authority every binary imports | the Windows FFI module it already has; unix arms implemented once, correctly | one-function crate that delegates its own durability contract to callers who drop it; CLI and app bypass the path authority |

### The enabling trait split

`InputSource` (`core/src/input/traits.rs:641-1057`) is a role union: six
universal data-plane methods plus ~29 role-specific defaulted ones (the
exact screen-publication protocol alone is 12 methods that exactly one
type per platform implements), with `is_audio_source` and friends
re-deriving what the type system already knew, and a wrong-by-default
`unwrap_or(SourceKind::Interaction)` at `input/mod.rs:398,775`.

The split needs an object model, not just intent, because registration
erases sources into `Box<dyn InputSource>` today (`input/mod.rs:375`)
and subtrait methods are unreachable after erasure. The shape:

```rust
/// Roles are exclusive: a source registers as exactly one variant.
/// Every trait here is object-safe, Send, and NOT Sync (the manager
/// owns sources behind its existing lock; control methods take &mut).
enum ManagedSourceRole {
    Audio(Box<dyn AudioSource>),        // ManagedSource + audio control plane
    Screen(Box<dyn ScreenSource>),      // ManagedSource + screen publication protocol
    Interaction(Box<dyn InteractionSource>), // ManagedSource + capture toggles
    Data(Box<dyn DataSource>),          // ManagedSource + kind (media, net, sensors)
}
```

`ManagedSource` carries name/start/stop/sample-and-drain/status. Each role
trait extends it with that role's control plane, so the common plane is
reachable from every variant without downcasts. `DataSource` is the
general-data role and declares one immutable `DataSourceKind` at
registration. The other kinds come from their variants; `CaptureDomain`
and the `is_*` flag inference disappear. The manager's three parallel
plan/commit/retire lanes collapse onto one generic generation-fenced swap
keyed by role and fenced by exact slot identity.

### Contract sketches: the load-bearing Rust decisions

These are the decisions each wave-1 implementing spec must honor; they
are stated here because getting them wrong invalidates the seam.

- **`CaptureBackend`** is generic (`ScreenCaptureAdapter<B:
  CaptureBackend>`, static dispatch, one adapter instantiation per
  platform at the composition root). The backend owns an associated
  `Session` type; frames arrive through a sink handle the adapter
  provides (callback-driven backends push; poll-driven backends pump),
  because macOS sessions are main-thread-bound and callback-driven while
  DXGI is poll-based. The contract must state thread affinity per
  method, cancellation (typed, deadline-bounded, per spec 77 H2.3), and
  Drop behavior (Drop aborts without blocking; graceful stop is an
  explicit async method). Native lease ownership stays backend-side;
  the adapter sees only neutral lease tokens.
- **`HostInputBackend`** produces ordered events into a bounded queue
  owned by the fold. No shared mutable state with the fold, no policy
  hooks. The fold derives repeat classification and held-state keying
  from event content (device identity present or absent), so platform
  variance is data, not behavior.
- **`AudioCaptureBackend`** is a push-producer: `open(config, ring:
  AudioFrameRing) -> Session`, where the session owns the OS stream and
  writes only through the preallocated ring
  (`audio/realtime.rs`'s existing contract). It is not async and not
  media's poll shape; media's provider deliberately uses object-erased
  non-Send futures (`media.rs:746`), audio owns RT callbacks. Loopback
  capability is reported, never guessed: a backend that cannot capture
  system audio says so instead of falling through to the microphone.
- **`LauncherPlan`** carries payloads: `Reuse { identity:
  ServiceIdentity, endpoint }`, `Start { identity, unit }`,
  `SpawnChild { command }`. The plan function takes an owner-preference
  policy stating which arms are permitted, because external-owner
  semantics forbid the SpawnChild fallback when the selected owner is
  offline (the app must hold and surface a remedy, spec 76:446-451;
  Windows "use the existing service or replace it" is the same
  constraint). `StopAuthority { SupervisedChild,
  ServiceManager(ServiceIdentity), UserDirected }` replaces the twin
  app/CLI enums. The pure-function plan shape
  (`supervisor/mod.rs:1141-1145`) survives; macOS becomes a tenant.
- **The GPU vocabulary crate** owns only neutral wgpu-facing types. No
  native handles, no per-target features: native format conversions and
  target-specific capabilities stay in the interop crates so workspace
  feature unification cannot leak platform deps into neutral consumers.
- **One launchd adapter.** All launchd interaction (app executor, CLI
  service verbs) goes through one module speaking modern
  `bootstrap/bootout/kickstart` verbs, with enable/disable distinct from
  start/stop and one shared agent-filename constant (today the merged
  CLI writes `Hypercolor.plist` while the cask zaps
  `tech.hyperbliss.hypercolor.app.plist`, so uninstall misses it).

### Macro policy 🪄

Most of the boilerplate this review found is a missing trait, not a
missing macro: a macro that stamps per-platform copies of logic
preserves drift in a harder-to-debug costume. Macros enter after the
layer lands, for mechanical residue only, in four sanctioned shapes:

1. **`platform_modules!`** encoding the crate-layout convention (real
   module on the target OS, stubs elsewhere, feature-gated
   `servo_context`, mirrored `pub use`) so the convention is written
   once instead of reviewed by eye. The layer itself shrinks the stub
   surface first; the macro covers what remains.
2. **Conformance-suite stamping**: one canonical neutral event corpus
   feeds the shared fold and pins its snapshots. Separate backend suites
   feed equivalent raw OS fixtures into each normalizer and assert the
   same neutral corpus, ordering, and gap semantics. Pure producer
   backends do not consume their own output vocabulary.
3. **Derive-style plumbing**: `#[derive(SourceStatus)]` for the
   status-reporter wiring every production source repeats verbatim; the
   same shape for GPU error-enum fallback-reason impls if they prove
   mechanical.
4. **Declarative tables** in the keymap style: const-fn data plus
   compile-time `panic!` on a missing platform column, so totality is a
   compile error.

Guardrails: declarative data, test stamping, convention encoding, and
derives only. Never macros for control flow, never to generate divergent
per-platform logic. Declarative macros over proc macros unless derive
ergonomics genuinely demand it (`hypercolor-leptos-ext-macros` is the
precedent when they do). Every macro documents its expansion with a
concrete example at the definition.

---

## 3. Findings by boundary, condensed 🎯

Full per-lane detail lives with the receipts above; this is the ranked
digest. CONFIRMED means read in the code by the reviewing lane or
verified during the cross-model round; the handful of hypotheses are
marked.

### Screen capture

1. Adapter scaffolding triplicated (CONFIRMED, receipts §1). The next
   protocol amendment, spec 74's event-driven re-arm, pays the tripled
   cost.
2. Two publication paths live on all three platforms (CONFIRMED): the
   legacy CPU+sRGB `ScreenData` path and the exact branch-lease path.
   Spec 73's own wave-4 gate schedules the legacy mirror for deletion;
   PR #158 minted a third copy of it. The legacy
   path's CPU+sRGB requirement is what forces GPU-native backends to
   keep a readback lane alive.
3. The 1,884-line `screen/mod.rs` surface mixes consumer, implementor,
   and planner vocabularies; three curated facades would fix the
   presentation without touching the machinery.
4. Platform code placement differs per platform (Linux fully in core,
   Windows crate-heavy, macOS crate plus core pooling); spec 74 wave 1's
   `hypercolor-pipewire-interop` aligns Linux and should state the
   placement rule.

### Host input

1. The fold is §1's finding; PR #158's scroll work is the cost proof
   (one shared projector took edits in evdev.rs +251, windows.rs +92,
   browser.rs +244). Backends become pure event producers; the fold owns
   all semantics.
2. `worker_retention` has a verbatim twin in `hypercolor-windows-input`
   (two reaper singleton threads run on Windows) and the macOS input
   crate mints a third policy (plain bounded join). One dependency-free
   crate fixes the direction problem.
3. The interaction router's consumer catalog is built separately by the
   render pipeline and interactive preview, and the copies have already
   diverged on revision tracking.
4. Branch adds `set_macos_*` verbs to `InputSource` itself; they move to
   the role trait that owns them, expressed through the rule-2 status
   boundary.
5. Entry-point nits: stale `sample_all` module doc, two enums named
   `CaptureDomain`, test-only samplers un-gated.

### GPU interop

1. Shared vocabulary crate (§2 table) deletes the mirrored types, the
   stub copies, the cfg re-export, the per-OS timings mapping, and most
   of the 220-line error classifier.
2. Frame orientation as data: native frames already carry `origin`; the
   importer drops it and the compositor re-derives it from cfg truth
   tables (`sparkleflinger/gpu/source.rs:259-313`).
3. Both screen-bridge impls live inline in shared
   `sparkleflinger/gpu.rs`, which now has 291 platform markers.
   Move to `gpu/screen_native/{windows,macos}.rs` with the shared
   scaffolding factored once.
4. `NativeScreenCacheLease` on all platforms kills the cfg'd
   fields/params and the current three-clause cfg union.
5. The GL ring/fence choreography exists three times inside the interop
   crates; tolerable (inside the quarantine), track for a shared
   skeleton.
6. HYPOTHESIS: the `OnceLock` one-shot Servo device install has no
   device-loss renegotiation path.

### Audio, media, sensors

1. Promote media's provider idiom to the named pattern; macOS media is a
   self-contained provider plus one factory arm, not dead-code cfg
   patches in shared files.
2. Sensors become a normal source under `ManagedSourceRole::Data`, plus
   `SourceKind::Sensors`; today a panicked sensor thread leaves a
   permanently stale snapshot with no health surface.
3. Audio's Pulse capture moves behind `AudioCaptureBackend`.
4. CONFIRMED path, impact hypothesis: on stock macOS the SystemMonitor
   device selection falls through to `default_input_device()`, the
   microphone, and reports healthy (`audio/mod.rs:1685-1691`). PR #158
   ships this as behavior since no macOS audio backend exists. The
   backend contract's loopback-capability rule exists to
   make this state unrepresentable.
5. `NetSource` samples inline under the manager lock; a second exception
   to "workers publish, sample reads". Don't copy it.

### Supervision and privilege

1. The macOS ownership layer must become a tenant of the launcher
   seam (CONFIRMED: `LauncherPlan`-shaped logic exists but macOS wraps
   and can suppress the supervisor, `supervisor/mod.rs:1238-1286`;
   external-owner mode re-derives `Reuse` under a macOS name; a 1 Hz
   verify poll runs beside a notify-based owner watch shipped in the
   same PR).
2. Fill the unix lifetime guard (pdeathsig on Linux, kqueue
   `EVFILT_PROC` on macOS) so sidecar lifetime is a kernel fact; this is
   design 46 §5.3's stated intent and removes the reason most of the
   stale-sidecar recovery machinery exists. The hole is latent on Linux
   too.
3. Artifact-count reduction in the ownership store is an open design
   question, not a wave item. Specs 76 and 77 agree the guard is the
   sole authority and the surrounding artifacts are
   diagnostics/evidence; doc 71's flock-file collapse as sketched has
   no viable update/read protocol (a lifetime-held flock cannot serve
   shared locked reads, and atomic replacement changes the locked
   inode, which is why spec 76 separates the coordination lock from
   replaced records). Any consolidation needs its own proven file
   protocol first.
4. De-macOS the shared vocabulary: `hypercolor-macos-owner` becomes a
   macOS-only dependency; seam-crossing types become neutral
   (`VerifiedConnection { session_id, credential }`); the parallel owner
   and owner-event vocabularies collapse onto `ServiceIdentity`.
5. Relocate the TCC canary (3,529 lines inside `hypercolor-daemon`) to
   a standalone harness crate; keep it as the release gate spec 76
   demands. Daemon `main.rs` returns to platform-clean from its current
   1,336 lines with arbitration inline.
6. The Windows privilege pattern (request file, verb allowlist, UAC,
   standalone signed helper, broker service, unprivileged daemon) and
   the Linux front-loading (udev + systemd) are the models; macOS needs
   neither elevation path, only identity-bound TCC.

### Desktop scaffolding

1. Service mode becomes first-class and neutral (CONFIRMED, §1
   receipts): `DaemonRunMode`/`ServiceIdentity` declared by the
   launcher, self-reported by the daemon, replacing the sc.exe/
   systemctl/launchctl mode probes, the UI's platform-named
   `detect_windows_daemon_service` bridge command, and three of the
   four owner enums. Fixes the brew-macOS and stopped-SCM-Windows
   inconsistencies.
2. Execute design 46's tray deletion (CONFIRMED drift with security
   divergence): `hypercolor-tray` retires, its ~1,200 duplicated lines
   fold into the app's already-neutral model modules (`menu.rs`,
   `AppState`, `DaemonClient`), and specs 61/76 plus AUR/Homebrew stop
   packaging and signing the retired binary in the same change. If a
   headless-tray niche must survive, it consumes the app's model as a
   library, never as a fork.
3. Session/power backends land daemon-side behind the existing
   `SessionMonitor` seam (CONFIRMED, the cheapest alignment in this
   document): a Windows monitor (message-only window standalone,
   `SERVICE_CONTROL_POWEREVENT` in SCM mode) and a macOS
   NSWorkspace/IOKit monitor; the app-side HTTP nudge retires.
4. One launchd adapter (CONFIRMED: legacy `load/unload` and modern
   `bootstrap/kickstart` verbs coexist against the same label; `bootout`
   appears nowhere; start conflated with enable; two app-agent
   filenames of which uninstall knows one).
5. Bridge types cross by hand-mirror (CONFIRMED, minor):
   `WindowsDaemonServiceStatus` re-typed in the WASM UI, helper `Verb`
   self-documented as a mirror. Shared envelopes in `hypercolor-types`,
   platform variance as data.

### Filesystem and paths

1. `hypercolor-platform-fs` grows into the single audited hygiene home
   (CONFIRMED contract gap): `durable_replace` performs the parent-dir
   sync itself on unix; secret-write and symlink-refusing open absorb
   the driver-api and macos-owner hand-rolls.
2. `paths.rs` becomes the imported path authority for every binary
   (CONFIRMED bypasses): the CLI migration is complete; app first-run,
   diagnostics, service installation, and the Servo cache route through
   it; macOS behavior gets documented explicitly.
3. The portable-lock helper in `scripts/cargo-cache-build.sh` is build
   infrastructure, not runtime; out of the layer's scope.

### Driver/HAL

1. Transport intent resolver in hal (§2 table); ASUS's macOS hole is
   the proof case, Nollie's three-way split is the per-driver workaround
   that shouldn't have to exist.
2. The four dead `hypercolor-core` deps in hue/wled/govee/nanoleaf are
   already deleted by spec 76 phase 0; the transport resolver remains.
3. Add an `UnsupportedPlatform` transport error variant and
   platform-aware transports in `DriverModuleDescriptor`, so a macOS UI
   can say "SMBus: not available on macOS" instead of an eternally
   empty scan.
4. HYPOTHESIS (needs hardware): nusb plain `claim_interface` likely
   fails on macOS for HID-class interfaces Apple's driver holds; the
   fix is the same hidapi routing the resolver provides.
5. The dependency graph documentation now shows driver-api depending on
   types only; no documentation correction remains in this package.

---

## 4. Keep 🌈

These are the load-bearing good decisions the platform layer builds on,
not incidental praise:

- The keymap: one physical table, per-platform identifier columns,
  provenance-typed resolution, totality tests; PR #158's compile-time
  `panic!` on missing macOS rows is exactly how three platforms stay
  honest.
- The interop-crate convention: unsafe quarantine, stubs everywhere,
  documented sync modes; PR #158 followed it verbatim for input and
  converged the gpu crate layouts.
- `CaptureFrame`, the hub's epoch fencing, `ScreenNativeTargetPreparer`
  (core never links wgpu), byte-admission-before-allocation.
- The supervisor core with its pure, testable plan enums;
  stop-through-launcher; the Windows Job Object.
- `SessionEvent` + `SessionMonitor` + the sleep-policy engine: the one
  capability whose neutral abstraction predates this review.
- The app tray's platform-neutral menu model (`app/tray/menu.rs`),
  testable without a Tauri runtime; the extraction target for tray
  consolidation.
- `windows_service.rs` as a thin SCM adapter around the identical
  `daemon::run`; service mode reuses the whole daemon unchanged.
- The 20-phase journaled handover with rollback parity in
  `hypercolor-macos-owner`: the machinery is excellent; the findings
  above are about naming, placement, and duplication, not design. Same
  for the launchd contender exiting zero to defeat `KeepAlive` respawn
  loops, and `packaging_tests.rs` pinning plist content, installers-as-
  tests discipline the rest of the triangle should adopt.
- `worker_retention` as a mechanism (it just needs to exist once).
- Media's provider seam; `audio/realtime.rs` as the RT-safety boundary
  (the July findings about FFT/locking in the callback are fixed).
- driver-api's discipline, SMBus operation framing, the identity story
  (fingerprints, `PortableIdentityClaim` refusal-by-default).
- Demand-driven capture, degraded-not-failed starts,
  synthesize-releases, typed `InteractionDegradation`.
- The delivery layer's total platform-freedom: graph, routing,
  gpu_sampling, the CPU compositor: zero platform names.
- `gpu_device.rs`'s runtime `cfg!()` inside data-driven, unit-testable
  checks: the model the rest of the seam should copy.

---

## 5. Implementation plan and goal state 🦋

### Scope and non-goals

The core build is fourteen review packages across three implementation
waves, followed by four extension packages. A package is a coherent
review surface, not a mandatory PR boundary. Adjacent packages may share
one PR when their dependency edge is local and the combined diff stays
reviewable.

The plan does not consolidate the macOS ownership artifacts, share the
platform GPU ring/fence implementations, solve Servo device-loss
renegotiation, or lower any performance baseline. The legacy screen
mirror deletes only through spec 73's exact-consumer gate. macOS audio
does not silently substitute microphone capture for system loopback.

### Goal state

When the core build is done, all of the following hold, each checkable
mechanically:

1. **Crate graph delta.** Added: `hypercolor-worker-retention` (leaf),
   `hypercolor-gpu-frame` (neutral GPU frame vocabulary),
   `hypercolor-pipewire-interop`, `hypercolor-linux-session`, and
   `hypercolor-windows-session`. Deleted: `hypercolor-tray`. No neutral
   crate depends unconditionally on a platform crate; fixture-only
   dependencies are dev-dependencies or explicit fixture features.
2. **The cfg budget.** `rg -l 'cfg\(target_os' crates/hypercolor-core/src
   crates/hypercolor-daemon/src` returns composition roots and their
   tests only: at most ten files, down from 32 on 2026-08-18.
3. **No platform nouns in shared contracts.** `rg -n
   'Macos|Windows|Linux'` over `hypercolor-types/src`,
   `core/src/input/status.rs`, and `core/src/input/traits.rs` finds only
   documentation and bounded opaque diagnostic labels. Typed
   `Macos*Owner` and owner-event vocabularies collapse onto
   `ServiceIdentity`; capture selection names APIs, not operating
   systems; `hypercolor-macos-owner` is target-gated everywhere.
4. **No mirrored GPU vocabulary.** Exactly one definition each of
   `ImportedEffectFrame`, `ImportedFrameFormat`,
   `ImportedFrameTimings`, and `FrameOrigin`; orientation and
   `content_generation` are frame data; per-OS flip tables and the cfg
   re-export are gone.
5. **One engine per capability.** The §2 table is live: one
   `ScreenCaptureAdapter<B>`, one `host_fold`, one supervisor with
   `LauncherPlan` payloads and three tenants, one launchd adapter, one
   session engine with Linux and Windows platform backends, one path
   authority, and one fs-hygiene home.
6. **Conformance is shaped at the seam.** Canonical neutral streams test
   shared folds and adapters. Equivalent raw platform fixtures test each
   producer or normalizer. Platform CI proves native construction and
   teardown without asking a pure producer to consume neutral events.
7. **Adding platform N+1** to any capability requires one seam
   implementation and one composition-root arm, with zero edits to
   another platform's files.
8. **Specs and performance remain contracts.** Specs 71, 72, 73, 74,
   76, and 77 plus designs 25 and 46 change with the code that changes
   their shape. Frame rate, capture cadence, resolution, output rate,
   and benchmark ceilings do not decrease.

### Execution rules

- The plan owner keeps this section and the matching Sibyl epic current.
  Each package has one task with exact files, dependencies, and receipts.
- Implementers work in the canonical worktree lane at
  `~/dev/worktrees/hypercolor/<prefix>/<branch>`. Two to three focused
  edits are followed by the tightest relevant check. Commits are atomic,
  conventional, and carry bodies.
- Packages may develop in parallel only when their implementation files
  do not overlap. Root `Cargo.toml`, `Cargo.lock`, CI, shared specs, and
  shared facade modules integrate serially.
- Every package runs its focused tests and `just verify`. Crate graph,
  cfg budget, platform nouns, fixture counts, and target isolation are
  reviewed at every wave boundary. Platform-specific packages require
  their Linux, Windows, and macOS CI lanes before completion.
- A separate reviewer receives the original package contract, changed
  files, and approach. Findings are adjudicated against code and tests;
  blocker fixes re-enter the same review gate.

### Prerequisite gates

These are dependencies, not duplicate platform-layer packages:

- **Capture gate:** spec 77 H2.3 must finish bounded native stream
  transactions. H6.1 must then finish its stable capture/GPU facade,
  which already depends on H3.5 and the native-only publication chain.
  C1 through C3 do not restructure those files before both tasks land.
- **Launcher gate:** spec 77 H1.5 must finish version-neutral launcher
  metadata. H6.2 must then finish the stable ownership/app/canary
  facades, which already depends on H5.2. L1 and L2 consume those
  facades rather than refactoring them twice.
- **Input-manager gate:** I2 is the `InputManager` conversion named by
  spec 76 §6.3. No separate manager-idiom package may modify
  `core/src/input/` concurrently.

Current status on 2026-08-18: H1.5 and H2.3 are `doing`; H6.1 and H6.2
are `todo` behind their behavior-hardening prerequisites.

### Wave 1: independent foundations

#### F1: Share worker retention

**Files:** new `crates/hypercolor-worker-retention/`,
`crates/hypercolor-core/src/input/worker_retention.rs`,
`crates/hypercolor-windows-input/src/worker_retention.rs`,
`crates/hypercolor-macos-input/src/macos.rs`, workspace manifests.
**Depends on:** none.
**Parallel:** implementation may run beside F2 and F3; workspace
manifest integration is serialized.

Implementation:

- Move the dependency-free queue, singleton reaper, spawn helper, and
  panic reporting into the leaf crate.
- Delete the core and Windows twins. Use one process singleton even when
  multiple input families retain workers.
- Give macOS start/stop failure paths the same bounded retention fallback;
  no Drop or timeout path blocks forever on `JoinHandle::join`.

Verify:

- `cargo test --locked -p hypercolor-worker-retention`
- `cargo test --locked -p hypercolor-windows-input`
- `cargo test --locked -p hypercolor-macos-input`
- One process-level test proves that core, Windows, and macOS clients use
  one reaper singleton.

#### F2: Resolve HAL transport intent once

**Files:** `crates/hypercolor-hal/src/transport.rs`, driver descriptor
tables under `crates/hypercolor-hal/src/drivers/`, transport metadata in
`hypercolor-driver-api`, compatibility fixtures.
**Depends on:** none.
**Parallel:** yes, with F1 and F3 outside shared manifests.

Implementation:

- Replace cfg-selected descriptor transports with platform-free HID
  intent and one `resolve_transport(intent, os)` function owned by HAL.
- Add `UnsupportedPlatform` and expose unavailable transports through
  driver inventory instead of returning an empty scan.
- Delete ASUS's `cfg(not(windows))` assumption and Nollie's three-way
  descriptor split. The four dead network-driver dependencies and graph
  documentation are already complete and are not part of this package.

Verify:

- `cargo test --locked -p hypercolor-hal`
- `cargo test --locked -p hypercolor-driver-api`
- `just compat-check`
- Table tests prove ASUS selects hidraw on Linux, HIDAPI on macOS and
  Windows, and SMBus reports unavailable where no backend exists.

#### F3: Centralize filesystem hygiene and paths

**Files:** `crates/hypercolor-platform-fs/`, driver-api credential
storage, `crates/hypercolor-macos-owner/`,
`crates/hypercolor-core/src/config/paths.rs`, app path consumers, Servo
cache paths, CLI service installation.
**Depends on:** launcher gate before touching macOS owner/install files.
**Parallel:** platform-fs primitives may start beside F1 and F2; consumer
migration waits for the launcher gate.

Implementation:

- Add `durable_replace`, `write_secret`, and `open_no_follow`; Unix
  durability includes parent-directory sync inside the API.
- Migrate driver credentials and macOS owner storage from hand-rolled
  permission, symlink, and replacement code.
- Route app, daemon, Servo cache, and service paths through
  `core/config/paths.rs`. The CLI config-path migration is already done.

Verify:

- `cargo test --locked -p hypercolor-platform-fs`
- `cargo test --locked -p hypercolor-driver-api -p hypercolor-macos-owner`
- Failure-injection tests cover replacement, parent sync, symlinks,
  permissions, and preserved prior content.
- `rg -n 'dirs::|home_dir\(' crates/hypercolor-{app,cli,core,daemon}/src`
  finds only `core/config/paths.rs` and documented non-runtime fixtures.

#### F4: Retire the standalone tray

**Files:** delete `crates/hypercolor-tray/`; update workspace manifests,
CI, release packaging, Homebrew, AUR, specs 61 and 76, and design 25.
**Depends on:** none; design 46's release-overlap gate is satisfied.
**Parallel:** implementation is isolated, but root manifest and CI
integration are serialized with F1 and F5.

Implementation:

- Remove the retired crate and every packaging, signing, CI, and
  documentation reference in one package.
- Keep `hypercolor-app` as the sole tray implementation and document
  `hypercolor-open` as the no-Tauri path.
- Preserve the app's server-identity and host-bound credential behavior;
  no headless fork survives with weaker reconnect semantics.

Verify:

- `rg -n 'hypercolor-tray' Cargo.toml Cargo.lock crates scripts .github docs`
  returns historical statements only.
- `cargo test --locked -p hypercolor-app`
- `just verify`
- Linux, Windows, and macOS release packaging jobs contain no tray binary.

#### F5: Put session backends behind the existing seam

**Files:** new `hypercolor-linux-session` and
`hypercolor-windows-session` crates, workspace manifests,
`core/src/session/`, daemon startup composition,
`daemon/src/windows_service.rs`, app Windows power events.
**Depends on:** none.
**Parallel:** no with other workspace-crate additions during integration.

Implementation:

- Move logind and screensaver implementations out of core into the Linux
  platform crate; keep the neutral engine and `SessionMonitor` trait in
  core.
- Implement the Windows backend with a message-only window in standalone
  mode and SCM power/session controls in service mode.
- Install both at the daemon composition root and delete the app-side
  HTTP discover nudge.

Verify:

- `cargo test --locked -p hypercolor-linux-session`
- `cargo test --locked -p hypercolor-windows-session`
- Core session tests prove dedup and sleep policy are unchanged.
- Windows fixtures prove suspend, resume, lock, and unlock enter the same
  `SessionEvent` stream in standalone and SCM modes.
- No platform module remains under `core/src/session/`.

### Wave 2: neutral input and launcher spines

The input spine is I1 then I2 then I3. The launcher spine is L1 then L2
after the launcher gate. The spines may develop in parallel, but edits to
`hypercolor-types` integrate serially.

#### I1: Make source status neutral

**Files:** `core/src/input/status.rs`, `core/src/input/traits.rs`, macOS
input and capture crates, daemon input-status API and WebSocket mapping,
`hypercolor-types` API types, CLI action text, UI status consumers,
generated SDKs, and the macOS input/capture spec.
**Depends on:** none.
**Parallel:** yes, with L1 until either touches shared type files.

Implementation:

- Keep neutral lifecycle, freshness, issue, remediation, and timing
  summaries in core. Move macOS authorization, Tahoe, selection, owner,
  and architecture types into their platform crates.
- Cross the seam through one versioned opaque diagnostics envelope with
  bounded neutral display fields. Core stores and relays the envelope
  without knowing its payload shape.
- Require each platform crate to construct its typed snapshot, opaque
  payload, presentation values, and privacy redaction. Core may collect
  typed primitives but must not author platform JSON keys or display
  fields.
- Replace `CurrentMacosProcess`, `set_macos_*`, and macOS screenshot
  action types with neutral capability actions implemented at the
  platform boundary.

Verify:

- `rg -n 'Macos|Windows|Linux' crates/hypercolor-core/src/input/status.rs crates/hypercolor-core/src/input/traits.rs`
  finds documentation only.
- Core, macOS input, macOS capture, daemon API, and UI status tests pass.
- Oversized, unknown-version, and malformed diagnostic payloads remain
  bounded and cannot break neutral status delivery.
- OpenAPI carries every representable bound, generated SDK checks are
  clean, and session-scoped capture identifiers never cross the platform
  boundary.
- Diagnostic artifact timeouts remain owned by the requesting consumer;
  no detached adapter worker can outlive a timed-out request.

#### I2: Split `InputSource` roles and convert `InputManager`

**Files:** `core/src/input/traits.rs`, `core/src/input/mod.rs`, source
builders, source tests, spec 76 manager-idiom status.
**Depends on:** I1.
**Parallel:** no with any other `core/src/input/` manager refactor.

Implementation:

- Introduce an object-safe common `ManagedSource` trait, object-safe
  `DataSource`, `AudioSource`, `ScreenSource`, and `InteractionSource`
  role traits, plus exclusive `ManagedSourceRole` storage.
- Give `DataSource` an exhaustive `DataSourceKind` for media, network,
  and sensors, mapped once into scheduling and status `SourceKind` values.
  Sensors must participate in the same graph publication and cadence path;
  the current manager-owned sensor watch is deleted rather than retained as
  a second data plane.
- Replace `is_*`, `CaptureDomain`, erased subtrait methods, and both
  `SourceKind::Interaction` fallbacks with typed registration.
- Collapse the three generation-fenced plan/commit/retire lanes onto one
  generic role swap. Register sensors as `Data` with honest status and
  failure reporting.
- Complete the `InputManager` portion of spec 76 §6.3 in this package,
  including private mutation publication and owned snapshots. This package
  intentionally pulls `InputManager` ahead of the older conversion order in
  spec 76; it does not wait for `SceneManager`, `SpatialEngine`, or
  `EffectRegistry` conversion.

Verify:

- `cargo test --locked -p hypercolor-core --test input_tests --test input_publication_tests --test input_sampling_tests`
- `rg -n 'is_(audio|screen|interaction)_source|enum CaptureDomain|unwrap_or\(SourceKind::Interaction\)' crates/hypercolor-core/src/input`
  returns zero production hits.
- Compile-time fixtures reject multi-role and role-less registration.
- Source replacement tests prove stale prepared generations cannot commit.

#### I3: Extract one host-input fold

**Files:** new neutral host event vocabulary in `hypercolor-types`, new
`hypercolor-linux-input` crate, `core/src/input/host_fold.rs`, Windows input
crate, macOS input crate, interaction-router consumers and tests.
**Depends on:** I2 and F1.
**Parallel:** yes, with L1/L2 after I2 lands.

Implementation:

- Make each backend a pure ordered producer of neutral raw edges, repeat
  evidence, device identity, motion, scroll, topology generation, and
  state gaps.
- Move Linux evdev discovery, device ownership, and raw event acquisition
  out of core and into `hypercolor-linux-input`. Core must not retain a
  target-gated evdev module after the fold lands.
- Move held state, repeat classification, release synthesis, epochs, and
  snapshots into one fold with no mutable platform hooks.
- Unify the interaction consumer catalog now duplicated by pipeline and
  preview routing.

Verify:

- One canonical neutral corpus pins fold snapshots, repeats, gaps,
  synthesized releases, scroll units, and topology changes.
- Equivalent evdev, Raw Input, and CGEvent fixtures normalize to that
  corpus without feeding neutral events back through the backends.
- `cargo test --locked -p hypercolor-core -p hypercolor-windows-input -p hypercolor-macos-input`
- Platform folds and duplicated router catalogs are deleted.

#### L1: Define neutral service identity

**Files:** `hypercolor-types` service and event vocabulary, daemon status
API, app/CLI/UI bridge types, service definitions and packaging fixtures.
**Depends on:** launcher gate.
**Parallel:** yes, with the input spine outside shared type files.

Implementation:

- Add `ServiceIdentity` and `DaemonRunMode`; every systemd unit, SCM
  registration, launchd plist, Homebrew block, and supervised child
  declares identity through the H1.5 metadata channel.
- Corroborate launcher claims through the platform authority before the
  daemon reports them. Self-report identity in the status API.
- Collapse macOS owner enums/events and Windows service bridge mirrors
  onto the neutral vocabulary while preserving H1.5 compatibility rules.

Verify:

- Old launcher/new daemon, new launcher/old daemon, conflicting claims,
  Homebrew, direct launchd, SCM stopped/running, and supervised-child
  fixtures all resolve deterministically.
- `rg -n 'MacosDaemonOwner|MacosCapabilityOwner|WindowsDaemonServiceStatus' crates/hypercolor-types crates/hypercolor-{app,cli,core,daemon,ui}/src`
  finds only platform adapters and compatibility fixtures.
- Status API round trips the same identity used by the launcher plan.

#### L2: Make every launcher a tenant of one supervisor

**Files:** app supervisor and ownership modules, CLI service modules,
daemon startup and signals, `hypercolor-macos-owner`, launchd adapter,
systemd/SCM integration, packaging tests.
**Depends on:** L1 and launcher gate.
**Parallel:** yes, with I3; no other supervisor or owner refactor.

Implementation:

- Generalize the pure plan to payload-bearing `LauncherPlan` arms and an
  explicit owner-preference policy. Add the stopped-SCM Start arm.
- Replace app/CLI stop enums with `StopAuthority`. Route all launchd
  operations through one modern adapter and one agent filename.
- Fill the Linux parent-death and macOS kqueue lifetime guards at the
  composition roots. Retire the daemon and app one-second parent/owner
  polls after kernel-backed lifetime and notify-backed owner changes are
  live.
- Target-gate `hypercolor-macos-owner` in app, daemon, and CLI.

Verify:

- Pure supervisor table tests cover Reuse, Start, SpawnChild, Hold, stop,
  rollback, and owner-preference policy for all three platforms.
- Packaging tests pin one launchd filename and modern verb semantics.
- `rg -n 'from_secs\(1\)|detect_windows_daemon_service|launchctl (load|unload)' crates/hypercolor-{app,cli,daemon}/src`
  finds no retired supervisor path.
- Linux orphan, macOS parent-death, Homebrew, direct launchd, and SCM
  integration fixtures pass.

### Wave 3: capture convergence

C1 through C3 begin only after the capture gate. They form one
coordinated review wave so the macOS facade, GPU vocabulary, Linux native
placement, and shared adapter move once rather than through four serial
refactors.

#### C1: Own GPU frame vocabulary once

**Files:** new `hypercolor-gpu-frame` crate, workspace manifests, all
three GPU interop crates, core effect traits and GPU source mapping,
daemon GPU compositor and telemetry.
**Depends on:** capture gate.
**Parallel:** yes, with C2 outside root manifests and shared screen files.

Implementation:

- Move neutral imported-frame, format, timing, origin, lease-token, and
  fallback-reason contracts into the leaf vocabulary crate.
- Make interop crates produce those types; keep native handles and
  conversions private to each platform crate.
- Rename imported `storage_id` to `content_generation`, preserve true
  allocation identity separately where needed, and delete cfg-selected
  re-exports, telemetry maps, and orientation flip tables.

Verify:

- `rg -n 'struct ImportedEffectFrame|enum ImportedFrameFormat|struct ImportedFrameTimings' crates`
  finds one definition of each.
- `cargo test --locked -p hypercolor-gpu-frame -p hypercolor-linux-gpu-interop -p hypercolor-windows-gpu-interop -p hypercolor-macos-gpu-interop`
- Cross-platform fixtures prove optional timing phases, orientation, and
  generation semantics without native handles in the shared crate.

#### C2: Quarantine PipeWire native code

**Files:** new `hypercolor-pipewire-interop` crate, workspace manifests,
Wayland screen backend, spec 74, Linux capture tests.
**Depends on:** capture gate.
**Parallel:** yes, with C1 outside manifests.

Implementation:

- Move PipeWire, portal, SPA metadata, DMA-BUF identity, and callback
  plumbing out of `core/src/input/screen/wayland.rs`.
- Keep pooling, publication planning, demand, and resource admission in
  neutral core; the platform crate owns native identity and transport.
- Follow the audited real/stub module convention and preserve portal
  session lifetime across demand changes.

Verify:

- `cargo test --locked -p hypercolor-pipewire-interop`
- Linux screen fixture tests cover portal cancellation, renegotiation,
  transforms, crop, DMA-BUF, and callback shutdown.
- `rg -n 'pipewire|spa_|ashpd|dmabuf' crates/hypercolor-core/src/input/screen`
  finds seam vocabulary and documentation only.

#### C3: Extract `ScreenCaptureAdapter<B>`

**Files:** `core/src/input/screen/`, platform capture crates, daemon screen
bridges, `hypercolor-types` capture configuration, specs 72, 73, and 74.
**Depends on:** C1, C2, I2, and capture gate.
**Parallel:** no; this owns the shared capture surface.

Implementation:

- Move owned-source ledgers, exact-runtime bind/reap, worker commands,
  preparation/retirement, publication slots, settings revisions, and
  generic generation fencing into one statically dispatched adapter.
- Reduce Wayland, Windows, and macOS modules to `CaptureBackend`
  implementations plus native-crate calls. Preserve H2.3 cancellation,
  deadlines, off-main waits, late-callback fencing, and nonblocking Drop
  behind the macOS facade.
- Replace platform-named capture configuration with API-named backend
  identity; target-gate core's platform input/capture dependencies.
- Publish consumer, implementer, and planner facades instead of the flat
  screen namespace.

Verify:

- One fake-backend suite proves prepare/commit/retire, rollback,
  cancellation, stale-generation rejection, latest-value delivery, and
  nonblocking Drop.
- Equivalent platform fixtures prove the backends produce the same
  neutral publication decisions while retaining native ownership.
- `cargo test --locked -p hypercolor-core --test screen_publication_contract_tests --test screen_publication_plan_tests --test screen_worker_ledger_tests --test screen_tests`
- Linux, Windows, Apple Silicon, and Intel macOS capture CI lanes pass
  with nonzero fixture counts.
- The cfg budget and unconditional platform-dependency checks decrease
  to their goal-state limits.

#### C4: Delete the compatibility screen mirror

**Files:** exact-screen consumers in core, daemon WebSocket preview and
zones, UI/SDK adapters, legacy scheduler and demand union, spec 73.
**Depends on:** C3 and spec 73 wave 4's exact-consumer migration.
**Parallel:** no with capture or preview work.

Implementation:

- Move interactive preview, WebSocket canvas/zones, renderers, and every
  remaining consumer to exact leases.
- Delete the compatibility mirror branch, component-wise demand union,
  and single screen schedule in the same package.
- Retain fixture-only CPU references where parity tests require them;
  production macOS CPU publication stays deleted by H3.5.

Verify:

- Spec 73 wave 4 and T19 fixtures pass for native, Canvas2D, WebGL,
  LightScript, preview, zones, repeat-key, and aspect behavior.
- `rg -n 'compatibility.*screen|legacy.*screen|ScreenData' crates sdk`
  finds explicit fixture/reference uses only.
- 1080p, 4K, 8K, portrait, ultrawide, mixed-cadence, and rapid-resize
  capture tests preserve the published performance contracts.

### Wave 4: capability extensions

Wave 4 does not block the core platform-layer exit. Each package reuses
the established seam and must not reopen shared architecture.

#### E1: Decide macOS system-audio capture

**Files:** a focused amendment to this design and the audio/capture spec.
**Depends on:** I2 and C3.
**Parallel:** yes, with E3 and E4.

Implementation:

- Compare ScreenCaptureKit audio, a virtual device, and any supported
  Core Audio process/system tap against entitlement, distribution,
  latency, sample-format, and fallback requirements.
- Select one production path with primary-source evidence and hardware
  probes. Microphone substitution is rejected explicitly.
- Record the crate boundary and executable tests that E2 must satisfy.

Verify:

- The amended spec names one selected mechanism, rejected alternatives,
  required entitlements, distribution support, and measured latency.
- A native probe proves the selected API can capture system audio in the
  signed production topology before E2 starts.

#### E2: Put audio capture behind its seam

**Files:** core audio engine, new Linux/macOS audio platform crates as
selected by E1, workspace manifests, source composition, audio tests.
**Depends on:** E1 and I2.
**Parallel:** no with other audio work.

Implementation:

- Keep the RT ring, analysis, recovery, and prepared reconfiguration in
  core. Move Pulse and macOS native stream ownership behind
  `AudioCaptureBackend` push sessions.
- Report input, loopback, device identity, and sample-format capability
  as data. No backend guesses or silently changes source class.

Verify:

- RT allocation and lock tests remain green.
- Linux microphone/monitor and macOS system-audio fixtures prove source
  identity, restart, disconnect, and unsupported-loopback behavior.
- `rg -n 'pulse|coreaudio|screen.*audio' crates/hypercolor-core/src/input/audio`
  finds seam vocabulary and documentation only.

#### E3: Add the macOS media provider

**Files:** new `hypercolor-macos-media` crate, workspace manifests, core
media factory arm, media fixtures and status tests.
**Depends on:** I2.
**Parallel:** yes, with E1 and E4.

Implementation:

- Implement `MediaMetadataProvider` without adding cfg-shaped variants to
  shared artwork or session types.

Verify:

- Existing provider-session tests plus macOS fixtures prove connect,
  metadata change, artwork replacement, disconnect, unsupported
  capability, and stale-session recovery.
- Shared artwork and session enums have no cfg-shaped fields or variants.

#### E4: Add the macOS session monitor

**Files:** new `hypercolor-macos-session` crate, workspace manifests,
daemon composition, session fixtures and spec 77 H7.3 status.
**Depends on:** F5.
**Parallel:** yes, with E1 and E3.

Implementation:

- Produce neutral sleep, wake, lock, and unlock events from
  NSWorkspace/IOKit behind `SessionMonitor`.

Verify:

- The same dedup and sleep-policy fixtures pass for Linux, Windows, and
  macOS.
- Native Apple Silicon and Intel construction/teardown tests execute with
  nonzero counts.

### Wave gates and final verification

Every wave closes with:

- `just verify`
- `just ui-test` when API or UI contracts changed
- `just compat-check` when driver metadata changed
- `cargo metadata --no-deps --format-version 1` inspected for neutral to
  platform dependency edges
- the goal-state cfg, platform-noun, and vocabulary searches above
- Linux, Windows, Apple Silicon, and Intel macOS CI receipts with nonzero
  test counts for touched platform crates
- an independent adversarial review of the wave diff

Macros are not scheduled work. After C4, inspect the residue. Add
`platform_modules!`, conformance stamping, or `#[derive(SourceStatus)]`
only when at least three mechanically identical sites remain and the
expansion can be shown concretely. Otherwise the simplest correct result
is no macro.

---

## 6. Review history

**Round 1 (2026-08-15, Codex `exec` at xhigh reasoning, read-only):**
NEEDS_CHANGES. Seven findings, all adopted after independent
verification; fact-check scorecard on the document's ten most
load-bearing claims: 7 confirmed, 3 wrong, 0 stale. The three wrong
claims (a spec 76/77 ownership contradiction that does not exist, Linux
`storage_id` mischaracterized as storage identity, "identical method
sets" overstating the publication-struct triplication) are corrected in
this revision; the structural corrections (fold owns all semantics with
no policy hooks, role-storage object model, status-boundary decision,
rule-4 narrowing, contract sketches, dependency-ordered waves,
`content_generation`) are folded into §2 and §5. The dispute with doc
71's flock-collapse remedy is recorded in §3 Supervision finding 3 and
was routed to the PR #158 lane at the time.

**Round 2 (2026-08-18, current-tree revalidation):** APPROVE DIRECTION,
REPLACE EXECUTION PLAN. PR #158 merged at `60463f6d`, but its final tree
did not deliver the old phase-0 status seam, role storage, kernel lifetime
guard, launcher convergence, or TCC-canary relocation. Spec 76 already
delivered the four dead driver-dependency removals and the CLI path fix.
The cfg baseline is 32 files rather than 43. The implementation plan now
consumes active H1.5 and H2.3 plus queued H6.1 and H6.2 as explicit gates,
places session backends in platform crates, splits fold tests from backend
normalizer tests, and assigns every shared platform-noun migration. The
macOS merge strengthens the architecture: native lifecycle and transaction
facades stay backend-side while the neutral adapter consumes them. An
independent Claude review did not start because the CLI account reached its
monthly spend limit, so this round records no second-model verdict.
