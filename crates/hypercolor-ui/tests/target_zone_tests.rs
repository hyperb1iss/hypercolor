//! The one target zone: which LED zone applies, presets, controls, and
//! the preview act on, and how the zone tabs move between zones.

use std::collections::HashMap;

use hypercolor_types::api::scene::{SceneDocument, ZoneMember, ZoneMemberId};
use hypercolor_types::event::LayerHealth;
use hypercolor_types::scene::{ZoneId, ZoneRole};
use hypercolor_ui::api::ZoneResource;
use hypercolor_ui::components::zone_tabs::{
    TabStatus, next_tab, status_description, surface_status, tab_status, tab_title,
};
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

#[test]
fn brackets_step_like_the_arrows() {
    assert_eq!(next_tab(Some(1), 3, "]"), Some(2));
    assert_eq!(next_tab(Some(0), 3, "["), Some(2));
    assert_eq!(next_tab(None, 3, "ArrowRight"), Some(0));
    assert_eq!(next_tab(None, 3, "ArrowLeft"), Some(2));
}

#[test]
fn a_paused_zone_shows_paused_before_any_warning() {
    assert_eq!(tab_status(false, true, 0), TabStatus::Paused);
    assert_eq!(tab_status(true, true, 4), TabStatus::LayerFailed);
    assert_eq!(tab_status(true, true, 0), TabStatus::LayerFailed);
    assert_eq!(tab_status(true, false, 0), TabStatus::NoDevices);
    assert_eq!(tab_status(true, false, 3), TabStatus::Clear);
}

#[test]
fn each_status_names_itself_for_titles_and_screen_readers() {
    assert_eq!(TabStatus::Clear.note(), None);
    assert_eq!(TabStatus::Paused.note(), Some("Paused"));
    assert_eq!(
        TabStatus::LayerFailed.note(),
        Some("A layer on this zone failed to render")
    );
    assert_eq!(
        TabStatus::NoDevices.note(),
        Some("No devices in this zone yet")
    );
    assert!(TabStatus::LayerFailed.is_warning() && TabStatus::NoDevices.is_warning());
    assert!(!TabStatus::Paused.is_warning() && !TabStatus::Clear.is_warning());
}

fn zone_resource(id: ZoneId, devices: &[&str]) -> ZoneResource {
    ZoneResource {
        id,
        name: "Keyboard".to_owned(),
        description: None,
        role: ZoneRole::Custom,
        enabled: true,
        brightness: 1.0,
        color: None,
        display_target: None,
        members: devices
            .iter()
            .enumerate()
            .map(|(index, device)| ZoneMember {
                id: ZoneMemberId(format!("member-{index}")),
                device_id: (*device).to_owned(),
                segment: None,
                name: format!("{device} output"),
            })
            .collect(),
        layout: None,
        layers: Vec::new(),
    }
}

fn scene_with(zones: Vec<ZoneResource>) -> SceneDocument {
    let mut scene: SceneDocument = serde_json::from_value(serde_json::json!({
        "id": "55555555-5555-4555-8555-555555555555", "name": "Rig", "kind": "named",
        "is_default": false, "mutation_mode": "live", "revision": 1, "zones": [],
    }))
    .expect("valid scene document");
    scene.zones = zones;
    scene
}

#[test]
fn the_live_status_reads_members_and_layer_health() {
    let zone_id = ZoneId::new();
    let mut keyboard = surface(&zone_id.to_string(), SurfaceKind::Light, ZoneRole::Custom);
    keyboard.layer_ids = vec!["layer-1".to_owned()];
    let empty = scene_with(vec![zone_resource(zone_id, &[])]);
    let filled = scene_with(vec![zone_resource(zone_id, &["kb"])]);
    let healthy = HashMap::new();
    assert_eq!(
        surface_status(&keyboard, Some(&empty), &healthy),
        TabStatus::NoDevices
    );
    assert_eq!(
        surface_status(&keyboard, Some(&filled), &healthy),
        TabStatus::Clear
    );

    let failing = HashMap::from([(
        format!("{}/{zone_id}/layer-1", filled.id),
        LayerHealth::Failed {
            reason: "shader".to_owned(),
        },
    )]);
    assert_eq!(
        surface_status(&keyboard, Some(&filled), &failing),
        TabStatus::LayerFailed
    );

    keyboard.enabled = false;
    assert_eq!(
        surface_status(&keyboard, Some(&filled), &failing),
        TabStatus::Paused
    );
}

#[test]
fn a_tab_is_described_by_its_status_only_while_it_has_one() {
    // The status describes the tab rather than joining its name, so a tab
    // keeps the zone's name as devices come and go.
    assert_eq!(status_description(TabStatus::Clear, "tab-status"), None);
    for status in [
        TabStatus::Paused,
        TabStatus::LayerFailed,
        TabStatus::NoDevices,
    ] {
        assert_eq!(
            status_description(status, "tab-status").as_deref(),
            Some("tab-status")
        );
    }
}

#[test]
fn every_strip_titles_a_tab_the_same_way() {
    assert_eq!(tab_title("Main", TabStatus::Clear), "Show and edit Main");
    assert_eq!(
        tab_title("Desk", TabStatus::Paused),
        "Show and edit Desk (paused)"
    );
    assert_eq!(
        tab_title("Fans", TabStatus::NoDevices),
        "Show and edit Fans (no devices in this zone yet)"
    );
}
