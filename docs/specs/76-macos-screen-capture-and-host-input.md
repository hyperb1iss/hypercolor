# 76 - macOS Screen Capture and Host Input

**Status:** Implementation-ready, revision 27; GPU-only amendment approved
**Author:** Nova
**Date:** 2026-08-10
**Platform floor:** macOS 15.2 Sequoia
**Build SDK:** macOS 26 Tahoe or newer
**Architectures:** Apple Silicon and Intel
**New crates:** `hypercolor-macos-input`, `hypercolor-macos-capture`
**Changed crates:** `hypercolor-core`, `hypercolor-daemon`,
`hypercolor-macos-gpu-interop`, `hypercolor-app`, `hypercolor-types`,
`hypercolor-windows-input`, `hypercolor-leptos-ext`, `hypercolor-cli`,
`hypercolor-ui`, `sdk/packages/core`
**Depends on:** specs 14, 57, 71, 72, and 73
**Hardening companion:** spec 77
**Supersedes:** the unimplemented macOS portions of specs 14 and 71, plus the
temporary macOS `device_query` bridge retained by spec 72

## 1. Mission

Give Hypercolor production-grade screen capture, keyboard input, and pointer
input on macOS without creating a second input pipeline or reducing the product
ceiling.

The completed platform path is:

```text
CGEventTap
  -> hypercolor-macos-input
  -> canonical InteractionData and InteractionBatch
  -> existing routing, privacy, WebSocket, SDK, and effect contracts

ScreenCaptureKit
  -> retained CVPixelBuffer and IOSurface
  -> hypercolor-macos-capture
  -> exact screen publication plan
  -> Metal texture import
  -> SparkleFlinger and spatial reduction
```

The implementation must feel native to Sequoia, exploit useful Tahoe
capabilities, and preserve one coherent cross-platform contract. macOS is a
producer and execution target for the architecture established by specs 71 and 73. It is not a reason to fork those contracts.

## 2. Product policy

### 2.1 Deployment and SDK policy

Hypercolor raises its macOS deployment target from 11.0 to 15.2.

The exact 15.2 floor is deliberate. Sequoia 15.0 provides the ScreenCaptureKit
HDR stream presets and dynamic-range selection. Sequoia 15.2 adds stream active
and inactive callbacks, content-filter introspection, and the display-space
screenshot API. Those lifecycle callbacks remove guesswork from source health
and make 15.2 the clean minimum for the complete design.

Every macOS artifact is built against the macOS 26 SDK. Runtime availability
checks protect Tahoe-only calls. No weak-linking maze preserves Big Sur through
Sonoma, and no compatibility helper keeps the old 11.0 product floor alive.

The supported matrix is:

| Host                                   | Support level                       | Capture range | GPU path                                              |
| -------------------------------------- | ----------------------------------- | ------------- | ----------------------------------------------------- |
| Apple Silicon, macOS 15.2 through 15.x | first class                         | SDR and HDR   | IOSurface and Metal                                   |
| Intel, macOS 15.2 through 15.x         | first class                         | SDR           | IOSurface and Metal                                   |
| Apple Silicon, macOS 26+               | first class plus Tahoe capabilities | SDR and HDR   | Metal, with benchmark-gated Metal 4 work when exposed |
| Intel, macOS 26+                       | first class plus Tahoe capabilities | SDR           | IOSurface and Metal                                   |
| macOS 15.0 or 15.1                     | unsupported                         | none          | none                                                  |
| macOS 14 and earlier                   | unsupported                         | none          | none                                                  |

Apple documents ScreenCaptureKit HDR capture as Apple Silicon only. Intel
Sequoia remains a supported SDR target instead of silently receiving an
ineffective HDR configuration.

### 2.2 Tahoe capability policy

Tahoe support is a runtime capability set, not a separate backend:

```rust
pub struct MacosTahoeCapabilities {
    pub host_architecture: MacosArchitecture,
    pub translated_process: bool,
    pub content_tone_mapping_info: bool,
    pub metal4: bool,
}

pub struct MacosTahoeSelectionCapabilities {
    pub source_id: MacosScreenSourceId,
    pub capture_session_generation: u64,
    pub hdr_capture: bool,
    pub dual_range_screenshots: bool,
}

pub enum MacosArchitecture {
    AppleSilicon,
    Intel,
}
```

The host record is stable for one process and active Metal device.
`host_architecture` describes native host hardware, not the executable slice.
An x86_64 process under Rosetta 2 reports `AppleSilicon` with
`translated_process: true`; a native process reports `false`. Resolution uses
the native host architecture and `sysctl.proc_translated`, then records the
running slice separately in diagnostics. The section 2.1 support rows use host
architecture, while storage selection and Metal 4 use active `MTLDevice` family
probes. The remaining booleans are runtime API and active-hardware probes, not
inferences from the OS major. `content_tone_mapping_info` requires the callable
Tahoe Core Graphics API. `metal4` is true only when the active `MTLDevice`
exposes every Metal 4 facility used by the prototype.

Selection capabilities are `None` before a source is selected and until its
first complete frame confirms the configured and delivered dynamic range. The
record is then published with the exact source identity and capture-session
generation. `hdr_capture` describes that selected source and delivered stream.
`dual_range_screenshots` additionally requires the Tahoe screenshot API for the
same filter. Repick or stream replacement creates a new record; a record whose
source or session generation does not match the active stream is diagnostic
history only and cannot select behavior.

Tahoe diagnostics resolve from the host and current selection records:

- An HDR-capable selected source must supply paired SDR and HDR screenshots from
  `SCScreenshotConfiguration`, plus `CGContentToneMappingInfo` reference output.
- An SDR-only Tahoe selection, including Intel, supplies one SDR screenshot with
  `CGContentToneMappingInfo`. It reports HDR and paired range as unsupported and
  never relabels an SDR image as HDR.
- A Tahoe host or selection missing an expected capability reports the failed
  runtime probe as a platform defect. It does not silently select a weaker
  diagnostic.

Neither API replaces `SCStream` for continuous capture. The live path remains
ScreenCaptureKit streaming because Tahoe does not introduce a better continuous
acquisition primitive.

Metal 4 evaluation is required on every active device whose runtime probe
exposes the required facilities. The evaluation builds a direct Metal 4
capture-reduction prototype using command allocators and residency sets, then
compares it with the existing wgpu Metal path on the same fixtures and hardware.
Metal 4 is not an Intel Tahoe acceptance requirement when the active device does
not expose it. The Metal 4 path ships only when it preserves exact output parity
and improves a named production metric by at least 10 percent at p95. Qualifying
metrics are capture-to-publication latency, CPU time, GPU reduction time, or
retained bytes. An architecture fork that does not clear that bar buys
maintenance without capacity and does not ship.

## 3. Verified baseline

### 3.1 Host input today

The daemon currently constructs `InteractionInput` on macOS. That bridge polls
`device_query` every 10 milliseconds and has confirmed contract gaps:

- it reports no Input Monitoring authorization state;
- it has no physical key code or macOS keymap;
- it derives press and release edges from snapshots and loses native repeat;
- it publishes no pointer button or wheel events;
- it leaves pointer mode unset and normalized coordinates at zero;
- it captures keyboard and pointer state together when either consent toggle is
  enabled; and
- it cannot report event-tap disable, session interruption, or revocation.

The bridge is the last macOS consumer of `device_query`. This spec deletes the
dependency and the bridge after the native source passes parity.

### 3.2 Screen capture today

The shared capture vocabulary already contains:

- `ScreenCaptureBackend::MacosScreenCaptureKit`;
- `PlatformGpuApi::Metal`;
- `ScreenPhysicalGpuDeviceIdentity::MetalRegistryId`;
- owner-backed opaque platform GPU surfaces;
- exact descriptor-keyed publication plans;
- source, topology, session, resource, and plan generations;
- byte and compute admission;
- explicit geometry, colorimetry, dynamic range, and cursor policy; and
- capture-source reselection hooks.

macOS still resolves to `CapturePlatform::Unsupported`, and daemon startup
constructs no macOS screen source or native execution target. SparkleFlinger's
screen target preparer is currently wired only for Windows D3D11.

The existing `hypercolor-macos-gpu-interop` crate proves the audited IOSurface
to Metal to wgpu import boundary for Servo frames on Apple-family devices. Its
current descriptor hardcodes `MTLStorageModeShared` in
`src/macos.rs::metal_texture_descriptor`, so Intel is not proven by the existing
path. Screen capture extends that crate through a feature, following the Windows
capture and GPU interop split, and W4 makes storage selection family-aware for
both importers. The implementation must not duplicate the importer in core.

### 3.3 Packaging and privacy today

The desktop app bundles `hypercolor-daemon` as an external sidecar. Native input
and capture sources currently open inside the daemon process. macOS Transparency,
Consent, and Control grants are attached to a signed code identity, so the final
owner of Input Monitoring and Screen Recording cannot be chosen from source
layout alone.

The current app metadata also describes keyboard input with
`NSAppleEventsUsageDescription`. Apple Events permission controls automation of
other applications. It does not authorize `CGEventTap` listening. The key is
wrong unless Hypercolor separately sends Apple Events.

The first implementation wave therefore proves TCC ownership in a signed
package before placing irreversible weight on either process topology.

## 4. Goals and non-goals

### 4.1 Goals

The design delivers:

1. Native, event-driven keyboard and pointer capture through a passive session
   event tap.
2. Independent keyboard and pointer consent and event masks.
3. ScreenCaptureKit display, window, application, and multi-window selection
   through Apple's system picker.
4. Exact native acquisition with descriptor-keyed derived publications.
5. A zero-full-frame-copy IOSurface and Metal production path with a
   fixture-only CPU correctness oracle.
6. SDR correctness on every supported Mac and HDR capture on supported Apple
   Silicon.
7. Explicit TCC state, remediation, revocation, and source health.
8. Signed packaging, macOS pull-request CI, diagnostics, and physical
   acceptance.
9. Tahoe dual-range diagnostics, content-aware tone mapping, and a measured
   Metal 4 decision.

### 4.2 Non-goals

The first complete release does not:

- capture system audio or microphone audio through ScreenCaptureKit;
- synthesize or inject keyboard or pointer events into macOS;
- claim per-device identity from `CGEventTap`;
- bypass the system content-sharing picker with a custom picker;
- capture the login window, lock screen, secure input, or another user session;
- support macOS 15.1 or earlier;
- serialize private `SCContentFilter` objects as restore tokens;
- expose raw screen frames or raw host events to network clients without the
  existing consent and routing gates; or
- force Metal 4 into production without a measured win.

Per-device keyboard and pointer identity would require an `IOHIDManager` path.
That is a separate product feature because it changes permissions, hotplug,
device identity, and event arbitration. The session source in this spec uses
the stable identity `macos:session`.

## 5. Non-negotiable invariants

1. Consent and demand remain separate. Permission can be granted while the
   native tap or stream is closed.
2. A system prompt appears only after an explicit user action. Restored config,
   daemon startup, and background effect demand may preflight but never prompt.
3. Keyboard and pointer capture honor independent booleans all the way to the
   `CGEventMask`. Disabling one kind makes those events invisible to Hypercolor.
4. The event tap is listen-only. Hypercolor never suppresses, alters, or
   reinjects a host event.
5. Capture acquisition preserves the selected source's native pixel ceiling.
   Consumer extents remain exact independent branches.
6. No implementation adds a fixed resolution, FPS, refresh-rate, queue, or
   architecture ceiling to hide a bottleneck.
7. Every width, height, stride, plane length, queue slot, and derived
   publication is checked and admitted before allocation. Framework-owned
   IOSurface pools use the two-phase reservation and reconciliation contract in
   section 11.1 because ScreenCaptureKit chooses their exact allocation size.
8. A byte claim lives exactly as long as the backing memory or imported resource
   it accounts for. Replacing a plan does not release pinned generations early.
9. The ScreenCaptureKit callback validates, retains, publishes latest value, and
   returns. It performs no scaling, color conversion, reduction, encoding, or
   blocking daemon work.
10. The render thread samples immutable latest-value state in constant time. It
    never calls AppKit, Core Graphics permission APIs, or ScreenCaptureKit.
11. Source, topology, capture session, resource, and plan generations stay
    distinct. A stale frame cannot enter a newer source or publication epoch.
12. Pixel geometry and color are explicit. Retina scale, content rect, screen
    origin, pixel format, color space, transfer function, dynamic range, and
    cursor composition never travel as assumptions.
13. HDR is converted through an explicit scene-referred working path and tone
    mapped for LED output. Clipping extended values to `[0, 1]` is a defect.
14. One broken native source degrades that source. It does not crash the daemon
    or roll back an unrelated input source.
15. Source teardown emits synthetic releases and clears held state before a new
    generation can publish.
16. The packaged app sidecar, direct launchd daemon service installed by
    `hypercolor service enable`, Homebrew service installed by
    `brew services start hypercolor`, and terminal-launched standalone daemon
    are separate TCC topologies. Diagnostics and remediation name the exact
    owner; the UI never claims that granting one code identity grants another.
17. Production macOS screen capture is GPU-only. Missing or failed Metal
    capability invalidates stale output, rebuilds native execution, and fails
    closed without selecting a CPU capture, conversion, publication, reduction,
    or compositor path.

## 6. Process topology and TCC canary

### 6.1 Preferred topology

The preferred topology keeps native sources in the daemon:

```text
Hypercolor.app
  -> supervises signed hypercolor-daemon sidecar
       -> owns CGEventTap
       -> owns SCStream
       -> publishes input and retained IOSurfaces in process
```

This path has the smallest latency and simplest lifetime model. The canary must
prove that the sidecar's stable designated requirement receives durable TCC
grants across app relaunch, daemon restart, and signed application update.

### 6.2 Canary matrix

Wave 0 produces a minimal signed package using the production bundle identifier,
sidecar embedding, signing shape, hardened runtime, and release launch path. It
tests keyboard listening, pointer listening, picker presentation, and streaming
as four independently scored capabilities without landing production
integration.

Every canary row and TCC persistence claim uses a Developer ID Application
signature with stable identifiers, timestamped hardened-runtime signatures,
and accepted Apple notarization. Ad-hoc builds may exercise pure fixtures and
native mechanics, but their changing code-directory hashes are explicitly out
of scope for grant persistence, update survival, designated-requirement checks,
and signed acceptance.

The matrix covers:

- a fresh TCC database;
- grant, deny, later grant, revoke while live, and grant after revocation;
- grant while the TCC-owning process remains live, with preflight and resource
  creation checked before and after an owner restart;
- app launch, supervised daemon restart, full app relaunch, and signed update;
- direct launchd daemon installation, login start, service restart, and signed
  binary update under the `tech.hyperbliss.hypercolor` label;
- Homebrew installation, `brew services` login start and restart, and signed
  binary update under the `homebrew.mxcl.hypercolor` label;
- the packaged app and direct launchd service installed together in both enable
  orders, with deterministic owner arbitration across repeated logins;
- the app, direct launchd service, and Homebrew service installed in every pair
  and all together, with one selected owner across repeated logins;
- standalone daemon launch from the terminal;
- System Settings identity and displayed process name;
- system picker presentation and stream creation in the same process;
- keyboard, pointer, and screen capture enabled independently; and
- Apple Silicon and Intel on Sequoia 15.2 and Tahoe 26.

Each row records the responsible audit token, bundle identifier, executable
path, code-signing designated requirement, prompt text, System Settings entry,
and resulting API state.

Each capability keeps the preferred daemon topology only if:

