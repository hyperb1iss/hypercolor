//! Contract tests for the Stage view-resolution rule.

#[path = "../src/pages/studio/stage_view.rs"]
mod stage_view;

use stage_view::{StageView, resolve_stage_view};

#[test]
fn output_is_the_default_view() {
    assert_eq!(StageView::default(), StageView::Output);
}

#[test]
fn a_light_surface_keeps_the_requested_view() {
    assert_eq!(
        resolve_stage_view(StageView::Output, false),
        StageView::Output
    );
    assert_eq!(
        resolve_stage_view(StageView::Layout, false),
        StageView::Layout
    );
}

#[test]
fn a_screen_surface_has_no_layout_view() {
    // §6.3: a single LCD has no spatial placement, so Layout falls back
    // to Output for a Screen even when it was the last requested view.
    assert_eq!(
        resolve_stage_view(StageView::Layout, true),
        StageView::Output
    );
    assert_eq!(
        resolve_stage_view(StageView::Output, true),
        StageView::Output
    );
}
