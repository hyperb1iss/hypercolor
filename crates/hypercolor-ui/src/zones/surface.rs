//! The §8 surface model — the UI presentation of one zone.
//!
//! A *surface* is a name, a Stage, and a layer stack. Zones and Screens
//! are the same shape, so a multi-zone scene is more rows in the rail,
//! never a rebuilt editor. Kept leptos-free for `#[path]` tests.

use hypercolor_types::layer::{LayerSource, SceneLayer};
use hypercolor_types::scene::ZoneRole;

use crate::api::ZoneResource;

/// Synthetic rail-entry id for the §9.4 Unassigned entry. It is not a
/// surface — it has no layer stack and no Stage — so it never collides
/// with a real `ZoneId` (a UUID, which this is not).
pub const UNASSIGNED_SURFACE_ID: &str = "__unassigned__";

/// Which rail section a surface belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    /// An LED zone — listed under **Zones**.
    Light,
    /// A display-face zone — listed under **Screens**.
    Screen,
}

/// One zone as the UI presents it. The zone id is held for
/// addressing mutations but is never shown to the user.
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    pub id: String,
    pub name: String,
    pub kind: SurfaceKind,
    pub enabled: bool,
    /// Semantic role of the backing zone. `Primary` is the §9.2
    /// Default zone — it cannot be deleted through the zone endpoints.
    pub role: ZoneRole,
    /// Optional UI accent color for the zone swatch (§9.2).
    pub color: Option<String>,
    /// Physical display device backing a Screen surface — the key the
    /// Stage subscribes to for that screen's live face preview. `None`
    /// for LED zones and for display zones with no target assigned yet.
    pub display_device_id: Option<String>,
    /// Ids of the layers this surface currently holds. The degraded
    /// indicator filters streamed layer health against this live set, so
    /// a stale entry for an already-removed layer cannot keep the surface
    /// flagged after the layer is gone.
    pub layer_ids: Vec<String>,
    /// Display label of the surface's top layer — the §9.5 tile caption.
    /// `None` when the surface has no layers.
    pub top_layer: Option<String>,
}

impl Surface {
    /// Whether this surface is an LED zone the user may delete. The
    /// `Primary` Default zone is permanent; `Custom` zones are removable.
    #[must_use]
    pub fn is_deletable_zone(&self) -> bool {
        self.kind == SurfaceKind::Light && self.role == ZoneRole::Custom
    }
}

/// The primary LED zone: the `Primary` role, else the first LED zone.
#[must_use]
pub fn primary_led_zone(zones: &[Surface]) -> Option<&Surface> {
    zones
        .iter()
        .find(|zone| zone.role == ZoneRole::Primary)
        .or_else(|| zones.iter().find(|zone| zone.kind == SurfaceKind::Light))
}

/// The LED zone effect applies, presets, controls, and the preview
/// target: the focused zone while it is still an LED zone of the scene,
/// otherwise the primary zone.
#[must_use]
pub fn target_zone<'a>(zones: &'a [Surface], focused: Option<&str>) -> Option<&'a Surface> {
    focused
        .and_then(|id| {
            zones
                .iter()
                .find(|zone| zone.id == id && zone.kind == SurfaceKind::Light)
        })
        .or_else(|| primary_led_zone(zones))
}

/// The zone an effect apply names. The primary zone is the daemon's
/// default target, so it goes unnamed and keeps the primary apply path.
#[must_use]
pub fn apply_zone_id(target: Option<&Surface>) -> Option<String> {
    target
        .filter(|zone| zone.role != ZoneRole::Primary)
        .map(|zone| zone.id.clone())
}

/// Count of LED-role zones in a scene. A scene is multi-zone when
/// this exceeds one — the trigger for the per-zone controls and the
/// zone-assignment panel.
#[must_use]
pub fn led_zone_count(zones: &[ZoneResource]) -> usize {
    zones
        .iter()
        .filter(|zone| zone.role != ZoneRole::Display)
        .count()
}

