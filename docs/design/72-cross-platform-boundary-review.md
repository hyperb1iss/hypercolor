# Cross-Platform Boundary Review: The Platform Layer

**Reviewed:** 2026-08-15, main `28edd518` plus the PR #158 worktree
`nova/macos-capture-input` `b881c795`.
**Method:** seven parallel read-only boundary reviews (screen capture, host
input, GPU interop, audio/media/sensor sources, process supervision and
privilege, driver/HAL, desktop scaffolding), each comparing code on both
trees against the governing specs, synthesized here, then hardened by one
cross-model review round (Codex, xhigh; see Review History). Companion to
`docs/design/71-macos-capture-input-pr158-review.md`, the bug-level review
of PR #158; this document is the abstraction-level review of every
platform boundary.

**Verdict: the boundaries are half-built, and the same half everywhere.**
The platform-neutral edges of each pipeline (vocabulary types, delivery
machinery, trait seams facing the engine) are genuinely shared, mostly
excellent, and platform-free. The middle layer between "OS API fires" and
"neutral type published" is hand-mirrored per platform: written once per
OS, kept in sync by convention, doc comments, and review rather than by
the compiler. macOS is the third mirror, and PR #158 shows exactly what
that costs: the branch could not add a platform without editing the other
platforms' files, and it grew several new sibling copies of code that
already existed twice.

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
   branch's `macos-capture`/`macos-input`) follow one layout convention:
   unsafe quarantined, `undocumented_unsafe_blocks` denied, stubs so every
   platform compiles everywhere. This layer is healthy.

Layer 2 is where "do we have the right abstractions" fails today.
Receipts, one per boundary:

- **Screen capture:** `ExactPublicationShared` exists three times with a
  shared core of functionally identical methods plus per-platform extras
  already accreting (`wayland.rs:533-645` adds session-scoped clearing,
  `windows.rs:390-505` adds descriptor allocation, branch
  `macos.rs:611-692` adds compute-policy helpers); the
  `begin_screen_publication_preparation` and retirement bodies are
  verbatim modulo the platform name in error strings. Each platform
  module is 4,400 to 5,800 lines, of which roughly 1.5k to 2k is cloned
  protocol-client scaffolding. Storage drift has begun too: macOS uses
  `Vec` where the others use `ExactBoxList`.
- **Host input:** spec 71 D1 promised "per-platform `HostInputBackend`
  implementations that are pure event producers; all state folding lives
  in one shared, platform-independent module"
  (`docs/specs/71-interactive-input-pipeline.md:78-79`).
  `HostInputBackend` appears in zero Rust files. Specs 72 and 76 quietly
  re-planned the fold as a hand-mirrored "sibling of evdev.rs", and today
  the ~300-line fold state machine (SharedState latch,
  snapshot/generation, release synthesis, epoch rotation, even an
  identical explanatory comment at `evdev.rs:495` and `windows.rs:623`)
  exists three times. No test asserts the three folds produce identical
  snapshots for equivalent event streams.
- **GPU interop:** three structurally identical `ImportedEffectFrame`
  definitions (`linux.rs:181`, `macos.rs:202`, `windows.rs:233`) unified
  by cfg-selected re-export in `core/src/effect/traits.rs:22-33`. Spec 58
  §5.1 says "mirror the macOS crate exactly", making the mirror a review
  obligation rather than a compiler guarantee, and it has already
  drifted: `ImportedFrameTimings` fields differ per platform (forcing
  per-OS telemetry mapping in core), and `storage_id` is a content
  generation wearing a storage-identity name: Linux mints a fresh value
  per issued import even when the slot texture is reused
  (`slot_pool.rs:373,403`), and core conflates identity with generation
  at `gpu/source.rs:260`.