1. The packaged sidecar receives stable grants under a recognizable Hypercolor
   identity.
2. Grants survive relaunch and a normally signed update.
3. Revocation is observable without process restart.
4. Any picker-created `SCContentFilter` remains in the process that owns its
   `SCStream`; filters never cross an IPC boundary or become restore tokens.
5. Standalone behavior is explicit and does not poison the packaged grant.
6. The direct launchd service either receives stable grants under its own
   designated requirement or delegates each protected capability to the
   authenticated app broker. It never borrows Terminal or app authorization.
7. The Homebrew service receives stable grants only for capabilities its own
   signed canary passes. It has no implicit app-broker delegation. A broker path
   would require a distinct verified reverse-bootstrap service in the generated
   Homebrew plist; until then, a failed Homebrew capability directs the user to
   select the packaged app owner.

The picker and stream criterion is a hard macOS constraint, not a canary
preference. If a headless sidecar cannot present the system picker, the app owns
both picker and stream. The daemon may still own keyboard and pointer taps when
their own rows pass. A screen failure never moves input ownership, and an input
failure never moves screen ownership.

The canary is a hard architecture gate. Spec implementation may proceed on pure
types and fixtures while it runs, but each native capability's process owner is
not finalized until its evidence exists.

At most one daemon topology may own protected capabilities in one user session.
The existing `SingleInstance` guard in `hypercolor-daemon/src/main.rs` remains
the final process arbiter. macOS augments it with a mode-0600 per-user owner
record next to the guard. The winning daemon records its owner variant, audit
token identity, executable path, designated-requirement hash, process ID, and
epoch. A losing app sidecar, direct launchd service, or terminal process writes
a typed `macos_daemon_owner_conflict` contender record instead of silently
succeeding. A launchd contender exits zero so its `KeepAlive` rule with
`SuccessfulExit = false` does not respawn it. A sidecar exits with the typed
nonzero owner-conflict code, which the app supervisor classifies as terminal and
never feeds into its watchdog restart loop. A terminal contender returns the
same nonzero code to its caller.

The winning daemon starts the native record watch before constructing the input
graph, regardless of input or capture configuration. It publishes the active
owner and conflict on the daemon system-status surface, mirrors the conflict in
any constructed `SourcePlatformStatus`, and emits one ownership bus event. It
coalesces an identical active owner, active epoch, contender owner, executable,
and designated-requirement tuple until either ownership or contender identity
changes. Repeated identical writes cannot create another state transition or
bus event. The record is diagnostic only and cannot override the guard or
authorize a peer.

The UI and CLI name the active owner and offer `choose_daemon_owner`, which
enables one autostart topology and disables every other installed daemon
autostart transactionally, including `brew services` when present. A login race
can affect startup order but never the selected owner, published state, or
remedy.

The transaction coordinator is the surviving local app or CLI process, never
the daemon being replaced. It validates the selected launcher and builds a
versioned handover journal containing transaction ID, requested and prior owner,
prior autostart states, allowed rollback operations, phase, active and contender
epochs, and any pending standalone PID. The mode-0600 journal is a separate file
beside the owner record. Before the first mutation, the coordinator writes the
journal with atomic replacement, file `fsync`, and parent-directory `fsync`.
Every completed phase is persisted the same way.

A dedicated, stable coordination lock file serializes both artifacts. Every
winning daemon, contender, coordinator, and recovery path takes its exclusive
lock for one owner-record or journal read-modify-write, releases it immediately
after the durable replacement, and reacquires it for the next write. No path
holds the lock across a transaction phase, process stop or start, guard wait,
supervisor operation, or incoming-daemon recovery. Locking the replaceable owner
record or journal inode is forbidden because atomic replacement would detach the
lock from later writers.

For app-sidecar, direct-launchd, and Homebrew incumbents, the coordinator
disables nonselected autostarts, flushes and stops the outgoing daemon, waits at
most 10 seconds for the single-instance guard to release, then starts the
selected topology. Guard-release or startup timeout restores the previous
autostart configuration and prior owner from the durable journal.

A terminal-launched incumbent has no supervisor or service manager and never
terminates itself. The coordinator returns the typed `stop_standalone_owner`
remedy with the authoritative active PID and asks the user to stop that terminal
process with Ctrl-C or `kill -TERM`. No autostart mutation occurs yet. The
coordinator waits through the guard's native notification for up to 60 seconds;
handover remains pending while the standalone owner is live, continues after
the guard frees, and returns the same pending remedy on timeout. The pending
intent remains in the journal, so the next local coordinator invocation resumes
it rather than asking the user to choose again.

External-owner mode is a persisted app setting. When launchd or Homebrew is the
selected daemon owner, app startup suppresses sidecar creation and connects its
UI to the external daemon on `:9420`. An unavailable selected owner produces an
offline-owner state and never silently spawns the sidecar. Only a later
`choose_daemon_owner` selecting `AppSidecar`, or an explicit owner-preference
reset, clears external-owner mode.

The incoming daemon emits `MacosDaemonOwnershipChanged` after it acquires the
guard and publishes its owner epoch. The app or CLI coordinator returns the
handover success or failure synchronously. WebSocket clients reconnect and read
`SystemStatus.macos_daemon_ownership` as the authoritative outcome. When
rollback restarts the prior owner, that daemon emits the restored ownership
event after reacquiring the guard.

Recovery reads and advances the separate journal under the shared coordination
lock, releasing the lock before it executes the recovered operation. The next
app or CLI coordinator completes or reverses any nonterminal phase before
accepting a new choice. An incoming daemon also runs a pre-runtime recovery
phase before binding network sockets or constructing sources. It may only
execute the typed, path-free operations already present in the validated
journal. If it is the requested owner and holds the guard, it completes and
commits the handover. If it is the prior owner after rollback, it records
rollback completion. Any other owner leaves the journal pending and publishes
recovery-required status. No startup path accepts an arbitrary executable or
command from the record.

### 6.3 Broker fallback

If a capability fails its preferred-topology criteria, an app-bundled broker
owns only that capability while the daemon keeps all generic semantics. The
screen broker always owns picker and stream together:

```text
tech.hyperbliss.hypercolor.capture-broker LaunchAgent
  -> owns SCContentSharingPicker and its SCStream
  -> optionally owns keyboard and/or pointer CGEventTap when their canary rows require it
  -> accepts authenticated local XPC connections from the app and daemon
  -> transfers plain input envelopes and IOSurface XPC objects

hypercolor-daemon sidecar
  -> validates broker epoch and sequence
  -> imports IOSurface into the existing publication plan
```

The fallback is designed now so the canary can select it without a second
architecture exercise:

- The broker protocol is versioned and contains no core or AppKit types.
- Hypercolor bundles
  `Contents/Library/LaunchAgents/tech.hyperbliss.hypercolor.capture-broker.plist`
  and registers it with `SMAppService.agent(plistName:)` only when the canary
  selects broker ownership. The Aqua-session LaunchAgent runs the signed app
  executable in broker mode and advertises the
  `tech.hyperbliss.hypercolor.capture-broker` Mach service.
- The broker owns `NSXPCListener(machServiceName:)`. The app UI and daemon use
  `NSXPCConnection(machServiceName:)`; no anonymous endpoint crosses a file
  descriptor or command line.
- The listener accepts only the same user and Hypercolor's signed designated
  requirement, checked from the connection audit token and Foundation's code
  signing requirement support.
- A supervised sidecar receives a random session capability from the app over
  an inherited descriptor after the app sends the same capability to the broker
  over authenticated XPC. The daemon must prove it in its first broker message.
- A direct launchd daemon cannot inherit from the app. When broker delegation is
  selected, its LaunchAgent declares the one-operation Mach service
  `tech.hyperbliss.hypercolor.daemon-bootstrap`. The daemon owns an
  `NSXPCListener` for that service. The broker connects through launchd, and
  both peers verify same-user audit tokens and the exact opposite executable's
  designated requirement. The broker generates a fresh random capability,
  sends it over that mutually authenticated reverse connection, and binds it to
  the daemon epoch. The daemon must present it on its first connection to the
  broker. Successful proof closes the bootstrap listener for that epoch.
  Daemon restart rotates the capability, and a stale daemon cannot reuse an
  earlier proof.
- Broker start always runs the reverse bootstrap before opening protected
  channels. Broker connection loss or broker epoch advance invalidates the old
  capability and makes the daemon reopen its bootstrap listener without
  changing daemon epoch. A restarted broker completes mutual verification,
  supplies a new capability bound to its broker epoch and the existing daemon
  epoch, and closes that listener only for the lifetime of the new broker
  connection. The broker-only restart remedy therefore restores service without
  restarting the daemon, while every in-flight message from the old broker
  epoch remains fenced.
- Neither bootstrap puts a capability in arguments, environment variables, or
  files. If the launchd daemon starts before the broker, protected sources stay
  in `NeedsUserAction` until an authenticated broker completes the reverse
  bootstrap.
- `IOSurfaceCreateXPCObject` transfers an owning reference without making the
  surface globally discoverable. The daemon reconstructs it with
  `IOSurfaceLookupFromXPCObject` and releases the XPC object after taking its own
  retained reference.
- Every message carries broker epoch, capture session generation, sequence, and
  exact descriptor. Reconnect advances the broker epoch and fences all old
  messages.
- Input messages use a bounded ordered ring. Screen frames use keyed
  latest-value replacement. Neither channel can grow without bound.
- Backpressure drops superseded screen frames. It never blocks the
  ScreenCaptureKit callback or reorders discrete input events.
- Connection loss stops only the capabilities owned by the broker. It publishes
  synthetic releases for a brokered input kind, invalidates brokered screen
  freshness, and preserves healthy in-process capabilities.

The broker exists only for capabilities whose signed canary proves it
necessary. W0 must prove that the registered LaunchAgent receives a recognizable
TCC identity and can present the picker in the active Aqua session. There is no
runtime option that lets two processes compete for the same capability.

## 7. Permission and lifecycle model

### 7.1 Protected resources

| Capability                           | TCC service                       | Preflight and request                                        | Metadata                                                                 |
| ------------------------------------ | --------------------------------- | ------------------------------------------------------------ | ------------------------------------------------------------------------ |
| Keyboard listening                   | Listen Event / Input Monitoring   | `CGPreflightListenEventAccess`, `CGRequestListenEventAccess` | no Apple Events key                                                      |
| Pointer listening                    | none for passive mouse events     | event-tap construction and health                            | none                                                                     |
| Screen frames and source enumeration | Screen Capture / Screen Recording | ScreenCaptureKit access and system picker                    | `NSScreenCaptureUsageDescription`                                        |
| Apple application automation         | Apple Events                      | not used by this design                                      | remove `NSAppleEventsUsageDescription` unless another feature proves use |

Apple's ScreenCaptureKit framework overview explicitly directs macOS apps to
add `NSScreenCaptureUsageDescription` with the reason screen recording is
needed. Section 23 cites that requirement directly; the app metadata test is a
platform requirement, not an inferred prompt customization.

Core Graphics may create a tap while silently clearing unauthorized keyboard
bits from its mask. Hypercolor therefore never infers keyboard authorization
from successful tap creation. Keyboard preflight, keyboard tap validation, and
pointer tap health are separate observations.

`NSMicrophoneUsageDescription` remains because audio-reactive effects use the
microphone through the audio input stack. ScreenCaptureKit explicitly sets
system audio and microphone capture to false.

Hypercolor's app, sidecar, standalone daemon, and broker are hardened-runtime
code but are not App-Sandboxed. Passive `CGEventTap` listening and the plain
launchd Mach service names in section 6 depend on that premise. Adding
`com.apple.security.app-sandbox` is an architecture change requiring a new input
and broker design, not a packaging hardening toggle.

### 7.2 Lifecycle states

The generic `SourceStatus` remains the external contract. Each macOS adapter
also owns a more precise internal state machine:

```rust
pub enum MacosProtectedSourceState {
    Disabled,
    NeedsUserAction,
    PermissionDenied,
    NeedsProcessRestart,
    NeedsSelection,
    ReadyIdle,
    Starting,
    Live,
    Interrupted,
    Revoked,
    Failed,
}
```

The macOS status payload publishes one state for keyboard, one for pointer, and
one for screen. Pointer uses the same vocabulary for lifecycle consistency but
never reports permission states. The generic combined interaction
`SourceStatus` is a deterministic rollup: a demanded live kind keeps the source
live, a demanded failed kind remains visible in per-kind details, and no kind's
authorization is inferred from another kind.

Transitions follow these rules:

- Enabling config performs a non-prompting preflight and publishes the result.
- An explicit UI or CLI `authorize` action may call the request API.
- A newly granted right that the active process cannot consume enters
  `NeedsProcessRestart`; it never loops tap or stream creation.
- An explicit `pick source` action presents Apple's picker.
- Consumer demand starts a ready source but never triggers a prompt or picker.
- Zero demand closes the tap or stream and returns to `ReadyIdle`.
- Revocation while live stops publication immediately and enters `Revoked`.
- A transient ScreenCaptureKit interruption enters `Interrupted` and attempts a
  bounded stateful restart only while demand remains active.
- A source that needs a new selection enters `NeedsSelection`; it does not fall
  back to a different display silently.

`NeedsProcessRestart` requires positive authorization evidence and a conflicting
resource result. Keyboard enters it only when `CGRequestListenEventAccess` or a
fresh `CGPreflightListenEventAccess` reports granted but a newly created
keyboard tap still lacks its requested key bits or fails with a permission
classification. Screen enters it only when the system picker has delivered a
filter or shareable-content enumeration succeeds, but a fresh stream fails with
a permission classification. A denied request with no positive evidence stays
`PermissionDenied`. W0 records these predicates before and after owner restart
on Sequoia and Tahoe so OS-specific behavior becomes a fixture, not folklore.

The supervisor restart action is explicit and scoped to the TCC-owning process.
For an in-process sidecar capability, the app stops and relaunches only the
daemon after its current state is flushed. For an app-owned capability, the UI
offers a full app relaunch. For a direct launchd daemon owner, the UI and CLI
offer `hypercolor service restart`, which unloads and reloads only the
`tech.hyperbliss.hypercolor` user agent after state is flushed. A direct launchd
daemon delegated to the app broker restarts only that broker. For a Homebrew
service owner, the UI and CLI offer `brew services restart hypercolor`, which
targets only `homebrew.mxcl.hypercolor`. Terminal-launched standalone mode
reports the exact command-level remediation and does not terminate itself.

If the canary cannot prove stable grants for the direct launchd daemon, service
mode may use a registered and authenticated app broker for protected sources.
When no qualifying broker is installed or active, the source publishes
`NeedsUserAction` with an `app_broker_required` remedy. It never prompts under
the launchd identity and then instructs the user to grant a different process.

Retries are event-driven by permission changes, picker callbacks, topology
notifications, stream delegate callbacks, configuration changes, or explicit
user action. There is no browser polling loop and no background prompt loop.

### 7.3 Source selection and persistence

Apple's system picker is authoritative. The app enables only the modes the
request supports and excludes Hypercolor's own windows where appropriate.

The `capture.source` grammar on macOS is:

```text
auto
primary_display
display:<canonical-display-uuid>
session_scoped
```

The display UUID is the canonical string produced from
`CGDisplayCreateUUIDFromDisplayID`; the numeric `CGDirectDisplayID` is a runtime
lookup value and is never persisted as identity. `auto` resolves through the
existing policy, while `primary_display` follows the current main display. A
missing persisted display UUID enters `NeedsSelection` rather than selecting a
different display. Window, application, and multi-window choices persist only
as `session_scoped` plus a redacted diagnostic label and enter
`NeedsSelection` after relaunch.

