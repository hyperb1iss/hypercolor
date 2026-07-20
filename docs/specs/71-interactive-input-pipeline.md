# Spec 71: Interactive Input Pipeline

Status: PROPOSED (cross-model reviewed: Codex gpt-5.6-sol adversarial pass, 2 blockers
+ 13 majors folded in)
Depends on: none. Related: spec 69 (faces share the payload/adapter machinery).

## Problem

Effects cannot react to keyboard or mouse input in practice, even though most of the
backend pipeline already exists. The failure concentrates in three places: the capture
layer is X11-only in a Wayland world, the SDK authoring surface exposes nothing, and
the parts that do exist have never had a real consumer, so their gaps (event ordering,
dirty-checks, permissions, privacy) have never been forced.

## Verified current state

**Working today (X11 sessions only):**

- `InteractionInput` (`core/src/input/interaction/mod.rs`): device_query polling at
  100 Hz → `InteractionData { keyboard: { pressed_keys, recent_keys }, mouse:
  { x, y, down, buttons } }`. Registered unconditionally
  (`daemon/src/startup/services.rs:568`). On Wayland it starts cleanly and produces
  nothing (device_query is X11; `DISPLAY` from XWayland satisfies its session check).
- Render plumbing: `InputData::Interaction` → `FrameInputs::sample`
  (`daemon/src/render_thread/pipeline_runtime.rs:172`) → `FrameInput.interaction`
  (`core/src/effect/traits.rs:75`), every renderer, every frame.
- Servo delivery, gated by `effect_uses_interaction_data`
  (`core/src/effect/servo/renderer.rs:350`): category `Interactive` or tags
  `interactive|input|mouse|keyboard` → `LightScriptInteractionPayload`
  (dirty-checked via full `InteractionData` equality, `lightscript.rs:525`) →
  `frame_payload_adapter.js` → `engine.keyboard.keys/recent`,
  `engine.mouse.{x,y,down,buttons}`.
- LightScript already installs `engine.keyboard.isKeyDown()`, `consumePressedKeys()`,
  `wasKeyPressed()`, `engine.mouse.isDown()` stubs (`lightscript.rs:347`), and the
  WebGL runtime registers an `iMouse` uniform permanently set to `[0, 0]`
  (`sdk/packages/core/src/effects/webgl-effect.ts:83`). The API contracts exist;
  nothing feeds them.
- `EvdevKeyboardInput` (`core/src/input/evdev.rs`, Linux): true evdev key event
  stream → `InputEvent::Key { source_id, key, state }` → drained per frame →
  `HypercolorEvent::InputEventReceived` on the bus. Keyboard-only, enumerates once
  (no hotplug), cannot distinguish "no hardware" from "permission denied".
- SDK effects CAN opt into the Servo gate today via `category: 'interactive'` —
  but there is no input capability, no typed accessor, and no effect does it.

**Confirmed gaps:**

1. No Wayland-viable capture; no mouse source outside device_query at all.
2. **Privacy leak (blocker):** `InputEventReceived` relays on the default-subscribed
   WS `events` channel; only screen channels require control-tier subscription
   (`daemon/src/api/ws/protocol.rs:96`). The moment capture works, key names +
   press/release order stream to any read-tier client. Ordered keys reconstruct
   typed text; "no text buffers" is not a defense.
3. **No SDK metadata path (blocker):** `dataSourcesFromDef` emits only
   `media|net|lighting`, `effectHtml()` takes no data-sources at all (faces-only
   template), and the daemon parser allowlists the same three names
   (`meta_parser.rs:498`). An `input` token dies before reaching the daemon.
4. Aggregate-state-only semantics: no release events, no timestamps, no wheel, no
   normalized coordinates, no per-source identity.
5. Zero consumers: no builtin, catalog, or conformance effect exercises any of it.
6. No config section, no live-apply, no status/diagnostics surface, no consent UX.

## Design principles (resolved by fiat)

- **udev, never a system group.** Linux access rides uaccess ACLs in the shipped
  rules file. `input`-group membership is never required or documented.
- **Cross-platform is first-class.** Linux, Windows, macOS each get a real
  event-driven backend behind one contract; no platform is fallback-quality.
- **X11 is not a target.** Linux support means Wayland-and-evdev; no X11-specific
  capture path exists or will be maintained.
- **Default-off with explicit consent.** Host input capture starts disabled.
  Enabling it is a deliberate act in config/UI, and input-derived events never
  reach unauthorized WS clients.

## D1. Source architecture

`InteractionInput` stays the public `InputSource` facade (name survives; browser,
MIDI, and remote sources will join the same interaction model, so the facade must
not be host-specific). Internally it drives per-platform `HostInputBackend`
implementations that are pure event producers; all state folding lives in one
shared, platform-independent module.

