# Spec 13 — Scenes & Automation Engine

> Technical specification for the scene graph, transition engine, scheduling system, trigger framework, rule engine, priority management, context awareness, and safety subsystem.

**Status:** Draft
**Design Doc:** [07-scenes-automation.md](../design/07-scenes-automation.md)
**Depends on:** Effect Engine, Spatial Layout Engine, Audio Pipeline, Event Bus (`HypercolorBus`)

---

## Table of Contents

1. [Scene](#1-scene)
2. [ZoneAssignment](#2-zoneassignment)
3. [TransitionSpec](#3-transitionspec)
4. [TransitionEngine](#4-transitionengine)
5. [ScheduleRule](#5-schedulerule)
6. [CircadianEngine](#6-circadianengine)
7. [TriggerSource Trait](#7-triggersource-trait)
8. [AutomationRule](#8-automationrule)
9. [TriggerExpr](#9-triggerexpr)
10. [ConditionExpr](#10-conditionexpr)
11. [ActionExpr](#11-actionexpr)
12. [PriorityStack](#12-prioritystack)
13. [ContextEngine](#13-contextengine)
14. [Safety](#14-safety)

---

## 1. Scene

A `Scene` is the fundamental unit of lighting state. It captures _what every targeted LED should look like_ -- a serializable, composable, restorable snapshot. Scenes are the nouns of the system; everything else (transitions, schedules, triggers, rules) exists to decide _when_ and _how_ scenes activate.

### Struct Definition

```rust
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use uuid::Uuid;

/// Opaque scene identifier. UUID v7 for time-sortable ordering.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SceneId(pub Uuid);

/// A complete lighting state definition.
///
/// Scenes are self-contained: they carry their own transition preference,
/// their target scope, and every zone assignment needed to reproduce the
/// lighting state from scratch. No ambient state is assumed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene {
    /// UUID v7 -- time-sortable, globally unique.
    pub id: SceneId,

    /// Human-readable display name. Must be non-empty, max 128 chars.
    /// Used as the primary key in CLI commands (`hypercolor scene activate "Cozy Evening"`).
    pub name: String,

    /// Optional long-form description. Rendered in web UI and scene galleries.
    pub description: Option<String>,

    /// Attribution for shared/community scenes.
    pub author: Option<String>,

    /// Freeform tags for filtering and grouping.
    /// Convention: lowercase, hyphenated (`["evening", "warm", "silkcircuit"]`).
    pub tags: Vec<String>,

    /// Which devices/zones this scene targets.
    /// Zones not covered by the scope are left unchanged when this scene activates.
    pub scope: SceneScope,

    /// Per-zone effect + parameter assignments. Order is irrelevant.
    /// Each zone_id MUST appear at most once. Duplicates are a validation error.
    pub assignments: Vec<ZoneAssignment>,

    /// Master dimmer applied multiplicatively to every zone's brightness.
    /// Range: `0.0` (full black) to `1.0` (full intensity).
    pub global_brightness: f32,

    /// Default transition used when activating this scene.
    /// Callers can override this at activation time.
    pub transition: TransitionSpec,

    /// Timestamps for storage ordering and staleness detection.
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

### Scene Scope

```rust
/// Determines which devices/zones a scene touches.
///
/// Applying a scene with a non-`Full` scope leaves all out-of-scope zones
/// in their current state. This enables independent PC vs. room control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SceneScope {
    /// Every device the daemon manages. The default for "save current state."
    Full,

    /// PC-attached devices only: USB HID devices, internal controllers, and
    /// other locally attached hardware.
    PcOnly,

    /// Network/room devices only: WLED strips, Hue bulbs, smart home endpoints.
    RoomOnly,

    /// Explicit device list. The scene only touches zones belonging to these devices.
    Devices(Vec<DeviceId>),

    /// Explicit zone list. Most granular targeting.
    Zones(Vec<ZoneId>),
}
```

### Scene Composition

Scenes can be layered. A base scene defines the full state; overlays modify specific zones without disturbing the rest.

```rust
/// A composed scene: one base with zero or more overlays.
///
/// Overlay resolution: for any given zone, the highest-priority overlay
/// that claims that zone wins. If no overlay claims a zone, the base's
/// assignment is used. Opacity controls blending between the overlay's
/// output and whatever would have been rendered without it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposedScene {
    /// The foundation scene. Must have `Full` scope or at least cover
    /// every zone that isn't claimed by an overlay.
    pub base: SceneId,

    /// Overlays applied on top of the base, evaluated in priority order.
    pub overlays: Vec<SceneOverlay>,
}

/// A single overlay in a composed scene.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneOverlay {
    /// The scene providing overlay assignments.
    pub scene_id: SceneId,

    /// Higher priority wins when two overlays claim the same zone.
    /// Range: 0-255.
    pub priority: u8,

    /// Blend factor between this overlay and the layer below it.
    /// `0.0` = fully transparent (overlay has no effect).
    /// `1.0` = fully opaque (overlay completely replaces).
    pub opacity: f32,

    /// If `Some`, restrict this overlay to only the listed zones.
    /// If `None`, the overlay applies to every zone its source scene covers.
    pub zones: Option<Vec<ZoneId>>,
}
```

### Validation Rules

| Constraint                                   | Error                                                             |
| -------------------------------------------- | ----------------------------------------------------------------- |
| `name` is empty or exceeds 128 chars         | `InvalidSceneName`                                                |
| `global_brightness` outside `[0.0, 1.0]`     | `BrightnessOutOfRange`                                            |
| Duplicate `zone_id` in `assignments`         | `DuplicateZoneAssignment`                                         |
| `ZoneAssignment` references nonexistent zone | `UnknownZone` (warning, not hard error -- zones may appear later) |
| Overlay `opacity` outside `[0.0, 1.0]`       | `OpacityOutOfRange`                                               |

### Storage

Scenes are stored as individual TOML files under `~/.config/hypercolor/scenes/`. The filename is the slugified scene name (e.g., `cozy-evening.toml`). The `SceneId` is the canonical identity; filenames are a convenience.

---

## 2. ZoneAssignment

A `ZoneAssignment` binds a single zone to an effect with specific parameters. It is the leaf node of the scene tree.

```rust
/// What a single zone should do within a scene.
///
/// The zone is identified by `zone_id` (a composite of device_id + zone_name).
/// The effect is referenced by string ID, matching the effect registry.
/// Parameters are effect-specific key-value pairs; unknown keys are ignored
/// by the effect (forward-compatible).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneAssignment {
    /// Target zone. Format: `"{device_id}:{zone_name}"` or a logical zone name
    /// from the spatial layout.
    pub zone_id: ZoneId,

    /// Effect to run on this zone.
    /// Special value `"static"` means a solid color with no animation.
    /// All other values must resolve in the effect registry.
    pub effect_id: String,

    /// Effect-specific parameters. Keys and value types are defined by
    /// each effect's parameter schema (see `ControlValue`).
    ///
    /// Examples:
    /// - `{ "color": "#e135ff", "speed": 0.4, "min_brightness": 0.1 }`
    /// - `{ "base_color": "#1a1a2e", "press_color": "#e135ff", "decay_ms": 400 }`
    pub parameters: HashMap<String, ControlValue>,

    /// Zone-level brightness override.
    /// Multiplied with the scene's `global_brightness`.
    /// `None` means the zone inherits `global_brightness` unmodified.
    /// Range: `0.0` to `1.0`.
    pub brightness: Option<f32>,

    /// Zone-level saturation override.
    /// Applied as a multiplier to the effect's output colors in Oklch space.
    /// `None` means no saturation adjustment.
    /// Range: `0.0` (fully desaturated) to `2.0` (double saturation, clamped to gamut).
    pub saturation: Option<f32>,

    /// For `"static"` effect: the solid color.
    /// For other effects: an optional tint overlay blended on top of the
    /// effect's output using multiply blending in Oklab space.
    pub color_override: Option<Rgb>,
}

/// A dynamically-typed control value for effect parameters.
/// Matches the effect system's `ControlValue` type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ControlValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
    Color(Rgb),
    FloatArray(Vec<f64>),
    ColorArray(Vec<Rgb>),
}
```

### Effective Brightness Calculation

The final brightness for a zone is:

```
effective_brightness = scene.global_brightness
                     * zone_assignment.brightness.unwrap_or(1.0)
                     * safety_limiter.clamp(...)
                     * ambient_light_multiplier  // if ambient sensor active
                     * circadian_brightness       // if circadian mode active
```

All multipliers are applied in `[0.0, 1.0]` space. The result is clamped to `[0.0, 1.0]` before reaching the effect renderer.

### Saturation Override Mechanics

When `saturation` is `Some(s)`:

1. Convert each output pixel from sRGB to Oklch.
2. Multiply the chroma channel by `s`.
3. Clamp back to sRGB gamut using chroma reduction (preserve hue and lightness).
4. Convert back to sRGB.

This enables scene-level "desaturated night mode" without modifying every effect's parameters.

---

## 3. TransitionSpec

A `TransitionSpec` fully describes _how_ the system moves between two scenes. It is carried on every `Scene` as a default, but can be overridden at activation time by the caller (schedule rule, automation rule, or manual API call).

```rust
use std::time::Duration;

/// Complete specification for a scene transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionSpec {
    /// The transition algorithm.
    pub transition_type: TransitionType,

    /// Total wall-clock duration of the transition.
    /// Subject to `SafetyLimiter.min_transition_ms` floor.
    pub duration: Duration,

    /// Easing curve applied to the progress value before it reaches
    /// the blending function.
    pub easing: EasingFunction,
}

/// Transition algorithms.
///
/// Each variant defines a different visual strategy for moving from
/// scene A to scene B. All variants receive the same inputs:
/// `from_colors`, `to_colors`, `layout`, and eased progress `t`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransitionType {
    /// Instant switch. Duration is ignored. `t` jumps from 0 to 1.
    Cut,

    /// Smooth per-LED interpolation in Oklab space.
    /// The workhorse transition -- perceptually uniform color blending.
    Crossfade,

    /// Directional sweep across the spatial layout.
    /// The wipe front advances through physical space; LEDs behind
    /// the front show `to_colors`, LEDs ahead show `from_colors`,
    /// with a gradient band of width `softness` at the boundary.
    Wipe {
        /// Direction of the wipe front's travel.
        direction: WipeDirection,

        /// Edge softness. `0.0` = razor-sharp boundary.
        /// `1.0` = the gradient spans the full layout width.
        /// Recommended: `0.3` to `0.6`.
        softness: f32,
    },

    /// Brief flash of a solid color, then reveal the new scene.
    /// Useful for alert overlays (sub notifications, build success).
    Flash {
        /// The color to flash. Typically white or an accent.
        flash_color: Rgb,

        /// How long the flash color is held at peak intensity.
        /// Must be less than the total `TransitionSpec.duration`.
        flash_duration: Duration,
    },

    /// Fade to black, hold, then fade into new scene.
    /// Cinematic scene changes.
    Blackout {
        /// Duration of the full-black hold between fade-out and fade-in.
        hold_duration: Duration,
    },

    /// Delegate to a custom transition effect.
    /// The effect receives two framebuffers (from/to) and a progress
    /// value. It renders the blended output.
    Effect {
        /// Transition effect name in the effect registry.
        effect_id: String,
        /// Effect-specific parameters.
        parameters: HashMap<String, ControlValue>,
    },
}

/// Spatial wipe directions.
///
/// For linear wipes, the direction names the travel direction of the
/// *reveal front* (the new scene appears behind the front).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WipeDirection {
    Left,
    Right,
    Up,
    Down,
    /// Wipe collapses inward from edges to center.
    RadialIn,
    /// Wipe expands outward from center to edges.
    RadialOut,
    /// Arbitrary angle in degrees. 0 = right, 90 = up, etc.
    Diagonal { angle: f32 },
}

/// Easing functions for transition progress curves.
///
/// The easing function maps raw linear progress `t in [0, 1]`
/// to an eased value `t' in [0, 1]` (may overshoot for spring curves).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EasingFunction {
    /// `t' = t`. Constant velocity.
    Linear,

    /// Slow start, fast end. Cubic: `t' = t^3`.
    EaseIn,

    /// Fast start, slow end. Cubic: `t' = 1 - (1 - t)^3`.
    EaseOut,

    /// Slow start and end. Cubic S-curve.
    EaseInOut,

    /// CSS-style cubic bezier with four control points.
    /// `(x1, y1, x2, y2)` where P0 = (0,0) and P3 = (1,1).
    CubicBezier(f32, f32, f32, f32),

    /// Discrete steps. Jumps between `count` evenly-spaced values.
    Steps {
        count: u32,
        jump: StepJump,
    },
}

/// When discrete step transitions land on each step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepJump {
    /// Jump at the start of each interval.
    Start,
    /// Jump at the end of each interval.
    End,
    /// Jump at both start and end (first and last steps are half-width).
    Both,
    /// Jump at neither boundary (all steps are interior).
    None,
}
```

### Transition Presets

The system ships with named presets. Users reference them by name in TOML configs and CLI flags.

| Preset Name   | Type                    | Duration | Easing      | Notes                           |
| ------------- | ----------------------- | -------- | ----------- | ------------------------------- |
| `instant`     | `Cut`                   | 0ms      | n/a         | No animation                    |
| `smooth`      | `Crossfade`             | 1000ms   | `EaseInOut` | Default for most scene switches |
| `slow-fade`   | `Crossfade`             | 3000ms   | `EaseInOut` | Schedule-based changes          |
| `dramatic`    | `Blackout` (500ms hold) | 2000ms   | `EaseInOut` | Scene reveals                   |
| `flash-white` | `Flash` (#fff, 200ms)   | 800ms    | `Linear`    | Alert / notification            |
| `sweep-right` | `Wipe` Right (0.4 soft) | 1500ms   | `EaseInOut` | Directional drama               |
| `sunrise`     | `Crossfade`             | 30000ms  | `EaseIn`    | 30s warm fade-in                |
| `sleep`       | `Crossfade`             | 60000ms  | `EaseOut`   | 60s fade to off                 |

---

## 4. TransitionEngine

The `TransitionEngine` manages active transitions in the render loop. It is a stateful component that lives inside the scene manager and produces blended framebuffers on each tick.

### TransitionState

```rust
use std::time::Instant;

/// Runtime state for an in-progress transition.
///
/// Created when a scene switch begins. Destroyed when `progress >= 1.0`.
/// At most one `TransitionState` is active at a time. If a new transition
/// is requested while one is in progress, the current frame becomes the
/// new "from" state (seamless interruption).
#[derive(Debug)]
pub struct TransitionState {
    /// Scene being transitioned away from.
    pub from_scene: SceneId,

    /// Scene being transitioned toward.
    pub to_scene: SceneId,

    /// The transition specification (type, duration, easing).
    pub spec: TransitionSpec,

    /// Wall-clock time when the transition started.
    pub started_at: Instant,

    /// Current linear progress. `0.0` at start, `1.0` at completion.
    /// Updated each frame: `progress = elapsed / duration`.
    pub progress: f32,
}
```

### Blending Pipeline

Every frame during a transition:

1. **Render both scenes** independently through the effect engine, producing `from_colors: Vec<DeviceColors>` and `to_colors: Vec<DeviceColors>`.
2. **Compute progress**: `progress = (now - started_at).as_secs_f32() / spec.duration.as_secs_f32()`, clamped to `[0.0, 1.0]`.
3. **Apply easing**: `t = spec.easing.apply(progress)`.
4. **Dispatch to blend function** based on `spec.transition_type`.
5. **Output blended colors** to the spatial layout engine.

### Oklab Color Interpolation

All color blending happens in **Oklab perceptual color space**. RGB interpolation produces muddy grays when blending between saturated colors (red-to-blue traverses brown). Oklab maintains perceptual uniformity across the blend.

```rust
/// Blend two framebuffers in Oklab space with uniform mix factor.
///
/// Each LED is independently interpolated. The mix factor `t` is the
/// eased transition progress:
/// - `t = 0.0` -> pure `from` colors
/// - `t = 1.0` -> pure `to` colors
fn blend_colors(
    from: &[DeviceColors],
    to: &[DeviceColors],
    t: f32,
) -> Vec<DeviceColors> {
    from.iter().zip(to.iter()).map(|(f, t_colors)| {
        DeviceColors {
            device_id: f.device_id.clone(),
            zone_name: f.zone_name.clone(),
            colors: f.colors.iter().zip(t_colors.colors.iter()).map(|(a, b)| {
                let a_lab = oklab::srgb_to_oklab(*a);
                let b_lab = oklab::srgb_to_oklab(*b);
                let mixed = Oklab {
                    l: lerp(a_lab.l, b_lab.l, t),
                    a: lerp(a_lab.a, b_lab.a, t),
                    b: lerp(a_lab.b, b_lab.b, t),
                };
                oklab::oklab_to_srgb(mixed)
            }).collect(),
        }
    }).collect()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
```

### Spatial Wipe Mechanics

Wipe transitions leverage the spatial layout engine. Each LED has a physical position; the wipe front sweeps across that space.

```
    Wipe Right (t = 0.4, softness = 0.3)
    +----------------------------------+
    | ############.....                |  <- LED strip (horizontal)
    | from scene  |soft|  to scene     |
    |             |edge|               |
    +----------------------------------+
                  ^
           wipe front at 40%
```

**Algorithm:**

1. **Normalize positions**: Map all LED positions to `[0.0, 1.0]` along the wipe axis. For `Right`, this is the X coordinate. For `RadialOut`, it is the distance from the centroid, normalized to the maximum distance.
2. **Compute per-LED blend factor**: For each LED at normalized position `p`:
   ```
   half_soft = softness / 2.0
   led_t = smoothstep(t - half_soft, t + half_soft, p)
   ```
   Where `smoothstep` is the Hermite interpolation (no discontinuities).
3. **Blend**: `output = blend_oklab(from_color, to_color, led_t)`.

For `Diagonal { angle }`, the wipe axis is rotated by `angle` degrees before position normalization. The rotation center is the spatial layout's centroid.

```rust
/// Compute per-LED blend factors for a spatial wipe.
fn spatial_wipe(
    from: &[DeviceColors],
    to: &[DeviceColors],
    layout: &SpatialLayout,
    direction: &WipeDirection,
    softness: f32,
    t: f32,
) -> Vec<DeviceColors> {
    let half_soft = softness / 2.0;

    from.iter().zip(to.iter()).map(|(f, t_colors)| {
        DeviceColors {
            device_id: f.device_id.clone(),
            zone_name: f.zone_name.clone(),
            colors: f.colors.iter().zip(t_colors.colors.iter())
                .enumerate()
                .map(|(i, (a, b))| {
                    // Get LED's normalized position along the wipe axis
                    let p = layout.wipe_position(
                        &f.device_id, &f.zone_name, i, direction,
                    );
                    let led_t = smoothstep(t - half_soft, t + half_soft, p);
                    blend_oklab(*a, *b, led_t)
                })
                .collect(),
        }
    }).collect()
}

/// Hermite smoothstep. Returns 0.0 when x <= edge0, 1.0 when x >= edge1.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
```

### Flash and Blackout Phase Breakdown

**Flash transition** has two phases within `duration`:

| Phase  | Time Range           | Behavior                                       |
| ------ | -------------------- | ---------------------------------------------- |
| Flash  | `[0, flash_ratio)`   | Blend `from_colors` toward solid `flash_color` |
| Reveal | `[flash_ratio, 1.0]` | Blend solid `flash_color` toward `to_colors`   |

Where `flash_ratio = flash_duration / total_duration`.

**Blackout transition** has three phases:

| Phase    | Time Range                      | Behavior                         |
| -------- | ------------------------------- | -------------------------------- |
| Fade Out | `[0, fade_each)`                | Blend `from_colors` toward black |
| Hold     | `[fade_each, fade_each + hold)` | Solid black                      |
| Fade In  | `[fade_each + hold, 1.0]`       | Blend black toward `to_colors`   |

Where `fade_each = (1.0 - hold_ratio) / 2.0`.

### Transition Interruption

If a new transition is requested while one is in progress:

1. Capture the **current blended frame** as a frozen snapshot.
2. Create a new `TransitionState` where `from_scene` is a synthetic scene representing the frozen frame.
3. The new transition proceeds from the interrupted blend point -- no visual discontinuity.

---

## 5. ScheduleRule

The scheduler runs as a background `tokio::spawn` task. It evaluates rules against the current time (and solar events) and fires scene changes when conditions match.

### Scheduler State

```rust
use chrono::NaiveDate;
use chrono_tz::Tz;

/// Top-level scheduler. Evaluates all rules every tick.
///
/// Tick interval: 1 second when near a rule boundary (within 5 minutes),
/// 30 seconds otherwise. This saves CPU while maintaining <1s accuracy
/// for rule evaluation.
pub struct Scheduler {
    /// All registered schedule rules, sorted by next-fire time.
    rules: Vec<ScheduleRule>,

    /// User's geographic location for solar calculations.
    /// `None` disables all `Solar` schedule expressions.
    location: Option<GeoLocation>,

    /// User's configured timezone. Defaults to system timezone.
    timezone: Tz,

    /// Pre-computed solar times for today. Recomputed at midnight.
    solar_cache: Option<SolarTimesCache>,
}

pub struct GeoLocation {
    pub lat: f64,
    pub lon: f64,
}

pub struct SolarTimesCache {
    pub date: NaiveDate,
    pub times: SolarTimes,
}

pub struct SolarTimes {
    pub sunrise: NaiveTime,
    pub sunset: NaiveTime,
    pub civil_dawn: NaiveTime,
    pub civil_dusk: NaiveTime,
    pub solar_noon: NaiveTime,
}
```

### ScheduleRule Struct

```rust
/// A single schedule rule.
///
/// Rules are evaluated in priority order. When multiple rules match
/// the same time slot, the highest-priority rule wins. Ties are broken
/// by the rule that was defined last (later in the file wins).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleRule {
    /// Unique rule identifier.
    pub id: RuleId,

    /// Human-readable name. Displayed in CLI, web UI, and logs.
    pub name: String,

    /// Whether this rule is currently active. Disabled rules are skipped
    /// during evaluation but retained in storage.
    pub enabled: bool,

    /// Priority for conflict resolution. Higher values win.
    /// See priority tiers in Section 12.
    pub priority: u8,

    /// When this rule fires.
    pub schedule: ScheduleExpr,

    /// What happens when this rule fires.
    pub action: ScheduleAction,
}
```

### Schedule Expressions

```rust
/// When a schedule rule should fire.
///
/// Each variant represents a different time-matching strategy.
/// The scheduler evaluates these against the current time every tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScheduleExpr {
    /// Standard cron expression. Evaluated by the `cron` crate.
    /// Example: `"0 9 * * 1-5"` = 9:00 AM on weekdays.
    ///
    /// Supports standard 5-field cron (minute, hour, day-of-month, month, day-of-week).
    /// Does NOT support seconds or year fields.
    Cron(String),

    /// Specific time with optional day filter.
    /// Simpler than cron for the common case of "every Tuesday at 9am."
    Time {
        hour: u8,
        minute: u8,
        days: DayFilter,
    },

    /// Relative to a solar event. Requires `Scheduler.location` to be set.
    /// Offset can be negative (before the event) or positive (after).
    ///
    /// Example: `Solar { event: Sunrise, offset: -30min, days: Weekdays }`
    /// fires 30 minutes before sunrise on weekdays.
    Solar {
        event: SolarEvent,
        offset: Duration,
        days: DayFilter,
    },

    /// Circadian rhythm -- not a point-in-time trigger but a continuous
    /// modifier. See Section 6 for the `CircadianEngine`.
    Circadian(CircadianProfile),

    /// Calendar date trigger. Fires once at midnight (00:00) on the
    /// matching date, or continuously throughout the matching date
    /// depending on the action type.
    Date {
        month: u8,
        day: u8,
        recurrence: DateRecurrence,
    },
}

/// Solar events for schedule expressions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SolarEvent {
    /// Sun crosses the horizon, rising.
    Sunrise,
    /// Sun crosses the horizon, setting.
    Sunset,
    /// Civil twilight begins (~30 min before sunrise). Enough light to
    /// see outdoors without artificial illumination.
    CivilDawn,
    /// Civil twilight ends (~30 min after sunset).
    CivilDusk,
    /// Sun at highest point. Useful for "brightest time of day" logic.
    SolarNoon,
}

/// Day-of-week filter for schedule expressions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DayFilter {
    /// Fires every day.
    Every,
    /// Monday through Friday.
    Weekdays,
    /// Saturday and Sunday.
    Weekends,
    /// Explicit list of weekdays.
    Specific(Vec<chrono::Weekday>),
}

/// Recurrence pattern for date-specific schedules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DateRecurrence {
    /// Same date every year (holidays, anniversaries).
    Yearly,
    /// A specific year only. For one-time events.
    Once(u16),
    /// A date range. Fires every day within the range.
    /// Supports year-wrapping for ranges like Dec 20 - Jan 5.
    Range { start: NaiveDate, end: NaiveDate },
}

