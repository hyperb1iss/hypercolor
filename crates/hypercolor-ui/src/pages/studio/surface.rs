//! The §8 surface model — the UI presentation of one render group.
//!
//! A *surface* is a name, a Stage, and a layer stack. Lights, Screens, and
//! "All Lights" are the same shape, so multi-zone (Wave 9) is more rows in
//! the rail, never a rebuilt editor. Kept leptos-free for `#[path]` tests.

use hypercolor_types::scene::{RenderGroup, RenderGroupRole};

/// Which rail section a surface belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    /// An LED render group — listed under **Lights**.
    Light,
    /// A display-face render group — listed under **Screens**.
    Screen,
}

/// One render group as the UI presents it. The group id is held for
/// addressing mutations but is never shown to the user.
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    pub id: String,
    pub name: String,
    pub kind: SurfaceKind,
    pub enabled: bool,
    /// Physical display device backing a Screen surface — the key the
    /// Stage subscribes to for that screen's live face preview. `None`
    /// for Lights and for display groups with no target assigned yet.
    pub display_device_id: Option<String>,
    /// Ids of the layers this surface currently holds. The degraded
    /// indicator filters streamed layer health against this live set, so
    /// a stale entry for an already-removed layer cannot keep the surface
    /// flagged after the layer is gone.
    pub layer_ids: Vec<String>,
}

/// Build the surface list from the active scene's render groups, in scene
/// order. LED-role groups become Lights; display-role groups become
/// Screens.
#[must_use]
pub fn surfaces_from_groups(groups: &[RenderGroup]) -> Vec<Surface> {
    let led_count = groups
        .iter()
        .filter(|group| group.role != RenderGroupRole::Display)
        .count();
    groups
        .iter()
        .map(|group| {
            let kind = if group.role == RenderGroupRole::Display {
                SurfaceKind::Screen
            } else {
                SurfaceKind::Light
            };
            Surface {
                id: group.id.to_string(),
                name: surface_name(group, kind, led_count),
                kind,
                enabled: group.enabled,
                display_device_id: group
                    .display_target
                    .as_ref()
                    .map(|target| target.device_id.to_string()),
                layer_ids: group
                    .effective_layers()
                    .iter()
                    .map(|layer| layer.id.to_string())
                    .collect(),
            }
        })
        .collect()
}

/// Display name for a surface. While a single LED group owns every output
/// it reads as **"All Lights"** (§9.2); the moment a second LED zone
/// exists the §9.2 Default-zone relabel takes over and the user's typed
/// names are shown.
fn surface_name(group: &RenderGroup, kind: SurfaceKind, led_count: usize) -> String {
    if kind == SurfaceKind::Light && group.role == RenderGroupRole::Primary && led_count == 1 {
        "All Lights".to_owned()
    } else {
        group.name.clone()
    }
}
