//! The Stage's views and the rules binding them to surface kind.
//!
//! Leptos-free so the resolve rules stay unit-testable; `stage.rs` owns
//! the reactive wiring.

/// Which view the Stage center shows for the selected surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StageView {
    /// The live preview — a Light's LED canvas or a Screen's face.
    #[default]
    Output,
    /// The spatial device-placement editor. Lights only.
    Layout,
    /// A tiled, scene-wide glance — one preview tile per LED zone (§9.5).
    /// Offered only while the scene is genuinely multi-zone.
    AllZones,
}

/// Resolve the view actually shown. The Layout view is hidden for Screen
/// surfaces (§6.3) — a single LCD has no spatial placement to edit — and
/// the All-zones view is only meaningful with more than one zone. A
/// Screen, or a single-zone scene, falls back to Output while the
/// requested view stays latched for when a Light is reselected.
#[must_use]
pub fn resolve_stage_view(requested: StageView, is_screen: bool, multi_zone: bool) -> StageView {
    match requested {
        _ if is_screen => StageView::Output,
        StageView::AllZones if !multi_zone => StageView::Output,
        view => view,
    }
}