/// Actions that a schedule rule can perform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScheduleAction {
    /// Activate a scene, optionally overriding its default transition.
    ActivateScene {
        scene_id: SceneId,
        transition: Option<TransitionSpec>,
    },

    /// Adjust global brightness without changing the active scene.
    SetBrightness {
        brightness: f32,
        transition_ms: u64,
    },

    /// Fire an automation rule by ID. Allows schedules to trigger
    /// complex rule chains.
    TriggerRule(RuleId),

    /// Power off all devices (or scoped devices).
    PowerOff { transition_ms: u64 },

    /// Power on and activate a specific scene.
    PowerOn { scene_id: SceneId },
}
```

### Evaluation Algorithm

```
every tick:
    now = current_time(scheduler.timezone)

    for rule in scheduler.rules (sorted by priority DESC):
        if !rule.enabled:
            continue

        match rule.schedule:
            Cron(expr) ->
                if cron::Schedule::from(expr).upcoming(now).next() is within 1 tick:
                    fire(rule.action)

            Time { hour, minute, days } ->
                if now.hour == hour && now.minute == minute && days.matches(now.weekday()):
                    fire(rule.action)

            Solar { event, offset, days } ->
                target = solar_cache.time_for(event) + offset
                if now is within 1 tick of target && days.matches(now.weekday()):
                    fire(rule.action)

            Date { month, day, recurrence } ->
                if now.month == month && now.day == day && recurrence.matches(now.year()):
                    fire(rule.action)

            Circadian(_) ->
                // Handled by CircadianEngine (Section 6), not the discrete scheduler