/// Build the surface list from the active scene's zones, in scene
/// order. LED-role zones become zone surfaces; display-role zones become
/// Screens.
#[must_use]
pub fn surfaces_from_zones(zones: &[ZoneResource]) -> Vec<Surface> {
    zones
        .iter()
        .map(|zone| {
            let kind = if zone.role == ZoneRole::Display {
                SurfaceKind::Screen
            } else {
                SurfaceKind::Light
            };
            Surface {
                id: zone.id.to_string(),
                name: zone_display_name(&zone.name, zone.role),
                kind,
                enabled: zone.enabled,
                role: zone.role,
                color: zone.color.clone(),
                display_device_id: zone
                    .display_target
                    .as_ref()
                    .map(|target| target.device_id.to_string()),
                layer_ids: zone
                    .layers
                    .iter()
                    .map(|layer| layer.id.to_string())
                    .collect(),
                top_layer: top_layer_label(zone),
            }
        })
        .collect()
}

/// Display label of a zone's top layer, the last entry of the
/// bottom-to-top authored stack. Uses the layer's user-set name when it
/// has one, otherwise a plain-words label for its source kind.
fn top_layer_label(zone: &ZoneResource) -> Option<String> {
    let top = zone.layers.last()?;
    Some(
        top.name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| layer_source_kind(&top.source).to_owned()),
    )
}

/// Display label of one layer: its user-set name, else the effect it
/// runs (when the caller resolved one), else a plain-words label for its
/// source kind.
#[must_use]
pub fn layer_label(layer: &SceneLayer, effect_name: Option<&str>) -> String {
    layer
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| effect_name.map(ToOwned::to_owned))
        .unwrap_or_else(|| layer_source_kind(&layer.source).to_owned())
}

/// Plain-words label for a layer source kind — never an internal enum
/// name (§4 hard rule).
fn layer_source_kind(source: &LayerSource) -> &'static str {
    match source {
        LayerSource::Effect { .. } => "Effect",
        LayerSource::Media { .. } => "Media",
        LayerSource::ScreenRegion { .. } => "Screen capture",
        LayerSource::WebViewport { .. } => "Web page",
        LayerSource::ColorFill { .. } => "Color",
    }
}

/// The name a zone shows everywhere in the UI: its stored name, trimmed.
/// The primary zone reads **"Main"** while it has no name of its own,
/// meaning an empty name or the daemon's seeded "Primary" in any case.
#[must_use]
pub fn zone_display_name(name: &str, role: ZoneRole) -> String {
    let trimmed = name.trim();
    let unnamed = trimmed.is_empty() || trimmed.eq_ignore_ascii_case("primary");
    if role == ZoneRole::Primary && unnamed {
        "Main".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// What the Stage's now-playing chip names for a surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowPlaying {
    pub label: String,
    /// The label names the display's stored default face rather than a
    /// layer in the scene.
    pub is_default_face: bool,
}

/// Resolve the now-playing label. A surface's top scene layer wins; a
/// Screen with no scene layers that paints its stored default names that
/// face instead, so a visibly running face never reads as "No layers".
#[must_use]
pub fn now_playing(surface: Option<&Surface>, default_face: Option<&str>) -> NowPlaying {
    if let Some(label) = surface.and_then(|surface| surface.top_layer.clone()) {
        return NowPlaying {
            label,
            is_default_face: false,
        };
    }
    let default_face = surface
        .filter(|surface| surface.kind == SurfaceKind::Screen)
        .and(default_face)
        .map(str::trim)
        .filter(|name| !name.is_empty());
    match default_face {
        Some(name) => NowPlaying {
            label: name.to_owned(),
            is_default_face: true,
        },
        None => NowPlaying {
            label: "No layers".to_owned(),
            is_default_face: false,
        },
    }
}

/// The display device behind the selected surface, when that surface is
/// a Screen with a bound target.
#[must_use]
pub fn selected_screen_device_id(zones: &[ZoneResource], selected_id: &str) -> Option<String> {
    zones
        .iter()
        .find(|zone| zone.id.to_string() == selected_id && zone.role == ZoneRole::Display)
        .and_then(|zone| zone.display_target.as_ref())
        .map(|target| target.device_id.to_string())
}