The validator accepts only this grammar. The resolver owns display UUID lookup
and picker session state. Hypercolor does not archive `SCContentFilter`,
`SCWindow`, or private framework state.

Picker cancellation preserves the current stream when repicking. Cancellation
with no current source leaves `NeedsSelection`. Picker failure publishes the
native error domain and code through structured remediation.

## 8. Native host input

### 8.1 Crate boundary

The new `hypercolor-macos-input` crate owns:

- Core Graphics permission functions;
- event-tap creation and teardown;
- the dedicated `CFRunLoop` thread;
- native event decoding;
- virtual desktop geometry snapshots; and
- native interruption and failure classification.

The crate has `unsafe_code = "allow"`, denies undocumented unsafe blocks, and
uses macOS-only modules plus cross-platform stubs. Its public API exposes plain
Rust values and no Core Foundation pointers. Pure key mapping and event folding
compile and test on every host.

`hypercolor-core` owns canonical held state, event ordering, recent-key policy,
motion aggregates, source generations, synthetic releases, and status mapping.
The dependency runs from core to the platform crate, never the reverse.

### 8.2 Native event vocabulary

```rust
pub struct MacosInputConfig {
    pub keyboard: bool,
    pub pointer: bool,
    pub epoch: u64,
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

pub enum MacosInputEvent {
    Key {
        virtual_keycode: u16,
        pressed: bool,
        autorepeat: bool,
    },
    ModifierFlags {
        virtual_keycode: u16,
        flags: MacosModifierFlags,
    },
    Button {
        button: MacosPointerButton,
        pressed: bool,
    },
    Motion {
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
    },
    Wheel {
        fixed_delta_x: i64,
        fixed_delta_y: i64,
        unit: MacosScrollUnit,
        phase: MacosScrollPhase,
        momentum_phase: MacosScrollPhase,
    },
    MediaKey {
        nx_key_type: u16,
        pressed: bool,
        repeat: bool,
    },
    StateGap {
        reason: MacosInputGapReason,
    },
}

pub struct MacosInputBatch<'a> {
    pub epoch: u64,
    pub at_ms: u64,
    pub events: &'a [MacosInputEvent],
    pub virtual_desktop: MacosVirtualDesktop,
}
```

The interop crate stamps the batch immediately before draining its bounded
queue by calling core's injected monotonic clock. The sink folds the whole
batch under one canonical interaction lock so held state and discrete edges
cannot describe different instants.

### 8.3 Event tap

The source creates separate keyboard and pointer session event taps with
`kCGEventTapOptionListenOnly` on one dedicated run-loop thread. Separate taps
are required because Core Graphics can silently remove unauthorized keyboard
bits from a combined mask while leaving pointer bits active. Each callback
performs only fixed-cost field reads and a non-blocking bounded enqueue.

The keyboard mask includes:

- key down;
- key up; and
- flags changed; and
- system-defined events required for media keys.

The pointer mask includes:

- moved and every dragged variant;
- left, right, and other button down and up; and
- scroll wheel.

The two masks are constructed from the two config booleans. A keyboard-only
source creates no pointer tap. A pointer-only source creates no keyboard tap,
does not request Input Monitoring, and may enter `Live` while keyboard state is
`PermissionDenied`.

The tap callback recognizes timeout and user-input disable notifications. It
publishes `StateGap`, clears canonical held state, reenables the tap once, and
records a counter. Repeated disable inside a rolling health window degrades the
source instead of spinning. Teardown signals the run loop, removes the source,
invalidates the tap, joins the worker, and only then advances the session
generation.

### 8.4 Keyboard semantics

`CGKeyCode` is treated as the physical location code for the active Apple
keyboard family. Logical characters and the active keyboard layout never drive
the canonical physical inventory.

`keymap.rs` gains a macOS virtual-keycode column beside Linux evdev and Windows
scan codes. `MEDIA_KEYS` gains a macOS `NX_KEYTYPE_*` column beside Linux evdev
and Windows virtual-key codes. Total inventory tests prove that every canonical
physical and media key maps either to a macOS code or to an explicit unsupported
entry. Left and right modifiers remain distinct.

macOS media keys arrive as `NX_SYSDEFINED` event type 14, subtype 8, with their
key type, press state, and repeat bit packed in the native data fields. The
decoder accepts only subtype 8, validates the packed fields, and routes the
result through the shared media inventory. Other system-defined events remain
counted diagnostics and never become guessed keys.

Key down uses the native autorepeat field:

- first down becomes `Pressed`;
- autorepeat down becomes `Repeated` without reentering held or recent state;
- key up becomes `Released`; and
- an impossible up or repeat is preserved as a diagnostic counter while
  canonical state remains consistent.

Modifier keys arrive through `flagsChanged`, whose event shape does not directly
name press or release. Core derives the edge from the specific key's mask and
its per-key held state, not from the aggregate flags alone. Caps Lock receives a
dedicated fixture because it is a locking modifier rather than an ordinary held
key.

Secure input and secure desktop transitions may create missing edges. Any tap
disable, permission loss, session lock, worker exit, or source stop emits one
ordered `StateGap`, which synthesizes releases for every held key and button.

### 8.5 Pointer semantics

Core Graphics supplies global display-space coordinates. The backend snapshots
the union of active display bounds, including negative origins, and publishes:

- raw signed global coordinates;
- normalized coordinates across the current virtual desktop;
- native deltas;
- accumulated distance; and
- velocity through the existing frame delta contract.

Display reconfiguration advances a pointer-topology generation and resets the
motion baseline. The first event in a new topology establishes position without
manufacturing a large delta.

Button numbers map into the canonical pointer vocabulary with left, right,
middle, and stable numbered extras.

Scroll decoding reads `kCGScrollWheelEventIsContinuous`, both 16.16 fixed-point
axis fields, both point-delta fields, scroll phase, and momentum phase. The
fixed-point values are authoritative; point deltas are retained as diagnostic
cross-checks. A non-continuous event arrives as 16.16 notches. Core multiplies
that signed fixed-point value by 120 with checked arithmetic to produce Q16.16
`Line120` units, where one integral unit is exactly 1/120 notch. The Q16.16
representation preserves every signed fractional movement directly.

A continuous event has pixel units because macOS defines no universal
pixels-per-notch conversion. The canonical input vocabulary uses a two-axis
`PointerScroll` event and `ScrollAggregate` with explicit `Line120` or `Pixels`
units, scroll phase, and momentum phase. Effects consume exact horizontal,
trackpad, phase, and momentum data without a guessed scale. Coalescing adds
only like units and preserves phase boundaries.

## 9. ScreenCaptureKit acquisition

### 9.1 Target crate boundary

The GPU-only contract in this section is the target state. The current
implementation retains one bounded production CPU fallback until H3.5 native
GPU execution lands. Spec 77 records that temporary deviation under
"Non-negotiable invariants"; no new code may depend on it.

The new `hypercolor-macos-capture` crate owns:

- ScreenCaptureKit classes, protocols, and delegate callbacks;
- Core Media and Core Video sample validation;
- retained `CVPixelBuffer` ownership;
- IOSurface extraction and identity;
- stream configuration and lifecycle;
- display and content-filter topology;
- screen permission classification; and
- pure cross-platform fixtures for metadata and state transitions.

The crate follows the same audit posture as the other platform capture crates.
It exposes no Objective-C object in its public contract. The native owner is an
opaque `Arc` whose production-safe operations are metadata inspection and
handoff to the macOS GPU interop crate. Fixture-gated tests may map retained
storage for parity oracles, but production types expose no CPU mapping path.

`hypercolor-macos-gpu-interop` first moves its existing Servo-only dependencies
and module behind a `servo-context` feature. It then gains an independent
`screen-capture` feature that depends on the capture crate and exposes a
core-agnostic `MacosScreenBridge`. The bridge imports and validates native Metal
resources but names no core trait or type. The capture crate never depends on
wgpu or core.

`hypercolor-core`'s `servo-gpu-import` feature gains
`hypercolor-macos-gpu-interop?/servo-context`, mirroring its Linux and Windows
feature edges, so macOS Servo imports keep compiling after the split.

The daemon owns a local `MacosScreenTargetPreparer` wrapper around that bridge
and implements core's `ScreenNativeTargetPreparer` for the wrapper. The
dependency edges are exact:

- core depends unconditionally on the capture crate;
- macOS GPU interop stays optional in core and is enabled there only by
  `servo-gpu-import`;
- the daemon's `screen-capture` feature depends on core and macOS GPU interop;
- the interop crate depends on capture only through its own `screen-capture`
  feature; and
- no interop crate depends on core.

### 9.2 Platform frame vocabulary

```rust
pub struct MacosCaptureFrame {
    pub epoch: u64,
    pub sequence: u64,
    pub display_time: u64,
    pub storage_extent: MacosPixelExtent,
    pub planes: Arc<[MacosCapturePlane]>,
    pub pixel_format: MacosCapturePixelFormat,
    pub color: MacosCaptureColorimetry,
    pub geometry: MacosCaptureGeometry,
    pub damage: Arc<[MacosPixelRect]>,
    pub cursor_composed: bool,
    pub surface: MacosCaptureSurface,
}

pub struct MacosCapturePlane {
    pub index: u32,
    pub extent: MacosPixelExtent,
    pub bytes_per_row: usize,
    pub length_bytes: u64,
}

pub struct MacosCaptureSurface {
    pub iosurface_id: u32,
    pub allocation_bytes: u64,
    owner: Arc<MacosRetainedPixelBuffer>,
}

pub enum MacosCapturePixelFormat {
    Bgra8,
    Argb2101010,
    Rgba16Float,
    Yuv420VideoRange,
    Yuv420FullRange,
    Yuv44410BiPlanar,
}
```

The actual retained type keeps the `CVPixelBuffer` alive. An IOSurface pointer
is derived only while that owner is live. Retaining only a borrowed pointer from
the callback is forbidden.

The callback copies small attachment values into Rust storage and retains the
pixel buffer before returning. It never keeps the full `CMSampleBuffer` merely
for convenience.

### 9.3 Stream configuration

The system picker produces the content filter. Hypercolor then configures one
video-only stream:

- `capturesAudio = false`;
- `captureMicrophone = false`;
- `captureResolution = Best`;
- width and height equal the selected source's resolved native pixel extent;
- `sourceRect` is expressed in content points and `destinationRect` is
  expressed in output pixels;
- `preservesAspectRatio = true`;
- `scalesToFit = false` for native display capture;
- `minimumFrameInterval` reflects negotiated acquisition cadence, with zero
  allowed when a native-refresh consumer explicitly requests it;
- `showsCursor` follows the resolved cursor policy;
- `showMouseClicks = false`;
- the stream name identifies Hypercolor; and
- queue depth is admitted as native in-flight memory.

The native display ceiling is calculated with checked arithmetic as
`ceil(contentRect.width * pointPixelScale)` by
`ceil(contentRect.height * pointPixelScale)`. Window, application, and
multi-window selections use the same point-to-pixel rule over their resolved
content bounds. The configuration and every delivered frame validate scale,
content scale, source points, destination pixels, and resulting storage extent
as separate units.

ScreenCaptureKit advises that queue depth should not exceed eight. Hypercolor
uses the framework's full default depth of eight and pre-admits its
conservative residency bound. It does not silently shrink the queue to fit a
machine. Failure to reserve the depth returns a typed resource error without
lowering extent or cadence. A future explicit queue control must remain visible
in configuration, status, and benchmark dimensions.

The stream requests one native acquisition for the resolved source epoch.
Every exact logical branch resolves independently against that frame. An
ultrawide and a portrait branch never create a component-wise maximum surface.
Equal resolved physical work may share after equality is proven.

### 9.4 Frame validation and metadata

The sample callback accepts only screen output with:

- a valid and ready `CMSampleBuffer`;
- a complete `SCFrameStatus`;
- a `CVPixelBuffer` image buffer;
- a supported pixel format;
- checked nonzero storage extent, plane count, plane extent, stride, and
  length;
- an IOSurface-backed pixel buffer for the native path; and
- well-formed ScreenCaptureKit attachment dictionaries.

The adapter maps these attachment keys into canonical metadata:

- display time;
- display scale factor;
- content scale;
- content rect;
- dirty rects;
- screen rect; and
- bounding rect for multi-window content.

ScreenCaptureKit content and bounding rect attachments are in logical points.
Dirty rects are already in pixels. Storage and destination extents are also in
pixels. The adapter validates both delivered scale attachments, converts point
rects with the display scale factor, applies outward rounding for coverage,
clips only after conversion, and rejects a frame whose converted storage-local
bounds exceed its plane storage. Content and bounding rect fixtures include
fractional Retina origins so a point value can never be mistaken for a pixel
value. Content scale remains explicit geometry metadata describing how the
original content was scaled into the surface; applying it again during point to
pixel conversion would double-scale the frame.

`Idle`, `Blank`, `Suspended`, `Started`, and `Stopped` frames update lifecycle
telemetry but do not masquerade as complete image data. A malformed present
attachment drops the frame with a per-reason counter. A documented optional
attachment may be absent and maps to an explicit unknown or full-frame value.

The adapter derives:

- stable source identity from the picker result and resolved display set;
- topology generation from content style, display membership, physical origin,
  logical rect, scale, and native extent;
- capture session generation from each `SCStream` instance;
- resource generation from storage descriptor changes; and
- frame sequence from complete frames only.

For window, application, and multi-window filters, the 15.2 active and inactive
delegate callbacks drive selected-content liveness. Inactive means every
selected window is closed or otherwise unavailable; active marks its return. A
display filter records these callbacks as telemetry only and stays `Live`
unless frame delivery, display topology, or `didStopWithError` proves a real
loss. `didStopWithError` classifies permission, source disappearance,
interruption, and backend failure into structured status.

### 9.5 Cursor policy

ScreenCaptureKit can compose or omit the cursor, but it does not provide the
clean separate cursor-shape contract used by Windows Desktop Duplication.

The macOS source advertises `composed_or_hidden` cursor capability:

- include policy sets `showsCursor = true` and marks the frame composed;
- exclude policy sets `showsCursor = false`; and
- a consumer requiring a clean separate cursor rejects the source as
  incompatible.

The pointer input stream is not used to reconstruct a screen cursor. Its timing,
shape, visibility, hotspot, and secure-input behavior are not equivalent.

### 9.6 Topology and recovery

Display changes, Spaces changes, window closure, application exit, sleep,
wake, and source repicking are control-plane events.

The source follows these rules:

- A display mode, scale, rotation, origin, or membership change advances
  topology generation and transactionally replans exact branches.
- A storage format or stride change advances resource generation.
- A new stream advances capture session generation.
- Repicking preserves the old stream until the new filter, configuration,
  admission, and first complete frame succeed.
- Window or application disappearance enters `NeedsSelection` after the 15.2
  inactive signal confirms no selected content remains.
- Sleep and session lock stop or suspend delivery, clear freshness, and resume
  only after the protected session becomes active.
- Every callback checks epoch before publication, so a late frame from a stopped
  stream is dropped.

Recovery is bounded by state transitions and native notifications. Repeated
blind timer restart is forbidden.

## 10. Target fixture-only CPU correctness oracle

The contract below becomes current state when H3.5 removes the temporary CPU
fallback recorded in Spec 77. Until then it remains the acceptance target, not
a description of every production path in this branch.

