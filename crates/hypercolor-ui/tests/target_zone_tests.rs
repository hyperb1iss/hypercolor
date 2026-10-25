//! The one target zone: which LED zone applies, presets, controls, and
//! the preview act on, and how the zone tabs move between zones.

use hypercolor_types::scene::ZoneRole;
use hypercolor_ui::components::zone_tabs::next_tab;
use hypercolor_ui::zones::surface::{
    Surface, SurfaceKind, apply_zone_id, primary_led_zone, target_zone,
};

fn surface(id: &str, kind: SurfaceKind, role: ZoneRole) -> Surface {
    Surface {
        id: id.to_owned(),
        name: id.to_owned(),
        kind,
        enabled: true,
        role,
        color: None,
        display_device_id: None,
        layer_ids: Vec::new(),
        top_layer: None,
    }
}

fn scene_zones() -> Vec<Surface> {
    vec![
        surface("screen", SurfaceKind::Screen, ZoneRole::Display),
        surface("main", SurfaceKind::Light, ZoneRole::Primary),
        surface("roli", SurfaceKind::Light, ZoneRole::Custom),
    ]
}

fn target_id(zones: &[Surface], focused: Option<&str>) -> Option<String> {
    target_zone(zones, focused).map(|zone| zone.id.clone())
}

#[test]
fn nothing_focused_targets_the_primary_zone() {
    assert_eq!(target_id(&scene_zones(), None).as_deref(), Some("main"));
}

#[test]
fn a_focused_led_zone_is_the_target() {
    assert_eq!(
        target_id(&scene_zones(), Some("roli")).as_deref(),
        Some("roli")
    );
    assert_eq!(
        target_id(&scene_zones(), Some("main")).as_deref(),
        Some("main")
    );
}

#[test]
fn a_focused_screen_or_a_zone_from_another_scene_falls_back_to_primary() {
    assert_eq!(
        target_id(&scene_zones(), Some("screen")).as_deref(),
        Some("main")
    );
    assert_eq!(
        target_id(&scene_zones(), Some("gone")).as_deref(),
        Some("main")
    );
}

#[test]
fn a_scene_without_a_primary_zone_targets_its_first_led_zone() {
    let zones = vec![
        surface("screen", SurfaceKind::Screen, ZoneRole::Display),
        surface("desk", SurfaceKind::Light, ZoneRole::Custom),
    ];
    assert_eq!(
        primary_led_zone(&zones).map(|zone| zone.id.as_str()),
        Some("desk")
    );
    assert_eq!(target_id(&zones, None).as_deref(), Some("desk"));
    let screens_only = vec![surface("screen", SurfaceKind::Screen, ZoneRole::Display)];
    assert_eq!(target_id(&screens_only, None), None);
}

#[test]
fn the_primary_zone_applies_unnamed_and_any_other_zone_by_id() {
    let zones = scene_zones();
    assert_eq!(apply_zone_id(target_zone(&zones, None)), None);
    assert_eq!(apply_zone_id(target_zone(&zones, Some("main"))), None);
    assert_eq!(
        apply_zone_id(target_zone(&zones, Some("roli"))).as_deref(),
        Some("roli")
    );
    assert_eq!(apply_zone_id(None), None);
}

#[test]
fn horizontal_arrows_move_between_tabs_and_wrap() {
    assert_eq!(next_tab(Some(0), 3, "ArrowRight"), Some(1));
    assert_eq!(next_tab(Some(2), 3, "ArrowRight"), Some(0));
    assert_eq!(next_tab(Some(0), 3, "ArrowLeft"), Some(2));
    assert_eq!(next_tab(Some(1), 3, "Home"), Some(0));
    assert_eq!(next_tab(Some(1), 3, "End"), Some(2));
}

#[test]
fn other_keys_leave_the_tabs_alone() {
    // Vertical arrows belong to whatever scrolls around the strip.
    assert_eq!(next_tab(Some(1), 3, "ArrowDown"), None);
    assert_eq!(next_tab(Some(1), 3, "ArrowUp"), None);
    assert_eq!(next_tab(Some(1), 3, "Enter"), None);
    assert_eq!(next_tab(None, 0, "ArrowRight"), None);
}
