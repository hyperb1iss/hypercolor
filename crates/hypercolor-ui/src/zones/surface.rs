//! The surface model: the UI presentation of one zone.
//!
//! A *surface* is a name, a Stage, and a layer stack. A multi-zone scene
//! is more tabs, never a rebuilt editor. A screen is not a surface: it is
//! an output of the zone that places it, and the display zones a daemon
//! lists for older clients are left out. Kept leptos-free for tests.

use hypercolor_types::layer::{LayerSource, SceneLayer};
use hypercolor_types::scene::ZoneRole;

use crate::api::ZoneResource;
use crate::components::layer_panel::scope::is_screen_layer;

/// One zone as the UI presents it. The zone id is held for
/// addressing mutations but is never shown to the user.
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Semantic role of the backing zone. The `Primary` zone (§9.2)
    /// cannot be deleted through the zone endpoints.
    pub role: ZoneRole,
    /// Optional UI accent color for the zone swatch (§9.2).
    pub color: Option<String>,
    /// Ids of the layers this surface currently holds. The degraded
    /// indicator filters streamed layer health against this live set, so
    /// a stale entry for an already-removed layer cannot keep the surface
    /// flagged after the layer is gone.
    pub layer_ids: Vec<String>,
    /// Display label of the layer the zone is known by, the tile caption.
    /// `None` when the zone has no layer of its own.
    pub top_layer: Option<String>,
}

/// The primary zone: the `Primary` role, else the first zone.
#[must_use]
pub fn primary_led_zone(zones: &[Surface]) -> Option<&Surface> {
    zones
        .iter()
        .find(|zone| zone.role == ZoneRole::Primary)
        .or_else(|| zones.first())
}

/// The zone effect applies, presets, controls, and the preview target:
/// the focused zone while the scene still holds it, otherwise the primary
/// zone.
#[must_use]
pub fn target_zone<'a>(zones: &'a [Surface], focused: Option<&str>) -> Option<&'a Surface> {
    focused
        .and_then(|id| zones.iter().find(|zone| zone.id == id))
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
/// order. A display zone is a screen's stack, or a view of it, and is no
/// surface of its own.
#[must_use]
pub fn surfaces_from_zones(zones: &[ZoneResource]) -> Vec<Surface> {
    zones
        .iter()
        .filter(|zone| zone.role != ZoneRole::Display)
        .map(|zone| Surface {
            id: zone.id.to_string(),
            name: zone_display_name(&zone.name, zone.role),
            enabled: zone.enabled,
            role: zone.role,
            color: zone.color.clone(),
            layer_ids: zone
                .layers
                .iter()
                .map(|layer| layer.id.to_string())
                .collect(),
            top_layer: top_layer_label(zone),
        })
        .collect()
}

/// The layers of `zone` that are its own: every layer but a screen's,
/// which plays on a panel and is no part of what the zone is showing.
#[must_use]
pub fn zone_own_layers(zone: &ZoneResource) -> Vec<&SceneLayer> {
    zone.layers
        .iter()
        .filter(|layer| !is_screen_layer(zone, layer))
        .collect()
}

/// The layer `zone` is known by: [`featured_position`] among its own
/// layers.
#[must_use]
pub fn zone_featured_layer(zone: &ZoneResource) -> Option<&SceneLayer> {
    let own = zone_own_layers(zone);
    featured_position(own.iter().copied()).and_then(|index| own.get(index).copied())
}

/// The place among `layers`, given bottom to top, of the layer a zone is
/// known by: the top of the layers that cover the whole zone. A layer
/// limited to some outputs plays on part of the zone, so it stands for
/// the zone only when nothing covers all of it.
#[must_use]
pub fn featured_position<'a>(layers: impl IntoIterator<Item = &'a SceneLayer>) -> Option<usize> {
    let mut whole_zone = None;
    let mut top = None;
    for (index, layer) in layers.into_iter().enumerate() {
        top = Some(index);
        if layer.scope.is_none() {
            whole_zone = Some(index);
        }
    }
    whole_zone.or(top)
}

/// Display label of the layer a zone is known by (see
/// [`zone_featured_layer`]). Uses the layer's user-set name when it has one,
/// otherwise a plain-words label for its source kind.
fn top_layer_label(zone: &ZoneResource) -> Option<String> {
    let top = zone_featured_layer(zone)?;
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
/// The primary zone reads **"Main"** while it has no name of its own:
/// an empty name, or one of the names older daemons seeded ("Primary",
/// "Default zone"), in any case.
#[must_use]
pub fn zone_display_name(name: &str, role: ZoneRole) -> String {
    let trimmed = name.trim();
    let unnamed = trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("primary")
        || trimmed.eq_ignore_ascii_case("default zone");
    if role == ZoneRole::Primary && unnamed {
        "Main".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// What the Stage's now-playing chip names for a zone: the layer it is
/// known by, or that it has none.
#[must_use]
pub fn now_playing(surface: Option<&Surface>) -> String {
    surface
        .and_then(|surface| surface.top_layer.clone())
        .unwrap_or_else(|| "No layers".to_owned())
}