- **Supervision:** main's supervisor is one neutral state machine with
  two platform seams (launcher plan, lifetime guard). The macOS branch
  built a second philosophy beside it (ownership recovery that can
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
  branch first-classed mode awareness for one platform only, minting
  four parallel `Macos*Owner` enums plus a string vocabulary for one
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
  discover-nudge (`app/src/power_events/windows_impl.rs:174-183`), so
  suspend fade/release policy is dead on Windows, dead in headless SCM
  mode (whose handler declines `PowerEvent`,
  `daemon/src/windows_service.rs:54-63`), and absent on macOS on both
  trees, despite spec 77 H7.3 requiring sleep/wake recovery.
- **Filesystem and paths:** `hypercolor-platform-fs` is a crate for one
  function whose unix arm hands its own job back to the caller ("Unix
  callers remain responsible for syncing the parent directory",
  `lib.rs:16-21`), and no caller does. Atomic-write/symlink/permission
  hygiene is now hand-rolled in three places (platform-fs, driver-api's
  `#[cfg(unix)]` 0600 credential store, the branch's
  `hypercolor-macos-owner`). Path resolution has a canonical module
  (`core/src/config/paths.rs`) and bypassers: the CLI hand-rolls
  `config_path()` with a literal `PathBuf::from("~/.config")` fallback
  that Rust never tilde-expands (`cli/src/config/mod.rs:86`), and app
  first-run, diagnostics, and the Servo cache all call `dirs::`
  directly.
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

- **Platform nouns are leaking into neutral vocabulary.** The branch adds
  `MacosProtectedSourceState`, `MacosTahoeCapabilities`,
  `set_macos_daemon_ownership`, and `set_macos_metal4_capability` to
  shared `status.rs`, `InputManager`, and `InputSource` itself;
  `hypercolor-macos-owner` is an unconditional dependency of app,
  daemon, and CLI; the shared event vocabulary goes from zero platform
  nouns on main to 14 `Macos` mentions. Main is not clean either:
  `CaptureConfigPersistenceUpdate` has platform-cfg'd enum variants
  (`daemon/src/startup/services.rs:1082-1099`), and media's
  `ArtworkSource::WindowsSession` carries a cfg-shaped native variant in
  a shared enum (`media.rs:115,134`). The correct precedent exists in
  the same files: `InteractionDegradation` keeps failure modes neutral
  by design (`traits.rs:188-206`).
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
   variant (the branch's `NativeScreenCacheLease` generalization is the
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
| Service identity | `DaemonRunMode`/`ServiceIdentity` in `hypercolor-types`, declared by the launcher (generalize spec 77 H1.5's env channel to all platforms), self-reported by the daemon in its status API | each service definition (systemd unit, SCM registration, launchd plist, brew block) declares its identity on its own ExecStart line | mode knowledge scattered across seven sites, four parallel `Macos*Owner` enums on the branch |
| Session/power | `SessionEvent` + dedup forwarder + `SleepPolicy` + controller (all exist, all neutral) | `SessionMonitor` (exists): Linux logind/screensaver shipped; Windows and macOS monitors to be added daemon-side | engine live on one platform; Windows built as an app-side HTTP nudge instead; macOS absent |
| HID transport | one hal-owned `resolve_transport(intent, os)` | descriptors declare HID *intent* once, platform-free; the resolver picks hidraw/hidapi/interrupt-claim | cfg'd const fns in nine device tables |
| Filesystem + paths | `hypercolor-platform-fs` grown into the audited hygiene home: `durable_replace` that syncs the parent itself, secret-write with the 0600/owner discipline, symlink-refusing open; `core/config/paths.rs` as the single path authority every binary imports | the Windows FFI module it already has; unix arms implemented once, correctly | one-function crate that delegates its own durability contract to callers who drop it; CLI and app bypass the path authority |

### The enabling trait split

`InputSource` (`core/src/input/traits.rs:470-830`) is a role union: six
universal data-plane methods plus ~29 role-specific defaulted ones (the
exact screen-publication protocol alone is 12 methods that exactly one
type per platform implements), with `is_audio_source` and friends
re-deriving what the type system already knew, and a wrong-by-default
`unwrap_or(SourceKind::Interaction)` at `input/mod.rs:350`.

The split needs an object model, not just intent, because registration
erases sources into `Box<dyn InputSource>` today (`input/mod.rs:326`)
and subtrait methods are unreachable after erasure. The shape:

```rust
/// Roles are exclusive: a source registers as exactly one variant.
/// Every trait here is object-safe, Send, and NOT Sync (the manager
/// owns sources behind its existing lock; control methods take &mut).
enum ManagedSourceRole {
    Audio(Box<dyn AudioSource>),        // DataSource + audio control plane
    Screen(Box<dyn ScreenSource>),      // DataSource + screen publication protocol
    Interaction(Box<dyn InteractionSource>), // DataSource + capture toggles
    Data(Box<dyn DataSource>),          // sample/status only (media, net, sensors)
}
```

`DataSource` carries name/start/stop/sample-and-drain/status; each role
trait extends it with that role's control plane (supertrait, so the data
plane is reachable from every variant without downcasts). Kind is the
variant, declared at registration; `CaptureDomain` and the `is_*` flag
inference disappear; the manager's three parallel plan/commit/retire
lanes collapse onto one generic generation-fenced swap keyed by role.

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
  (`supervisor/mod.rs:455-461`) survives; macOS becomes a tenant.
- **The GPU vocabulary crate** owns only neutral wgpu-facing types. No
  native handles, no per-target features: native format conversions and
  target-specific capabilities stay in the interop crates so workspace
  feature unification cannot leak platform deps into neutral consumers.
- **One launchd adapter.** All launchd interaction (app executor, CLI
  service verbs) goes through one module speaking modern
  `bootstrap/bootout/kickstart` verbs, with enable/disable distinct from
  start/stop and one shared agent-filename constant (today the branch
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
2. **Conformance-suite stamping**: `conformance_tests!(EvdevBackend,
   WindowsBackend, MacosBackend)` feeding one canonical event stream
   through every backend and asserting identical fold snapshots. This is
   how the parity guarantee stays mechanical as platforms are added.
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
   PR #158 minted a third copy of it (`macos.rs:2482`). The legacy
   path's CPU+sRGB requirement is what forces GPU-native backends to
   keep a readback lane alive.
3. The 228-name (248 on the branch) flat `screen/mod.rs` namespace mixes
   consumer, implementor, and planner vocabularies; three curated
   facades would fix the presentation without touching the machinery.
4. Platform code placement differs per platform (Linux fully in core,
   Windows crate-heavy, macOS crate plus core pooling); spec 74 wave 1's
   `hypercolor-pipewire-interop` aligns Linux and should state the
   placement rule.

### Host input

1. The fold is §1's finding; the branch's scroll work is the cost proof
   (one shared projector took edits in evdev.rs +251, windows.rs +92,
   browser.rs +244). Backends become pure event producers; the fold owns
   all semantics.
2. `worker_retention` has a verbatim twin in `hypercolor-windows-input`
   (two reaper singleton threads run on Windows) and the macOS input
   crate mints a third policy (plain bounded join). One dependency-free
   crate fixes the direction problem.
3. The interaction router's consumer catalog is built twice
   (`pipeline_runtime.rs:460-497` vs `interactive_preview.rs:1274-1305`)
   and the copies have already diverged on revision tracking.
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
   importer drops it and the compositor re-derives it from two cfg truth
   tables (`gpu/source.rs:240-250, 278-288`).
3. Both screen-bridge impls live inline in shared
   `sparkleflinger/gpu.rs` (main: 45 platform markers; branch: 122).
   Move to `gpu/screen_native/{windows,macos}.rs` with the shared
   scaffolding factored once.
4. `NativeScreenCacheLease` on all platforms kills the cfg'd
   fields/params and the branch's three-clause cfg union.
5. The GL ring/fence choreography exists three times inside the interop
   crates; tolerable (inside the quarantine), track for a shared
   skeleton.
6. HYPOTHESIS: the `OnceLock` one-shot Servo device install has no
   device-loss renegotiation path.

### Audio, media, sensors

1. Promote media's provider idiom to the named pattern; macOS media is a
   self-contained provider plus one factory arm, not the dead-code cfg
   patches the branch currently carries.
2. Sensors become a normal source under `ManagedSourceRole::Data`, plus
   `SourceKind::Sensors`; today a panicked sensor thread leaves a
   permanently stale snapshot with no health surface.
3. Audio's Pulse capture moves behind `AudioCaptureBackend`.
4. CONFIRMED path, impact hypothesis: on stock macOS the SystemMonitor
   device selection falls through to `default_input_device()`, the
   microphone, and reports healthy (`audio/mod.rs:1684-1687`). PR #158
   ships this as behavior since no macOS audio backend exists on the
   branch. The backend contract's loopback-capability rule exists to
   make this state unrepresentable.
5. `NetSource` samples inline under the manager lock; a second exception
   to "workers publish, sample reads". Don't copy it.

### Supervision and privilege

1. The branch's ownership layer must become a tenant of the launcher
   seam (CONFIRMED: `LauncherPlan`-shaped logic exists but macOS wraps
   and can suppress the supervisor, `supervisor/mod.rs:1152-1176`;
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
   (`VerifiedConnection { session_id, credential }`); the four parallel
   owner enums collapse onto `ServiceIdentity`.
5. Relocate the TCC canary (3,529 lines inside `hypercolor-daemon`) to
   a standalone harness crate; keep it as the release gate spec 76
   demands. Daemon `main.rs` returns to platform-clean (251 lines on
   main, 1,352 on the branch with arbitration inline).
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
   (CONFIRMED bypasses): the CLI's hand-rolled `config_path()` with its
   never-expanded `~/.config` fallback joins core's resolver; app
   first-run/diagnostics and the Servo cache route through it; macOS
   behavior gets documented instead of implied by the not-linux branch.
3. The portable-lock helper in `scripts/cargo-cache-build.sh` is build
   infrastructure, not runtime; out of the layer's scope.

### Driver/HAL

1. Transport intent resolver in hal (§2 table); ASUS's macOS hole is
   the proof case, Nollie's three-way split is the per-driver workaround
   that shouldn't have to exist.
2. Delete the four dead `hypercolor-core` deps from hue/wled/govee/
   nanoleaf manifests (verified zero source usage; contradicts the
   stated layering rule).
3. Add an `UnsupportedPlatform` transport error variant and
   platform-aware transports in `DriverModuleDescriptor`, so a macOS UI
   can say "SMBus: not available on macOS" instead of an eternally
   empty scan.
4. HYPOTHESIS (needs hardware): nusb plain `claim_interface` likely
   fails on macOS for HID-class interfaces Apple's driver holds; the
   fix is the same hidapi routing the resolver provides.
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

## 5. Sequencing 🦋

Ordered by dependency, not just by wave; the cross-model round showed
the original ordering hid prerequisites. PR #158 is in flight and owned
by its own lane; the extractions get *cheaper* after it lands (the
branch aligned the fold and capture surfaces), so nothing below races
it.

**Wave 0, on PR #158 itself (in addition to doc 71's blockers):**

1. Define the neutral seams the strip depends on FIRST: the rule-2
   status envelope and the `ManagedSourceRole` storage shape (types
   only, no engine work).
2. Then strip `Macos*` from shared vocabulary (status, events,
   `InputSource` verbs, unconditional deps) onto those seams.
3. Fill the unix lifetime guard (pdeathsig / `EVFILT_PROC`).
4. Route ownership through the launcher seam: `LauncherPlan` payloads +
   owner-preference policy + unified `StopAuthority`; the 1 Hz verify
   poll retires in favor of the notify watch.
5. Relocate the TCC canary; daemon `main.rs` returns to platform-clean.
6. No macOS media/audio cfg patches in shared files (implement a
   provider or leave the seam alone).
7. The doc-71 flock-file collapse is withdrawn as a wave item (see
   §3 Supervision finding 3); any artifact consolidation needs its own
   file-protocol design reviewed on that lane.

**Wave 1, immediately after merge (the platform layer proper), in
dependency order:**

1. Shared `worker-retention` crate (leaf, unblocks everything).
2. `InputSource` role split onto `ManagedSourceRole` + typed
   registration (the wave-0 types grow their engines).
3. GPU vocabulary crate with `origin` and `content_generation` as data;
   screen bridges move out of `gpu.rs`; `NativeScreenCacheLease`
   everywhere.
4. `hypercolor-pipewire-interop` (spec 74 wave 1), aligning Linux with
   the crate-per-platform shape.
5. `host_fold` extraction with the conformance-suite macro (same
   canonical stream through all backends, identical snapshots
   asserted).
6. `ScreenCaptureAdapter<B>` extraction, keeping the temporary legacy
   screen mirror above `CaptureBackend` so spec 73 wave 4's later
   deletion does not force a second backend-contract rewrite.
7. `ServiceIdentity`/`DaemonRunMode` vocabulary + launcher declaration
   + daemon self-report; the mode probes and the four owner enums
   retire.

**Wave 2, scheduled, independent:**

spec 73 wave 4 (delete the legacy screen mirror path); hal transport
resolver plus `UnsupportedPlatform`; sensors as a source; audio backend
seam plus a macOS loopback decision; media provider for macOS; router
catalog unification; session/power monitors for Windows and macOS
daemon-side, app nudge retired; tray deletion per design 46 with specs
61/76 and packaging updated in the same change; one launchd adapter;
platform-fs growth and the paths.rs consolidation; spec updates
recording the fold and vocabulary decisions so the specs stop ratifying
the old shape.

**Anytime, trivial:** four dead manifest deps; mermaid edge; stale
module docs; duplicate `CaptureDomain` name; test-only sampler gating;
the CLI `~/.config` literal.

The test of success is mechanical: after wave 1, adding a platform to
any capability means one seam impl, one composition-root arm, zero edits
to any other platform's files, and `grep -r 'cfg(target_os'
crates/hypercolor-core/src crates/hypercolor-daemon/src` returns only
composition roots. That grep currently hits 43 files; it should end
under ten.

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
routed to the PR #158 lane.
