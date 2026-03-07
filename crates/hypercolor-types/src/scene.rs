//! Scene, transition, and automation rule types.
//!
//! This module defines the vocabulary for the scene graph, transition engine,
//! and automation rule system. Scenes are the fundamental unit of lighting
//! state — serializable, composable, restorable snapshots that describe what
//! every targeted LED should look like.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use uuid::Uuid;

// ── Scene Identity ───────────────────────────────────────────────────────

/// Opaque scene identifier. UUID v7 for time-sortable ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SceneId(pub Uuid);

impl SceneId {
    /// Create a new random scene identifier (UUID v7).
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for SceneId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SceneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── Scene ────────────────────────────────────────────────────────────────

/// A complete lighting state definition.
///
/// Scenes are self-contained: they carry their own transition preference,
/// their target scope, and every zone assignment needed to reproduce the
/// lighting state from scratch. No ambient state is assumed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    /// UUID v7 — time-sortable, globally unique.
    pub id: SceneId,

    /// Human-readable display name. Must be non-empty, max 128 chars.
    pub name: String,

    /// Optional long-form description. Rendered in web UI and scene galleries.
    pub description: Option<String>,

    /// Which devices/zones this scene targets.
    pub scope: SceneScope,

    /// Per-zone effect + parameter assignments.
    /// Each zone must appear at most once.
    pub zone_assignments: Vec<ZoneAssignment>,

    /// Default transition used when activating this scene.
    pub transition: TransitionSpec,

    /// Scene priority for conflict resolution.
    pub priority: ScenePriority,

    /// Whether this scene is currently enabled.
    pub enabled: bool,

    /// Freeform key-value metadata for extensions and UI display.
    pub metadata: HashMap<String, String>,
}

// ── Scene Scope ──────────────────────────────────────────────────────────

/// Determines which devices/zones a scene touches.
///
/// Applying a scene with a non-`Full` scope leaves all out-of-scope zones
/// in their current state. This enables independent PC vs. room control.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SceneScope {
    /// Every device the daemon manages.
    Full,

    /// PC-attached devices only: USB HID and other internal controllers.
    PcOnly,

    /// Network/room devices only: WLED strips, Hue bulbs, smart home endpoints.
    RoomOnly,

    /// Explicit device list by ID.
    Devices(Vec<String>),

    /// Explicit zone list. Most granular targeting.
    Zones(Vec<String>),
}

// ── Zone Assignment ──────────────────────────────────────────────────────

/// What a single zone should do within a scene.
///
/// The zone is identified by name (a composite of device + zone from the
/// spatial layout). The effect is referenced by string ID matching the
/// effect registry. Parameters are effect-specific key-value pairs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoneAssignment {
    /// Target zone identifier.
    pub zone_name: String,

    /// Effect to run on this zone.
    /// Special value `"static"` means a solid color with no animation.
    pub effect_name: String,

    /// Effect-specific parameters. Keys and value types are defined by
    /// each effect's parameter schema.
    pub parameters: HashMap<String, String>,

    /// Zone-level brightness override.
    /// Multiplied with the scene's global brightness.
    /// `None` means the zone inherits global brightness unmodified.
    /// Range: `0.0` to `1.0`.
    pub brightness: Option<f32>,
}

// ── Transition Spec ──────────────────────────────────────────────────────

/// Complete specification for a scene transition.
///
/// Carried on every scene as a default, but can be overridden at activation
/// time by the caller (schedule rule, automation rule, or manual API call).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionSpec {
    /// Total wall-clock duration of the transition in milliseconds.
    pub duration_ms: u64,

    /// Easing curve applied to the progress value.
    pub easing: EasingFunction,

    /// Color space used for interpolation during the transition.
    pub color_interpolation: ColorInterpolation,
}

// ── Easing Functions ─────────────────────────────────────────────────────

/// Easing functions for transition progress curves.
///
/// Maps raw linear progress `t` in `[0, 1]` to an eased value `t'` in `[0, 1]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    CubicBezier { x1: f32, y1: f32, x2: f32, y2: f32 },
}

impl EasingFunction {
    /// Apply the easing function to a linear progress value.
    ///
    /// Input `t` is clamped to `[0.0, 1.0]`. Output is the eased progress.
    #[must_use]
    pub fn apply(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);

        match self {
            Self::Linear => t,
            Self::EaseIn => t * t * t,
            Self::EaseOut => {
                let inv = 1.0 - t;
                1.0 - inv * inv * inv
            }
            Self::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    let inv = -2.0 * t + 2.0;
                    1.0 - inv * inv * inv / 2.0
                }
            }
            Self::CubicBezier { x1, y1, x2, y2 } => cubic_bezier_y(*x1, *y1, *x2, *y2, t),
        }
    }
}