**Backends:**

- **Linux — evdev** (first-class, Wayland-proof). Extends the existing evdev code
  with: pointer devices (`REL_X/REL_Y`, `BTN_LEFT..BTN_EXTRA`,
  `REL_WHEEL/REL_WHEEL_HI_RES`), hotplug monitoring (re-enumerate on udev events,
  re-open after ACL changes), per-path open results (opened / permission-denied /
  ignored) so diagnostics can say exactly why input is idle, and a fixed keyboard
  classifier (the current one drops composite keyboard+pointer devices). v1 scope
  is keyboards + relative mice; touchpads/touchscreens (EV_ABS/multitouch) are
  explicitly out until a libinput-class backend (v2) — raw ABS without compositor
  normalization is wrong-shaped for effects.
- **Windows — Raw Input** (`WM_INPUT`, message-only window, `RIDEV_INPUTSINK`,
  `RIDEV_DEVNOTIFY` for hotplug). Lives in a new audited `hypercolor-windows-input`
  interop crate (core forbids unsafe; the PawnIO crate stays SMBus-scoped).
  Elevated-process keystrokes are invisible (UIPI) — documented, accepted.
- **macOS — CGEventTap** at **session tap** placement (HID-level taps require
  root; the daemon runs per-session). Requires Input Monitoring TCC; request on
  first enable, degrade gracefully, surface TCC state in diagnostics.
- **Browser preview — injection source** (all platforms, no permissions). The
  web UI's `CanvasPreview` gains an interactive mode: a control-authorized WS
  upstream message carries pointer/key input with a per-connection `source_id`,
  content-rect-normalized coordinates, pointer capture, move coalescing, and
  synthesized releases on blur/visibility-loss/unmount/disconnect. This is the
  cross-platform dev/test path and a real feature (drive your rig from a phone).
  Browser sources and host sources are never merged implicitly.
- **device_query — Windows/macOS bridge only, then deleted.** Linux is evdev-only:
  we owe X11 nothing — the modern world is Wayland, and evdev works identically
  under both anyway. device_query survives solely as the health-reported interim
  backend on Windows/macOS until their native backends land (W6), then leaves the
  dependency tree entirely. It is never constructed on Linux.

**Per-device source association (prior art: uchroma).** The Linux backend resolves
each opened event node to its owning managed device when the node's USB parent
matches a HAL device fingerprint — so `source_id` can carry a `DeviceId`, not just
a devnode path. uchroma proved this model (`uchroma/server/input.py`:
`InputManager(driver, input_devices)` opens exactly the RGB device's own event
nodes, on-demand, closing when the last consumer detaches). It aligns three
things at once: the vendor-scoped udev uaccess rules cover precisely these nodes,
demand gating gets per-device granularity, and phase-2 key-to-LED mapping needs
device identity on every event — which this preserves from day one.

**Demand + lifecycle contract:**

- Capture runs only when `[input].enabled` (consent) AND an active effect declares
  input. Extend `EffectDemand`/`CaptureDemandState`
  (`daemon/src/render_thread/capture_demand.rs`) with an interaction field,
  mirroring audio/screen.
- Permission denial is a **degraded backend state**, never an `InputSource::start()`
  error (start errors roll back the entire input graph, `input/mod.rs:164`).
- Demand-off closes capture handles and clears held keys, buttons, and queued
  events. Device removal, seat deactivation, session lock, and backend shutdown
  synthesize releases — no stuck inputs, ever.
- One source produces both snapshot and events: `FrameInputs::sample` gets an
  atomic fan-out (single hub drain feeding both) so nothing is consumed twice or
  lost between `sample_all` and `drain_events`.
- Multi-device semantics: held state tracked per `source_id` and unioned (releasing
  `A` on keyboard B must not clear `A` held on keyboard A). Pointer positions are
  never averaged: one documented primary pointer (most recently active), per-source
  identity exposed. Linux applies an active-seat policy so a user daemon cannot
  observe another session's devices.

**Linux permissions (udev uaccess):**

Extend `udev/99-hypercolor.rules` with `SUBSYSTEM=="input"` entries for supported
vendor families — uaccess ONLY for input event nodes, deliberately no
`GROUP="users"` fallback there (a group-readable event node is a keylogging grant
to every member; the hidraw fallback rationale does not transfer). Contract-test in
`hal/tests/udev_rules_tests.rs`. Honest boundary, documented: vendor-scoped rules
cover the RGB gear we drive (verified live: Dygma DEFY event nodes already carry
the per-user ACL); laptop internals, Bluetooth keyboards, and generic mice need the
optional, clearly-labeled `udev/99-hypercolor-input-all.rules` — a deliberate,
opt-in security posture change. Install stays `just udev-install`.