The CPU implementation is a bring-up and parity oracle compiled only for tests
with the macOS capture fixture feature. It is not an `InputSource`, publication
executor, runtime fallback, diagnostic recovery mode, or production feature.
Production macOS types cannot request, construct, or select it.

The oracle locks a retained fixture `CVPixelBuffer` read-only, validates every
plane, stride, extent, and length, and converts into fixture-owned output. It
never publishes into the render thread or participates in source lifecycle.
Fixture generations still fence asynchronous test work so stale conversions
cannot make a parity assertion pass against the wrong source.

Supported oracle inputs are:

- `BGRA` BGRA8 SDR;
- `l10r` ARGB2101010 HDR;
- `RGhA` RGBA16Float HDR;
- `420v` two-plane video-range YUV 4:2:0;
- `420f` two-plane full-range YUV 4:2:0; and
- `xf44` two-plane 10-bit YUV 4:4:4.

Each YUV frame carries the delivered matrix, range, transfer function,
primaries, and chroma siting. Missing metadata is an unsupported descriptor,
not permission to guess BT.709 or full range. The CPU oracle maps and converts
each plane independently before the shared linear color transform.

The feature described by this spec is not complete until every listed format
passes the native GPU path and HDR acceptance where supported. Missing native
capability never lowers capture resolution or FPS and never activates the
oracle. It publishes typed native-unavailable state and no screen frame.

The oracle and GPU output use the same golden fixture suite. Packed and float
RGB formats match exactly. YUV formats may differ by at most one 8-bit output
code value per channel because Metal device families can contract floating-point
operations differently at a UNORM rounding boundary. A native path that cannot
match the canonical transform within that tolerance does not become active.

## 11. IOSurface and Metal path

### 11.1 Ownership and admission

The native frame owner retains the `CVPixelBuffer`, which retains the IOSurface
storage. `PlatformGpuSurface` retains that owner until every downstream
publication drops.

The shared byte coordinator charges:

- the full ScreenCaptureKit queue reservation and every observed queue surface;
- overlapping old and candidate stream generations;
- native import metadata and any normalization target;
- exact derived publication textures; and
- fixture-only oracle storage when a parity test explicitly admits it.

ScreenCaptureKit owns queue allocation and does not expose an IOSurface before
the first callback. Native queue memory therefore uses two-phase admission:

1. Before stream start, Hypercolor reserves eight times a checked conservative
   per-surface bound derived from native extent, requested format, plane layout,
   and platform alignment, plus stream metadata.
2. The first complete frame reads `IOSurfaceGetAllocSize`, validates every
   plane against that allocation, and atomically rebases the pool reservation
   to eight times the observed allocation before retaining the frame. If the
   coordinator cannot cover an increase, the callback drops the frame, the
   control plane stops the stream, and the source enters
   `macos_screen_resource_exhausted`.
3. Every later unique IOSurface repeats exact validation. A larger allocation
   rebases the pool claim before retention. If the coordinator cannot cover the
   increase, the callback drops the frame, the control plane stops the stream,
   and the source enters `macos_screen_resource_exhausted`.
4. Metrics report reserved bytes, exact observed pool bytes, retained frame
   bytes, and reservation variance separately. After all live pool slots are
   observed, the exact pool claim must equal their summed allocation sizes.

The operating system may allocate the first pool before Hypercolor can measure
it. The conservative reservation is the only exception to exact pre-allocation
admission. Hypercolor never retains or imports an over-budget surface, and a
candidate stream must reserve alongside every pinned old generation.

All Hypercolor-owned claims are acquired before fallible allocation and retire
only when the actual backing owner drops. A stopped stream may still have
pinned frames, so stopping the source alone does not release their claims.

### 11.2 Import and execution target

SparkleFlinger's Metal-backed wgpu device registers a
`ScreenNativeExecutionTarget` with:

- `PlatformGpuApi::Metal`;
- the `MTLDevice.registryID` as `MetalRegistryId`;
- the device's maximum 2D texture dimension; and
- a daemon-owned native target preparer wrapping `MacosScreenBridge`.

The daemon-owned preparer calls `MacosScreenBridge`, which validates physical
GPU identity, IOSurface descriptor, pixel format, every plane, usage, and
allocation before creating one `MTLTexture` per plane with
`newTextureWithDescriptor:iosurface:plane:`. It wraps the Metal textures through
`wgpu-hal` and `create_texture_from_hal` on the same device. Packed RGB uses one
texture. Bi-planar YUV uses two textures and an explicit conversion kernel.

Storage mode is not hardcoded or left at the descriptor default. The direct
IOSurface importer queries `MTLDevice.supportsFamily(MTLGPUFamilyApple1)`. An
Apple-family device requests `MTLStorageModeShared`; every non-Apple family
requests `MTLStorageModeManaged`, because shared texture storage is unavailable
for non-Apple-family textures. The bridge records the predicate, requested mode,
created texture's actual mode, and any rejection.

The bridge has two native importer candidates. Apple-family devices try direct
`newTextureWithDescriptor:iosurface:plane:` first, then
`CVMetalTextureCacheCreateTextureFromImage`. Non-Apple devices try the Core
Video texture cache first, then the direct managed IOSurface importer. The Core
Video path consumes the retained `CVPixelBuffer`, creates one `CVMetalTexture`
per plane, retains each wrapper through GPU completion, and validates that each
resulting `MTLTexture` names the expected IOSurface, plane, format, and extent.
Both candidates must remain zero-copy and pass the same structural validation.
A candidate that returns nil, selects an incompatible mode, copies, or names the
wrong IOSurface plane is rejected before the next candidate runs. Production
startup does not map frame bytes or consult a CPU parity result when selecting
an importer.

Apple-family textures are expected to use `MTLStorageModeShared`; Intel
discrete textures are expected to use `MTLStorageModeManaged`. If both importer
candidates fail, the source reports `macos_screen_metal_import_failed` with both
bounded native results.

No `synchronizeResource` operation runs before GPU sampling. That operation
makes GPU writes visible to the CPU and is the wrong direction for a
ScreenCaptureKit-produced IOSurface. Import-side coherency relies on the
framework's complete-frame callback, retained `CVPixelBuffer` ownership through
command-buffer completion, and the driver contract exercised by the W4 fixture
and signed physical acceptance. Those gates alternate incompatible byte patterns
across every reused queue slot, sample immediately and after sustained load,
check the imported texture's IOSurface and plane identity, and compare completed
GPU output with the fixture-only CPU oracle. They qualify the shipped importer
implementation and never run as production path-selection logic.

`synchronizeResource` appears only on managed GPU-to-CPU readback resources in
the parity fixture, after the GPU writes and before CPU mapping. Startup records
device family, actual storage mode, importer, structural validation result, and
every mismatch.
If neither direct IOSurface import nor `CVMetalTextureCache` lets Intel sample a
ScreenCaptureKit pixel buffer coherently without a full-frame copy, Intel native
acceptance fails and the release is blocked until a coherent GPU mechanism
lands. The fixture oracle may diagnose the failure, but production remains
native-unavailable and cannot satisfy the first-class Intel claim until the GPU
mechanism passes.

Imported textures are cached by the complete storage identity:

```text
capture session generation
+ resource generation
+ IOSurface ID
+ plane
+ width and height
+ pixel format
+ storage mode
+ Metal registry ID
```

Frame sequence is content identity, not storage identity. Reusing an IOSurface
for a later frame reuses the wrapper while advancing content sequence.

### 11.3 Synchronization

ScreenCaptureKit delivers a complete `CVPixelBuffer` to the callback queue.
The initial native path treats callback delivery as producer completion, then
submits all Metal and wgpu work on the renderer's device queue without a CPU
readback.

If live validation shows producer and consumer overlap on reused IOSurfaces,
the implementation adds an explicit synchronization primitive at the interop
boundary. It must not paper over a race with a per-frame CPU wait. Any added
primitive records wait time and storage identity so stalls are diagnosable.

A bounded diagnostic may copy only a completed GPU result into CPU-visible
memory after the owning command buffer signals completion. The readback belongs
to the final publication, uses a size-bounded staging allocation charged to the
diagnostic budget, and is dropped after protocol, device-output, or diagnostic
delivery. No readback result may select an importer, mutate capture state, feed a
later capture or composition pass, or provide a production recovery path.

### 11.4 Native reduction

The source IOSurface feeds the exact publication DAG:

1. normalize geometry and source color once;
2. apply cursor policy exactly once;
3. share equal physical reduction descriptors;
4. derive exact surface and zone branches; and
5. publish immutable owner-backed textures.

The steady-state native path performs no full-frame CPU copy. Damage metadata
may skip work only when the output algorithm proves incremental equivalence.
Absence of damage never changes output correctness.

### 11.5 Native execution recovery

The daemon owns one transactional native execution state:

```text
Ready(target N)
  -> Invalidating(error)
  -> Rebuilding
  -> Ready(target N+1)
  -> Unavailable(last error)
```

A structural import, target-owner, extent, descriptor, encoder, or submission
failure clears the compositor's retained screen layer, releases every
screen-specific GPU cache, fences the failed target generation, and rebuilds
the bridge, reducer, preparer, and execution target. The replacement target is
published only after full construction succeeds, and active demand is then
resolved against its new identity.

Only a specifically typed transient not-ready result may retain a still-fresh
current frame. Persistent and unclassified errors invalidate immediately.
`Unavailable` retains GPU-required demand and may attempt native reconstruction
after new demand or frame activity, but it exposes no CPU execution edge.

## 12. Color and HDR

### 12.1 SDR

SDR capture uses BGRA8 and explicit source color-space metadata. The pipeline
decodes the transfer function, converts into Hypercolor's linear working space,
performs spatial reduction there, applies temporal smoothing and color tuning,
then encodes for LED output. It does not treat every BGRA byte as sRGB merely
because the storage format is eight bit.

### 12.2 HDR on Sequoia

On supported Apple Silicon, Hypercolor starts from
`SCStreamConfigurationPresetCaptureHDRStreamCanonicalDisplay`. Canonical HDR is
the correct source because LED output is not the captured display. Hypercolor
reads the resulting configuration and first complete frame back, then records
the actual dynamic range, pixel format, color space, matrix, range, and chroma
siting. A preset is a requested configuration, not evidence that one exact
format arrived.

`RGhA` half-float preserves extended linear values and maps directly to an
`Rgba16Float` GPU texture, so it is preferred when the resolved preset supplies
it. A machine or framework path that supplies `l10r`, `420v`, `420f`, or `xf44`
is identified exactly and routed through its packed or multi-plane conversion
kernel. Unsupported format does not fall through as BGRA.

The shared capture vocabulary gains the pixel formats and color metadata needed
to represent these inputs without `Other` strings. All format ranking and
descriptor equality matches become exhaustive.

### 12.3 LED tone mapping

HDR reduction must preserve SDR contrast and roll highlights into the LED
device's available headroom. The working contract carries:

- source reference white;
- source content headroom when available;
- transfer function and primaries;
- target LED white point, reference white, and calibrated peak;
- user exposure; and
- tone-mapping algorithm revision.

`CaptureConfig` supplies the target and user inputs through five additive,
serde-defaulted fields:

```rust
pub target_led_white_x: f32,                // default 0.3127
pub target_led_white_y: f32,                // default 0.3290
pub target_led_reference_white_nits: f32,   // default 203.0
pub target_led_peak_nits: f32,              // default 406.0
pub exposure_ev: f32,                       // default 0.0
```

The default chromaticity is D65. Hypercolor's nominal calibration maps resolved
source reference white to a 203-nit target and reserves one full stop of output
headroom through a 406-nit peak. These values are tone-mapping coordinates, not
claims about unmeasured hardware. White-point components must be finite and
strictly inside the CIE xy chromaticity triangle: `x > 0`, `y > 0`, and
`x + y < 1`. Target reference white must be finite and within
`1.0..=5_000.0` nits. Peak luminance must be finite and within
`1.0..=10_000.0` nits and strictly greater than target reference white.
Exposure must be finite and within `-8.0..=8.0` EV. Invalid API or
configuration values are rejected rather than clamped.

At zero exposure, the SDR path maps resolved source reference white to
normalized `1.0`, matching the existing Windows and Linux capture paths. The HDR
path maps resolved source reference white to
`target_led_reference_white_nits / target_led_peak_nits`. Its default normalized
value is `0.5`, and its highlight shoulder maps values above source reference
white into the remaining `0.5..=1.0` range. The target reference-white and peak
fields govern only the HDR shoulder. Measured device profiles may replace the
target white point, target reference white, and peak. The user's explicit
exposure remains authoritative. Source reference white, content headroom,
transfer function, and primaries continue to come from the resolved frame
metadata. The algorithm revision is an internal fixture and cache key.

An SDR/HDR mode change begins at a frame boundary and interpolates the complete
old and new tone-mapping curves with a monotonic smoothstep over 250 ms. The
curve is applied per source sample in linear light before spatial reduction and
the shared temporal smoothers. A new mode change during that interval starts
from the current interpolated curve and restarts both the blend and its marker
for a full 250 ms from the new frame boundary. The marker is active exactly
while the current blend is active, including every restarted interval.

The marker remains private transition state and does not enter
`ScreenPublicationMetadata` or any published payload. Each macOS frame threads
`suppress_scene_cut_bypass: bool` directly beside the existing history-reset
flag at both smoothing seams. `PreparedTemporalSmoother::stage` gains the
parameter beside `reset_history`; `downscale_frame` gains it beside
`reset_smoother` and forwards it into
`TemporalSmoother::stage_for_elapsed_grid`. The public `TemporalSmoother::apply`,
`apply_for_elapsed`, and `apply_for_elapsed_grid` wrappers keep their existing
signatures and forward `false` internally. The macOS source passes `true`
exactly while its current blend is active. Every Windows, Linux, and
non-transition caller passes `false`.

When suppression is true, `PreparedTemporalSmoother` skips the
`scene_cut_detected` reset gate in `input/screen/smooth.rs`, and
`TemporalSmoother` skips its mean-difference scene-cut bypass in the same file.
Ordinary exponential smoothing still follows its configured policy, so it may
extend the visible settling time but cannot turn the deliberate curve blend
into a scene-cut snap.

The transition never changes source reference white, infers scene brightness,
or feeds output luminance back into exposure, so it is deterministic curve
handover rather than auto exposure. At zero exposure after the interval, SDR
reference white is exactly `1.0` and default HDR reference white is exactly
`0.5`.

The default algorithm is reference-white based. It preserves ordering and
contrast at and below source reference white within each dynamic range, rolls
HDR highlights smoothly, and applies gamut compression before device encoding.
Clipping, global normalization by the brightest pixel, and frame-to-frame
auto-exposure pumping are rejected.

The fixture-only CPU oracle and production GPU kernels share vectors for SDR
white, saturated primaries, wide-gamut colors, diffuse HDR, specular peaks,
gradients, and scene cuts.

### 12.4 Tahoe diagnostics and calibration

On an HDR-capable Tahoe selection, the diagnostic harness runs paired SDR and HDR
configurations through the same ScreenCaptureKit, IOSurface, Metal, and
SparkleFlinger path used in production. On an SDR-only Tahoe selection, it runs
one SDR configuration and records HDR and paired range as unsupported. Both
reports compare reference white, gamut conversion, and final zone colors. Only
the paired report compares highlight rolloff across ranges.