/// Solve cubic bezier for the Y value at a given progress `t`.
///
/// Uses Newton-Raphson iteration to find the parameter value on the
/// bezier curve that corresponds to X = `t`, then evaluates Y at
/// that parameter.
fn cubic_bezier_y(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    // Find parameter `s` such that bezier_x(s) == t via Newton-Raphson.
    let mut s = t; // initial guess
    for _ in 0..8 {
        let x = bezier_component(x1, x2, s) - t;
        let dx = bezier_component_derivative(x1, x2, s);
        if dx.abs() < 1e-7 {
            break;
        }
        s -= x / dx;
        s = s.clamp(0.0, 1.0);
    }

    bezier_component(y1, y2, s)
}

/// Evaluate a single component of a cubic bezier at parameter `s`.
/// Control points P0=0, P1=c1, P2=c2, P3=1.
fn bezier_component(c1: f32, c2: f32, s: f32) -> f32 {
    let inv = 1.0 - s;
    // B(s) = 3(1-s)^2*s*c1 + 3(1-s)*s^2*c2 + s^3
    3.0 * inv * inv * s * c1 + 3.0 * inv * s * s * c2 + s * s * s
}

/// Derivative of a single bezier component with respect to `s`.
fn bezier_component_derivative(c1: f32, c2: f32, s: f32) -> f32 {
    let inv = 1.0 - s;
    // B'(s) = 3(1-s)^2*c1 + 6(1-s)*s*(c2-c1) + 3*s^2*(1-c2)
    3.0 * inv * inv * c1 + 6.0 * inv * s * (c2 - c1) + 3.0 * s * s * (1.0 - c2)
}

// ── Color Interpolation ──────────────────────────────────────────────────

/// Color space used for interpolation during transitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorInterpolation {
    /// Standard sRGB linear interpolation.
    Srgb,

    /// Oklab perceptual color space — maintains uniformity across blends.
    Oklab,
}

// ── Scene Priority ───────────────────────────────────────────────────────

/// Scene priority for conflict resolution. Higher values win.
///
/// When multiple scenes or rules compete for the same zones,
/// priority determines which one takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ScenePriority(pub u8);

impl ScenePriority {
    /// Background ambient lighting — lowest priority.
    pub const AMBIENT: Self = Self(0);

    /// User-activated scene — normal interactive priority.
    pub const USER: Self = Self(50);

    /// Trigger-activated scene — elevated priority from automation rules.
    pub const TRIGGER: Self = Self(75);

    /// Alert scene — highest priority for notifications and alarms.
    pub const ALERT: Self = Self(100);
}

impl Default for ScenePriority {
    fn default() -> Self {
        Self::USER
    }
}

impl fmt::Display for ScenePriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self.0 {
            0 => "ambient",
            50 => "user",
            75 => "trigger",
            100 => "alert",
            _ => return write!(f, "priority({})", self.0),
        };
        write!(f, "{label}")
    }
}

// ── Trigger Source ───────────────────────────────────────────────────────

/// Event sources that can trigger automation rules.
///
/// Each variant represents a different domain the system monitors.
/// The rule engine evaluates incoming trigger events against these
/// to decide when rules should fire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TriggerSource {
    /// Fire at a specific time of day (24-hour clock).
    TimeOfDay {
        /// Hour (0–23).
        hour: u8,
        /// Minute (0–59).
        minute: u8,
    },

    /// Fire at sunset (requires geolocation configuration).
    Sunset,

    /// Fire at sunrise (requires geolocation configuration).
    Sunrise,

    /// Fire when a specific application is launched.
    AppLaunched(String),

    /// Fire when system audio level crosses a threshold.
    AudioLevel {
        /// Normalized level threshold (0.0–1.0). Fires when audio exceeds this.
        threshold: f32,
    },

    /// Fire when a game is detected running.
    GameDetected,

    /// Manual activation via CLI or API.
    Manual,
}

// ── Automation Rule ──────────────────────────────────────────────────────

/// An automation rule: WHEN trigger fires AND conditions pass, DO action.
///
/// Rules are the declarative building blocks of Hypercolor's reactive
/// intelligence. They are event-driven (unlike schedules, which are
/// time-driven).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationRule {
    /// Human-readable rule name.
    pub name: String,

    /// The trigger that initiates rule evaluation.
    pub trigger: TriggerSource,

    /// Conditions that must all pass for the action to execute.
    /// Freeform string expressions — evaluated at trigger time.
    pub conditions: Vec<String>,

    /// The action to execute when trigger fires and conditions pass.
    pub action: ActionKind,

    /// Minimum seconds between consecutive firings of this rule.
    /// Prevents rapid-fire activation.
    pub cooldown_secs: u64,

    /// Whether this rule is currently active.
    pub enabled: bool,
}

// ── Action Kind ──────────────────────────────────────────────────────────

/// Actions that automation rules can perform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ActionKind {
    /// Activate a scene by name.
    ActivateScene(String),

    /// Adjust global brightness. Range: `0.0` to `1.0`.
    SetBrightness(f32),

    /// Pop the current scene and restore the previous one.
    RestorePrevious,
}
