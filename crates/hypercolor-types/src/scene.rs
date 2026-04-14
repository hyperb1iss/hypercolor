//! Scene, transition, and automation rule types.
//!
//! This module defines the vocabulary for the scene graph, transition engine,
//! and automation rule system. Scenes are the fundamental unit of lighting
//! state — serializable, composable, restorable snapshots that describe what
//! every targeted LED should look like.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use uuid::{Uuid, uuid};

use crate::device::DeviceId;
use crate::effect::{ControlBinding, ControlValue, EffectId};
use crate::library::PresetId;
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

/// Opaque render group identifier. UUID v7 for time-sortable ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RenderGroupId(pub Uuid);

impl RenderGroupId {
    /// Create a new random render group identifier (UUID v7).
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for RenderGroupId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RenderGroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An independent rendering pipeline within a scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderGroup {
    /// Unique identifier.
    pub id: RenderGroupId,

    /// Human-readable display name.
    pub name: String,

    /// Optional long-form description.
    pub description: Option<String>,

    /// Effect assigned to this group. `None` means the group is intentionally empty.
    pub effect_id: Option<EffectId>,

    /// Effect control overrides for this group.
    #[serde(default)]
    pub controls: HashMap<String, ControlValue>,

    /// Live sensor bindings applied to controls in this group.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub control_bindings: HashMap<String, ControlBinding>,

    /// Optional preset applied to the group.
    pub preset_id: Option<PresetId>,

    /// Spatial layout used to sample this group.
    pub layout: SpatialLayout,

    /// Per-group brightness multiplier.
    #[serde(default = "default_group_brightness")]
    pub brightness: f32,

    /// Whether this group is currently active.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Optional UI accent color.
    pub color: Option<String>,

    /// Direct display target for face-style render groups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_target: Option<DisplayFaceTarget>,

    /// Semantic role inside the scene.
    pub role: RenderGroupRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderGroupRole {
    #[default]
    Custom,
    Primary,
    Display,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayFaceBlendMode {
    #[default]
    Replace,
    Alpha,
}

fn is_default_display_face_blend_mode(value: &DisplayFaceBlendMode) -> bool {
    matches!(value, DisplayFaceBlendMode::Replace)
}

fn default_display_face_opacity() -> f32 {
    1.0
}

fn is_default_display_face_opacity(value: &f32) -> bool {
    (*value - default_display_face_opacity()).abs() <= f32::EPSILON
}

/// Direct LCD target for a display-face render group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DisplayFaceTarget {
    /// Physical display-capable device that should consume this group's canvas.
    pub device_id: DeviceId,
    /// How the face layer should compose with the effect layer beneath it.
    #[serde(
        default,
        skip_serializing_if = "is_default_display_face_blend_mode"
    )]
    pub blend_mode: DisplayFaceBlendMode,
    /// Face-layer opacity used when alpha blending with the effect layer.
    #[serde(
        default = "default_display_face_opacity",
        skip_serializing_if = "is_default_display_face_opacity"
    )]
    pub opacity: f32,
}

impl DisplayFaceTarget {
    #[must_use]
    pub fn new(device_id: DeviceId) -> Self {
        Self {
            device_id,
            blend_mode: DisplayFaceBlendMode::Replace,
            opacity: default_display_face_opacity(),
        }
    }

    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.opacity = self.opacity.clamp(0.0, 1.0);
        self
    }

    #[must_use]
    pub fn blends_with_effect(self) -> bool {
        matches!(self.blend_mode, DisplayFaceBlendMode::Alpha)
    }
}

impl RenderGroup {
    /// Flatten this render group into zone assignments.
    #[must_use]
    pub fn zone_assignments(&self) -> Vec<ZoneAssignment> {
        if !self.enabled {
            return Vec::new();
        }

        let Some(effect_id) = self.effect_id else {
            return Vec::new();
        };

        let parameters = self
            .controls
            .iter()
            .map(|(key, value)| (key.clone(), control_value_parameter(value)))
            .collect::<HashMap<_, _>>();

        self.layout
            .zones
            .iter()
            .map(|zone| ZoneAssignment {
                zone_name: zone.id.clone(),
                effect_name: effect_id.to_string(),
                parameters: parameters.clone(),
                brightness: Some(self.brightness),
            })
            .collect()
    }
}

fn default_group_brightness() -> f32 {
    1.0
}

fn default_true() -> bool {
    true
}

fn control_value_parameter(value: &ControlValue) -> String {
    match value {
        ControlValue::Float(value) => value.to_string(),
        ControlValue::Integer(value) => value.to_string(),
        ControlValue::Boolean(value) => value.to_string(),
        ControlValue::Enum(value) | ControlValue::Text(value) => value.clone(),
        ControlValue::Color([r, g, b, a]) => format!("{r:.6},{g:.6},{b:.6},{a:.6}"),
        ControlValue::Rect(value) => {
            format!(
                "{:.6},{:.6},{:.6},{:.6}",
                value.x, value.y, value.width, value.height
            )
        }
        ControlValue::Gradient(_) => serde_json::to_string(value).unwrap_or_default(),
    }
}