```

Fired rules are recorded in a `VecDeque<RuleExecution>` to prevent double-firing within the same tick. A rule is considered "already fired" if it fired within the last 60 seconds for the same schedule match.

---

## 6. CircadianEngine

The circadian engine is a special continuous modifier, not a discrete event trigger. It runs alongside the normal effect pipeline and adjusts color temperature and brightness throughout the day using a keyframed curve.

### Profile Definition

```rust
use chrono::NaiveTime;

/// A circadian profile defines a day-long curve of color temperature
/// and brightness keyframes.
///
/// Between keyframes, values are interpolated using the configured
/// interpolation mode. The curve wraps at midnight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircadianProfile {
    /// Display name for this profile.
    pub name: String,

    /// Ordered keyframes throughout the day. Must be sorted by `time`.
    /// Minimum 2 keyframes required. Maximum 48 (one per half hour).
    pub keyframes: Vec<CircadianKeyframe>,

    /// How values are interpolated between keyframes.
    pub interpolation: InterpolationMode,
}

/// A single point on the circadian curve.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircadianKeyframe {
    /// Time of day for this keyframe.
    pub time: NaiveTime,

    /// Color temperature in Kelvin.
    /// Range: `1800` (deep amber) to `6500` (cool daylight).
    /// Typical range: 2700K (warm incandescent) to 6500K (D65 daylight).
    pub color_temp: u32,

    /// Target brightness at this time.
    /// Range: `0.0` to `1.0`.
    pub brightness: f32,

    /// Extra saturation for accent colors at this time of day.
    /// `1.0` = no change. `0.5` = desaturated. `1.5` = boosted.
    pub saturation_boost: f32,
}

