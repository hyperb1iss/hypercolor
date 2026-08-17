//! The live scene tree — `/api/v1/scene` (Spec 78 §1, §2.3).
//!
//! One model on the wire: the active scene's zones, each with its
//! member device segments and its layer stack. These types are the
//! REST adapter's vocabulary for `SceneMutation`/`commit_scene`; they
//! carry no behavior of their own.
//!
//! Layer identity is contractual (Spec 78 §1.4): every layer id is
//! minted at creation, replacement mints a fresh id, and only control
//! patches and reorders mutate in place. The document carries one wire
//! version — `revision`, the commit generation — served as `ETag` and
//! honored via optional `If-Match` on structural mutations (§1.6).
//!
//! No `ToSchema` here: the embedded scene graph does not derive it,
//! and schema registration arrives with the `utoipa-axum` catalog
//! (Spec 78 §7.2, after wave 78.5) rather than as orphan schemas now.

use std::collections::BTreeMap;

use crate::effect::ControlValue;
use crate::identity::LayoutId;
use crate::layer::{
    LayerAdjust, LayerBlendMode, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use crate::library::PresetId;
use crate::scene::{DisplayFaceTarget, SceneId, UnassignedBehavior, ZoneId, ZoneRole};
use crate::spatial::SpatialLayout;
use serde::{Deserialize, Serialize};

/// The `GET /scene` document: the full live tree.
///
/// Always present — an active scene always exists (Spec 78 §1.1), so
/// there is no idle sentinel and no all-optional shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneDocument {
    pub id: SceneId,
    pub name: String,
    /// Whether this is the auto-managed default scene, which cannot be
    /// renamed or deleted.
    pub is_default: bool,
    #[serde(default)]
    pub unassigned_behavior: UnassignedBehavior,
    /// The named spatial layout this scene references, if any
    /// (Spec 78 §3.2). Activation applies it; a dangling reference is
    /// kept and skipped with a warning event.
    #[serde(default)]
    pub layout_id: Option<LayoutId>,
    /// The commit generation. Served as `ETag`; the one wire version
    /// token (Spec 78 §1.6).
    pub revision: u64,
    /// Every authored zone with its full stack, declaration order.
    pub zones: Vec<ZoneResource>,
}

/// One authored zone inside the live document (Spec 78 §1.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoneResource {
    pub id: ZoneId,
    pub name: String,
    #[serde(default)]
    pub role: ZoneRole,
    pub enabled: bool,
    pub brightness: f32,
    #[serde(default)]
    pub color: Option<String>,
    /// Present on Display-role zones only.
    #[serde(default)]
    pub display_target: Option<DisplayFaceTarget>,
    /// Device segments assigned to this zone, addressed by membership
    /// id (Spec 78 §1.2) — never by device-scoped segment name.
    pub members: Vec<ZoneMember>,
    /// The authored bottom-to-top layer stack. Layers are the real,
    /// addressable unit: clients patch the layer id they read here.
    pub layers: Vec<SceneLayer>,
}

/// One zone membership: a device segment's assignment, with its own
/// identity (Spec 78 §1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneMember {
    /// The membership's globally unique id — the resource identity for
    /// `DELETE .../members/{member}`.
    pub id: String,
    /// Backend device identifier (`"<backend>:<device_id>"`).
    pub device_id: String,
    /// The device segment this membership assigns. `None` for
    /// single-segment devices.
    #[serde(default)]
    pub segment: Option<String>,
    /// Human-readable name carried from the layout output.
    pub name: String,
}

/// `PATCH /scene` — scene-level fields only (Spec 78 §1.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenePatchRequest {
    /// Rename; rejected for the default scene.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unassigned_behavior: Option<UnassignedBehavior>,
}

/// `POST /scene/clear` — the "stop" gesture (Spec 78 §1.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClearSceneRequest {
    /// Clear one zone's stack; omitted clears every zone's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<ZoneId>,
}

/// `POST /scene/zones` — create a zone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateZoneRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<ZoneRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// `PATCH /scene/zones/{zone}` — name and structural fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchZoneRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// `PUT /scene/zones/{zone}/layout` — zone-scoped spatial override.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoneLayoutRequest {
    pub layout: SpatialLayout,
}

/// `POST /scene/zones/{zone}/members` — assign device segments.
///
/// The request names a device and its segments; the response's zone
/// resource carries the minted membership ids (Spec 78 §1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignMembersRequest {
    pub device_id: String,
    /// Segment names to assign; empty assigns the whole device on
    /// single-segment hardware.
    #[serde(default)]
    pub segments: Vec<String>,
}

/// `POST /scene/zones/{zone}/layers` — append a layer to the stack.
///
/// The server mints the layer id (Spec 78 §1.4); the response's zone
/// resource carries it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateLayerRequest {
    pub source: LayerSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend: Option<LayerBlendMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<LayerTransform>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjust: Option<LayerAdjust>,
}

/// `PUT /scene/zones/{zone}/layers/{layer}` — whole-layer replace.
///
/// Replacement is creation: every successful `PUT` mints a fresh layer
/// id, same effect or not (Spec 78 §1.4). The request shape is the
/// creation shape; the path names the layer being replaced.
pub type ReplaceLayerRequest = CreateLayerRequest;

/// `PATCH /scene/zones/{zone}/layers/order` — reorder the stack.
///
/// `order` names every layer in the zone exactly once, bottom to top;
/// anything else is a validation error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReorderLayersRequest {
    pub order: Vec<SceneLayerId>,
}

/// The one control-patch shape, used verbatim at every scope: layer
/// controls, display face controls, control-surface values
/// (Spec 78 §5.7).
///
/// `clear_bindings` is meaningful only where bindings exist (layers);
/// other scopes reject a non-empty list with a validation error. A
/// patch naming a control key with an active input binding is rejected
/// 409 `control_bound` unless the same request clears that binding —
/// removal and the accompanying values land in one atomic commit
/// (Spec 78 §1.6).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchControlsRequest {
    #[serde(default)]
    pub values: BTreeMap<String, ControlValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clear_bindings: Vec<String>,
}

/// The closed transition vocabulary (Spec 78 §2.3).
///
/// Grows when the engine does; the request field does not accept
/// aspirational values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionType {
    /// Immediate switch — the only transition the engine performs.
    #[default]
    Cut,
}

/// `POST /effects/{id}/apply` — the sugar request (Spec 78 §2.3).
///
/// Replaces the target zone's layer stack with a single new layer
/// running this effect; a projection of the same `SceneMutation` a
/// layer-stack replacement performs, never a second code path.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyEffectRequest {
    /// Target zone; omitted means the primary zone, created if the
    /// scene has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<ZoneId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controls: Option<BTreeMap<String, ControlValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_id: Option<PresetId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<TransitionType>,
}

/// `POST /effects/{id}/apply` — the sugar response: the updated zone
/// resource carrying the new layer's id, and the applied transition.
///
/// Post-commit side-effect failures (power wake) are reported inside a
/// 200 per Spec 78 §2.3; repair goes through the side effect's own
/// route, never a blind re-apply, because apply mints a fresh layer id
/// and is deliberately not idempotent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplyEffectResponse {
    pub zone: ZoneResource,
    pub transition: TransitionType,
    /// `false` when the post-commit power wake failed; the commit
    /// stands and `PATCH /output` repairs.
    pub output_woken: bool,
}
