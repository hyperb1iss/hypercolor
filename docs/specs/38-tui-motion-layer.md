# Spec 38 — TUI Motion Layer

> Implementation-ready specification for integrating tachyonfx into the
> Hypercolor TUI, turning a functional but static terminal interface into a
> living control surface that breathes with audio, pulses with device
> events, and transitions between states with the same energy the daemon
> renders to physical LEDs.

**Status:** Draft
**Author:** Nova
**Date:** 2026-04-09
**Crates:** `hypercolor-tui`
**Related:** `docs/specs/37-cli-completeness-and-styling.md`, `docs/specs/15-cli-commands.md`
**Depends On:** Spec 37 Phase 4 (shared `hypercolor-types::ws` module)

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Design Principles](#4-design-principles)
5. [Motion System Architecture](#5-motion-system-architecture)
6. [Effect Catalog](#6-effect-catalog)
7. [Reactive Layers](#7-reactive-layers)
8. [Accessibility and Motion Sensitivity](#8-accessibility-and-motion-sensitivity)
9. [Performance Budget](#9-performance-budget)
10. [Dependency Changes](#10-dependency-changes)
11. [Migration Plan](#11-migration-plan)
12. [Verification Strategy](#12-verification-strategy)
13. [Open Questions](#13-open-questions)
14. [Recommendation](#14-recommendation)

---

## 1. Overview

Hypercolor is an RGB lighting engine. Its daemon renders 60 FPS color
animations to hundreds of physical LEDs. Its TUI, today, is a table of
text with one shimmer animation in the title bar. The gap between what the
daemon is doing and what the user sees in the terminal is the entire
emotional payload of the product.

This spec introduces a motion layer powered by tachyonfx 0.25 that makes
the TUI feel like it belongs to the same system that controls the lights.
The motion layer is not decorative chrome. It is a communication channel:
device events become visible arrivals and departures, effect transitions
become crossfades rather than instant swaps, the audio spectrum drives
border energy so you can feel the beat without looking at the spectrum bar,
and idle state becomes a slow breathing pulse rather than a static frame.

The spec covers three things:

- **Architecture.** An `EffectManager<MotionKey>` integrated into the
  existing `App` render loop, ticked on every frame, keyed so that new
  events cancel stale animations without stacking. Effect triggers are
  `Action` variants, so the existing component broadcast system drives
  motion without parallel state.
- **Effect catalog.** Twelve specific motion effects, each mapped to a
  daemon event or UI state transition, with tachyonfx composition details,
  easing curves, durations, and spatial patterns.
- **Reactive layers.** Three continuous data streams from the daemon
  (spectrum, canvas, metrics) drive persistent visual modulation on borders,
  backgrounds, and accent regions, giving the TUI a living baseline on top
  of which discrete event animations play.

The result is a terminal that looks and feels like a lighting controller.

---

## 2. Problem Statement

### 2.1 Static in a Dynamic Domain

The TUI has exactly one animation: the title bar shimmer
(`chrome/title_bar.rs:15-197`), which advances a phase counter by 0.12
radians per frame and applies four stacked sine waves to color-grade each
character of "HYPERCOLOR" through the brand gradient. It is well-executed
and attractive. It is also the only thing on screen that moves.

Everything else renders once when data arrives and holds still until the
next data update. Effect activation is an instant table-row highlight swap.
Device connection is a row appearing in a list. Scene activation is a
status bar text change. The canvas preview updates at 15 FPS (the only
other thing that moves, because it literally is a video feed), but the
chrome around it is frozen.

This makes the TUI feel like a monitoring dashboard rather than a control
surface for a reactive lighting system. The disconnect is sharpest when
the daemon is rendering an audio-reactive effect to physical hardware:
the LEDs are pulsing to music, and the TUI displaying that information
looks like a spreadsheet.

### 2.2 Manual Animation Code

The title bar shimmer is hand-rolled: four trigonometric layers, manual
phase advancement, inline gradient interpolation. It works but does not
compose: adding a second animation requires duplicating the pattern. There
is no shared lifecycle, no easing library, no way to sequence or combine
effects, no way to cancel an in-progress animation when a new event
arrives.

tachyonfx already solves all of these problems for ratatui applications.
It provides 50+ composable shaders, 32 easing curves, spatial patterns,
cell filters, keyed lifecycle management, and a `Frame::render_effect()`
extension trait that integrates cleanly with ratatui's existing draw
model. The library targets `ratatui-core 0.1` and is tested against
`ratatui 0.30`; hypercolor currently uses `ratatui 0.29`. Compatibility
is addressed in Section 10.

### 2.3 Untapped Data Streams

The bridge already subscribes to four WebSocket channels:

- **canvas** — 15 FPS RGB pixel data (used by the half-block preview)
- **spectrum** — 15 FPS audio frequency bins (used by the spectrum bar)
- **events** — device/effect/scene state changes (used for REST refresh)
- **metrics** — render timing, FPS, LED count (used by dashboard)

The canvas and spectrum streams carry the same energy the daemon is
rendering to hardware, but the TUI only uses them in their designated
widgets. The rest of the chrome (borders, backgrounds, separators,
status text) ignores this data entirely. These streams are the raw
material for reactive motion; this spec puts them to work.

---

## 3. Goals and Non-Goals

### Goals

- Integrate tachyonfx 0.25 as the motion engine for all TUI animations,
  replacing the hand-rolled title bar shimmer and establishing a pattern
  every future animation follows.
- Twelve discrete motion effects keyed to daemon events and UI state
  transitions (Section 6).
- Three continuous reactive layers driven by live daemon data streams
  (Section 7).
- A `MotionSensitivity` setting (off / subtle / full) that controls the
  amplitude of all motion effects for accessibility and terminal
  compatibility.
- Motion effects degrade gracefully when the terminal does not support
  truecolor: effects that depend on HSL shifts or gradient interpolation
  fall back to style-only changes (bold, dim, reverse) in 256-color mode.
- The render budget for all motion effects combined is under 3ms per frame
  at 15 FPS, verified by timing instrumentation.

### Non-Goals

- No changes to the daemon's WebSocket protocol or stream content.
- No new WebSocket channels; the existing four are sufficient.
- No GPU-accelerated rendering or sixel/kitty image protocol usage. The
  motion layer operates entirely within ratatui's cell-based `Buffer`.
- No physics simulation, particle systems, or generative art. The effects
  are predefined compositions of tachyonfx primitives, not runtime-
  generated shader programs.
- No persistence of motion state across TUI restarts. Effects are
  ephemeral; the TUI starts clean every time.
- No motion in the CLI. Spec 37 covers CLI styling; the CLI is one-shot
  and has no render loop. Motion belongs only in the TUI.

---

## 4. Design Principles

**The TUI is a window into the lighting system, not a separate surface.**
If the daemon is rendering an audio-reactive effect, the TUI should
visually reflect that energy. If nothing is happening, the TUI should
feel calm. The motion layer is the mechanism that ties the terminal to
the physical experience.

**Motion communicates, decoration distracts.** Every animation must convey
information: a device connected, an effect changed, the audio spectrum
has energy, the system is idle. If removing an effect would cause the
user to miss an event, the effect is justified. If removing it would
change nothing but aesthetics, it should be subtle or gated behind the
"full" sensitivity level.

**Compose, don't code.** Individual effects are tachyonfx compositions
(sequences, parallels, patterns, filters), not hand-rolled shader
functions. Custom `effect_fn` shaders are used only for reactive layers
that need per-frame external data (spectrum bins, canvas colors).

**Cancel cleanly.** When a new event arrives, the corresponding keyed
effect replaces the in-progress one. No stacking, no jarring interrupts.
tachyonfx's `EffectManager::add_unique_effect(key, effect)` handles this
natively.

**Fail silent, never stall.** If an effect panics (it should not), the
render loop continues without it. If the terminal does not support
truecolor, effects degrade to style-based alternatives. If the motion
budget is exceeded, reactive layers are the first to shed load.

---

## 5. Motion System Architecture

### 5.1 Core Types

```
crates/hypercolor-tui/src/
├── motion/
│   ├── mod.rs            # MotionSystem struct, public API
│   ├── keys.rs           # MotionKey enum (unique effect identifiers)
│   ├── catalog.rs        # Effect constructors (one fn per catalog entry)
│   ├── reactive.rs       # Continuous reactive layers (spectrum, canvas, idle)
│   └── sensitivity.rs    # MotionSensitivity enum + scaling helpers
```

### 5.2 MotionKey

Every motion effect that can be superseded is identified by a key. When a
new effect is added with the same key, tachyonfx's `EffectManager`
cancels the old one and starts the new one from scratch.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MotionKey {
    // ── Chrome ──────────────────────────────────────────
    TitleShimmer,
    IdleBreathing,

    // ── Device events ───────────────────────────────────
    DeviceArrival,
    DeviceDeparture,

    // ── Effect transitions ──────────────────────────────
    EffectTransition,
    ControlPatch,

    // ── Scene events ────────────────────────────────────
    SceneActivation,

    // ── System state ────────────────────────────────────
    ConnectionLost,
    ConnectionRestored,
    ErrorFlash,

    // ── Navigation ──────────────────────────────────────
    ScreenTransition,
    PanelFocus,

    // ── Reactive (continuous, keyed for cancellation) ───
    SpectrumPulse,
    CanvasBleed,
}
```

### 5.3 MotionSystem

The motion system wraps tachyonfx's `EffectManager` and adds
hypercolor-specific behavior: sensitivity scaling, reactive layer
management, and timing instrumentation.

```rust
pub struct MotionSystem {
    manager: tachyonfx::EffectManager<MotionKey>,
    sensitivity: MotionSensitivity,
    last_tick: Instant,

    // Reactive layer state
    spectrum_energy: f32,
    idle_seconds: f32,
    canvas_dominant_hue: Option<f32>,

    // Performance
    frame_budget_us: u64,
    last_process_us: u64,
}

impl MotionSystem {
    pub fn new(sensitivity: MotionSensitivity) -> Self { ... }

    /// Tick the motion system. Call once per render frame.
    /// Returns the elapsed Duration for callers that need it.
    pub fn tick(&mut self, buf: &mut Buffer, area: Rect) -> Duration {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick);
        self.last_tick = now;

        let start = Instant::now();
        self.manager.process_effects(elapsed.into(), buf, area);
        self.last_process_us = start.elapsed().as_micros() as u64;

        elapsed
    }

    /// Fire a discrete event effect.
    pub fn trigger(&mut self, key: MotionKey, effect: tachyonfx::Effect) {
        if self.sensitivity == MotionSensitivity::Off {
            return;
        }
        self.manager.add_unique_effect(key, effect);
    }

    /// Fire a non-keyed (stackable) effect.
    pub fn add(&mut self, effect: tachyonfx::Effect) {
        if self.sensitivity == MotionSensitivity::Off {
            return;
        }
        self.manager.add_effect(effect);
    }

    /// Update reactive layer inputs. Called on every spectrum/canvas frame.
    pub fn update_spectrum(&mut self, energy: f32) { ... }
    pub fn update_canvas_hue(&mut self, dominant_hue: Option<f32>) { ... }
    pub fn update_idle(&mut self, seconds_since_last_event: f32) { ... }

    pub fn sensitivity(&self) -> MotionSensitivity { self.sensitivity }
    pub fn set_sensitivity(&mut self, s: MotionSensitivity) { ... }
    pub fn last_process_us(&self) -> u64 { self.last_process_us }
    pub fn is_active(&self) -> bool { self.manager.is_running() }
}
```

### 5.4 Integration into App

The `MotionSystem` is owned by `App` and ticked after widget rendering,
before the frame is flushed. This means effects are applied as a
post-process over already-rendered widget content, which is exactly how
tachyonfx is designed to work.

**Modified render path** (`app.rs`):

```rust
fn render(&mut self, frame: &mut Frame) {
    let area = frame.area();

    // 1. Render chrome shell, get content area
    let content_area = self.chrome.render(frame, area, &self.state, ...);

    // 2. Render active screen into content area
    if let Some(screen) = self.screens.get(&self.active_screen) {
        screen.render(frame, content_area);
    }

    // 3. Render overlays (notifications, help)
    self.render_overlays(frame, area);

    // 4. Apply motion effects over the entire composed frame
    self.motion.tick(frame.buffer_mut(), area);
}
```

Effects that target specific regions (a device row, a panel border, the
title bar) use `Effect::with_area(rect)` to scope themselves. Effects
that target the entire frame operate on the full `area`.

### 5.5 Action Integration

New `Action` variants trigger motion effects. The mapping from action to
effect is centralized in `motion/catalog.rs`, not scattered across
component handlers.

```rust
// In action.rs — new variants
pub enum Action {
    // ... existing variants ...

    // Motion triggers
    MotionDeviceArrived { device_name: String, row_area: Rect },
    MotionDeviceDeparted { device_name: String, row_area: Rect },
    MotionEffectChanged { from: Option<String>, to: String },
    MotionSceneActivated { name: String, transition_ms: u32 },
    MotionScreenChanged { from: ScreenId, to: ScreenId },
    MotionPanelFocused { area: Rect },
    MotionError { area: Rect },
    MotionConnectionLost,
    MotionConnectionRestored,
}
```

In `App::process_action`, these translate directly to
`self.motion.trigger(key, catalog::build_xxx(...))` calls.

### 5.6 Title Bar Migration

The existing `TitleBar` shimmer (`chrome/title_bar.rs:15-197`) is
replaced with a `never_complete` tachyonfx effect using a custom
`effect_fn` that reproduces the four-wave gradient, but composed through
the motion system rather than maintained as standalone animation code.
This unifies all animation under one lifecycle and removes the special
`tick()` call in the render path.

```rust
// In catalog.rs
pub fn title_shimmer(area: Rect) -> Effect {
    fx::never_complete(
        fx::effect_fn(
            TitleShimmerState::new(),
            EffectTimer::from_ms(12_500, Interpolation::Linear),
            |state, ctx, cell_iter| {
                let phase = ctx.timer.alpha() * std::f32::consts::TAU;
                cell_iter.for_each_cell(|pos, cell| {
                    let i_f = (pos.x - area.x) as f32;
                    let primary = (phase + i_f * 0.4).sin() * 0.25;
                    let secondary = (phase * 0.6 + i_f * 0.7).sin() * 0.15;
                    let drift = (phase * 0.03).sin() * 0.2;
                    let t = ((i_f / area.width as f32) + primary + secondary + drift)
                        .clamp(0.0, 1.0);
                    let color = gradient_color(t, &theme::BRAND_GRADIENT);
                    cell.set_fg(color);
                });
            },
        )
    )
}
```

The `TitleBar` struct drops its `phase` field entirely. On initialization,
`MotionSystem` receives the title bar area and spawns the shimmer as a
keyed `TitleShimmer` effect.

---

## 6. Effect Catalog

Each entry describes a specific motion effect: what triggers it, what it
looks like, which tachyonfx primitives compose it, duration, easing, and
the `MotionKey` that governs its lifecycle.

### 6.1 Device Arrival Ceremony

**Trigger:** `Action::MotionDeviceArrived`
**Key:** `MotionKey::DeviceArrival`
**Area:** The table row where the new device appears
**Duration:** 600ms
**Description:** The device row sweeps in from the left edge with a neon
cyan leading edge that fades to the row's final colors.

```rust
fx::sequence(&[
    fx::sweep_in(Motion::LeftToRight, theme::accent_secondary(), (400, SineOut)),
    fx::fade_from_fg(theme::accent_secondary(), (200, Linear)),
])
.with_area(row_area)
```

At the "subtle" sensitivity level, the sweep is replaced by a simple
`fade_from` over 300ms.

### 6.2 Device Departure

**Trigger:** `Action::MotionDeviceDeparted`
**Key:** `MotionKey::DeviceDeparture`
**Area:** The row being removed
**Duration:** 400ms
**Description:** The row dissolves outward with a red tint before the
component removes it from the list.

```rust
fx::parallel(&[
    fx::dissolve((400, ExpoOut)),
    fx::fade_to_fg(theme::error(), (300, Linear)),
])
.with_area(row_area)
```

The component delays row removal until the effect completes (400ms timer).

### 6.3 Effect Transition Crossfade

**Trigger:** `Action::MotionEffectChanged`
**Key:** `MotionKey::EffectTransition`
**Area:** The canvas preview region
**Duration:** 500ms
**Description:** A directional sweep wipes the preview region, momentarily
showing the brand gradient as a curtain before the new effect's canvas
frames fill in behind it.

```rust
fx::sweep_in(Motion::LeftToRight, Color::Reset, (500, CubicInOut))
    .with_pattern(SweepPattern::left_to_right(8))
    .with_area(preview_area)
```

This works because the new canvas frames start arriving from the daemon
within one or two frames of the effect change event, so the sweep reveals
the new content as it arrives naturally.

### 6.4 Control Patch Pulse

**Trigger:** `Action::MotionControlPatch` (when a slider moves)
**Key:** `MotionKey::ControlPatch`
**Area:** The affected slider widget
**Duration:** 250ms
**Description:** A brief brightness pulse on the slider's fill region,
confirming the daemon accepted the control change.

```rust
fx::sequence(&[
    fx::lighten_fg(0.3, (100, QuadOut)),
    fx::darken_fg(0.3, (150, QuadIn)),
])
.with_area(slider_area)
.with_filter(CellFilter::Text)
```

### 6.5 Scene Activation Ripple

**Trigger:** `Action::MotionSceneActivated`
**Key:** `MotionKey::SceneActivation`
**Area:** Full screen
**Duration:** 800ms (matched to the daemon's own transition duration when
available)
**Description:** A radial HSL hue shift ripples outward from the center
of the terminal, sweeping the entire frame through the brand gradient
before settling back. This is the most dramatic effect in the catalog and
is gated to "full" sensitivity only.

```rust
fx::hsl_shift_fg([30.0, 20.0, 5.0], (800, SineInOut))
    .with_pattern(RadialPattern::default())
    .with_area(full_area)
```

At "subtle" sensitivity this is reduced to a border-only color flash
lasting 400ms.

### 6.6 Screen Transition

**Trigger:** `Action::MotionScreenChanged`
**Key:** `MotionKey::ScreenTransition`
**Area:** Content area (inside chrome)
**Duration:** 300ms
**Description:** The outgoing screen slides out and the incoming screen
fades in. Since ratatui doesn't have true offscreen buffering at the
component level, this is approximated as a dissolve of the old content
followed by a coalesce of the new.

```rust
fx::sequence(&[
    fx::dissolve((150, QuadIn))
        .with_pattern(SweepPattern::left_to_right(6)),
    fx::coalesce((150, QuadOut))
        .with_pattern(SweepPattern::left_to_right(6)),
])
.with_area(content_area)
```

### 6.7 Panel Focus Glow

**Trigger:** `Action::MotionPanelFocused`
**Key:** `MotionKey::PanelFocus`
**Area:** The focused panel's border cells
**Duration:** 300ms
**Description:** When keyboard focus moves to a new panel, the border
brightens from muted to accent color with a quick ease-out.

```rust
fx::fade_to_fg(theme::accent_secondary(), (300, CubicOut))
    .with_area(panel_area)
    .with_filter(CellFilter::Outer(Margin::new(1, 1)))
```

### 6.8 Connection Lost Glitch

**Trigger:** `Action::MotionConnectionLost`
**Key:** `MotionKey::ConnectionLost`
**Area:** Full screen
**Duration:** Indefinite (until connection restores)
**Description:** A low-frequency glitch effect on the status bar and
border cells, with periodic red HSL shifts, conveying that the daemon
connection is down without being so aggressive that the UI is unusable.

```rust
fx::never_complete(
    fx::parallel(&[
        fx::glitch(0.02, 500..2000, 50..200),
        fx::repeating(fx::sequence(&[
            fx::hsl_shift_fg([0.0, 0.0, -10.0], (1000, SineInOut)),
            fx::hsl_shift_fg([0.0, 0.0, 10.0], (1000, SineInOut)),
        ])),
    ])
)
.with_filter(CellFilter::Outer(Margin::new(1, 1)))
```

### 6.9 Connection Restored Flash

**Trigger:** `Action::MotionConnectionRestored`
**Key:** `MotionKey::ConnectionRestored`
**Area:** Full screen
**Duration:** 500ms
**Description:** Cancels the connection-lost glitch. A single green flash
sweeps across all borders, confirming the link is back.

```rust
fx::sequence(&[
    fx::fade_to_fg(theme::success(), (200, QuadOut)),
    fx::fade_from_fg(theme::success(), (300, QuadIn)),
])
.with_filter(CellFilter::Outer(Margin::new(1, 1)))
```

### 6.10 Error Flash

**Trigger:** `Action::MotionError`
**Key:** `MotionKey::ErrorFlash`
**Area:** The widget or region where the error occurred
**Duration:** 400ms
**Description:** A quick red flash that draws attention without
persisting. Used for failed API calls, invalid input, etc.

```rust
fx::sequence(&[
    fx::fade_to_fg(theme::error(), (150, QuadOut)),
    fx::fade_from_fg(theme::error(), (250, CubicIn)),
])
.with_area(error_area)
```

### 6.11 Idle Breathing

**Trigger:** Automatic after 10 seconds of no user input or daemon events
**Key:** `MotionKey::IdleBreathing`
**Area:** All border cells
**Duration:** Indefinite (canceled on any input)
**Description:** A slow, continuous HSL lightness oscillation on border
cells, giving the TUI a calm "breathing" quality when idle. The amplitude
is very small (lightness shift of 3-5%) so it reads as alive rather than
animated.

```rust
fx::never_complete(
    fx::repeating(fx::ping_pong(
        fx::lighten_fg(0.05, (3000, SineInOut))
    ))
)
.with_filter(CellFilter::Outer(Margin::new(1, 1)))
```

### 6.12 Notification Slide

**Trigger:** `Action::Notify`
**Key:** Non-keyed (stackable)
**Area:** Notification toast region
**Duration:** 300ms in, hold, 300ms out
**Description:** Toasts slide in from the bottom-right and dissolve out
when dismissed.

```rust
// Entry
fx::sweep_in(Motion::RightToLeft, theme::bg_panel(), (300, CubicOut))
    .with_area(toast_area)

// Exit
fx::dissolve((300, QuadIn))
    .with_area(toast_area)
```

---

## 7. Reactive Layers

Reactive layers are continuous visual modulations driven by live daemon
data. Unlike the discrete effects in Section 6, reactive layers do not
have a start and end — they persist as long as the data stream is active
and are updated every frame.

### 7.1 Spectrum Border Pulse

**Data source:** Spectrum WebSocket channel (15 FPS, 64 bins)
**Visual:** Border cell foreground brightness modulated by the bass/sub-bass
energy of the audio spectrum. When the bass hits, borders get slightly
brighter; when the music is quiet, borders return to their theme default.

**Implementation:** A custom `effect_fn` shader registered as a
`never_complete` effect under `MotionKey::SpectrumPulse`. On each frame,
the motion system feeds the latest spectrum energy value into the effect's
state.

```rust
pub fn spectrum_border_pulse() -> Effect {
    fx::never_complete(
        fx::effect_fn(
            SpectrumPulseState { energy: 0.0 },
            EffectTimer::from_ms(66, Interpolation::Linear),
            |state, _ctx, cell_iter| {
                let boost = state.energy.clamp(0.0, 1.0) * 0.15;
                cell_iter.for_each_cell(|_pos, cell| {
                    if let Color::Rgb(r, g, b) = cell.fg() {
                        let factor = 1.0 + boost;
                        cell.set_fg(Color::Rgb(
                            (r as f32 * factor).min(255.0) as u8,
                            (g as f32 * factor).min(255.0) as u8,
                            (b as f32 * factor).min(255.0) as u8,
                        ));
                    }
                });
            },
        )
    )
    .with_filter(CellFilter::Outer(Margin::new(1, 1)))
}
```

The energy value is updated on every `Action::SpectrumUpdated` by
averaging the lowest 8 bins (sub-bass through bass) and passing the
result to `motion.update_spectrum(energy)`.

**Sensitivity scaling:**
- Off: no spectrum pulse
- Subtle: energy multiplied by 0.5
- Full: energy used directly

### 7.2 Canvas Ambient Bleed

**Data source:** Canvas WebSocket channel (15 FPS, RGB pixel data)
**Visual:** The dominant color of the effect canvas subtly tints the
background of the chrome regions (title bar, status bar, audio strip),
creating an ambient glow effect similar to Ambilight or bias lighting.

**Implementation:** On each `Action::CanvasFrameReceived`, the bridge (or
a small helper) samples the canvas border pixels, computes a weighted
average hue, and passes it to `motion.update_canvas_hue(hue)`. The
reactive layer applies a very subtle background tint to chrome areas.

```rust
pub fn canvas_ambient_bleed() -> Effect {
    fx::never_complete(
        fx::effect_fn(
            AmbientBleedState { hue: None, prev_bg: None },
            EffectTimer::from_ms(100, Interpolation::Linear),
            |state, _ctx, cell_iter| {
                let Some(hue) = state.hue else { return; };
                let tint = hsl_to_rgb(hue, 0.4, 0.08);
                cell_iter.for_each_cell(|_pos, cell| {
                    cell.set_bg(tint);
                });
            },
        )
    )
    .with_filter(CellFilter::AnyOf(vec![
        CellFilter::BgColor(Color::Reset),
        CellFilter::Background,
    ]))
}
```

**Sensitivity scaling:**
- Off: no bleed
- Subtle: lightness capped at 0.04 (barely perceptible)
- Full: lightness at 0.08 (clearly visible but not overwhelming)

### 7.3 Idle Fade-Down

**Data source:** Internal timer (seconds since last user/daemon action)
**Visual:** After 10 seconds of inactivity, the entire frame slowly
dims by reducing lightness, and the idle breathing effect (6.11) begins.
Any input immediately cancels both, snapping the frame back to full
brightness.

**Implementation:** The `MotionSystem` tracks `idle_seconds`. When it
crosses the threshold, it triggers the breathing effect and starts a slow
`darken_fg` on the full area. On any `Action::Tick` that coincides with
user input or a daemon event, `idle_seconds` resets and the effects are
canceled.

```rust
pub fn idle_dim(area: Rect) -> Effect {
    fx::darken_fg(0.15, (5000, SineIn))
        .with_area(area)
}
```

**Sensitivity scaling:**
- Off: no idle dimming or breathing
- Subtle: breathing only, no dimming
- Full: breathing + gradual dimming

---

## 8. Accessibility and Motion Sensitivity

### 8.1 MotionSensitivity Enum

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionSensitivity {
    Off,
    Subtle,
    #[default]
    Full,
}
```

The default is `Full`. Users who prefer reduced motion set it to `Subtle`
(shorter durations, smaller amplitudes, no full-screen effects) or `Off`
(no motion at all; the TUI behaves exactly as it does today).

### 8.2 Scaling Rules

| Effect | Off | Subtle | Full |
|--------|-----|--------|------|
| Title shimmer | static gradient | slower phase | full shimmer |
| Device arrival | instant appear | 300ms fade | 600ms sweep |
| Device departure | instant remove | 200ms fade | 400ms dissolve |
| Effect transition | instant swap | 250ms fade | 500ms sweep |
| Control patch | no feedback | 150ms pulse | 250ms pulse |
| Scene activation | status text only | 400ms border flash | 800ms radial ripple |
| Screen transition | instant swap | 200ms fade | 300ms dissolve+coalesce |
| Panel focus | instant border change | 200ms fade | 300ms glow |
| Connection lost | status text only | red border tint | glitch + hsl shift |
| Connection restored | status text only | green border flash | green sweep |
| Error flash | status text only | 200ms flash | 400ms flash |
| Idle breathing | none | breathing only | breathing + dim |
| Notification slide | instant appear | 200ms fade | 300ms sweep |
| Spectrum pulse | none | 50% amplitude | full amplitude |
| Canvas bleed | none | 50% lightness | full lightness |
| Idle dim | none | none | full dim |

### 8.3 Environment Detection

The `REDUCE_MOTION` environment variable, if set to any value, forces
`MotionSensitivity::Subtle` as the maximum regardless of the configured
setting. This follows the accessibility convention used by web browsers
and macOS's `prefers-reduced-motion`.

Additionally, if the terminal is detected as not supporting truecolor
(via `$COLORTERM` check), the motion system automatically degrades to
`Subtle` because many tachyonfx effects depend on RGB color interpolation
that looks wrong with 256-color quantization.

### 8.4 Runtime Toggle

The user can cycle sensitivity with a keybinding (proposed: `M` for
"motion"). Each press cycles Off → Subtle → Full → Off. The current
level is shown in the status bar.

---

## 9. Performance Budget

### 9.1 Targets

| Metric | Budget | Measured By |
|--------|--------|-------------|
| Motion processing per frame | < 3ms | `MotionSystem::last_process_us()` |
| Total render per frame | < 20ms | Existing render timing in bridge metrics |
| Memory: effect instances | < 64 KB | Estimated from tachyonfx object sizes |
| Active effects (typical) | 3-5 | `EffectManager::is_running()` inventory |
| Active effects (peak) | 12-15 | During scene transition with all reactive layers |

### 9.2 Budget Enforcement

The `MotionSystem` records `last_process_us` on every tick. If three
consecutive frames exceed the 3ms budget:

1. Reactive layers (spectrum pulse, canvas bleed) are paused first.
2. If still over budget, discrete effects with the longest remaining
   duration are dropped.
3. If still over budget, sensitivity auto-downgrades to `Subtle`.

This shedding is logged at `tracing::debug` level and is not visible to
the user. It should be extremely rare at 15 FPS on modern hardware; the
budget exists as a safety rail, not an expected operating mode.

### 9.3 Render Rate Interaction

The TUI renders at 15 FPS (66ms interval). tachyonfx effects are designed
for higher frame rates (30-60 FPS in game-like TUIs), so at 15 FPS some
fast effects (250ms or less) will complete in only 3-4 visible frames.
This is acceptable: the effects are designed to be perceptible at 15 FPS,
and the durations in the catalog account for it (the shortest effect is
250ms = ~4 frames at 15 FPS).

If the TUI's render rate is increased in the future (e.g., to 30 FPS for
smoother canvas preview), all motion effects automatically benefit because
tachyonfx processes elapsed real time, not frame counts.

---

## 10. Dependency Changes

### 10.1 New Dependencies

Add to `crates/hypercolor-tui/Cargo.toml` `[dependencies]`:

```toml
tachyonfx = { workspace = true }
```

Add to root `Cargo.toml` `[workspace.dependencies]`:

```toml
tachyonfx = "0.25"
```

### 10.2 Ratatui Version

The TUI currently uses `ratatui 0.29`. tachyonfx 0.25 is built against
`ratatui-core 0.1` and tested with `ratatui 0.30`.

Options:

1. **Bump ratatui to 0.30.** This is the cleanest path. The 0.29 → 0.30
   migration is mostly additive (new widget APIs, improved rendering);
   breaking changes are minor. This is the recommended path.
2. **Pin tachyonfx to 0.24.** If 0.25 requires ratatui 0.30, the
   previous version may target 0.29. This is a fallback.
3. **Use git dependency with version override.** If both 0.25 and 0.24
   target 0.30, a fork or git pin may be needed. This is a last resort.

The migration plan assumes option 1. Phase 0 bumps ratatui before any
tachyonfx work begins.

### 10.3 Feature Flags

Use the default features (`std`, `dsl`). The `sendable` feature is not
needed because the TUI's render loop is single-threaded (all
`MotionSystem` access happens in the `App::render` call, never across
tasks). The `wasm` feature is not needed.

---

## 11. Migration Plan

Six phases, each ending in a green `just check` (workspace check does
not cover the UI crate, so verification is `cd crates/hypercolor-tui &&
cargo check`). Each phase can ship independently.

### Phase 0: Ratatui Bump

Files:

- `Cargo.toml` (workspace) — bump ratatui version
- `crates/hypercolor-tui/` — fix any compile errors from the upgrade

Tasks:

- Bump `ratatui` from 0.29 to 0.30 in workspace dependencies
- Fix any breaking API changes in widget rendering, style construction,
  or terminal backend usage
- Verify all existing TUI tests pass

Verification:

- `cd crates/hypercolor-tui && cargo check`
- `just verify` (workspace builds unaffected since TUI is excluded)
- Manual smoke test: `just tui` launches, dashboard renders, effects
  browser works, spectrum bar updates

### Phase 1: Motion System Scaffold

Files:

- `crates/hypercolor-tui/Cargo.toml` — add tachyonfx
- `crates/hypercolor-tui/src/motion/mod.rs` — new
- `crates/hypercolor-tui/src/motion/keys.rs` — new
- `crates/hypercolor-tui/src/motion/sensitivity.rs` — new
- `crates/hypercolor-tui/src/motion/catalog.rs` — new (empty shell)
- `crates/hypercolor-tui/src/motion/reactive.rs` — new (empty shell)
- `crates/hypercolor-tui/src/app.rs` — add `MotionSystem` field, call
  `motion.tick()` in render path
- `crates/hypercolor-tui/src/lib.rs` — re-export motion module

Tasks:

- Implement `MotionSystem` with `EffectManager<MotionKey>`, sensitivity
  setting, and timing instrumentation
- Wire `motion.tick(buf, area)` into the render path after widget
  rendering
- Add `--motion` CLI flag (off/subtle/full, default full) and
  `HYPERCOLOR_MOTION` env var
- Add `M` keybinding to cycle sensitivity at runtime
- Show current sensitivity in status bar
- No effects yet — the system exists but does nothing

Verification:

- `cd crates/hypercolor-tui && cargo check`
- `just tui` runs without errors
- Status bar shows motion sensitivity level
- `M` cycles through levels
- `HYPERCOLOR_MOTION=off just tui` starts in off mode
- `MotionSystem::last_process_us()` reads near-zero (no active effects)

### Phase 2: Title Bar Migration + Discrete Events

Files:

- `crates/hypercolor-tui/src/motion/catalog.rs` — implement catalog
  constructors for all 12 effects
- `crates/hypercolor-tui/src/chrome/title_bar.rs` — remove manual
  animation code, delegate to motion system
- `crates/hypercolor-tui/src/action.rs` — add `Motion*` action variants
- `crates/hypercolor-tui/src/app.rs` — map Motion actions to
  `motion.trigger()` calls

Tasks:

- Implement all 12 catalog entries from Section 6
- Replace the title bar's manual `phase` + `tick()` with
  `MotionKey::TitleShimmer` effect
- Wire `Action::MotionDeviceArrived` to fire on device connect events
  from the bridge
- Wire `Action::MotionDeviceDeparted` on device disconnect
- Wire `Action::MotionEffectChanged` on effect activation
- Wire `Action::MotionSceneActivated` on scene activation
- Wire `Action::MotionScreenChanged` on screen navigation
- Wire `Action::MotionPanelFocused` on focus change
- Wire `Action::MotionError` on API call failures
- Wire `Action::MotionConnectionLost` / `MotionConnectionRestored` on
  bridge disconnect/reconnect
- Implement sensitivity scaling for each effect

Verification:

- Title bar shimmer looks identical to the old implementation
- `HYPERCOLOR_MOTION=off just tui` renders a static title bar (no
  shimmer) — confirms the old code path is fully removed
- Connect a device while the TUI is open → sweep-in animation
- Switch screens with keyboard → dissolve transition
- Error flash on a failed API call
- All effects respect the sensitivity setting
- `MotionSystem::last_process_us()` stays under 3000 during normal
  operation

### Phase 3: Reactive Layers

Files:

- `crates/hypercolor-tui/src/motion/reactive.rs` — implement spectrum
  pulse, canvas bleed, idle fade-down
- `crates/hypercolor-tui/src/bridge.rs` — add spectrum energy extraction,
  canvas hue sampling
- `crates/hypercolor-tui/src/app.rs` — wire reactive data updates into
  motion system

Tasks:

- Implement `spectrum_border_pulse()` custom effect
- Implement `canvas_ambient_bleed()` custom effect
- Implement `idle_dim()` and wire the idle timer
- Add spectrum energy calculation to the bridge (average of lowest 8
  bins from the 64-bin snapshot)
- Add canvas dominant hue sampling (average border pixels of the canvas
  frame, convert to HSL hue)
- Wire `Action::SpectrumUpdated` → `motion.update_spectrum(energy)`
- Wire `Action::CanvasFrameReceived` → `motion.update_canvas_hue(hue)`
- Wire idle timer into the action processing loop
- Implement load shedding (pause reactive layers if budget exceeded)

Verification:

- Play music through the daemon's audio input → border cells pulse
  visibly with bass energy
- Activate a red-dominant effect → chrome backgrounds gain a subtle red
  tint
- Sit idle for 15 seconds → borders begin breathing, then frame
  slowly dims
- Press any key during idle → immediate snap back to full brightness
- With all reactive layers running, `last_process_us()` stays under 3000
- `HYPERCOLOR_MOTION=subtle` reduces amplitude by 50%
- `HYPERCOLOR_MOTION=off` disables reactive layers entirely

### Phase 4: Truecolor Degradation

Files:

- `crates/hypercolor-tui/src/motion/sensitivity.rs` — add terminal
  capability detection
- `crates/hypercolor-tui/src/motion/catalog.rs` — add `_256color`
  variants for effects that degrade

Tasks:

- Detect truecolor support via `$COLORTERM` and terminal capability
  query
- On non-truecolor terminals, cap sensitivity to `Subtle` and replace
  color-interpolation effects with style-based alternatives (bold/dim
  toggles, reverse video flashes)
- Detect `$REDUCE_MOTION` env var and cap sensitivity

Verification:

- `COLORTERM='' HYPERCOLOR_MOTION=full just tui` renders in Subtle mode
  with style-based effects
- `REDUCE_MOTION=1 HYPERCOLOR_MOTION=full just tui` renders in Subtle
  mode
- Effects are visually correct in 256-color mode (no garbled colors)

### Phase 5: Polish

Files:

- `crates/hypercolor-tui/src/chrome/status_bar.rs` — show motion
  sensitivity and `last_process_us` in debug mode
- Test files as needed

Tasks:

- Tune effect durations and easing curves based on visual testing on
  real hardware
- Adjust spectrum energy averaging window for responsiveness vs.
  smoothness
- Profile memory usage of `EffectManager` under peak load
- Write integration tests for motion system lifecycle (trigger, cancel,
  sensitivity scaling)
- Document motion keybinding and sensitivity in help overlay

Verification:

- Full visual walkthrough on a real daemon with audio-reactive effects
  running
- `last_process_us` stays under budget across a 10-minute session
- Help overlay shows motion controls
- All tests pass

---

## 12. Verification Strategy

### 12.1 Unit Tests

Files: `crates/hypercolor-tui/tests/motion_tests.rs`

- `MotionSystem` respects sensitivity: Off suppresses all triggers,
  Subtle scales durations, Full uses catalog defaults
- `MotionKey` deduplication: triggering the same key twice cancels the
  first effect
- Idle timer fires breathing effect after threshold
- Idle timer resets on simulated input
- Budget enforcement: simulate over-budget frames, verify reactive
  layers are shed
- Sensitivity cycle: Off → Subtle → Full → Off

### 12.2 Integration Tests

- Render a frame with an active motion effect and verify the buffer
  contains modified cells (not identical to a no-motion render)
- Render with `MotionSensitivity::Off` and verify the buffer is
  identical to a no-motion render
- Title shimmer produces the same visual output as the old manual
  implementation (snapshot comparison of the title bar row)

### 12.3 Visual Verification

Every phase includes manual visual testing on a real terminal because
motion effects are inherently visual. Automated tests verify correctness
(effects fire, cancel, scale); visual tests verify aesthetics (effects
look good, feel right, don't distract).

Recommended terminal matrix:

| Terminal | Truecolor | Notes |
|----------|-----------|-------|
| Ghostty | Yes | Primary dev terminal |
| Kitty | Yes | Secondary, verify rendering |
| WezTerm | Yes | Third option |
| macOS Terminal.app | 256 | Degradation path |
| tmux | Passthrough | Verify effects survive multiplexer |

### 12.4 Performance Verification

Add a `--motion-debug` flag that overlays `last_process_us` and active
effect count in the status bar. Run the TUI for 10 minutes with all
reactive layers active and all discrete effects triggered at least once.
Verify:

- No frame exceeds 3ms motion budget for more than 3 consecutive frames
- No memory growth (effect count returns to baseline after discrete
  effects complete)
- No panics or error log lines from the motion system

---

## 13. Open Questions

1. **Should the title shimmer use tachyonfx's built-in `hsl_shift` or a
   custom `effect_fn`?** The current shimmer uses four stacked sine waves
   that are more complex than any built-in effect. A custom `effect_fn`
   preserves the exact visual, but means the first effect in the catalog
   is already a custom shader. The spec assumes custom for fidelity.

2. **Canvas bleed: sample border pixels or center region?** Border pixels
   give ambilight-style edge bleeding; center pixels give the dominant
   color of the effect. The spec assumes border for the ambilight feel,
   but center might be more useful. Can be A/B tested during Phase 3.

3. **Should the render rate increase from 15 to 30 FPS when motion is
   enabled?** 15 FPS is fine for static content but makes fast effects
   (250ms = 4 frames) look choppy. 30 FPS doubles the terminal write
   load but dramatically improves motion smoothness. The spec defers
   this to Phase 5 tuning. A conditional increase (30 FPS while effects
   are active, 15 FPS when idle) would be the best of both worlds.

4. **Should reactive layers support user-configurable amplitude?**
   The spec uses fixed amplitudes per sensitivity level. A per-layer
   amplitude slider in a future Settings screen would be more flexible
   but is out of scope for v1.

5. **Should device arrival/departure effects work on logical devices
   too?** Spec 37 adds logical device CLI support; the TUI does not yet
   display logical devices. When it does, the motion system should
   naturally extend to them. The spec defers this to whatever spec adds
   logical device views.

6. **ratatui 0.30 compatibility.** The spec assumes a clean bump from
   0.29 to 0.30 in Phase 0. If the migration is more invasive than
   expected, Phase 0 may need its own spec. Assess during implementation.

---

## 14. Recommendation

Build this in the phase order laid out in Section 11. Phase 0 (ratatui
bump) and Phase 1 (scaffold) are mechanical and low-risk. Phase 2 (title
bar migration + discrete events) is the milestone with the most visible
impact and is the phase that proves the architecture actually works: if
the title bar shimmer and a device-arrival sweep look right, the remaining
effects are just more of the same pattern.

The reactive layers in Phase 3 are the phase with the most novelty risk.
Spectrum-driven border pulsing is straightforward, but canvas ambient
bleed requires tuning: too much lightness and the chrome looks gaudy; too
little and no one notices. Budget a tuning pass specifically for this
phase.

The clear choice is to adopt tachyonfx rather than continue hand-rolling
animation code. The library already solves composition, easing, lifecycle
management, and cell-level targeting. The TUI's event loop and component
architecture are compatible with tachyonfx's `process(elapsed, buffer,
area)` model. The daemon's existing WebSocket streams provide exactly the
reactive data a motion layer needs. The only missing piece is the glue
that ties these together — which is what this spec defines.

Hypercolor controls living, breathing light. Its terminal should match.
