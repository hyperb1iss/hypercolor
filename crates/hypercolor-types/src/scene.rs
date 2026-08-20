//! Scene, transition, and automation rule types.
//!
//! This module defines the vocabulary for the scene graph, transition engine,
//! and automation rule system. Scenes are the fundamental unit of lighting
//! state — serializable, composable, restorable snapshots that describe what
//! every targeted LED should look like.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use uuid::{Uuid, uuid};

use crate::canvas::BlendMode;
use crate::device::DeviceId;
use crate::layer::{LayerSource, SceneLayer};
use crate::spatial::SpatialLayout;

// ── Scene Identity ───────────────────────────────────────────────────────

/// Opaque scene identifier. UUID v7 for time-sortable ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SceneId(pub Uuid);

impl SceneId {
    pub const DEFAULT: Self = Self(uuid!("00000000-0000-0000-0000-000000000000"));

    /// Create a new random scene identifier (UUID v7).
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::DEFAULT
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

// ── Render Groups ────────────────────────────────────────────────────────

/// Opaque zone identifier. UUID v7 for time-sortable ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ZoneId(pub Uuid);

impl ZoneId {
    /// Create a new random zone identifier (UUID v7).
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for ZoneId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ZoneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An independently laid out layer stack within a scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Zone {
    /// Unique identifier.
    pub id: ZoneId,

    /// Human-readable display name.
    pub name: String,

    /// Optional long-form description.
    pub description: Option<String>,

    /// Authored bottom-to-top layer stack for this zone.
    #[serde(default)]
    pub layers: Vec<SceneLayer>,

    /// Spatial layout used to sample this zone.
    pub layout: SpatialLayout,

    /// Per-zone brightness multiplier.
    #[serde(default = "default_zone_brightness")]
    pub brightness: f32,

    /// Whether this zone is currently active.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Optional UI accent color.
    pub color: Option<String>,

    /// Direct display target for face-style zones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_target: Option<DisplayFaceTarget>,

    /// Semantic role inside the scene.
    pub role: ZoneRole,

    /// Monotonic version counter for the control mutation stream.
    ///
    /// Bumped every time controls on any effect layer are patched.
    #[serde(default)]
    pub controls_version: u64,

    /// Monotonic version counter for the layer mutation stream.
    #[serde(default)]
    pub layers_version: u64,
}

impl Zone {
    /// Effect identities authored into this zone's layer stack.
    pub fn effect_ids(&self) -> impl Iterator<Item = crate::effect::EffectId> + '_ {
        self.layers.iter().filter_map(|layer| match layer.source {
            LayerSource::Effect { effect_id, .. } => Some(effect_id),
            _ => None,
        })
    }