/// Interpolation mode between circadian keyframes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InterpolationMode {
    /// Straight-line interpolation between keyframes.
    /// Simple, predictable, but can look "robotic" at keyframe boundaries.
    Linear,

    /// Catmull-Rom spline through keyframes.
    /// Smooth transitions with no sharp corners. Requires >= 4 keyframes
    /// for full smoothness (otherwise falls back to linear).
    CatmullRom,
}
```

### Engine Behavior

```rust
/// Runtime state for the circadian engine.
///
/// The engine is evaluated every frame (not just on schedule ticks)
/// because it produces a continuously-varying color temperature filter.
pub struct CircadianEngine {
    /// The active profile, if circadian mode is enabled.
    pub profile: Option<CircadianProfile>,

    /// Current interpolated state.
    pub current: CircadianState,

    /// Whether the engine is actively modifying output.
    pub enabled: bool,
}

/// Instantaneous circadian output values.
pub struct CircadianState {
    /// Current color temperature in Kelvin.
    pub color_temp: u32,
    /// Current brightness multiplier.
    pub brightness: f32,
    /// Current saturation boost factor.
    pub saturation_boost: f32,
}
```

### Color Temperature Application

The circadian engine applies as a **white-point adaptation filter** on top of the active scene's output. It does not replace colors -- it _shifts_ them toward the target color temperature.

**Algorithm:**

1. Compute the target white point from `color_temp` using the CIE standard illuminant series (Planckian locus approximation).
2. For each output pixel:
   a. Convert sRGB to Oklab.
   b. Apply chromatic adaptation using a simplified Bradford transform: shift the `a` and `b` channels proportionally toward the target white point's chromaticity.
   c. Scale lightness by the circadian `brightness` multiplier.
   d. Adjust chroma by `saturation_boost`.
   e. Convert back to sRGB, clamping to gamut.

This means a rainbow wave effect at midnight still displays a "warm" rainbow -- the blue channel is pulled toward amber, greens become olive, and reds become deep warm red. The effect's aesthetic character is preserved while the overall tone shifts.

### Default Profile

```
Time     Temp    Brightness   Character
------   -----   ----------   ---------
06:00    2700K   0.10         Pre-dawn warm glow
07:00    3500K   0.40         Sunrise golden warm-up
09:00    5000K   0.85         Morning alert
12:00    6500K   1.00         Solar noon full daylight
15:00    5500K   0.90         Afternoon still bright
18:00    4000K   0.70         Golden hour begins
20:00    3200K   0.50         Twilight relaxation
22:00    2700K   0.30         Night cozy minimum
23:00    2200K   0.15         Late near-dark amber
00:00    1800K   0.05         Midnight barely-visible
```

---

## 7. TriggerSource Trait

Trigger sources are the event producers that feed the automation rule engine. Each source monitors a specific domain (desktop, apps, system, audio, etc.) and emits `TriggerEvent`s when interesting things happen.

### Core Trait

```rust
use tokio::sync::broadcast;

/// A source of trigger events.
///
/// Implementors monitor a specific domain and emit events when
/// interesting state changes occur. The rule engine subscribes to
/// all registered trigger sources and evaluates incoming events
/// against automation rules.
///
/// # Contract
///
/// - `subscribe()` returns a broadcast receiver. Dropped receivers
///   are fine -- the source does not block on full channels.
/// - `current_state()` must be cheap (no I/O). It returns a cached
///   snapshot of the source's last-known state, used for condition
///   evaluation when a trigger fires.
/// - Sources must be `Send + Sync` for use in tokio tasks.
pub trait TriggerSource: Send + Sync {
    /// Unique identifier for this trigger source.
    /// Convention: lowercase, no spaces. Examples: `"desktop"`, `"app"`, `"ha"`.
    fn id(&self) -> &str;

    /// Human-readable display name.
    fn name(&self) -> &str;

    /// Subscribe to this source's event stream.
    /// Returns a broadcast receiver. Lagged receivers skip missed events.
    fn subscribe(&self) -> broadcast::Receiver<TriggerEvent>;

    /// Current state snapshot for condition evaluation.
    /// Called synchronously when a rule's trigger fires; must return
    /// immediately from cached data.
    fn current_state(&self) -> TriggerState;
}

/// An event emitted by a trigger source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerEvent {
    /// Which trigger source emitted this event.
    pub source: String,

    /// Event type identifier. Dot-separated namespace.
    /// Examples: `"screen_locked"`, `"launched"`, `"entity_changed"`.
    pub event_type: String,

    /// Event payload. Structure varies by source and event type.
    /// The rule engine matches payload fields against trigger filters.
    pub payload: serde_json::Value,

    /// When this event was generated. Used for sequence trigger timing.
    pub timestamp: Instant,
}

/// Cached state snapshot from a trigger source.
/// Used by `ConditionExpr::State` to check source state at rule evaluation time.
#[derive(Debug, Clone)]
pub struct TriggerState {
    /// Which source this state belongs to.
    pub source: String,

    /// Key-value state map. Keys are source-defined.
    /// Examples for desktop source:
    /// - `"screen_locked"`: `true`/`false`
    /// - `"idle_seconds"`: `300`
    /// - `"foreground_app"`: `"code"`
    pub values: HashMap<String, serde_json::Value>,
}
```

### Built-in Trigger Sources

| Source ID  | Struct                       | Events Emitted                                                                                                                                     | State Keys                                                                |
| ---------- | ---------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `desktop`  | `DesktopTriggerSource`       | `screen_locked`, `screen_unlocked`, `workspace_changed`, `fullscreen_entered`, `fullscreen_exited`, `idle_entered`, `idle_exited`, `power_state`   | `screen_locked`, `idle_seconds`, `workspace`, `fullscreen`, `power_state` |
| `app`      | `AppTriggerSource`           | `launched`, `exited`, `focused`, `unfocused`                                                                                                       | `foreground_app`, `running_apps`, `foreground_category`                   |
| `system`   | `SystemTriggerSource`        | `usb_connected`, `usb_disconnected`, `network_connected`, `network_disconnected`, `suspend`, `resume`, `display_connected`, `display_disconnected` | `ac_power`, `connected_displays`, `network_ssid`                          |
| `audio`    | `AudioTriggerSource`         | `silence`, `loud`, `beat_detected`, `music_started`, `music_stopped`                                                                               | `audio_playing`, `bpm`, `level`                                           |
| `mqtt`     | `MqttTriggerSource`          | Configurable per subscription                                                                                                                      | Configurable per subscription                                             |
| `ha`       | `HomeAssistantTriggerSource` | `entity_changed`, `event`                                                                                                                          | Mirrors watched entity states                                             |
| `stream`   | `StreamTriggerSource`        | `started`, `ended`, `subscription`, `bits`, `raid`, `follow`, `channel_point_redeem`, `hype_train`                                                 | `live`, `viewer_count`                                                    |
| `calendar` | `CalendarTriggerSource`      | `event_starting`, `event_started`, `event_ended`, `focus_time_started`, `focus_time_ended`                                                         | `current_event`, `next_event`                                             |
| `external` | Webhook REST endpoint        | User-defined                                                                                                                                       | n/a                                                                       |

### Application Detection

```rust
/// How to detect a specific application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AppDetection {
    /// Match process name (case-insensitive substring).
    ProcessName(String),

    /// Match X11 WM_CLASS or Wayland app_id (glob pattern).
    WindowClass(String),

    /// Match freedesktop `.desktop` file ID.
    DesktopEntry(String),

    /// Match process name + command-line arguments.
    ProcessArgs { name: String, args_contain: String },
}
```

---

## 8. AutomationRule

An `AutomationRule` connects a trigger expression (WHEN) to conditions (IF) to an action expression (THEN). It is the core unit of the reactive automation system.

```rust
use std::time::Duration;

