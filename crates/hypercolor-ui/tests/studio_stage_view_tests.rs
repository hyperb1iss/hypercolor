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
        resolve_stage_view(StageView::Output, false, false),
        StageView::Output
    );
    assert_eq!(
        resolve_stage_view(StageView::Layout, false, false),
        StageView::Layout
    );
}

#[test]
fn a_screen_surface_has_no_layout_view() {
    // §6.3: a single LCD has no spatial placement, so Layout falls back
    // to Output for a Screen even when it was the last requested view.
    assert_eq!(
        resolve_stage_view(StageView::Layout, true, true),
        StageView::Output
    );
    assert_eq!(
        resolve_stage_view(StageView::Output, true, false),
        StageView::Output
    );
}

#[test]
fn the_all_zones_view_needs_more_than_one_zone() {
    // §9.5: the tiled All-zones glance is meaningless with a single zone,
    // so it falls back to Output until the scene is genuinely multi-zone.
    assert_eq!(
        resolve_stage_view(StageView::AllZones, false, false),
        StageView::Output
    );
    assert_eq!(
        resolve_stage_view(StageView::AllZones, false, true),
        StageView::AllZones
    );
    // A Screen never shows the All-zones view.
    assert_eq!(
        resolve_stage_view(StageView::AllZones, true, true),
        StageView::Output
    );
}