Diagnostics may inspect only the bounded completed-GPU egress described in
section 11.3. Core Graphics snapshots and CPU-side platform reference capture
are not part of the shipped diagnostic. Fixture tests retain the platform-neutral
CPU oracle for deterministic parity vectors.

## 13. Core and daemon integration

### 13.1 Platform selection

`CapturePlatform` gains `MacosScreenCaptureKit`. Config validation accepts it
only for a macOS build. The 15.2 floor is a build-time guarantee enforced by
`.cargo/config.toml`, Tauri's minimum system version, CI availability auditing,
and the finished Mach-O minimum OS check. The binary cannot launch on an older
host, so `hypercolor-types` gains no runtime OS-version dependency.

Daemon startup constructs:

- `MacosHostInput` when input is enabled and either native kind is allowed;
- `MacosScreenCaptureInput` when screen capture is configured;
- shared byte and compute capacity from the existing coordinators; and
- the Metal native execution target when SparkleFlinger runs on Metal.

The old `InteractionInput` construction and macOS `device_query` dependency are
deleted in the same wave that makes native input the default. The removal
includes the workspace and core dependency entries, `input/interaction`, its
`input/mod.rs` export, daemon startup wiring, `interaction_input_tests.rs`, the
legacy case in `input_tests.rs`, stale backend labels in shared fixtures, and
the public backend list in `input/traits.rs`, plus the deleted lock-order entry
in `docs/design/32-lock-ordering.md`. The same lock-ordering edit adds
`MacosHostInput::shared` for the canonical batch fold and
`MacosScreenCaptureInput::latest_frame` for the bounded native-surface
latest-value handoff. Neither lock is held while acquiring `input_manager`,
calling native APIs, joining a worker, or running renderer work. There is no
hidden fallback to privacy-buggy polling.

### 13.2 Live reconfiguration

The existing input graph transaction owns config changes:

- changing keyboard or pointer consent builds a candidate event mask and swaps
  tap generation transactionally;
- changing source or cursor policy stages a candidate stream and exact plan;
- changing capture cadence updates `SCStreamConfiguration` when the source and
  storage descriptor remain compatible;
- changing target LED white point, target reference white, calibrated peak, or
  exposure validates a candidate tone-mapping configuration and atomically
  swaps shared tone-map transition constants and GPU uniforms at a frame
  boundary without reopening the native stream. Fixture oracles consume the
  same constants only under test;
- changing extent branches replans derived publications without reopening the
  native stream unless native source geometry changes; and
- disabling a source stops its native worker after the replacement graph is
  committed.

Failure preserves the last known-good graph unless the previous permission or
source has become invalid. Invalidation clears freshness immediately.

### 13.3 Status and metrics

`hypercolor-core::input::status` owns the platform state structs so the adapters
can publish them without depending on daemon API types. The source status
surface publishes the state directly rather than asking clients to reconstruct
it from generic issues:

```rust
pub enum MacosCapabilityOwner {
    AppSidecar,
    App,
    LaunchdService,
    HomebrewService,
    Broker,
    Standalone,
}

pub struct MacosDaemonOwnerConflict {
    pub active: MacosCapabilityOwner,
    pub contender: MacosCapabilityOwner,
    pub observed_at_ms: u64,
}

pub struct MacosInputPlatformStatus {
    pub keyboard: MacosProtectedSourceState,
    pub pointer: MacosProtectedSourceState,
    pub keyboard_tcc: MacosAuthorizationState,
    pub keyboard_owner: MacosCapabilityOwner,
    pub pointer_owner: MacosCapabilityOwner,
    pub owner_conflict: Option<Arc<MacosDaemonOwnerConflict>>,
}

pub struct MacosScreenPlatformStatus {
    pub state: MacosProtectedSourceState,
    pub tcc: MacosAuthorizationState,
    pub owner: MacosCapabilityOwner,
    pub selection: MacosSelectionState,
    pub tahoe_selection: Option<MacosTahoeSelectionCapabilities>,
    pub owner_conflict: Option<Arc<MacosDaemonOwnerConflict>>,
}

pub enum SourcePlatformStatus {
    MacosInput(MacosInputPlatformStatus),
    MacosScreen(MacosScreenPlatformStatus),
}
```

`SourceStatus` gains `platform: Option<Arc<SourcePlatformStatus>>`. Every
constructor, writer update, and retired snapshot carries or clears `platform`
explicitly. The daemon's `api/system.rs::InputSourceStatus`
gains `platform: Option<InputSourcePlatformStatus>`, where the daemon-local
serde enum is tagged as `macos_input` or `macos_screen` and derives `ToSchema`.
`input_source_status` maps the core enum field by field, including
`tahoe_selection` on the daemon-local `macos_screen` variant and
`owner_conflict` on both macOS variants. This diagnostic payload stays
daemon-local, matching the existing system-status boundary; the web UI
deserializes a tolerant local subset. REST and OpenAPI fixtures cover both
variants, absence on other platforms, and unknown future fields.

The owner arbiter is daemon state, not input-source state. `AppState` owns its
latest snapshot from startup even when no source exists, and
`api/system.rs::SystemStatus` gains:

```rust
pub enum MacosCapabilityOwnerApi {
    AppSidecar,
    App,
    LaunchdService,
    HomebrewService,
    Broker,
    Standalone,
}

pub struct MacosDaemonOwnerConflictApiStatus {
    pub active: MacosCapabilityOwnerApi,
    pub contender: MacosCapabilityOwnerApi,
    pub observed_at_ms: u64,
}

pub struct MacosDaemonOwnershipApiStatus {
    pub active_owner: MacosCapabilityOwnerApi,
    pub owner_epoch: u64,
    pub conflict: Option<MacosDaemonOwnerConflictApiStatus>,
}
```

`SystemStatus` adds
`macos_daemon_ownership: Option<MacosDaemonOwnershipApiStatus>`. The field is
`None` off macOS and present from daemon startup on macOS. A
`HypercolorEvent::MacosDaemonOwnershipChanged` event carries the same bounded
snapshot over the existing events WebSocket channel. The daemon-local API enums
use snake-case serde names, derive `ToSchema`, and map the core owner and
conflict types field by field. The UI and CLI consume the
system field and event, so `choose_daemon_owner` remains reachable with input
and capture disabled. Per-source conflict fields are convenience mirrors only.
`protocol/websocket-v1.json` gains the
`macos_daemon_ownership_changed_v1` JSON payload contract on the `events`
channel with `"schema_version": 1`. Its event name is
`macos_daemon_ownership_changed`, its required fields are `active_owner` and
`owner_epoch`, and its optional `conflict` field defaults to `null`.
`crates/hypercolor-daemon/src/api/ws/tests.rs` loads the manifest and pins the
new entry's schema version, channel, event name, required fields, and optional
default beside the existing JSON payload conformance tests. REST, OpenAPI, bus,
manifest-generated WebSocket, and no-source startup fixtures cover the surface.
The OpenAPI change regenerates the vendored Python models for `SystemStatus` and
`InputSourceStatus`. Both new fields are additive and optional, and
`python-generate-check` and `python-ws-protocol-check` must return no diff after
regeneration.

The source status surface also adds structured macOS fields:

- TCC owner process and designated-requirement hash;
- native host architecture, executable slice, and Rosetta translation state;
- authorization state and last transition;
- selected content style and diagnostic label;
- stream active, inactive, or stopped state;
- source, topology, session, resource, and plan generations;
- pixel format, dynamic range, color space, scale, and native extent;
- queue depth, admitted native bytes, and pinned generations;
- frames received, published, superseded, malformed, stale, and dropped by
  reason;
- event-tap timeout disables, user-input disables, reenables, and gaps;
- callback, retain, import, conversion, reduction, and publication timing; and
- native-ready, native-invalidating, native-rebuilding, native-pending, or
  native-unavailable execution state with an exact bounded reason;
- invalidation epoch, active target generation, rejected stale-publication
  count, and the last completed recovery transaction state.

High-cardinality labels such as IOSurface ID, window title, and application name
stay in bounded diagnostics rather than metrics labels.

### 13.4 Shared scroll contract

Two-axis scroll is a cross-platform contract, not a macOS-only event. The
shared vocabulary in `hypercolor-types::event` gains:

```rust
pub enum PointerScrollUnit {
    Line120,
    Pixels,
}

pub enum PointerScrollPhase {
    None,
    MayBegin,
    Began,
    Changed,
    Stationary,
    Ended,
    Cancelled,
}

InputEvent::PointerScroll {
    source_id: String,
    delta_x_q16_16: i64,
    delta_y_q16_16: i64,
    unit: PointerScrollUnit,
    phase: PointerScrollPhase,
    momentum_phase: PointerScrollPhase,
}

pub struct ScrollAggregate {
    pub line120_x_q16_16: i64,
    pub line120_y_q16_16: i64,
    pub pixel_x_q16_16: i64,
    pub pixel_y_q16_16: i64,
}
```

Signed Q16.16 integers preserve fractional motion while keeping `InputEvent`
`Eq` and its JSON representation deterministic. `Line120` uses 1/120 notch as
its integral unit, while `Pixels` uses one pixel. W2 migrates every platform
producer to `PointerScroll`, with no synthesized compatibility shadow.
`InteractionBatch` carries `scroll: ScrollAggregate`, includes all four totals
in emptiness and every coalescing path, and saturates on overflow. Phase and
momentum remain on ordered events; event coalescing never crosses their
boundaries.

The effect path changes end to end:

- `LightScriptInputEventPayload` maps `PointerScroll` to `kind: "scroll"` with
  floating `deltaX`, `deltaY`, `unit`, `phase`, and `momentumPhase` fields.
- `LightScriptMousePayload` adds a `scroll` object with `line120X`, `line120Y`,
  `pixelX`, and `pixelY` aggregate fields.
- `sdk/packages/core` turns `MouseInputEvent` into a discriminated union with a
  typed scroll member and exposes exact aggregates on `MouseInputState`.
- The WebSocket `input_events` envelope stays at schema 1 because
  `TimedInputEventPayload.event` is intentionally opaque, retains unknown JSON,
  and changes no envelope field. New tests prove an older schema-1 decoder
  round-trips the unknown `pointer_scroll` kind and updated clients deserialize
  it exactly.

Every host follows one producer rule:

- macOS multiplies non-continuous Q16.16 notch values by 120 into `Line120` and
  emits pixel Q16.16 with native phase and momentum for continuous gestures;
- Linux maps each `REL_WHEEL_HI_RES` and `REL_HWHEEL_HI_RES` integer as
  `value << 16` in `Line120`, using `(value * 120) << 16` for low-resolution
  counterparts;
- Windows maps each signed `RI_MOUSE_WHEEL` and `RI_MOUSE_HWHEEL` delta as
  `value << 16` in `Line120` inside `hypercolor-windows-input`, instead of
  dropping horizontal wheel data; and
- browser injection accepts two-axis `Line120` or pixel Q16.16 values and uses
  `None` phases when the sender supplies no lifecycle.

The inbound `input_inject` wire uses this tagged edge:

```rust
BrowserInputEdgeWire::Scroll {
    delta_x_q16_16: i64,
    delta_y_q16_16: i64,
    unit: PointerScrollUnitWire,
    phase: PointerScrollPhaseWire,
    momentum_phase: PointerScrollPhaseWire,
}
```

`unit` is required. Both phase fields default to `none` when absent. The daemon
validates both axes against `MAX_INPUT_SCROLL_Q16_16` with checked
absolute-value arithmetic and rejects values outside that inclusive bound
before conversion into a core edge. The UI's `InputInjectEdge` mirrors the same
tagged shape and enum spellings. Integration tests deserialize the inbound
shape, prove its exact canonical event, and serialize the UI mirror back to the
daemon contract.

Pure parity fixtures assert sign, axis orientation, units, serde shape,
WebSocket round-trip, LightScript payloads, SDK parsing, and coalescing for all
four producer families.

## 14. User experience and API

The existing input settings page gains native macOS state and actions:

- `Enable keyboard input`;
- `Enable pointer input`;
- `Authorize Input Monitoring`;
- `Enable screen capture`;
- `Authorize Screen Recording`;
- `Choose screen source` or `Change screen source`;
- `Enable app broker for service mode` when the launchd service cannot own the
  protected capability;
- `Choose active daemon owner` when another installed topology holds the
  single-instance guard;
- selected source and dynamic range;
- an advanced LED tone-mapping panel with D65 white-point coordinates, target
  reference-white nits, calibrated peak nits, exposure EV, and
  `Reset calibration`. Reset restores the two white-point coordinates, target
  reference white, and peak to their defaults while preserving the user's
  explicit exposure;
- active consumer count; and
- exact remediation with a deep link to the relevant System Settings pane.

When the platform publishes `NeedsProcessRestart`, the UI offers `Restart
capture owner` and names the process that will restart. It never renders the
state as another permission denial. Keyboard, pointer, and screen cards read
their published platform states directly; they do not infer authorization,
selection, or ownership from generic status text.

The UI distinguishes:

- configured but not authorized;
- authorized and idle because no effect demands data;
- authorized but needing a source selection;
- direct launchd service awaiting installation, registration, or startup of the
  authenticated app broker;
- another daemon topology active, with both active and attempted owners named;
- selected external daemon owner offline, with the selected owner and its local
  start action named;
- granted but requiring a process restart;
- live;
- interrupted;
- revoked; and
- unsupported hardware capability such as Intel HDR.

The REST control plane keeps source status read-only and exposes explicit action
endpoints for authorization and picker presentation. The existing capture pick
endpoint routes to the macOS system picker. WebSocket events announce state
changes so the UI never polls.

`choose_daemon_owner` is never a daemon REST action. A browser-only session
receives `requires_app_ui` and cannot mutate autostart or stop a process. Inside
`Hypercolor.app`, the same UI invokes a native Tauri command implemented by the
local app coordinator. The CLI invokes the local coordinator directly. Both
paths consume the durable journal and never proxy the operation through the
daemon's network listener.

The app-broker action registers the bundled broker through `SMAppService`,
waits for the reverse bootstrap, and retries only the requested protected
source. A Homebrew or CLI-only installation without `Hypercolor.app` prints the
typed `app_broker_required` remediation with the required app install and launch
action. It does not pretend the direct launchd service can self-install an app
broker.

The CLI gains equivalent explicit commands where the process topology permits
them and prints which process owns the grant. A headless command that cannot
present the picker returns a typed `requires_app_ui` remediation instead of
attempting private UI.

## 15. Security and privacy

The macOS implementation is a privacy-sensitive subsystem and follows these
rules:

1. Defaults remain off.
2. Prompts and picker presentation require explicit local user actions.
3. Raw frames and raw host events remain process-local unless an existing
   consented consumer route explicitly exposes a derived form.
4. Logs never contain key names, typed text, window titles, application names,
   raw pixels, or screenshot paths by default.
5. Diagnostics redact selected content labels unless the request is local and
   authenticated under the existing daemon policy.
6. The system picker is mandatory for user-selected content.
7. Hypercolor excludes its own UI from display capture when ScreenCaptureKit
   can express the exclusion without changing the selected source.
8. Lock, logout, fast-user-switch, secure input, and TCC revocation clear held
   input state and invalidate screen freshness.
9. The broker fallback authenticates the peer by audit token and code signing,
   not merely by filesystem permissions or claimed process ID.
10. The app ships `NSScreenCaptureUsageDescription` with direct language about
    lighting effects. The unrelated Apple Events purpose string is removed.