/// An automation rule: WHEN trigger fires AND conditions pass, DO action.
///
/// Rules are the declarative building blocks of Hypercolor's reactive
/// intelligence. They are event-driven (unlike schedules, which are
/// time-driven). The rule engine evaluates all enabled rules against
/// every incoming `TriggerEvent`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationRule {
    /// Unique rule identifier.
    pub id: RuleId,

    /// Human-readable rule name. Displayed in CLI output, web UI, logs.
    pub name: String,

    /// Optional long-form description of what this rule does and why.
    pub description: Option<String>,

    /// Whether this rule is currently active.
    /// Disabled rules are retained in storage but skipped during evaluation.
    pub enabled: bool,

    /// Priority for conflict resolution. Range: 0-255.
    /// Higher priority rules win when multiple rules fire simultaneously.
    /// See priority tiers in Section 12.
    pub priority: u8,

    /// WHEN: the trigger expression that initiates rule evaluation.
    /// See Section 9 for the full expression tree.
    pub trigger: TriggerExpr,

    /// IF: conditions that must all pass for the action to execute.
    /// Evaluated at the moment the trigger fires. All conditions must
    /// return `true` (implicit AND). For OR logic, use `ConditionExpr::Or`.
    pub conditions: Vec<ConditionExpr>,

    /// THEN: the action to execute when trigger fires and conditions pass.
    /// See Section 11 for the full expression tree.
    pub action: ActionExpr,

    /// Minimum time between consecutive firings of this rule.
    /// Prevents rapid-fire activation (e.g., stream sub → celebration
    /// effect shouldn't strobe if 10 subs come in 2 seconds).
    /// `None` means no cooldown.
    pub cooldown: Option<Duration>,

    /// Restrict this rule to a time window.
    /// `None` means the rule is active 24/7.
    pub active_hours: Option<TimeRange>,

    /// Freeform tags for grouping and bulk management.
    /// Convention: lowercase, hyphenated.
    pub tags: Vec<String>,
}

/// A time range within a single day.
/// Supports wrapping past midnight (e.g., 22:00 to 03:00).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeRange {
    pub start: NaiveTime,
    pub end: NaiveTime,
}

impl TimeRange {
    /// Check whether a given time falls within this range.
    /// Handles midnight wrapping: if `start > end`, the range wraps
    /// (e.g., 22:00..03:00 matches 23:00 and 01:00 but not 12:00).
    pub fn contains(&self, time: NaiveTime) -> bool {
        if self.start <= self.end {
            time >= self.start && time < self.end
        } else {
            // Wraps past midnight
            time >= self.start || time < self.end
        }
    }
}

/// Opaque rule identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuleId(pub Uuid);
```

### Rule Evaluation Flow

```
on TriggerEvent received:
    for rule in rules (sorted by priority DESC):
        if !rule.enabled:
            continue

        if rule.active_hours is Some(range) && !range.contains(now):
            continue

        if rule.cooldown is Some(cd) && rule.last_fired + cd > now:
            continue

        if !rule.trigger.matches(&event):
            continue

        if !rule.conditions.iter().all(|c| c.evaluate(&state_snapshot)):
            continue

        // All checks pass -- execute action
        rule.last_fired = now
        execute(rule.action, &event)
        push_to_priority_stack(rule)
```

---

## 9. TriggerExpr

Trigger expressions form a composable tree that matches incoming `TriggerEvent`s. The tree structure enables complex event matching without custom code.

````rust
/// A composable trigger expression tree.
///
/// Trigger expressions match against incoming `TriggerEvent`s.
/// They can be simple (match a single event type) or composed
/// with boolean/temporal combinators.
///
/// # Tree Structure
///
/// ```text
/// Any
///  +-- Event { source: "app", event_type: "launched", filter: { category: "game" } }
///  +-- Sequence { within: 5s }
///       +-- Event { source: "desktop", event_type: "fullscreen_entered" }
///       +-- Event { source: "audio", event_type: "music_started" }
/// ```
///
/// This tree matches if a game launches OR if the user enters
/// fullscreen followed by music starting within 5 seconds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TriggerExpr {
    /// Match a single event.
    ///
    /// The event matches if:
    /// 1. `event.source == source`
    /// 2. `event.event_type == event_type`
    /// 3. If `filter` is `Some`, every key-value pair in the filter
    ///    must exist in `event.payload` with a matching value.
    ///    Matching is recursive: nested objects are compared field-by-field.
    ///    Missing filter fields in the payload cause a non-match.
    Event {
        /// Trigger source ID. Must match a registered `TriggerSource::id()`.
        source: String,

        /// Event type to match. Exact string equality.
        event_type: String,

        /// Optional payload filter. JSON object whose fields must all
        /// match corresponding fields in the event payload.
        ///
        /// Example: `{ "category": "game" }` matches events with
        /// `payload.category == "game"`.
        ///
        /// Supports wildcards in string values: `"steam_app_*"`.
        filter: Option<serde_json::Value>,
    },

    /// All child expressions must match.
    ///
    /// For `Event` children, all specified events must have fired
    /// (in any order) within a sliding window of 10 seconds.
    /// The window resets when any child resets.
    All(Vec<TriggerExpr>),

    /// Any child expression matching is sufficient.
    ///
    /// Short-circuits: the first matching child triggers the rule.
    Any(Vec<TriggerExpr>),

    /// Ordered event sequence with a time constraint.
    ///
    /// Events must fire in the specified order, and the entire
    /// sequence must complete within `within` duration.
    /// The clock starts when the first event in the sequence fires.
    ///
    /// If the sequence times out, accumulated progress is discarded.
    Sequence {
        /// Ordered list of trigger expressions. Each must match
        /// in sequence after the previous one matched.
        events: Vec<TriggerExpr>,

        /// Maximum time from first event to last event.
        within: Duration,
    },
}
````

### Match Semantics

| Variant    | Match Logic                                                      | State Required                       |
| ---------- | ---------------------------------------------------------------- | ------------------------------------ |
| `Event`    | Stateless point match against each incoming event                | None                                 |
| `All`      | Stateful: tracks which children have matched within a 10s window | `HashSet<usize>` of matched children |
| `Any`      | Stateless: delegates to children, first match wins               | None                                 |
| `Sequence` | Stateful: tracks current position in sequence + start time       | `(usize, Instant)` cursor            |

**Filter matching** supports three value comparison modes:

1. **Exact equality**: `"game"` matches `"game"`.
2. **Glob wildcard**: `"steam_app_*"` matches `"steam_app_570"`.
3. **Numeric comparison**: `{ "amount": { "$gte": 500 } }` matches `{ "amount": 1000 }`. Supported operators: `$gt`, `$gte`, `$lt`, `$lte`, `$ne`.

---

## 10. ConditionExpr

Condition expressions are checked **at the instant a trigger fires**. They inspect the current state of the system to decide whether the action should proceed. Unlike triggers (which are event-driven), conditions are state-driven.

````rust
/// A composable condition expression tree.
///
/// Conditions are evaluated synchronously when a trigger fires.
/// They inspect cached state from trigger sources, current time,
/// the active scene, and rule firing history.
///
/// Conditions support boolean combinators (And, Or, Not) for
/// arbitrary logical composition.
///
/// # Example Tree
///
/// ```text
/// And
///  +-- TimeRange { 17:00..03:00 }
///  +-- Not
///       +-- CurrentScene { scene: "gaming-reactive" }
///  +-- State { source: "desktop", key: "screen_locked", op: Eq, value: false }
/// ```
///
/// Passes if: it's between 5pm and 3am, the active scene is NOT
/// "gaming-reactive", and the screen is not locked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConditionExpr {
    /// Check the current state of a trigger source.
    ///
    /// Reads `TriggerSource::current_state()` and compares
    /// a specific key against an expected value.
    State {
        /// Trigger source ID.
        source: String,

        /// State key to check. Must exist in `TriggerState::values`.
        /// If the key is absent, the condition evaluates to `false`.
        key: String,

        /// Comparison operator.
        op: ComparisonOp,

        /// Expected value. Type must be compatible with the operator
        /// (numeric for Gt/Lt/Gte/Lte, string for Contains/Matches).
        value: serde_json::Value,
    },

    /// Check whether the current time falls within a range.
    /// Supports midnight wrapping (see `TimeRange`).
    TimeRange {
        start: NaiveTime,
        end: NaiveTime,
    },

    /// Check whether today's day of the week matches.
    DayOfWeek(DayFilter),

    /// Check whether the currently active scene matches (or doesn't match).
    CurrentScene {
        /// Scene to check against.
        scene_id: SceneId,

        /// If `true`, the condition passes when the active scene
        /// is NOT this scene (logical inversion).
        negated: bool,
    },

    /// Check whether a specific rule has NOT fired within a duration.
    ///
    /// Useful for "don't fire this rule if that other rule fired recently"
    /// patterns. Prevents cascading rule interactions.
    CooldownExpired {
        /// The rule to check.
        rule_id: RuleId,

        /// The cooldown window. Condition passes if the target rule
        /// last fired more than `duration` ago (or never fired).
        duration: Duration,
    },

    /// Logical AND: all children must pass.
    And(Vec<ConditionExpr>),

    /// Logical OR: at least one child must pass.
    Or(Vec<ConditionExpr>),

    /// Logical NOT: the child must fail for this to pass.
    Not(Box<ConditionExpr>),
}

/// Comparison operators for state checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComparisonOp {
    /// Equal (`==`). Works with any JSON value type.
    Eq,
    /// Not equal (`!=`).
    Ne,
    /// Greater than (`>`). Numeric values only.
    Gt,
    /// Less than (`<`). Numeric values only.
    Lt,
    /// Greater than or equal (`>=`). Numeric values only.
    Gte,
    /// Less than or equal (`<=`). Numeric values only.
    Lte,
    /// String contains substring. String values only.
    Contains,
    /// Regex match. String values only.
    /// Uses the `regex` crate. Invalid patterns cause the condition to
    /// evaluate to `false` and log a warning.
    Matches(String),
}
````

### Evaluation Semantics

