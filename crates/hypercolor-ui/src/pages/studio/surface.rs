//! The §8 surface model — the UI presentation of one zone.
//!
//! A *surface* is a name, a Stage, and a layer stack. Zones and Screens
//! are the same shape, so a multi-zone scene is more rows in the rail,
//! never a rebuilt editor. Kept leptos-free for `#[path]` tests.

use hypercolor_types::layer::LayerSource;
use hypercolor_types::scene::{Zone, ZoneRole};

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

/// One zone as the UI presents it. The group id is held for
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
    /// for LED zones and for display groups with no target assigned yet.
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

/// Count of LED-role zones in a scene. A scene is multi-zone when
/// this exceeds one — the trigger for the per-zone controls and the
/// zone-assignment panel.
#[must_use]
pub fn led_zone_count(groups: &[Zone]) -> usize {
    groups
        .iter()
        .filter(|group| group.role != ZoneRole::Display)
        .count()
}

/// Build the surface list from the active scene's zones, in scene
/// order. LED-role groups become zone surfaces; display-role groups become
/// Screens.
#[must_use]
pub fn surfaces_from_groups(groups: &[Zone]) -> Vec<Surface> {
    groups
        .iter()
        .map(|group| {
            let kind = if group.role == ZoneRole::Display {
                SurfaceKind::Screen
            } else {
                SurfaceKind::Light
            };
            Surface {
                id: group.id.to_string(),
                name: surface_name(group, kind),
                kind,
                enabled: group.enabled,
                role: group.role,
                color: group.color.clone(),
                display_device_id: group
                    .display_target
                    .as_ref()
                    .map(|target| target.device_id.to_string()),
                layer_ids: group
                    .effective_layers()
                    .iter()
                    .map(|layer| layer.id.to_string())
                    .collect(),
                top_layer: top_layer_label(group),
            }
        })
        .collect()
}

/// Display label of a group's top layer — the last entry of the
/// bottom-to-top authored stack. Uses the layer's user-set name when it
/// has one, otherwise a plain-words label for its source kind.
fn top_layer_label(group: &Zone) -> Option<String> {
    let layers = group.effective_layers();
    let top = layers.last()?;
    Some(
        top.name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| layer_source_kind(&top.source).to_owned()),
    )
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

/// Display name for a surface. A non-`Primary` group shows its stored
/// name. The `Primary` group is the Default zone (§3): it shows the
/// user's typed name, or **"Default zone"** while still unnamed. The
/// default zone is a zone at every scale.
fn surface_name(group: &Zone, kind: SurfaceKind) -> String {
    if kind != SurfaceKind::Light || group.role != ZoneRole::Primary {
        return group.name.clone();
    }
    if is_blank_default_name(&group.name) {
        "Default zone".to_owned()
    } else {
        group.name.clone()
    }
}

/// Whether the `Primary` group still carries its un-customized name. The
/// daemon seeds the Default zone as "Primary"; until the user renames it,
/// the rail shows the friendlier "Default zone" instead of leaking that
/// internal label.
fn is_blank_default_name(name: &str) -> bool {
    let trimmed = name.trim();
    trimmed.is_empty() || trimmed.eq_ignore_ascii_case("primary")
}