/// How zones not claimed by any render group should behave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UnassignedBehavior {
    /// Unassigned zones render black.
    #[default]
    Off,
    /// Unassigned zones retain their previous colors.
    Hold,
    /// Route unassigned zones to a fallback render group.
    Fallback(RenderGroupId),
}

fn is_default_unassigned_behavior(value: &UnassignedBehavior) -> bool {
    matches!(value, UnassignedBehavior::Off)
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

    /// Independent render pipelines owned by this scene.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<RenderGroup>,

    /// Default transition used when activating this scene.
    pub transition: TransitionSpec,

    /// Scene priority for conflict resolution.
    pub priority: ScenePriority,

    /// Whether this scene is currently enabled.
    pub enabled: bool,

    /// Freeform key-value metadata for extensions and UI display.
    pub metadata: HashMap<String, String>,

    /// Policy for zones not claimed by any render group.
    #[serde(default, skip_serializing_if = "is_default_unassigned_behavior")]
    pub unassigned_behavior: UnassignedBehavior,

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
    /// Whether this scene uses render groups instead of flat zone assignments.
    #[must_use]
    pub fn has_render_groups(&self) -> bool {
        !self.groups.is_empty()
    }

    /// Derive the effective scope for the currently active scene representation.
    #[must_use]
    pub fn effective_scope(&self) -> SceneScope {
        if !self.has_render_groups() {
            return self.scope.clone();
        }

        let zone_ids = self
            .groups
            .iter()
            .filter(|group| group.enabled)
            .flat_map(|group| group.layout.zones.iter().map(|zone| zone.id.clone()))
            .collect::<Vec<_>>();

        if zone_ids.is_empty() {
            SceneScope::Full
        } else {
            SceneScope::Zones(zone_ids)
        }
    }

    /// Flatten the active scene into zone assignments.
    #[must_use]
    pub fn effective_zone_assignments(&self) -> Vec<ZoneAssignment> {
        if !self.has_render_groups() {
            return self.zone_assignments.clone();
        }

        self.groups
            .iter()
            .flat_map(RenderGroup::zone_assignments)
            .collect()
    }

    #[must_use]
    pub fn primary_group(&self) -> Option<&RenderGroup> {
        self.groups
            .iter()
            .find(|group| group.role == RenderGroupRole::Primary)
    }

    pub fn primary_group_mut(&mut self) -> Option<&mut RenderGroup> {
        self.groups
            .iter_mut()
            .find(|group| group.role == RenderGroupRole::Primary)
    }

    #[must_use]
    pub fn display_group_for(&self, device_id: DeviceId) -> Option<&RenderGroup> {
        self.groups.iter().find(|group| {
            group.role == RenderGroupRole::Display
                && group
                    .display_target
                    .as_ref()
                    .is_some_and(|target| target.device_id == device_id)
        })
    }

    pub fn display_group_for_mut(&mut self, device_id: DeviceId) -> Option<&mut RenderGroup> {
        self.groups.iter_mut().find(|group| {
            group.role == RenderGroupRole::Display
                && group
                    .display_target
                    .as_ref()
                    .is_some_and(|target| target.device_id == device_id)
        })
    }

    #[must_use]
    pub fn blocks_runtime_mutation(&self) -> bool {
        self.kind == SceneKind::Named && self.mutation_mode == SceneMutationMode::Snapshot
    }

    /// Ensure no zone is claimed by multiple render groups.
    pub fn validate_group_exclusivity(&self) -> Result<(), Vec<String>> {
        if !self.has_render_groups() {
            return Ok(());
        }

        let mut seen = HashMap::<&str, &str>::new();
        let mut conflicts = Vec::new();

        for group in &self.groups {
            for zone in &group.layout.zones {
                if let Some(existing_group) = seen.insert(zone.id.as_str(), group.name.as_str()) {
                    conflicts.push(format!(
                        "zone '{}' claimed by both '{}' and '{}'",
                        zone.id, existing_group, group.name
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

        if let Err(mut conflicts) = self.validate_group_exclusivity() {
            errors.append(&mut conflicts);
        }

        let primary_count = self
            .groups
            .iter()
            .filter(|group| group.role == RenderGroupRole::Primary)
            .count();
        if primary_count > 1 {
            errors.push("scene has more than one primary render group".to_owned());
        }

        let mut display_targets = HashMap::<DeviceId, RenderGroupId>::new();
        for group in &self.groups {
            match (&group.role, &group.display_target) {
                (RenderGroupRole::Display, None) => errors.push(format!(
                    "display render group '{}' is missing a display target",
                    group.name
                )),
                (RenderGroupRole::Custom | RenderGroupRole::Primary, Some(_)) => {
                    errors.push(format!(
                        "render group '{}' has a display target but role '{}'",
                        group.name,
                        match group.role {
                            RenderGroupRole::Custom => "custom",
                            RenderGroupRole::Primary => "primary",
                            RenderGroupRole::Display => "display",
                        }
                    ));
                }
                (RenderGroupRole::Display, Some(target)) => {
                    if let Some(existing) = display_targets.insert(target.device_id, group.id) {
                        errors.push(format!(
                            "duplicate display render groups for device {} ({} and {})",
                            target.device_id, existing, group.id
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