- **Short-circuit**: `And` returns `false` on the first failing child. `Or` returns `true` on the first passing child.
- **Missing state keys**: If `State.key` is not present in the source's `TriggerState`, the condition evaluates to `false`. This is intentional -- it prevents rules from firing when a source hasn't initialized yet.
- **Type mismatches**: Comparing a string value with `Gt` evaluates to `false` and logs a warning. The system does not panic on type mismatches.
- **Regex compilation**: Patterns in `Matches(String)` are compiled lazily and cached. An invalid pattern causes `false` + a warning log (not a crash).

---

## 11. ActionExpr

Action expressions define what happens when a rule fires. Like trigger and condition expressions, they form a composable tree -- enabling sequential workflows, parallel fan-out, delays, and nested actions.

````rust
/// A composable action expression tree.
///
/// Action expressions define the operations performed when an
/// automation rule fires. They can be simple (activate a scene)
/// or composed into complex workflows with sequences, parallelism,
/// delays, and external integrations.
///
/// # Example Tree
///
/// ```text
/// Sequence
///  +-- ActivateScene { scene: "celebration", transition: flash }
///  +-- Delay(3s)
///  +-- RestorePreviousScene { transition: crossfade 2s }
/// ```
///
/// This plays a 3-second celebration effect, then restores the
/// previous scene with a smooth crossfade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActionExpr {
    /// Activate a scene, pushing it onto the priority stack.
    ActivateScene {
        /// The scene to activate.
        scene_id: SceneId,

        /// Override the scene's default transition.
        /// If `None`, the scene's own `TransitionSpec` is used.
        transition: Option<TransitionSpec>,
    },

    /// Pop the current scene from the priority stack and restore
    /// the scene that was active before it.
    ///
    /// If the stack is empty (no previous scene), this is a no-op
    /// and logs a warning.
    RestorePreviousScene {
        /// Transition for the restoration.
        /// If `None`, uses the restored scene's default transition.
        transition: Option<TransitionSpec>,
    },

    /// Adjust global brightness without changing the active scene.
    SetBrightness {
        /// Target brightness. Range: `0.0` to `1.0`.
        brightness: f32,
        /// Duration of the brightness ramp in milliseconds.
        transition_ms: u64,
    },

    /// Set a specific zone's effect and parameters.
    ///
    /// Does NOT change the active scene. This is a surgical override
    /// on a single zone, useful for status indicators (build progress,
    /// notification accents).
    SetZoneEffect {
        /// The zone to modify.
        zone_id: ZoneId,
        /// The effect to apply.
        effect_id: String,
        /// Effect parameters.
        parameters: HashMap<String, ControlValue>,
    },

    /// Apply a temporary scene overlay that auto-removes after a duration.
    ///
    /// The overlay is pushed onto the priority stack with the rule's
    /// priority. After `duration` elapses, the overlay is popped and
    /// the previous state is restored.
    ///
    /// Perfect for transient celebrations (sub alerts, build success).
    TemporaryOverlay {
        /// Scene to overlay.
        scene_id: SceneId,
        /// How long the overlay remains active.
        duration: Duration,
        /// Transition for entering the overlay.
        /// Exit uses `RestorePreviousScene` semantics.
        transition: Option<TransitionSpec>,
    },

    /// Execute child actions sequentially, one after another.
    ///
    /// Each child must complete before the next begins.
    /// `Delay` nodes insert pauses between steps.
    Sequence(Vec<ActionExpr>),

    /// Execute child actions in parallel (all start simultaneously).
    ///
    /// Use with care: multiple `ActivateScene` in parallel is a conflict.
    /// Intended for parallel side-effects (webhook + MQTT + scene change).
    Parallel(Vec<ActionExpr>),

    /// Wait for a duration before the next action in a `Sequence`.
    ///
    /// Outside a `Sequence`, this is a no-op and logs a warning.
    Delay(Duration),

    /// Fire an outbound HTTP webhook.
    ///
    /// Fire-and-forget: the action completes immediately after sending.
    /// Response status is logged but does not affect rule execution.
    /// Timeout: 5 seconds.
    Webhook {
        /// Target URL. Must be HTTPS in production (HTTP allowed in dev).
        url: String,
        /// HTTP method. Typically `"POST"`.
        method: String,
        /// Optional JSON body.
        body: Option<serde_json::Value>,
    },

    /// Publish a message to an MQTT topic.
    ///
    /// Uses the daemon's MQTT client connection (configured in main config).
    MqttPublish {
        /// MQTT topic to publish to.
        topic: String,
        /// Message payload (typically JSON string).
        payload: String,
    },

    /// No operation. Useful for testing, conditional branches,
    /// and placeholder rules under development.
    Noop,
}
````

### Execution Semantics

| Variant                | Async?                   | Blocks Sequence?                                        | Side Effects               |
| ---------------------- | ------------------------ | ------------------------------------------------------- | -------------------------- |
| `ActivateScene`        | Yes (transition)         | No (returns immediately, transition runs in background) | Pushes onto priority stack |
| `RestorePreviousScene` | Yes (transition)         | No                                                      | Pops priority stack        |
| `SetBrightness`        | Yes (ramp)               | No                                                      | Modifies global brightness |
| `SetZoneEffect`        | No                       | No                                                      | Surgical zone override     |
| `TemporaryOverlay`     | Yes (transition + timer) | No (timer runs in background)                           | Pushes/pops priority stack |
| `Sequence`             | Yes                      | Yes (awaits all children in order)                      | Depends on children        |
| `Parallel`             | Yes                      | Yes (awaits all children concurrently)                  | Depends on children        |
| `Delay`                | Yes                      | Yes (sleeps)                                            | None                       |
| `Webhook`              | Yes (HTTP)               | No (fire-and-forget)                                    | Outbound HTTP              |
| `MqttPublish`          | Yes (publish)            | No                                                      | Outbound MQTT              |
| `Noop`                 | No                       | No                                                      | None                       |

### Action Cancellation

When a higher-priority rule fires while a `Sequence` is in progress:

1. The in-progress `Sequence` is **interrupted** after the current step completes.
2. Steps that haven't started yet are discarded.
3. The interrupted rule's context is pushed down the priority stack.
4. If/when the higher-priority rule's context ends, the interrupted sequence does NOT resume -- the previous _scene_ is restored, but multi-step sequences are one-shot.

---

## 12. PriorityStack

The `PriorityStack` manages the layered scene state. It is the central arbitrator that determines what's actually rendering at any moment.

### Design

Hypercolor's automation produces potentially conflicting scene requests. The priority stack resolves this with a simple invariant: **the highest-priority entry in the stack is always the active scene**. When that entry expires or is removed, the next-highest entry becomes active.

```rust
use std::collections::BTreeMap;

/// Priority-based scene management with automatic restore-on-expire.
///
/// The stack maintains an ordered collection of active scene entries.
/// The entry with the highest priority is the "winner" -- it controls
/// what's currently rendering. Lower-priority entries are "shadowed"
/// but retained: when the winner is removed, the next entry takes over
/// with its configured transition.
///
/// This is NOT a traditional stack (no strict LIFO). It's a priority
/// queue with automatic eviction of expired entries.
pub struct PriorityStack {
    /// Active scene entries, keyed by priority.
    /// `BTreeMap` keeps entries sorted by priority.
    /// If two entries share a priority, the more recent one wins.
    entries: BTreeMap<u8, Vec<StackEntry>>,

    /// The currently active (rendering) entry's key.
    active: Option<StackEntryKey>,

    /// History of stack operations for debugging and simulation.
    history: VecDeque<StackOperation>,
}

/// A single entry in the priority stack.
#[derive(Debug, Clone)]
pub struct StackEntry {
    /// Unique key for this entry.
    pub key: StackEntryKey,

    /// The scene this entry renders.
    pub scene_id: SceneId,

    /// Which rule or source activated this entry.
    pub activated_by: ActivationSource,

    /// Priority level. Maps to the priority tiers below.
    pub priority: u8,

    /// When this entry was pushed.
    pub entered_at: Instant,

    /// If set, this entry auto-expires at this time.
    /// Used by `TemporaryOverlay` actions.
    pub auto_expire: Option<Instant>,

    /// The transition to use when this entry becomes active
    /// (either on push or when a higher-priority entry is removed).
    pub enter_transition: Option<TransitionSpec>,

    /// The transition to use when this entry is removed and the
    /// next entry takes over.
    pub exit_transition: Option<TransitionSpec>,
}

/// Identifies what activated a stack entry.
#[derive(Debug, Clone)]
pub enum ActivationSource {
    /// An automation rule.
    Rule(RuleId),
    /// A schedule rule.
    Schedule(RuleId),
    /// The context engine.
    Context(UserContext),
    /// Manual user action (CLI, API, UI).
    Manual,
    /// The circadian engine (base layer).
    Circadian,
}

/// Opaque entry key for stack operations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StackEntryKey(pub Uuid);
```

### Priority Tiers

```
Priority    Tier              Examples
--------    ----              --------
  0-19      Base              Circadian rhythm, ambient defaults
 20-29      Background        Ambient adjustments, temperature-reactive
 30-49      Normal            Screen lock dimming, idle detection, time-based schedules
 50-69      Active            App launches (games, media), context switches
 70-89      Important         Video calls, stream events
 90-99      Critical          Manual override, alerts, hotkey-triggered
100-127     System            Safety limiters, seizure protection, fail-safe
```