11. Daemon-owner selection, process handover, and autostart mutation require the
    local app or CLI coordinator. No REST, WebSocket, MCP, or other network
    client can invoke them. Pre-runtime daemon recovery may execute only a
    previously journaled typed operation and cannot create a new owner choice.

## 16. Failure taxonomy

Native errors map into stable codes rather than formatted strings:

```text
macos_input_permission_denied
macos_input_permission_revoked
macos_input_process_restart_required
macos_input_tap_create_failed
macos_input_tap_disabled_timeout
macos_input_tap_disabled_user_input
macos_input_run_loop_exited
macos_screen_permission_denied
macos_screen_permission_revoked
macos_screen_process_restart_required
macos_screen_selection_required
macos_screen_picker_cancelled
macos_screen_picker_failed
macos_screen_source_inactive
macos_screen_source_disappeared
macos_screen_stream_stopped
macos_screen_frame_malformed
macos_screen_format_unsupported
macos_screen_iosurface_unavailable
macos_screen_resource_exhausted
macos_screen_gpu_identity_mismatch
macos_screen_metal_import_failed
macos_screen_hdr_unsupported
macos_broker_authentication_failed
macos_broker_disconnected
macos_daemon_owner_conflict
macos_daemon_owner_offline
```

User-action remedies use a separate stable vocabulary:

```text
authorize_input_monitoring
authorize_screen_recording
restart_app_sidecar
restart_app
restart_launchd_service
restart_homebrew_service
restart_broker
restart_standalone
stop_standalone_owner
start_app_sidecar
start_launchd_service
start_homebrew_service
select_screen_source
requires_app_ui
app_broker_required
choose_daemon_owner
```

`app_broker_required` means the direct launchd service cannot own the requested
protected capability and no authenticated broker has completed reverse
bootstrap. `requires_app_ui` means the owner is valid but the next action, such
as presenting the system picker or registering the broker, must run in
`Hypercolor.app`. `restart_homebrew_service` invokes
`brew services restart hypercolor` for the recorded Homebrew owner.
`stop_standalone_owner` names the authoritative standalone PID and requires
user-directed Ctrl-C or `SIGTERM`; it never grants another process termination
authority.
`macos_daemon_owner_offline` means the persisted external owner is selected but
does not hold the guard. Its remedy is topology-specific: `start_app_sidecar`
invokes the app supervisor, `start_launchd_service` invokes
`hypercolor service start`, and `start_homebrew_service` invokes
`brew services start hypercolor`. A browser receives `requires_app_ui` for all
three actions. Only the local app or CLI coordinator executes them.
`choose_daemon_owner` means two or more installed autostart topologies contended
for the single-instance guard and requires one explicit owner choice.

Every issue states whether retry is automatic, requires a user action, requires
source reselection, or is terminal for the current configuration. Raw native
domain and code are preserved as bounded diagnostic fields.

## 17. Diagnostics and development tools

The platform crates ship examples or CLI hooks that exercise production
boundaries without starting the full daemon:

- `dump_macos_input` prints redacted event kinds, physical codes, pointer
  geometry, generation, and health counters. It never prints logical text.
- `dump_macos_frame` captures a bounded frame count and prints descriptor,
  attachments, color metadata, IOSurface allocation, and timing.
- `capture_macos_screenshot_reference` runs Tahoe paired SDR and HDR diagnostics
  for an HDR-capable selected source and the single SDR reference diagnostic for
  an SDR-only selected source. Before first-frame capability resolution, it
  reports that source capability is pending and captures nothing.
- `probe_macos_tcc_owner` records the canary evidence for the current signed
  process topology.
- `bench_macos_reduction` compares CPU, wgpu Metal, and qualifying Metal 4
  reduction with identical fixtures.

Tools default to metadata only. Writing pixels requires an explicit output path
and prints the privacy implication before the write.

## 18. Verification strategy

### 18.1 Pure input tests

Cross-platform tests cover:

- total macOS physical key inventory;
- total macOS media-key inventory and subtype-8 decoding;
- left and right modifiers;
- Caps Lock transitions;
- native repeat classification;
- press, release, and impossible-edge behavior;
- independent keyboard and pointer masks;
- pointer normalization with negative display origins;
- topology changes and first-event baseline reset;
- pointer-only `Live` while Input Monitoring is denied;
- buttons, two-axis line wheel, continuous pixel scroll, scroll phase, and
  momentum phase;
- signed 16.16 preservation for repeated sub-unit scroll events;
- bounded queue overflow and ordered `StateGap`;
- timeout disable, user-input disable, revocation, stop, and synthetic releases;
- epoch fencing after restart; and
- source status mapping.

### 18.2 Pure capture tests

Fixture tests cover:

- every complete and non-complete `SCFrameStatus`;
- missing, malformed, and valid attachments;
- checked extent, stride, plane, and allocation arithmetic;
- BGRA8, ARGB2101010, RGBA16Float, YUV420 video range, YUV420 full range,
  YUV44410 bi-planar, and unsupported formats;
- content rect, display scale, content scale, negative screen origin, and
  multi-window bounding rect;
- point-to-pixel conversion for content and bounding rects with fractional
  Retina origins and outward rounding;
- dirty rect validation;
- cursor composed and hidden capability matching;
- source, topology, session, resource, and plan generation fencing;
- absent Tahoe selection capabilities before first frame, exact publication
  after first frame, and stale selection capability rejection after repick;
- stale callback after stop or repick;
- transactional source replacement and picker cancellation;
- queue-depth reservation, first-frame exact rebase, larger-surface rejection,
  reservation variance, and pinned old generation;
- display-filter inactive telemetry without a false liveness transition;
- window and application inactive liveness transitions;
- CPU and GPU color parity;
- SDR, HDR, wide gamut, tone mapping, and scene-cut vectors;
- D65 and measured white points, the nominal 203-nit reference white and
  406-nit peak, measured calibration, exact zero-exposure SDR reference white
  at `1.0`, and exact default HDR reference white at `0.5`;
- specular-peak and rolloff vectors with peak strictly above target reference
  white;
- deterministic 250 ms SDR/HDR curve handover measured at publication with
  `smoothing = 1.0` and `exposure_ev = 0.0`, including a second mode change
  during the first transition, no scene-cut bypass, and exact final values;
- Windows and Linux call sites always pass `suppress_scene_cut_bypass = false`
  and preserve their existing scene-cut behavior in fixtures for both
  `PreparedTemporalSmoother` and `TemporalSmoother`;
- exposure limits plus nonfinite, out-of-range, and invalid cross-field
  calibration rejection; and
- calibration reset restoring all four target calibration fields while
  preserving `exposure_ev`.

### 18.3 Integration tests

`hypercolor-core` gains a `macos-native-fixtures` feature. Behind it,
`MacosHostInput::new_deterministic_fixture(MacosInputFixtureBackend)` injects
preflight and request results, effective event masks, tap callbacks, and owner
restart results. `MacosScreenCaptureInput::new_deterministic_fixture(
MacosScreenFixtureBackend)` injects picker outcomes, authorization evidence,
stream callbacks, complete frames, and owner restart results. The fixture
backends implement the same narrow platform interfaces as production and never
call TCC, present UI, or require a display.

Repository integration tests prove:

- config accepts macOS capture and rejects it on other platforms;
- daemon startup wires native input and capture with exact consent;
- disabling pointer capture produces no pointer registration;
- denied keyboard permission does not prevent pointer-only liveness;
- zero demand owns no tap or stream;
- active demand opens once and idle demand closes once;
- app sidecar and direct launchd contenders cannot both win the single-instance
  guard; the loser records `macos_daemon_owner_conflict`, and the winner
  publishes both owner variants through system status and the ownership event
  even when every input source is disabled. Direct and Homebrew launchd losers
  exit zero under `KeepAlive.SuccessfulExit = false`, the sidecar loser returns
  its non-restartable typed code, and repeated identical records yield one state
  transition and one bus event;
- managed-owner handover stops the incumbent, waits at most 10 seconds, starts
  the selected owner, persists or clears external-owner mode, and rolls back on
  stop, guard-release, or startup failure;
- coordinator termination after each mutating phase leaves a durable journal;
  the next local coordinator or incoming daemon pre-runtime recovery resumes or
  reverses the exact transaction, preserves the last viable owner, and commits
  every phase through atomic replacement plus file and parent-directory
  `fsync`;
- winning-daemon, contender, coordinator, and recovery writes interleave under
  the stable coordination lock without losing the owner record or separate
  journal. Tests prove each lock hold covers exactly one read-modify-write and
  no wait, supervisor operation, transaction phase, or atomic replacement can
  strand a lock on an obsolete inode;
- malformed journals, unknown operation variants, and operations carrying a
  path, executable, command, or argument vector are rejected without mutation;
- standalone-owner handover performs no autostart mutation, returns
  `stop_standalone_owner`, proceeds after user-driven guard release, and remains
  pending after its 60-second wait expires;
- the incoming or restored daemon publishes the ownership event, while the
  surviving coordinator returns the synchronous result and reconnect reads the
  matching system status;
- daemon-owner choice is absent from REST, OpenAPI, WebSocket, and MCP control
  surfaces. Browser-only invocation returns `requires_app_ui`, while the native
  app command and local CLI complete the same journaled choice;
- an unavailable persisted external owner publishes
  `macos_daemon_owner_offline` with the matching `start_app_sidecar`,
  `start_launchd_service`, or `start_homebrew_service` remedy and never starts a
  different topology;
- revocation updates status and freshness without daemon restart;
- a grant requiring relaunch publishes `NeedsProcessRestart` and invokes only
  the explicit supervisor action;
- exact descriptors remain independent;
- resolved Tahoe selection capabilities enter the core and daemon-local
  `macos_screen` status with matching source and capture-session generations,
  while preselection and stale generations publish `None`;
- Metal target matching uses registry ID and rejects a mismatch;
- imported packed and multi-plane ownership outlives the callback and releases
  with the final publication;
- Apple-family shared and non-Apple managed storage probes produce CPU-oracle
  byte parity with correct import-side coherency and readback synchronization;
- direct IOSurface and Core Video texture-cache candidates follow their
  per-family order, preserve plane identity, and collapse dual failure into
  `macos_screen_metal_import_failed`;
- injected structural import, conversion, reduction, and device-loss failures
  atomically clear retained output and native caches, fence the failed target
  generation, reject every stale publication, rebuild the complete native
  target, and either resume with a newer generation or become
  native-unavailable without a CPU recovery path;
- the existing Servo IOSurface importer selects shared storage on Apple-family
  devices and managed storage on non-Apple-family devices with parity on both;
- the fixture CPU oracle fences stale native frames and accepts only matching
  source and session generations without registering a production publisher;
- `PointerScroll` round-trips through serde and the schema-1 WebSocket envelope,
  maps into LightScript, and parses in the SDK without a synthesized shadow;
- browser injection accepts and validates the two-axis Q16.16 `scroll` edge,
  while the UI serializes the matching form;
- screen and interaction WebSocket privacy gates remain unchanged;
- packaged, direct launchd, Homebrew, terminal, and broker restart remedies
  target only the recorded TCC owner;
- the supervised-sidecar broker bootstrap rejects a missing inherited
  capability, while the direct launchd reverse bootstrap mutually verifies
  audit tokens and designated requirements, accepts no inherited descriptor,
  rotates its capability on daemon or broker restart, rejects a stale epoch,
  and recovers after the broker-only restart remedy without restarting the
  daemon; and
- the app-side broker protocol, when selected by the canary, rejects an
  unauthenticated peer and stale epoch.

### 18.4 CI

Every Rust-touching pull request gains jobs pinned to GitHub's `macos-26` Apple
Silicon image and `macos-26-intel` Intel image. Both set
`MACOSX_DEPLOYMENT_TARGET=15.2` for Cargo, build scripts, and the Tauri bundle.
The workflow pins one repository-declared Xcode 26 minor, prints
`xcodebuild -version` and `xcrun --show-sdk-version`, and fails before build if
the SDK major is not 26.

The repository sets
`MACOSX_DEPLOYMENT_TARGET = { value = "15.2", force = true }` in the `[env]`
table of `.cargo/config.toml`, so an inherited shell value cannot lower the
floor, and sets Tauri's `bundle.macOS.minimumSystemVersion` to `15.2`. The
`build-native-app` macOS matrix uses `macos-26` and `macos-26-intel`;
`build-release` gains `macos-arm64` on `macos-26` and `macos-amd64` on
`macos-26-intel`, producing standalone artifacts for both first-class
architectures. Pull-request jobs build at least one final Mach-O executable per
architecture and inspect its minimum OS. Both release
lanes select the same declared Xcode version, enforce the SDK-major gate,
inherit the deployment target, and inspect every finished Mach-O minimum OS
before uploading an artifact.

macOS release jobs reject `APPLE_SIGNING_IDENTITY = "-"` and any missing signing
secret. Every Mach-O receives an explicit architecture-independent `codesign -i`
identifier from a checked signing manifest. The required project-owned mapping
is:

| Code object                            | Identifier                            | Entitlements                                |
| -------------------------------------- | ------------------------------------- | ------------------------------------------- |
| `Hypercolor.app`                       | `tech.hyperbliss.hypercolor`          | `crates/hypercolor-app/entitlements.plist`  |
| embedded `hypercolor-daemon-*` sidecar | `tech.hyperbliss.hypercolor.sidecar`  | `packaging/macos/daemon.entitlements.plist` |
| standalone `hypercolor-daemon`         | `tech.hyperbliss.hypercolor.daemon`   | `packaging/macos/daemon.entitlements.plist` |
| standalone `hypercolor`                | `tech.hyperbliss.hypercolor.cli`      | none                                        |
| standalone `hypercolor-app`            | `tech.hyperbliss.hypercolor.app-host` | `crates/hypercolor-app/entitlements.plist`  |
| standalone `hypercolor-tray`           | `tech.hyperbliss.hypercolor.tray`     | none                                        |

The sidecar and standalone daemon identifiers are intentionally distinct, so
packaged and direct launchd grants cannot satisfy each other's TCC checks. Intel
and Apple Silicon sidecar file names differ by target suffix but share the one
`.sidecar` identifier and designated requirement. The broker runs inside the
signed app executable and uses the app identifier. Bundled dylibs and any future
Mach-O must also have a stable manifest entry with an explicit entitlements file
or `none`; an unlisted object fails release.

The daemon entitlement profile carries the six keys currently present in
`crates/hypercolor-app/entitlements.plist` forward verbatim. Audio input, JIT,
and unsigned executable memory are hardened-runtime capabilities needed by
microphone capture and Servo. USB, network client, and network server are
App-Sandbox resource keys; they do not gate those capabilities while Hypercolor
remains non-sandboxed and are not the basis for any access claim in this spec.
They stay in the profile to preserve current signed behavior. The sidecar and
standalone daemon both receive the exact profile. A missing or divergent profile
is a release failure.

The release job Developer ID Application-signs every object with hardened
runtime, secure timestamps, and the expected team identifier. The app bundle is
signed from the inside out. No signing invocation may derive an identifier from
a file name.

One repository script, `scripts/sign-macos-artifacts.sh`, is the signing actor
for CI and local release-ready builds. The order is exact:

1. Stage the target-suffixed sidecar.
2. Sign that staged source with
   `codesign -i tech.hyperbliss.hypercolor.sidecar` before `cargo tauri build`.
3. Run `cargo tauri build --bundles app` without treating Tauri's nested signing
   pass as final. The combined `dmg,app` invocation is forbidden.
4. Discover every Mach-O inside the completed app, reapply its manifest
   identifier and entitlements inside out, and sign `Hypercolor.app` last.