## D2. Data model

Two layers, replacing "extend InteractionData and hope":

- **`InteractionState`** (stable, comparison-friendly): pressed key set, button
  set, primary pointer `virtual_pointer { x, y }` in `[0,1]²` (named for what it
  is — accumulated gesture position, not the OS cursor), coordinate availability
  mode (`absolute | virtual | none`), wheel accumulator. Finite-sanitized floats.
  Kept `PartialEq`-cheap; drives the payload dirty check via an explicit
  generation counter bumped on real change, not full-struct equality.
- **`InteractionBatch`** (transient, per-frame): bounded, ordered event list —
  extending the canonical `InputEvent` vocabulary (which already carries
  `source_id` and MIDI variants) with mouse button/wheel/motion-aggregate events,
  each stamped with capture timestamp and sequence number. Pointer motion is
  aggregated per frame, never queued per 1000 Hz event. Overflow drops oldest and
  increments a visible dropped-event counter.

`FrameInput.interaction` carries both. The existing `InteractionData` shape remains
as the derived compatibility view until the payload migration completes. No serde
concern exists on the Rust struct (the wire contract is the LightScript payload).

## D3. Delivery (Servo/LightScript)

- Payload v2: state (on generation change) + batch (when non-empty), keeping all
  existing field shapes so deployed HTML keeps working. Idle frames skip cleanly
  because generation doesn't move and batches are empty.
- Servo frame queue: superseded frames **append** their batches into the surviving
  frame (the queue currently string-dedups `recent_keys`, which destroys ordering
  and repeat counts — `frame_queue.rs:316`). Timestamps and sequence numbers
  survive coalescing; dropped counts are reported.
- Adapter: feed the existing `engine.keyboard.isKeyDown`/`wasKeyPressed`/
  `consumePressedKeys`/`engine.mouse.isDown` contracts for real, add
  `engine.keyboard.events` (ordered, timestamped) and
  `engine.mouse.{nx, ny, wheel, velocity, available}`.
- **WS privacy fix lands in the same wave capture is enabled:** input events move
  off the default relay behind a control-tier-authorized subscription (same
  mechanism as screen channels, `protocol.rs:96`).

## D4. SDK

- Canonical **`input-reactive`** capability: `input?: boolean` on
  `effect()`/`canvas()` options → `inputReactive` in `ExtractedArtifactMetadata` →
  `<meta input-reactive="true"/>` emitted by `effectHtml()` (mirroring
  `audio-reactive`, not the faces-only data-sources channel) → parsed in
  `meta_parser.rs` next to `detect_audio_reactive_meta_tag` → one shared
  `requires_interaction()` predicate used by BOTH Servo injection and capture
  demand. `category: 'interactive'` keeps working as a legacy gate.
- New `sdk/packages/core/src/input/` module mirroring the audio module:
  `getInputData()` with typed state + events, `available`/`degraded` exposed
  honestly (no silent zero-fill masking a missing declaration). Helpers computed
  SDK-side from timestamped events: `pressEnvelope()`, `wasdVector()`,
  `typingRate()` (timestamp-based, frame-rate-independent), and
  `keyToGridPosition()` — documented as an approximate QWERTY projection. Physical
  key-code and `source_id` fields are preserved in the types now so real
  key-to-LED spatial mapping (phase 2) needs no breaking change. Envelope design
  follows uchroma's `InputQueue` (`~/dev/uchroma/uchroma/input_queue.py`): each
  event carries timestamp + duration so completion percentage is derivable by any
  consumer — the event is its own decay curve; re-press replaces the live event
  for that key.
- WebGL: populate the existing **`iMouse`** with documented normalized semantics;
  add `iMouseDown` and `iWheel` (the `i*` convention is what shader validation
  recognizes — no `u_*` parallel namespace). Envelope-style values stay SDK-side;
  no lossy presentation state in uniforms.
- Validation: `input: true` is authoritative. A source-scan lint warns when input
  APIs appear without the declaration (lint, not gate — the scan can't see through
  imports).

## D5. Proof: conformance fixtures + showcase

Focused fixtures first (these are the pipeline proof):

- Canvas2D fixture: renders last N events with timestamps/ordering visible.
- WebGL fixture: `iMouse`/`iMouseDown`/`iWheel` sanity.
- Native fixture: minimal builtin reading `FrameInput.interaction`.
- Browser-injection fixture: preview-sourced input, no host capture.