### Stack Operations

```rust
impl PriorityStack {
    /// Push a new entry onto the stack.
    ///
    /// If the new entry has higher priority than the current active entry,
    /// it becomes the new active scene (triggering a transition).
    /// If it has equal or lower priority, it's stored but shadowed.
    pub fn push(&mut self, entry: StackEntry) -> StackTransition { ... }

    /// Remove an entry by key.
    ///
    /// If the removed entry was active, the next-highest entry becomes
    /// active (triggering a restore transition).
    pub fn remove(&mut self, key: &StackEntryKey) -> Option<StackTransition> { ... }

    /// Tick the stack: expire any entries past their `auto_expire` time.
    ///
    /// Called every frame. Returns transitions if the active entry changed.
    pub fn tick(&mut self, now: Instant) -> Option<StackTransition> { ... }

    /// Get the currently active entry.
    pub fn active_entry(&self) -> Option<&StackEntry> { ... }

    /// Get all entries, ordered by priority (highest first).
    pub fn entries(&self) -> Vec<&StackEntry> { ... }

    /// Remove all entries activated by a specific source.
    /// Used when a context ends (e.g., game exits -> remove all
    /// entries pushed by the gaming context).
    pub fn remove_by_source(&mut self, source: &ActivationSource)
        -> Vec<StackTransition> { ... }
}

/// Describes a state change in the priority stack.
#[derive(Debug)]
pub struct StackTransition {
    /// The entry that was previously active (if any).
    pub from: Option<StackEntry>,
    /// The entry that is now active (if any).
    pub to: Option<StackEntry>,
    /// The transition to use for this change.
    pub transition: TransitionSpec,
}
```

### Auto-Restore Behavior

When a high-priority context ends (e.g., a game exits, a video call ends, a temporary overlay expires):

1. The stack entry for that context is removed.
2. The stack's `tick()` or `remove()` method returns a `StackTransition`.
3. The scene manager applies the transition, restoring the next-highest entry's scene.

This cascades naturally. If a user was in circadian mode (P20), then a game launched (P50), then a stream sub alert fired (P70 temp overlay, 3s), the stack unwinds:

```
t=0:   [P20: circadian] <-- active
t=1:   [P50: gaming, P20: circadian] <-- gaming active
t=5:   [P70: sub-alert (3s), P50: gaming, P20: circadian] <-- alert active
t=8:   [P50: gaming, P20: circadian] <-- alert expired, gaming restored
t=60:  [P20: circadian] <-- game exited, circadian restored
```

---

## 13. ContextEngine

The context engine sits above individual triggers and maintains an inferred "mode" representing what the user is doing. It feeds the priority stack with context-based scene entries.

### Architecture

```rust
use std::collections::VecDeque;

/// Infers the user's current activity from system signals.
///
/// Multiple `ContextDetector` implementations run in parallel,
/// each voting with a confidence score. The highest-confidence
/// context wins and becomes the active context.
///
/// Context changes are published to the event bus as `ContextChanged`
/// events, and optionally trigger scene changes via the context-to-scene
/// mapping configuration.
pub struct ContextEngine {
    /// Current inferred context.
    pub current_context: UserContext,

    /// Confidence score of the current context (0.0 to 1.0).
    pub confidence: f32,

    /// Registered context detectors.
    pub detectors: Vec<Box<dyn ContextDetector>>,

    /// Recent context changes for debugging and pattern analysis.
    pub history: VecDeque<ContextChange>,

    /// Minimum confidence threshold for a context switch.
    /// Prevents flickering between contexts when signals are ambiguous.
    /// Default: 0.6.
    pub confidence_threshold: f32,

    /// Hysteresis: a new context must maintain higher confidence than
    /// the current context for this many consecutive evaluations
    /// before a switch occurs. Default: 3 (at 1 eval/second = 3 seconds).
    pub hysteresis_count: u32,
}

/// A context change record.
#[derive(Debug, Clone)]
pub struct ContextChange {
    pub from: UserContext,
    pub to: UserContext,
    pub confidence: f32,
    pub timestamp: Instant,
}
```

### User Contexts

```rust
/// The inferred user activity mode.
///
/// Each variant carries context-specific metadata used by the
/// context-to-scene mapping to select variants (e.g., "working late
/// at night" gets a different scene than "working during the day").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserContext {
    /// User is in a productivity application.
    Working {
        /// Detected app category: `"ide"`, `"browser"`, `"terminal"`, `"office"`.
        app_category: String,
        /// How long the user has been in this context.
        /// Used for progressive dimming ("deep focus after 30 minutes").
        focus_duration: Duration,
    },

    /// User is playing a game.
    Gaming {
        /// Game name (if identified). `None` if detected by heuristic
        /// (fullscreen + high GPU) but process isn't in the game registry.
        game_name: Option<String>,
        /// Whether the game is running fullscreen.
        fullscreen: bool,
    },

    /// User is consuming media.
    Media {
        /// What kind of media.
        media_type: MediaType,
        /// Application name.
        app: String,
    },

    /// User is live streaming.
    Streaming {
        /// Platform: `"twitch"`, `"youtube"`.
        platform: String,
        /// Current OBS scene name (if detectable).
        scene: String,
    },

    /// User is in a video/voice call.
    InCall {
        /// Call application: `"zoom"`, `"teams"`, `"discord"`.
        app: String,
    },

    /// User is present but idle (no input for a while).
    Idle {
        /// How long they've been idle.
        idle_duration: Duration,
    },

    /// Extended idle or screen locked. Lights should dim or turn off.
    Away,

    /// User explicitly overrode automation. No context-based scene
    /// changes until the user switches back to auto mode.
    Manual,
}

/// Media type for the `Media` context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MediaType {
    Music,
    Movie,
    Stream,
}
```

### Context Detector Trait

```rust
/// A detector that votes on the current user context.
///
/// Each detector examines a subset of system signals and returns
/// a `(UserContext, confidence)` pair if it has an opinion.
/// Returning `None` means "I don't know / not my domain."
///
/// Confidence scores:
/// - `0.0 - 0.3`: Weak signal (e.g., browser open, could be work or leisure)
/// - `0.4 - 0.6`: Moderate signal (e.g., IDE focused, likely working)
/// - `0.7 - 0.8`: Strong signal (e.g., IDE focused + actively typing)
/// - `0.9 - 1.0`: Near-certain (e.g., fullscreen game + high GPU + known game process)
pub trait ContextDetector: Send + Sync {
    /// Examine system signals and vote on the user's context.
    fn detect(&self, signals: &SystemSignals) -> Option<(UserContext, f32)>;
}

/// Aggregated system signals available to context detectors.
///
/// Updated every second from the registered trigger sources.
/// All fields are optional -- detectors must handle missing data
/// gracefully (return `None` or reduce confidence).
#[derive(Debug, Clone, Default)]
pub struct SystemSignals {
    /// The application in the foreground window.
    pub foreground_app: Option<String>,

    /// The foreground window's title bar text.
    pub foreground_window_title: Option<String>,

    /// Whether a window is in exclusive fullscreen mode.
    pub fullscreen: bool,

    /// Seconds since last keyboard/mouse input.
    pub idle_seconds: u64,

    /// Whether any audio output is currently playing.
    pub audio_playing: bool,

    /// Whether a microphone input is active (voice call indicator).
    pub audio_input_active: bool,

    /// GPU utilization percentage (0-100).
    pub gpu_usage_percent: f32,

    /// CPU utilization percentage (0-100).
    pub cpu_usage_percent: f32,

    /// Network activity summary.
    pub network_activity: NetworkActivity,

    /// Whether the screen is locked.
    pub screen_locked: bool,

    /// Number of connected displays.
    pub active_displays: u32,
}

#[derive(Debug, Clone, Default)]
pub struct NetworkActivity {
    pub bytes_sent_per_sec: u64,
    pub bytes_recv_per_sec: u64,
}
```

### Built-in Detectors

| Detector         | Confidence Signals                          | Detected Context          |
| ---------------- | ------------------------------------------- | ------------------------- |
| `WorkDetector`   | IDE/terminal/browser focused, low idle time | `Working` at 0.4-0.8      |
| `GamingDetector` | Fullscreen + high GPU + game process        | `Gaming` at 0.7-0.95      |
| `MediaDetector`  | Media player focused, audio playing         | `Media` at 0.6-0.85       |
| `StreamDetector` | OBS running + streaming platform active     | `Streaming` at 0.8-0.9    |
| `CallDetector`   | Call app focused + microphone active        | `InCall` at 0.7-0.9       |
| `AfkDetector`    | Screen locked or idle > 5 minutes           | `Idle`/`Away` at 0.6-0.99 |

### Evaluation Cycle

The context engine runs once per second:

1. Collect `SystemSignals` from all trigger sources.
2. Run every detector, collecting `Vec<(UserContext, f32)>` votes.
3. Select the vote with the highest confidence above `confidence_threshold`.
4. Apply hysteresis: the candidate must win for `hysteresis_count` consecutive evaluations.
5. If context changes: push `ContextChange` to history, emit event, update priority stack.

---

## 14. Safety

Safety systems operate at the engine level, below the rule engine. They cannot be overridden by automation rules (only by explicit user configuration). Their purpose is to prevent photosensitive harm and excessive hardware wear.