5. Submit the app for notarization, staple it, and validate the staple.
6. Run a separate DMG packaging command that consumes that exact signed and
   stapled app, then sign, notarize, staple, and validate the DMG.
7. Sign the standalone artifact set from the same manifest and submit those
   exact binary bits in a notarization ZIP.

The accepted standalone receipt ships in release provenance even though the
tar container cannot carry a staple. Release verification runs only after the
post-bundle signing pass. It discovers every Mach-O in each artifact instead of
checking a fixed list, runs `codesign --verify --strict`, extracts and compares
its manifest identifier and designated requirement, runs `xcrun stapler
validate` on both the `.app` and DMG, normalizes and compares `codesign -d
--entitlements :-` output with the manifest profile, and requires accepted
notarization before upload.

The Apple Silicon job runs:

```text
cargo check for the workspace
clippy with warnings denied for changed shared and macOS crates
nextest for macOS platform fixtures, core input, and daemon integration
```

The Intel job compiles and runs pure SDR fixtures plus synthetic direct and
Core Video texture-cache import, storage-mode probing, queue-slot reuse, and
readback parity on every pull request. It begins by requiring
`MTLCreateSystemDefaultDevice` to return a non-Apple-family device that can
create the fixture IOSurface textures. A missing or nonconforming device fails
runner qualification; the native fixture never silently skips. Before this job
becomes required, an equivalent self-hosted Intel Tahoe runner replaces a
hosted label that cannot meet the precondition.

Hosted or self-hosted pull-request results are regression evidence only. They
do not satisfy the section 11.2 Intel coherency and zero-copy release gate,
which requires the signed physical hardware matrix in section 18.5. TCC flows
and the 30-minute 4K60 SDR performance contract also run only in signed physical
acceptance. Before the workflow pin lands, a temporary `workflow_dispatch`
smoke job must run on both labels and record runner architecture, Xcode, SDK
major, Metal device name, registry ID, and family probes. If a label is
unavailable, never existed, loses the required SDK or Metal device, or later
ends, an equivalent required self-hosted runner must be online before the
affected support claim remains in a release.

A separate availability check rejects unguarded Tahoe symbols in the Sequoia
artifact and inspects the built deployment target. A compile-only success does
not substitute for the native Intel import fixture.

### 18.5 Signed physical acceptance

Release acceptance uses the signed packaged app, not `cargo run` alone.

The Apple Silicon matrix covers Sequoia 15.2 and current Tahoe with:

- fresh grant, deny, later grant, revoke, and regrant;
- app launch, direct launchd daemon service, Homebrew service, and
  terminal-launched standalone daemon;
- app and service autostart installed together, with explicit owner switching
  and stable arbitration across login;
- keyboard-only, pointer-only, and both;
- modifiers, repeat, extra pointer buttons, trackpad phases, and high-resolution
  scrolling;
- primary and secondary displays;
- negative origins, Retina and non-Retina mixes, rotation, and display hotplug;
- display, window, application, and multi-window picker modes;
- picker cancel and live repick;
- SDR display capture;
- HDR display capture and SDR/HDR transitions. The exact 250 ms smoothstep and
  endpoints are measured at publication with `smoothing = 1.0` and
  `exposure_ev = 0.0`; a second run with default smoothing proves scene-cut
  bypass remains suppressed and no output step occurs at either boundary;
- Spaces, full-screen applications, minimized and closed windows;
- sleep, wake, lock, unlock, fast user switching, and logout;
- 30 Hz, 60 Hz, 120 Hz, and native-refresh demand where hardware supports it;
- 1080p, 4K, 5K, portrait, and ultrawide sources; and
- a four-hour combined input and HDR capture soak.

The Intel matrix covers Sequoia 15.2 and current Tahoe with the same lifecycle
and SDR rows available on that hardware. It also requires native IOSurface byte
parity against the CPU oracle and the same 4K60 SDR duration, latency, and
memory contracts as Apple Silicon. The existing Servo IOSurface importer must
select managed storage on the Intel device and achieve exact CPU-oracle byte
parity under queue-slot reuse. Intel Tahoe runs the single SDR reference
diagnostic with tone-mapping metadata. HDR, paired range, and Metal 4 are
expected to report unsupported when the active hardware does not expose them,
not to emit SDR under an HDR label or fail a first-class SDR acceptance row.

## 19. Performance contracts

The feature is accepted only when each contract holds on every platform and
process topology named by that contract:

1. Native 4K60 SDR capture sustains demand for 30 minutes without lowering
   extent or cadence, unbounded memory growth, callback timeout, or stale-frame
   accumulation.
2. Native 4K60 HDR capture meets the same contract on supported hardware.
3. Native 4K120 SDR capture sustains the same contract on hardware whose
   selected display and ScreenCaptureKit path support 120 Hz. Native-refresh
   demand is measured at the display's reported refresh without an internal
   Hypercolor cap.
4. Intel native 4K60 SDR meets the same duration, latency, exact-byte, and
   zero-full-frame-copy contracts as Apple Silicon SDR.
5. The native GPU path performs zero full-frame CPU copies in steady state.
6. ScreenCaptureKit callback work stays below 1 millisecond at p99 excluding
   scheduler preemption. Retain and enqueue are reported separately.
7. The newest complete frame reaches the native publication stage within one
   source frame interval at p95 and two intervals at p99.
8. This spec establishes a 1 millisecond p95 total input-stage budget with
   screen, audio, and interaction active. Measurement starts immediately before
   `InputManager::sample_all()` reads the first source and ends after the final
   `InputData` snapshot is assembled for the frame. The screen measurement is
   the constant-time latest-value latch only. Native validation, import, and GPU
   reduction are reported separately as capture-to-native-publication latency.
9. Host input callback entry to canonical event publication stays below 2
   milliseconds at p95 and 5 milliseconds at p99.
10. An active broker topology meets the same end-to-end screen and input
    latency budgets as in-process ownership. Benchmarks also report XPC encode,
    transit, decode, and IOSurface handoff separately so IPC cannot disappear
    inside the total.
11. Steady-state retained bytes reconcile exactly with admitted native queue,
    import, and publication claims.
12. Replanning or repicking may temporarily overlap old and candidate resources
    only when the byte coordinator admits both generations.
13. Missing or failed GPU capability reports native-unavailable state and drops
    the screen layer. It never rewrites a request or selects CPU work to make a
    benchmark green.

Benchmarks report source pixels, output pixels, bytes, dynamic range, queue
depth, display refresh, and publication branches. A single blessed 1080p number
cannot hide superlinear work.

## 20. Implementation waves

### W0: signed TCC canary

1. Pull the W1 Developer ID signing prerequisite forward, then build the
   production-shaped signed and notarized canary.
2. Run the full ownership matrix.
3. Record the preferred-daemon or app-broker decision with receipts.
4. Benchmark end-to-end and per-hop latency for every capability that requires
   XPC.
5. Freeze each capability's process boundary before native session integration.

Exit: every capability has a process topology that satisfies section 6, and
each designated requirement is documented.

### W1: platform floor and shared vocabulary

1. Raise the Tauri and distribution minimum to 15.2. Add
   `depends_on macos: ">= :sequoia"` to
   `packaging/homebrew/hypercolor.rb` and
   `packaging/homebrew/hypercolor-app.rb`. The cask adds an exact 15.2
   `preflight` block. The formula defines a custom `Requirement` with a
   `satisfy` block so Homebrew rejects 15.0 and 15.1 before download. Add a
   numeric `sw_vers -productVersion` 15.2 floor check to
   `scripts/get-hypercolor.sh` before download or launchd mutation. Every check
   compares major, minor, and patch as integer components, never as a string or
   floating-point number. Formula, cask, and shell tests cover 14.9, 15.0, 15.1,
   15.2, 15.10, 26.0, and 26.10.
2. Update public install docs and packaging design references.
3. Add `MacosScreenCaptureKit` platform selection.
4. Add packed RGB, HDR, bi-planar YUV, range, matrix, and chroma metadata.
5. Add the macOS physical keymap and shared fixture vocabulary.
6. Add workspace dependencies for the required objc2 framework crates.
7. Smoke-test the hosted macOS runner labels and SDKs. Provision the self-hosted
   Intel or Apple Silicon replacement first if either label is unavailable,
   then pin pull-request, `build-native-app`, and `build-release` runners and
   Xcode with SDK-major and finished-artifact deployment-target audits. Add a
   `macos-amd64` standalone release row on `macos-26-intel` beside the existing
   `macos-arm64` row, matching the existing Linux `amd64` naming and
   `get-hypercolor.sh` architecture mapping. Extend `.github/workflows/ci.yml`'s
   fixed checksum and release-notes platform loop to `macos-amd64`. Give
   `packaging/homebrew/hypercolor.rb` separate ARM and Intel macOS URLs, add
   `SHA256_MACOS_AMD64`, and populate it in the workflow substitution. Admit
   `macos-amd64` in `scripts/get-hypercolor.sh`, delete its source-only Intel
   warning branch, and cover the matching signed artifact, installer, launchd
   service, and terminal path in acceptance. Change the formula service from
   `keep_alive true` to `keep_alive successful_exit: false`, assert the generated
   `homebrew.mxcl.hypercolor` plist semantics, and cover its owner-conflict zero
   exit without a respawn loop.
8. Set `MACOSX_DEPLOYMENT_TARGET = { value = "15.2", force = true }` in
   `.cargo/config.toml` and set Tauri's minimum system version to 15.2. Build and
   inspect one finished Mach-O per architecture in pull-request CI as well as
   every release artifact.
9. Add `scripts/sign-macos-artifacts.sh` as the only release signing
   orchestrator used by `.github/workflows/ci.yml` and
   `scripts/build-mac-installer.sh`. Replace the workflow's ad-hoc
   `APPLE_SIGNING_IDENTITY = "-"` release path with Developer ID Application
   signing and notarization. Update `scripts/stage-app-bundle-assets.sh` to stage
   the sidecar, then have the orchestrator pre-sign it with the explicit
   `.sidecar` identifier before `cargo tauri build`. After the app build, the
   orchestrator reapplies every manifest identifier inside out and signs the app
   last before app notarization and DMG creation. Change the CI macOS bundle
   matrix and `scripts/build-mac-installer.sh` default from `dmg,app` to `app`;
   the orchestrator runs the separate DMG packaging step only after the app is
   stapled. Update `scripts/dist.sh` to
   hand every standalone Mach-O to the same manifest-driven actor with stable
   identifiers, hardened runtime, and timestamps. Submit their exact bits for
   notarization, and make
   `scripts/verify-release-artifact.sh` reject missing signatures, mismatched
   designated requirements, identifiers, team IDs, unlisted Mach-O files, or
   notarization receipts.
   Create `packaging/macos/daemon.entitlements.plist` with the exact six Boolean
   keys carried by the current app profile:
   `com.apple.security.cs.allow-jit`,
   `com.apple.security.cs.allow-unsigned-executable-memory`,
   `com.apple.security.device.audio-input`,
   `com.apple.security.device.usb`,
   `com.apple.security.network.client`, and
   `com.apple.security.network.server`.
   This signing slice is a prerequisite pulled forward before W0 executes.

Exit: all pure types compile on every platform, Sequoia availability checks
pass, and public support claims agree.

### W2: native host input

1. Add `hypercolor-macos-input` with permission and event-tap fixtures.
2. Implement the run-loop worker and native decoder.
3. Fold events into the canonical interaction source.
4. Implement `PointerScroll` across shared serde, every host producer,
   WebSocket round-trip, LightScript, browser injection, and the TypeScript SDK.
   Correct the existing SDK comments at `sdk/packages/core/src/input/types.ts`
   for event `delta` and state `wheel`: both values are integral 1/120-notch
   units, not notches and not values divided by 120.
5. Wire independent consent, demand, status, deterministic fixture backends,
   and live reconfiguration.
6. Delete the macOS `device_query` bridge, workspace dependency, core
   dependency, exports, startup branch, tests, stale fixture labels, and the
   obsolete lock-order entry. Add the macOS native input fold lock to the same
   lock inventory.
   Update spec 72 D9 and its W3 roll-up to record that the final macOS-only
   dependency and bridge are gone.
7. Run signed keyboard and pointer acceptance.

Exit: every input test and signed acceptance row passes with no polling fallback.

### W3: ScreenCaptureKit source and fixture oracle

1. Add `hypercolor-macos-capture` and frame fixtures.
2. Implement permission preflight, picker callbacks, and source state.
3. Configure and run one native stream.
4. Validate and retain complete frames.
5. Implement the fixture-only BGRA8 correctness oracle.
6. Wire status, API actions, UI remediation, and diagnostics.

Exit: signed native acquisition is correct across topology, lifecycle, picker,
and permission acceptance. The fixture oracle matches golden fixtures and is
unavailable to production builds.

### W4: IOSurface and Metal publication

1. Split current Servo dependencies behind the `servo-context` feature and add
   the matching macOS edge to core's `servo-gpu-import` feature.
   Replace the Servo importer's hardcoded shared storage descriptor with the
   same Apple-family shared and non-Apple-family managed predicate, coherency
   probe, and readback parity required for screen capture.
2. Add the independent `screen-capture` bridge feature to macOS GPU interop.
3. Define the daemon-owned core trait wrapper and register the Metal target.
4. Import every retained IOSurface plane into wgpu.
5. Implement Apple-family detection, direct IOSurface and Core Video
   texture-cache candidates, import coherency and readback probes, wrapper
   caching, and two-phase pool admission.
6. Add the macOS capture latest-frame lock to the lock inventory, then run
   native reduction, Servo import, and fixture-oracle GPU parity on Apple
   Silicon and Intel.
7. Prove the steady-state zero-full-frame-copy contract.

Exit: native SDR capture is the only production path, production targets contain
no CPU capture, conversion, publication, reduction, or compositor executor, the
injected structural-failure matrix proves full GPU invalidation and rebuild, and
the path passes the 4K60 soak.

### W5: HDR and Tahoe capabilities

1. Implement canonical HDR stream configuration.
2. Add RGBA16Float, ARGB2101010, YUV420 video/full-range, YUV44410, and all
   required packed and multi-plane conversion kernels.
3. Implement reference-white-based LED tone mapping. Add the serde-defaulted
   `target_led_white_x`, `target_led_white_y`,
   `target_led_reference_white_nits`, `target_led_peak_nits`, and `exposure_ev`
   fields to `CaptureConfig`, their exact defaults and cross-field validation,
   the frame-boundary live-reconfiguration path from section 13.2, and the
   advanced controls and reset scope from section 14. Keep shared reference
   constants, GPU uniforms, golden vectors, and the algorithm revision in one
   parity contract.
   Thread `suppress_scene_cut_bypass` from the private macOS transition state to
   `PreparedTemporalSmoother::stage` beside `reset_history` and to
   `downscale_frame` beside `reset_smoother`, forwarding the latter into
   `TemporalSmoother::stage_for_elapsed_grid`. Keep the public `apply`,
   `apply_for_elapsed`, and `apply_for_elapsed_grid` signatures unchanged and
   have them forward `false`. Pass `true` only for the complete current macOS
   blend and `false` from every Windows, Linux, and non-transition caller.
4. Add paired SDR/HDR screenshots for HDR-capable Tahoe selections, single SDR
   reference screenshots for SDR-only Tahoe selections, and Core Graphics
   reference output for both.
5. Build and benchmark the Metal 4 reduction prototype on active devices that
   expose its required facilities.
