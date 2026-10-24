//! Which zone the main preview follows.

use hypercolor_ui::preview_zone::preview_zone_for;

#[test]
fn a_single_zone_scene_keeps_the_scene_canvas() {
    // One LED zone: the scene canvas already is that zone.
    assert_eq!(
        preview_zone_for(false, true, Some("roli"), Some("default")),
        None
    );
    assert_eq!(preview_zone_for(false, false, None, Some("default")), None);
}

#[test]
fn a_page_with_a_zone_picker_shows_the_focused_zone() {
    assert_eq!(
        preview_zone_for(true, true, Some("roli"), Some("default")),
        Some("roli".to_owned())
    );
}

#[test]
fn a_zone_picker_with_nothing_focused_shows_the_default_zone() {
    assert_eq!(
        preview_zone_for(true, true, None, Some("default")),
        Some("default".to_owned())
    );
}

#[test]
fn a_page_without_a_zone_picker_always_shows_the_default_zone() {
    // The sidebar thumbnail has no picker, so a zone focused elsewhere
    // never leaks into it.
    assert_eq!(
        preview_zone_for(true, false, Some("roli"), Some("default")),
        Some("default".to_owned())
    );
}

#[test]
fn a_multi_zone_scene_without_a_default_zone_falls_back_to_the_scene_canvas() {
    assert_eq!(preview_zone_for(true, false, None, None), None);
}