### SafetyLimiter

```rust
/// Engine-level safety constraints on lighting behavior.
///
/// Applied in the final output stage, after all scene composition,
/// transitions, and effects have rendered. The safety limiter has
/// the absolute last word on what reaches hardware.
///
/// These limits are configurable but ship with conservative defaults.
/// Users can relax them (at their own risk) but not disable them entirely
/// -- the `min_transition_ms` and `max_strobe_frequency` floors are
/// hardcoded at 16ms and 5Hz respectively.
pub struct SafetyLimiter {
    /// Maximum scene switches per second.
    /// Prevents strobe-like rapid scene cycling from automation bugs.
    /// Default: `2.0` (one switch every 500ms).
    /// Hardcoded floor: `0.1` (one switch every 10s minimum cannot be set).
    pub max_switches_per_second: f32,

    /// Maximum brightness change per frame (at 60fps).
    /// Prevents instantaneous full-black-to-full-white transitions
    /// that could cause discomfort.
    /// Default: `0.1` (10% per frame = full ramp in ~170ms at 60fps).
    pub max_brightness_delta_per_frame: f32,

    /// Minimum transition duration in milliseconds.
    /// Overrides any `TransitionSpec` that specifies a shorter duration
    /// (including `Cut`, which becomes a very fast crossfade).
    /// Default: `100`.
    /// Hardcoded floor: `16` (one frame at 60fps).
    pub min_transition_ms: u64,

    /// Photosensitive mode. When enabled:
    /// - All `Flash` transitions become `Crossfade`.
    /// - All `Cut` transitions become `Crossfade` with `min_transition_ms`.
    /// - `max_brightness_delta_per_frame` is halved.
    /// - `max_strobe_frequency` is reduced to `1.0 Hz`.
    /// Default: `false`.
    pub photosensitive_mode: bool,

    /// Maximum frequency for strobe/flash effects (Hz).
    /// Applied to effects that produce rapid brightness oscillation.
    /// Default: `3.0 Hz`.
    /// Hardcoded ceiling: `5.0 Hz` (above this, even non-photosensitive
    /// users report discomfort).
    pub max_strobe_frequency: f32,

    // -- Internal state --

    /// Timestamp of the last scene switch. Used by `allow_switch()`.
    last_switch: Instant,

    /// Rolling window of recent switches for burst detection.
    switch_history: VecDeque<Instant>,
}
```

### Rate Limiter Logic

```rust
impl SafetyLimiter {
    /// Check whether a scene switch is allowed right now.
    ///
    /// Returns `false` (and logs a warning) if the switch rate
    /// would exceed `max_switches_per_second`.
    pub fn allow_switch(&mut self) -> bool {
        let now = Instant::now();
        let min_interval = Duration::from_secs_f32(
            1.0 / self.max_switches_per_second
        );

        if now.duration_since(self.last_switch) < min_interval {
            tracing::warn!(
                elapsed_ms = now.duration_since(self.last_switch).as_millis(),
                min_ms = min_interval.as_millis(),
                "Scene switch rate-limited"
            );
            return false;
        }

        self.last_switch = now;
        true
    }

    /// Clamp a brightness change to the per-frame maximum.
    ///
    /// Called per-LED per-frame in the output stage.
    pub fn clamp_brightness_change(
        &self,
        current: f32,
        target: f32,
    ) -> f32 {
        let delta = (target - current).clamp(
            -self.max_brightness_delta_per_frame,
            self.max_brightness_delta_per_frame,
        );
        (current + delta).clamp(0.0, 1.0)
    }

    /// Apply photosensitive mode transformations to a transition spec.
    ///
    /// If photosensitive mode is active, this replaces aggressive
    /// transitions with gentler alternatives.
    pub fn sanitize_transition(&self, spec: &TransitionSpec) -> TransitionSpec {
        if !self.photosensitive_mode {
            return spec.clone();
        }

        let mut sanitized = spec.clone();

        // Replace Flash and Cut with Crossfade
        sanitized.transition_type = match &spec.transition_type {
            TransitionType::Cut => TransitionType::Crossfade,
            TransitionType::Flash { .. } => TransitionType::Crossfade,
            other => other.clone(),
        };

        // Enforce minimum duration
        let min = Duration::from_millis(self.min_transition_ms);
        if sanitized.duration < min {
            sanitized.duration = min;
        }

        sanitized
    }
}
```

### Conflict Resolution

When multiple rules or sources request conflicting actions simultaneously:

1. **Priority wins**: Higher-priority rule's action is executed. The lower-priority action is discarded (not queued).
2. **Same priority, same source**: Most recently triggered wins.
3. **Same priority, different sources**: The source with higher inherent priority wins. Source priority order: `Manual > Schedule > Rule > Context > Circadian`.
4. **Overlapping scopes**: If two rules target different scopes (e.g., one targets `PcOnly`, another targets `RoomOnly`), both execute -- they don't conflict because they target disjoint zones.

### Photosensitive Mode Transformations

| Original                      | Photosensitive Replacement                        |
| ----------------------------- | ------------------------------------------------- |
| `Cut`                         | `Crossfade` with `min_transition_ms`              |
| `Flash`                       | `Crossfade` with `max(flash_duration * 3, 500ms)` |
| Strobe effects > 1Hz          | Capped to 1Hz pulse                               |
| Brightness delta > 0.05/frame | Clamped to 0.05/frame                             |
| `TemporaryOverlay` < 1s       | Extended to 1s minimum                            |

### Crash Recovery and State Persistence

The scene manager persists its state to disk at two points:

1. **On every scene change**: The active `SceneId` and priority stack are serialized to `~/.config/hypercolor/state.json`.
2. **On graceful shutdown**: A `clean_exit` flag is set to `true`.

On startup:

1. Read `state.json`. If `clean_exit` is `false`, this was a crash.
2. On crash recovery: restore the last scene with a gentle crossfade.
3. On normal startup: evaluate the schedule for the current time and activate the appropriate scene.
4. Set `clean_exit = false` (will be set to `true` on next graceful shutdown).

---

## Crate Dependencies

| Crate        | Version | Purpose                           | License        |
| ------------ | ------- | --------------------------------- | -------------- |
| `cron`       | 0.13+   | Cron expression parsing           | MIT/Apache-2.0 |
| `sun`        | 0.2+    | Sunrise/sunset solar position     | MIT            |
| `chrono`     | 0.4+    | Date/time handling                | MIT/Apache-2.0 |
| `chrono-tz`  | 0.10+   | Timezone database                 | MIT/Apache-2.0 |
| `rumqttc`    | 0.24+   | Async MQTT client                 | Apache-2.0     |
| `oklab`      | 1.0+    | Perceptual color space conversion | MIT            |
| `uuid`       | 1.0+    | UUID v7 scene/rule identifiers    | MIT/Apache-2.0 |
| `serde_json` | 1.0+    | Trigger payloads, HA API          | MIT/Apache-2.0 |
| `regex`      | 1.0+    | Condition expression matching     | MIT/Apache-2.0 |
| `tokio`      | 1.0+    | Async runtime (scheduler, timers) | MIT            |

---

## Module Layout

```
hypercolor-core/src/
  scenes/
    mod.rs              // Scene, SceneScope, ComposedScene, SceneOverlay
    assignment.rs       // ZoneAssignment, ControlValue
    storage.rs          // TOML serialization, file I/O
    composition.rs      // Overlay resolution logic
  transitions/
    mod.rs              // TransitionSpec, TransitionType, EasingFunction
    engine.rs           // TransitionEngine, TransitionState, blend pipeline
    oklab.rs            // Oklab conversion, lerp, blend_colors
    wipe.rs             // Spatial wipe mechanics, smoothstep
  schedule/
    mod.rs              // Scheduler, ScheduleRule, ScheduleExpr
    solar.rs            // SolarTimes, sunrise/sunset computation
    circadian.rs        // CircadianEngine, CircadianProfile, color temp filter
    vacation.rs         // VacationMode simulation
  triggers/
    mod.rs              // TriggerSource trait, TriggerEvent, TriggerState
    desktop.rs          // DesktopTriggerSource (D-Bus)
    app.rs              // AppTriggerSource (process monitoring)
    system.rs           // SystemTriggerSource (udev, network)
    audio.rs            // AudioTriggerSource
    mqtt.rs             // MqttTriggerSource
    homeassistant.rs    // HomeAssistantTriggerSource
    stream.rs           // StreamTriggerSource (Twitch EventSub)
    calendar.rs         // CalendarTriggerSource
  automation/
    mod.rs              // AutomationRule, RuleId
    trigger_expr.rs     // TriggerExpr (tree-structured event matching)
    condition_expr.rs   // ConditionExpr (tree-structured state checks)
    action_expr.rs      // ActionExpr (tree-structured actions)
    engine.rs           // Rule evaluation loop, cooldown tracking
  priority/
    mod.rs              // PriorityStack, StackEntry, StackTransition
  context/
    mod.rs              // ContextEngine, UserContext, SystemSignals
    detectors.rs        // Built-in ContextDetector implementations
  safety/
    mod.rs              // SafetyLimiter, photosensitive mode, crash recovery
```

---

_This specification is part of the Hypercolor technical spec series. See also: [ARCHITECTURE.md](../../ARCHITECTURE.md) for system overview._
