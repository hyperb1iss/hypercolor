//! The Stage's two views and the rule binding them to surface kind.
//!
//! Leptos-free so the resolve rule stays unit-testable; `stage.rs` owns
//! the reactive wiring.

/// Which view the Stage center shows for the selected surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StageView {
    /// The live preview — a Light's LED canvas or a Screen's face.
    #[default]
    Output,
    /// The spatial device-placement editor. Lights only.
    Layout,
}

/// Resolve the view actually shown. The Layout view is hidden for Screen
/// surfaces (§6.3) — a single LCD has no spatial placement to edit — so a
/// Screen always falls back to Output even while Layout stays the last
/// requested view for when a Light is reselected.
#[must_use]
pub fn resolve_stage_view(requested: StageView, is_screen: bool) -> StageView {
    if is_screen {
        StageView::Output
    } else {
        requested
    }
}