Then **Keystrike**, the showcase (Canvas2D, category Interactive, `input: true`):
key presses spawn chromatic ripples from `keyToGridPosition()` origins; the
virtual pointer is a roaming light source; click = shockwave; wheel rotates
palette hue; velocity feeds trail intensity; idle = low ambient breathing. No
audio blend in v1 (it would mask input failures); LED discipline per
rgb-effect-design. Reference implementation: uchroma's Ripple renderer
(`~/dev/uchroma/uchroma/fxlib/ripple.py`) — quintic ease over event lifetime,
multi-ring color schemes stepped from one base color, per-key timestamp dedup
against repeats, radius normalized by canvas hypotenuse. Port the shape, not
the code (LGPL source, Apache target — Bliss authored it and can relicense,
but a clean rewrite in SDK idiom is the plan anyway).

## D6. Config, UI, status (a real wave, not a footnote)

- `[input]` config section (`enabled` default **false**, `keyboard`, `mouse`);
  live-apply path alongside audio/capture handling; `system/status` + MCP
  `diagnose` report backend, per-device open results, and degraded states.
- Web UI: consent + enable/disable, backend health, permission remediation
  (udev install hint, macOS TCC deep-link), preview interactive mode toggle.
- TUI: never forwards its own navigation keys; interactive capture only via an
  explicit focused mode.
- Verification must include `just ui-test`/`just ui-build` explicitly
  (`hypercolor-ui` is outside the workspace).

## Waves

- **W0** — land this spec; dependency/license/unsafe-boundary investigation for
  all three platforms + browser path (cargo-deny early, before the backend
  contract freezes).
- **W1** — data model (`InteractionState`/`InteractionBatch`, `InputEvent`
  extensions) + shared state-folding + WS privacy fix + config section +
  demand plumbing. Platform-independent tests throughout.
- **W2** — Linux evdev backend v2 (pointer, hotplug, open-results, classifier
  fix) + udev rules extension + payload/adapter/frame-queue v2 + fixtures
  (canvas2d/webgl/native).
- **W3** — SDK: input-reactive capability end-to-end, input module, iMouse
  family, validation lint.
- **W4** — browser-preview injection source (daemon WS message + UI) +
  injection fixture.
- **W5** — Keystrike + screenshots + effect-reviewer pass.
- **W6** — Windows Raw Input crate + macOS CGEventTap backend (contract-gated,
  can trail the Linux ship) + device_query retirement.
- **W7** — docs (docs/content interactive-effects page, permissions guide),
  UI consent polish, cross-model review of the full diff.

## Open product questions

1. Consent UX: config-flag default-off (chosen) — is a first-run UI prompt wanted
   on top, or is settings-page opt-in enough?
2. Browser-preview input priority: W4 as scheduled, or promote it ahead of the
   Linux backend since it unblocks all-platform dev immediately?
3. libinput backend for touchpads: v2 confirmed, or is relative-mice-only too
   thin for the v1 story on laptops?
4. Per-key LED spatial mapping (key → physical LED on the device → canvas
   position): phase 2 confirmed; hooks (key codes, source ids) are preserved now.
   Concrete head start exists: uchroma ships curated evdev-keycode → LED-matrix
   `[row, col]` mappings (`~/dev/uchroma/uchroma/server/data/keyboard.yaml` +
   six laptop entries in `laptop.yaml`, `!!omap`, multi-cell keys supported).
   Phase 2 imports these into `data/drivers/vendors/` as a `key_map` section —
   re-derived under Apache-2.0 (Bliss authored the originals) — and resolves
   event `DeviceId` + keycode → LED position → canvas coords through the
   existing spatial layout, making Keystrike ripples physically exact on mapped
   keyboards.

## Post-validation follow-ups (codex adversarial pass, 2026-07-19)

The full-branch validation confirmed the pipeline shippable after the
fix wave, with three findings deliberately deferred as scoped follow-ups:

1. **Managed-device / active-seat scoping (evdev).** v1 opens every
   readable keyboard/pointer node and uses the devnode path as
   `source_id`. Follow-up: resolve nodes to HAL device fingerprints
   (udev USB-parent walk), prefer managed devices, apply an active-seat
   policy, and use stable identifiers. Pairs with the phase-2 key-to-LED
   work, which needs the same association.
2. **Degraded-health surface.** `EvdevHostInput::device_status()` exists
   but has no path through the `InputSource` trait into `system/status`,
   MCP diagnose, or the SDK's `available` semantics. Follow-up: a
   diagnostics channel on the trait + API/MCP/UI wiring (the spec's D6
   remediation UX depends on it).
3. **Timed metadata on the bus.** `InputEventReceived` relays the raw
   event; `at_ms`/`seq` are frame-batch-only, so WS automation clients
   cannot reconstruct capture order. Follow-up: carry `TimedInputEvent`
   on the bus event (breaking change for event consumers — coordinate).