6. Adopt or reject Metal 4 using the section 2.2 gate, with artifacts.

Exit: Apple Silicon HDR acceptance and 4K60 soak pass. Tahoe paired-range
diagnostics ship for HDR-capable selections, and the SDR reference diagnostic
ships for SDR-only selections. Each qualifying active device has a measured
Metal 4 decision.

### W6: packaging, diagnostics, and release hardening

1. Finalize purpose strings and remove the incorrect Apple Events string.
2. Invert `hypercolor-app/tests/config_tests.rs` to require the screen-capture
   purpose string and forbid the Apple Events string, then update spec 67's
   packaging inventory. The same tests parse
   `packaging/macos/daemon.entitlements.plist` and assert its exact six-key
   profile against the manifest contract in section 18.4.
3. Ship each selected TCC topology and only its required broker capabilities.
4. When direct launchd broker delegation is selected, add the
   `tech.hyperbliss.hypercolor.daemon-bootstrap` `MachServices` entry to
   `packaging/launchd/tech.hyperbliss.hypercolor.plist`. Update
   `scripts/verify-release-artifact.sh` to reject a delegated-service artifact
   whose packaged launchd plist template lacks that exact service or exposes it
   when delegation is not shipped. Packaging tests also pin the existing
   `KeepAlive.SuccessfulExit = false` rule and the launchd owner-conflict zero
   exit that prevents a three-second respawn loop.
5. Implement the per-user daemon-owner record, native watch, typed conflict
   publication through daemon system status and the ownership bus event,
   per-source convenience mirrors, identical-conflict coalescing, and
   `choose_daemon_owner` transaction across Tauri app autostart and the CLI
   launchd service plus `brew services`. Start the watch before source
   construction. Implement the separate durable, versioned handover journal
   with atomic replacement, file and parent-directory `fsync`, typed path-free
   operations, one stable coordination lock shared by every owner-record and
   journal writer, single-read-modify-write lock scope, and crash recovery from
   every mutating phase.
   Implement the bounded flush, stop, guard-release, selected-owner startup,
   synchronous result, and rollback sequence in the surviving local app or CLI
   coordinator. Keep owner selection unreachable from REST, WebSocket, MCP, and
   every other network surface. Persist external-owner mode, suppress sidecar
   startup while it is active, publish the offline-owner status with the
   topology-specific local start remedy, implement the pending standalone-stop
   remedy without remote termination, and teach the app supervisor that its
   typed sidecar owner-conflict exit is non-restartable.
6. Complete CLI, UI, metrics, and diagnostic tools.
7. Regenerate the vendored Python client after the additive optional
   `SystemStatus.macos_daemon_ownership` and `InputSourceStatus.platform`
   fields land. Add `macos_daemon_ownership_changed_v1` to
   `protocol/websocket-v1.json`, regenerate its Python protocol constants, and
   require both `python-generate-check` and `python-ws-protocol-check` to return
   no diff.
8. Run Apple Silicon and Intel signed acceptance.
9. Run the four-hour combined soak and memory reconciliation.
10. Update compatibility and installation documentation.
11. Update the canonical `AGENTS.md` file, also read through its `CLAUDE.md`
    symlink, with both new crates in the crate list and dependency graph and both
    audited unsafe opt-outs in the conventions inventory.
12. Update specs 14, 57, 71, and 72 to link to this spec as the macOS authority.
    In spec 57, revise the implemented status at line 3 to record that the
    hardcoded shared-storage Servo importer was Apple-Silicon-only, then mark its
    Intel parity precondition at lines 355-357 discharged only after W4's
    family-aware storage selection passes this spec's signed Intel acceptance.
    In spec 72, revise D9 at line 893 and the W3 roll-up at line 1028 after the
    final `device_query` bridge and dependency are deleted.

Exit: every section 21 criterion is satisfied.

## 21. Completion criteria

The macOS feature is complete when:

- the product floor is 15.2 everywhere users or packaging can observe it;
- every released macOS code object has its stable Developer ID identifier,
  hardened-runtime signature, designated requirement, and accepted notarization;
- the signed TCC owner is proven and stable;
- the single-instance arbiter exposes exactly one active daemon owner and a
  typed conflict for every losing installed topology;
- keyboard and pointer input are native, event-driven, independently gated, and
  free of `device_query`;
- screen capture uses Apple's system picker and complete lifecycle state;
- Metal output matches the fixture-only oracle on canonical fixtures;
- production artifacts contain no CPU capture, conversion, publication,
  reduction, or compositor executor for macOS screen input;
- injected structural GPU failures clear retained output, reject stale target
  generations, rebuild the complete GPU target, and become unavailable without
  CPU recovery when rebuilding cannot succeed;
- the Servo importer selects family-correct storage and passes signed Intel
  CPU-oracle byte parity under IOSurface reuse;
- native SDR passes every supported Mac row;
- native HDR and tone mapping pass Apple Silicon rows;
- Tahoe paired-range GPU diagnostics ship for HDR-capable selections, and the
  completed-GPU SDR diagnostic ships for SDR-only Tahoe selections;
- every active device exposing the required Metal 4 facilities has benchmark
  artifacts and a recorded adoption decision;
- no stale generation, pinned allocation, or held input survives teardown;
- pull-request CI covers macOS compilation, lint, and platform fixtures;
- signed physical acceptance and performance contracts pass; and
- specs 14, 57, 71, and 72 link to this spec as the implemented macOS authority;
  spec 57 records the family-aware importer and discharged signed Intel parity
  precondition, while spec 72 D9 and its W3 roll-up record full `device_query`
  retirement.

## 22. Rejected alternatives

### Keep `device_query`

Rejected because polling loses native event fidelity, cannot expose TCC state,
and violates independent keyboard and pointer consent.

### Use Accessibility permission for input listening

Rejected because passive listening belongs to Input Monitoring. Accessibility
would grant a broader capability Hypercolor does not need.

### Use `IOHIDManager` for the first release

Rejected because per-device identity is outside the current product contract and
would expand hotplug, permission, and key translation work. `CGEventTap` matches
the requested session-level input semantics.

### Build a custom source picker

Rejected because Apple's system picker is the privacy and platform integration
contract on supported macOS versions.

### Capture only a pre-scaled 640x480 or 1080p surface

Rejected because it permanently destroys source fidelity and violates exact
descriptor and arbitrary-resolution contracts.

### Start with CPU capture as the permanent macOS path

Rejected because a full-frame readback and upload cannot meet the intended
native-resolution, high-refresh product ceiling. CPU capture remains a
fixture-only oracle and is never a production fallback.

### Force every Tahoe system onto a separate Metal 4 renderer

Rejected because API novelty alone does not pay for a second command stack. The
required prototype and 10 percent gate turn Tahoe capability into measured
capacity rather than branding.

## 23. Primary sources

- [ScreenCaptureKit framework and required screen-capture purpose string](https://developer.apple.com/documentation/screencapturekit)
- [Capturing screen content in macOS](https://developer.apple.com/documentation/screencapturekit/capturing-screen-content-in-macos)
- [System content-sharing picker](https://developer.apple.com/documentation/screencapturekit/sccontentsharingpicker)
- [WWDC23: What's new in ScreenCaptureKit](https://developer.apple.com/videos/play/wwdc2023/10136/)
- [WWDC24: Capture HDR content with ScreenCaptureKit](https://developer.apple.com/videos/play/wwdc2024/10088/)
- [CGPreflightListenEventAccess](https://developer.apple.com/documentation/coregraphics/cgpreflightlisteneventaccess%28%29)
- [CGRequestListenEventAccess](https://developer.apple.com/documentation/coregraphics/cgrequestlisteneventaccess%28%29)
- [CGEventTapCreate](https://developer.apple.com/documentation/coregraphics/cgevent/tapcreate%28tap%3Aplace%3Aoptions%3Aeventsofinterest%3Acallback%3Auserinfo%3A%29)
- [IOSurface](https://developer.apple.com/documentation/iosurface)
- [IOSurfaceCreateXPCObject](https://developer.apple.com/documentation/iosurface/iosurfacecreatexpcobject%28_%3A%29)
- [Metal shared storage](https://developer.apple.com/documentation/metal/mtlstoragemode/shared)
- [Metal managed storage](https://developer.apple.com/documentation/metal/mtlstoragemode/managed)
- [Setting Metal resource storage modes](https://developer.apple.com/documentation/metal/setting-resource-storage-modes)
- [MTLDevice supportsFamily](https://developer.apple.com/documentation/metal/mtldevice/supportsfamily%28_%3A%29)
- [CVMetalTextureCacheCreateTextureFromImage](https://developer.apple.com/documentation/corevideo/1479231-cvmetaltexturecachecreatetexture)
- [NSXPCListener Mach services](https://developer.apple.com/documentation/foundation/nsxpclistener/init%28machservicename%3A%29)
- [SMAppService](https://developer.apple.com/documentation/servicemanagement/smappservice)
- [Hardened Runtime](https://developer.apple.com/documentation/security/hardened_runtime)
- [Notarizing macOS software before distribution](https://developer.apple.com/documentation/security/notarizing_macos_software_before_distribution)
- [Audio Input Entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.device.audio-input)
- [Allow JIT-compiled code entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.cs.allow-jit)
- [Allow unsigned executable memory entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.cs.allow-unsigned-executable-memory)
- [GitHub-hosted macOS runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
- [Resetting access to protected resources](https://developer.apple.com/documentation/xcode/resetting-access-to-protected-resources-in-macos)
- [NSAppleEventsUsageDescription](https://developer.apple.com/documentation/bundleresources/information-property-list/nsappleeventsusagedescription)

Local API availability and exact constants were verified against the installed
macOS 26.5 SDK headers for ScreenCaptureKit, Core Graphics, IOSurface, and Metal.

## 24. Review history

| Round | Reviewer    | Verdict       | Findings                     | Resolution                                                                                                                                                                                                 |
| ----- | ----------- | ------------- | ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1     | Claude Opus | NEEDS_CHANGES | 2 blocker, 8 high, 10 medium | All 20 adjudicated in revision 2; architecture, lifecycle, fidelity, resource, Intel, CI, and cleanup contracts revised                                                                                    |
| 2     | Claude Opus | NEEDS_CHANGES | 2 high, 7 medium, 5 low      | All 14 adjudicated in revision 3; Intel coherency, shared scroll, status carriage, fixtures, release lanes, broker bootstrap, and cleanup revised                                                          |
| 3     | Claude Opus | NEEDS_CHANGES | 5 medium, 3 low              | All 8 adjudicated in revision 4; storage selection, Core Video import, lossless scroll units, legacy events, runner availability, and cleanup revised                                                      |
| 4     | Claude Opus | NEEDS_CHANGES | 2 medium, 3 low              | All 5 adjudicated in revision 5; wheel units, Tahoe architecture capability, dependency edges, inbound injection, and spec 72 cross-links revised                                                          |
| 5     | Claude Opus | NEEDS_CHANGES | 4 medium, 5 low              | All 9 adjudicated in revision 6; Servo storage, CPU execution, Tahoe selection scope, launchd ownership, broker namespace, CI qualification, lock inventory, deployment floor, and crate inventory revised |
| 6     | Claude Opus | NEEDS_CHANGES | 1 medium, 4 low              | All 5 adjudicated in revision 7; launchd broker bootstrap, owner and remedy enums, Intel Servo acceptance, and Rosetta host detection revised                                                              |
| 7     | Claude Opus | NEEDS_CHANGES | 2 medium, 3 low              | All 5 adjudicated in revision 8; broker restart recovery, Developer ID release signing, daemon-owner arbitration, launchd plist packaging, and Tahoe status publication revised                            |
| 8     | Claude Opus | NEEDS_CHANGES | 2 medium, 3 low              | All 5 adjudicated in revision 9; launchd conflict exits, sidecar identity, exhaustive Mach-O signing, owner-arbitration implementation, and packaged-plist verification revised                            |
| 9     | Claude Opus | NEEDS_CHANGES | 2 medium, 2 low              | All 4 adjudicated in revision 10; deterministic post-Tauri signing, source-independent owner status, app stapling, and local installer parity revised                                                      |
| 10    | Claude Opus | NEEDS_CHANGES | 3 medium, 1 low              | All 4 adjudicated in revision 11; per-object entitlements, split app and DMG bundling, Intel standalone artifacts, and Python client regeneration revised                                                  |
| 11    | Claude Opus | NEEDS_CHANGES | 1 medium, 2 low              | All 3 adjudicated in revision 12; Intel artifact consumers, daemon entitlement creation, and non-sandbox entitlement semantics revised                                                                     |
| 12    | Claude Opus | NEEDS_CHANGES | 2 medium                     | Both findings adjudicated in revision 13; Homebrew service ownership and pre-install macOS floor enforcement revised                                                                                       |
| 13    | Claude Opus | NEEDS_CHANGES | 1 medium, 2 low              | All 3 adjudicated in revision 14; live owner handover, Homebrew requirement mechanics, and component-wise version tests revised                                                                            |
| 14    | Claude Opus | NEEDS_CHANGES | 2 medium, 2 low              | All 4 adjudicated in revision 15; standalone pending handover, persisted external-owner mode, bounded rollback, and incoming-daemon event publication revised                                              |
| 15    | Claude Opus | NEEDS_CHANGES | 2 medium, 1 low              | All 3 adjudicated in revision 16; durable handover recovery, local-only owner selection, and offline-owner remediation revised                                                                             |
| 16    | Claude Opus | NEEDS_CHANGES | 1 medium, 1 low              | Both findings adjudicated in revision 17; journal storage and locking plus the WebSocket manifest contract revised                                                                                         |
| 17    | Claude Opus | NEEDS_CHANGES | 1 medium, 1 low              | Both findings adjudicated in revision 18; LED target calibration and ownership-event schema conformance revised                                                                                            |
| 18    | Claude Opus | NEEDS_CHANGES | 1 medium, 1 low              | Both findings adjudicated in revision 19; one-stop default highlight headroom and calibration-reset scope revised                                                                                          |
| 19    | Claude Opus | NEEDS_CHANGES | 1 medium                     | The finding was adjudicated in revision 20; full-scale SDR parity and deterministic SDR/HDR transition behavior revised                                                                                    |
| 20    | Claude Opus | NEEDS_CHANGES | 1 medium, 1 low              | Both findings adjudicated in revision 21; smoother interaction, measurement conditions, and zero-exposure endpoints revised                                                                                |
| 21    | Claude Opus | NEEDS_CHANGES | 2 low                        | Both findings adjudicated in revision 22; marker restart and cross-platform no-op semantics revised                                                                                                        |
| 22    | Claude Opus | NEEDS_CHANGES | 1 medium, 1 low              | Both findings adjudicated in revision 23; real smoother targets and the private-field-safe metadata builder revised                                                                                        |
| 23    | Claude Opus | NEEDS_CHANGES | 1 medium                     | The finding was adjudicated in revision 24; the unreachable metadata carrier was replaced with direct smoother parameters                                                                                  |
| 24    | Claude Opus | NEEDS_CHANGES | 1 low                        | The finding was adjudicated in revision 25; the final smoother seam and wrapper defaults were made exact                                                                                                   |
| 25    | Claude Opus | NEEDS_CHANGES | 1 low                        | The finding was adjudicated in revision 26; spec 57 authority and Intel parity reconciliation were added                                                                                                   |
| 26    | Claude Opus | PASS          | None                         | No actionable issue remained at any severity; implementation-ready                                                                                                                                         |