    #[must_use]
    pub fn has_effect(&self, effect_id: crate::effect::EffectId) -> bool {
        self.effect_ids().any(|candidate| candidate == effect_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoneRole {
    #[default]
    Custom,
    Primary,
    Display,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayFaceBlendMode {
    Replace,
    #[default]
    Alpha,
    Tint,
    LumaReveal,
    Add,
    Screen,
    Multiply,
    Overlay,
    SoftLight,
    ColorDodge,
    Difference,
}

impl DisplayFaceBlendMode {
    #[must_use]
    pub fn blends_with_effect(self) -> bool {
        !matches!(self, Self::Replace)
    }

    #[must_use]
    pub fn standard_canvas_blend_mode(self) -> Option<BlendMode> {
        match self {
            Self::Replace | Self::Tint | Self::LumaReveal => None,
            Self::Alpha => Some(BlendMode::Normal),
            Self::Add => Some(BlendMode::Add),
            Self::Screen => Some(BlendMode::Screen),
            Self::Multiply => Some(BlendMode::Multiply),
            Self::Overlay => Some(BlendMode::Overlay),
            Self::SoftLight => Some(BlendMode::SoftLight),
            Self::ColorDodge => Some(BlendMode::ColorDodge),
            Self::Difference => Some(BlendMode::Difference),
        }
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_default_display_face_blend_mode(value: &DisplayFaceBlendMode) -> bool {
    matches!(value, DisplayFaceBlendMode::Alpha)
}

fn default_display_face_opacity() -> f32 {
    1.0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_default_display_face_opacity(value: &f32) -> bool {
    (*value - default_display_face_opacity()).abs() <= f32::EPSILON
}

/// Direct LCD target for a display-face zone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DisplayFaceTarget {
    /// Physical display-capable device that should consume this group's canvas.
    pub device_id: DeviceId,
    /// How the face layer should compose with the effect layer beneath it.
    #[serde(default, skip_serializing_if = "is_default_display_face_blend_mode")]
    pub blend_mode: DisplayFaceBlendMode,
    /// Face-layer opacity used when compositing with the effect layer.
    #[serde(
        default = "default_display_face_opacity",
        skip_serializing_if = "is_default_display_face_opacity"
    )]
    pub opacity: f32,
}

impl DisplayFaceTarget {
    #[must_use]
    pub fn new(device_id: DeviceId) -> Self {
        // Matches the enum's serde default: a fresh target blends the face
        // over the live effect. Seeding Replace here blacked out the effect
        // for every face assigned through a path that never patched the
        // target (the Studio add-layer flow).
        Self {
            device_id,
            blend_mode: DisplayFaceBlendMode::default(),
            opacity: default_display_face_opacity(),
        }
    }

    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.opacity = self.opacity.clamp(0.0, 1.0);
        self
    }

    #[must_use]
    pub fn blends_with_effect(&self) -> bool {
        self.blend_mode.blends_with_effect()
    }
}

impl Zone {
    /// Validate layer-stack invariants owned by this group.
    pub fn validate_layers(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        let mut seen = HashSet::new();
        for layer in &self.layers {
            if !seen.insert(layer.id) {
                errors.push(format!(
                    "zone '{}' has duplicate layer id {}",
                    self.name, layer.id
                ));
            }
            if let Err(mut layer_errors) = layer.validate() {
                errors.extend(
                    layer_errors
                        .drain(..)
                        .map(|error| format!("layer {} in '{}': {error}", layer.id, self.name)),
                );
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

fn default_zone_brightness() -> f32 {
    1.0
}

fn default_true() -> bool {
    true
}

/// How zones not claimed by any zone should behave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UnassignedBehavior {
    /// Unassigned zones render black.
    #[default]
    Off,
    /// Unassigned zones retain their previous colors.
    Hold,
    /// Route unassigned zones to a fallback zone.
    Fallback(ZoneId),
}

fn is_default_unassigned_behavior(value: &UnassignedBehavior) -> bool {
    matches!(value, UnassignedBehavior::Off)
}

// ── Scene ────────────────────────────────────────────────────────────────

/// A complete lighting state definition.
///
/// Scenes are self-contained: they carry their transition preference and
/// every authored zone needed to reproduce the lighting state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    /// UUID v7 — time-sortable, globally unique.
    pub id: SceneId,

    /// Human-readable display name. Must be non-empty, max 128 chars.
    pub name: String,

    /// Optional long-form description. Rendered in web UI and scene galleries.
    pub description: Option<String>,

    /// Independently laid out layer stacks owned by this scene.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<Zone>,

    /// Monotonic version counter for zone structure.
    #[serde(default)]
    pub zones_revision: u64,

    /// Default transition used when activating this scene.
    pub transition: TransitionSpec,

    /// Scene priority for conflict resolution.
    pub priority: ScenePriority,

    /// Whether this scene is currently enabled.
    pub enabled: bool,

    /// Freeform key-value metadata for extensions and UI display.
    pub metadata: HashMap<String, String>,

    /// Policy for zones not claimed by any zone.
    #[serde(default, skip_serializing_if = "is_default_unassigned_behavior")]
    pub unassigned_behavior: UnassignedBehavior,

    /// Named spatial layout this scene references (Spec 78 §3.2).
    /// Activation applies it; a dangling reference is kept and skipped
    /// with a warning event, never silently dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_id: Option<crate::identity::LayoutId>,

    /// Brightness applied to `/output` on activation, when present
    /// (Spec 78 §3.2). Deliberately NOT captured by snapshot —
    /// brightness is global output state; the field exists so migrated
    /// profiles keep their restore-brightness behavior and so a user
    /// can opt a scene in explicitly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation_brightness: Option<f32>,

    /// Whether this scene is daemon-managed or user-visible.
    pub kind: SceneKind,

    /// Whether live runtime actions are allowed to rewrite this scene.
    pub mutation_mode: SceneMutationMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneKind {
    #[default]
    Named,
    Ephemeral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneMutationMode {
    #[default]
    Live,
    Snapshot,
}

impl Scene {
    #[must_use]
    pub fn primary_zone(&self) -> Option<&Zone> {
        self.zones
            .iter()
            .find(|zone| zone.role == ZoneRole::Primary)
    }

    pub fn primary_zone_mut(&mut self) -> Option<&mut Zone> {
        self.zones
            .iter_mut()
            .find(|zone| zone.role == ZoneRole::Primary)
    }

    #[must_use]
    pub fn display_zone_for(&self, device_id: DeviceId) -> Option<&Zone> {
        self.zones.iter().find(|zone| {
            zone.role == ZoneRole::Display
                && zone
                    .display_target
                    .as_ref()
                    .is_some_and(|target| target.device_id == device_id)
        })
    }

    pub fn display_zone_for_mut(&mut self, device_id: DeviceId) -> Option<&mut Zone> {
        self.zones.iter_mut().find(|zone| {
            zone.role == ZoneRole::Display
                && zone
                    .display_target
                    .as_ref()
                    .is_some_and(|target| target.device_id == device_id)
        })
    }

    #[must_use]
    pub fn blocks_runtime_mutation(&self) -> bool {
        self.kind == SceneKind::Named && self.mutation_mode == SceneMutationMode::Snapshot
    }

    /// Ensure no zone is claimed by multiple zones.
    pub fn validate_zone_exclusivity(&self) -> Result<(), Vec<String>> {
        let mut seen = HashMap::<&str, &str>::new();
        let mut conflicts = Vec::new();

        for authored_zone in &self.zones {
            for output in &authored_zone.layout.zones {
                if let Some(existing_zone) =
                    seen.insert(output.id.as_str(), authored_zone.name.as_str())
                {
                    conflicts.push(format!(
                        "zone '{}' claimed by both '{}' and '{}'",
                        output.id, existing_zone, authored_zone.name
                    ));
                }
            }
        }

        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(conflicts)
        }
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.name.trim().is_empty() {
            errors.push("scene name must not be empty".to_owned());
        }
        if self.name.chars().count() > 128 {
            errors.push("scene name must be at most 128 characters".to_owned());
        }

        if let Err(mut conflicts) = self.validate_zone_exclusivity() {
            errors.append(&mut conflicts);
        }

        if let Some(brightness) = self.activation_brightness
            && !(brightness.is_finite() && (0.0..=1.0).contains(&brightness))
        {
            errors.push(format!(
                "activation_brightness must be within 0.0..=1.0, got {brightness}"
            ));
        }

        // Derived Deserialize on identity types skips their validator;
        // re-run it here so a malformed persisted reference is loud.
        if let Some(layout_id) = &self.layout_id
            && let Err(error) = crate::identity::LayoutId::new(layout_id.as_str())
        {
            errors.push(format!("layout_id is not a valid layout id: {error}"));
        }

        let primary_count = self
            .zones
            .iter()
            .filter(|zone| zone.role == ZoneRole::Primary)
            .count();
        if primary_count > 1 {
            errors.push("scene has more than one primary zone".to_owned());
        }

        let mut display_targets = HashMap::<DeviceId, ZoneId>::new();
        for zone in &self.zones {
            if let Err(mut layer_errors) = zone.validate_layers() {
                errors.append(&mut layer_errors);
            }

            match (&zone.role, &zone.display_target) {
                (ZoneRole::Display, None) => errors.push(format!(
                    "display zone '{}' is missing a display target",
                    zone.name
                )),
                (ZoneRole::Custom | ZoneRole::Primary, Some(_)) => {
                    errors.push(format!(
                        "zone '{}' has a display target but role '{}'",
                        zone.name,
                        match zone.role {
                            ZoneRole::Custom => "custom",
                            ZoneRole::Primary => "primary",
                            ZoneRole::Display => "display",
                        }
                    ));
                }
                (ZoneRole::Display, Some(target)) => {
                    if let Some(existing) = display_targets.insert(target.device_id, zone.id) {
                        errors.push(format!(
                            "duplicate display zones for device {} ({} and {})",
                            target.device_id, existing, zone.id
                        ));
                    }
                }
                _ => {}
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
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
