#![allow(dead_code)]

//! Contract tests for the Studio §8 surface model.
//!
//! `surface.rs` is leptos-free and `crate::`-free so it can be pulled in
//! directly; the test crate supplies `hypercolor_types`.

#[path = "../src/pages/studio/surface.rs"]
mod surface;

use std::collections::HashMap;

use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::scene::{DisplayFaceTarget, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use uuid::Uuid;

use surface::{SurfaceKind, led_zone_count, surfaces_from_groups};

fn sample_layout() -> SpatialLayout {
    SpatialLayout {
        id: "layout".to_owned(),
        name: "Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn group(name: &str, role: ZoneRole, display_target: Option<DisplayFaceTarget>) -> Zone {
    Zone {
        id: ZoneId::new(),
        name: name.to_owned(),
        description: None,
        effect_id: None,
        controls: HashMap::new(),
        control_bindings: HashMap::new(),
        preset_id: None,
        layers: Vec::new(),
        layout: sample_layout(),
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target,
        role,
        controls_version: 0,
        layers_version: 0,
    }
}

#[test]
fn a_named_primary_group_shows_its_authored_name() {
    let surfaces = surfaces_from_groups(&[group("Zone A", ZoneRole::Primary, None)]);

    assert_eq!(surfaces.len(), 1);
    let surface = &surfaces[0];
    assert_eq!(surface.name, "Zone A");
    assert_eq!(surface.kind, SurfaceKind::Light);
    assert_eq!(surface.display_device_id, None);
}

#[test]
fn multiple_led_groups_keep_their_authored_names() {
    let surfaces = surfaces_from_groups(&[
        group("Desk Zone", ZoneRole::Primary, None),
        group("Shelf Zone", ZoneRole::Custom, None),
    ]);

    // Every LED zone keeps its authored name, in scene order.
    let names: Vec<&str> = surfaces.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Desk Zone", "Shelf Zone"]);
    assert!(surfaces.iter().all(|s| s.kind == SurfaceKind::Light));
}

#[test]
fn display_group_becomes_a_screen_carrying_its_device_id() {
    let device_id = DeviceId::new();
    let target = DisplayFaceTarget::new(device_id);
    let surfaces = surfaces_from_groups(&[group("Corsair LCD", ZoneRole::Display, Some(target))]);

    assert_eq!(surfaces.len(), 1);
    let surface = &surfaces[0];
    assert_eq!(surface.kind, SurfaceKind::Screen);
    assert_eq!(surface.name, "Corsair LCD");
    assert_eq!(surface.display_device_id, Some(device_id.to_string()));
}

#[test]
fn display_group_without_a_target_has_no_preview_device() {
    let surfaces = surfaces_from_groups(&[group("Pending Face", ZoneRole::Display, None)]);

    let surface = &surfaces[0];
    assert_eq!(surface.kind, SurfaceKind::Screen);
    assert_eq!(surface.display_device_id, None);
}

#[test]
fn a_surface_carries_its_groups_live_layer_ids() {
    let mut zone = group("Zone A", ZoneRole::Primary, None);
    let first = SceneLayer::from_effect(
        SceneLayerId::new(),
        EffectId::new(Uuid::nil()),
        HashMap::new(),
        HashMap::new(),
        None,
    );
    let second = SceneLayer::from_effect(
        SceneLayerId::new(),
        EffectId::new(Uuid::nil()),
        HashMap::new(),
        HashMap::new(),
        None,
    );
    let expected = vec![first.id.to_string(), second.id.to_string()];
    zone.layers = vec![first, second];

    // The surface mirrors the group's live layer ids, in stack order — the
    // set the degraded check filters streamed health against.
    let surfaces = surfaces_from_groups(&[zone]);
    assert_eq!(surfaces[0].layer_ids, expected);
}

#[test]
fn led_and_display_groups_split_into_zones_and_screens() {
    let surfaces = surfaces_from_groups(&[
        group("Zone A", ZoneRole::Primary, None),
        group(
            "AIO Screen",
            ZoneRole::Display,
            Some(DisplayFaceTarget::new(DeviceId::new())),
        ),
    ]);

    let lights = surfaces
        .iter()
        .filter(|s| s.kind == SurfaceKind::Light)
        .count();
    let screens = surfaces
        .iter()
        .filter(|s| s.kind == SurfaceKind::Screen)
        .count();
    assert_eq!((lights, screens), (1, 1));
    // The lone LED zone keeps its authored name; the screen is separate.
    assert_eq!(surfaces[0].name, "Zone A");
}

#[test]
fn a_renamed_primary_zone_shows_its_typed_name_when_multi_zone() {
    let surfaces = surfaces_from_groups(&[
        group("Living Room", ZoneRole::Primary, None),
        group("Case Fans", ZoneRole::Custom, None),
    ]);
    // A multi-zone Primary group keeps the user's typed name.
    assert_eq!(surfaces[0].name, "Living Room");
}

#[test]
fn an_unnamed_primary_zone_reads_as_default_zone() {
    // The daemon seeds the Default zone as "Primary"; until renamed, the
    // rail shows "Default zone" rather than leaking that internal label.
    let surfaces = surfaces_from_groups(&[
        group("Primary", ZoneRole::Primary, None),
        group("Case Fans", ZoneRole::Custom, None),
    ]);
    assert_eq!(surfaces[0].name, "Default zone");
    // The relabel holds at every scale — a solo unnamed zone reads the same.
    let solo = surfaces_from_groups(&[group("Primary", ZoneRole::Primary, None)]);
    assert_eq!(solo[0].name, "Default zone");
}

#[test]
fn a_surface_carries_its_groups_role_and_accent_color() {
    let mut zone = group("Case Fans", ZoneRole::Custom, None);
    zone.color = Some("#e135ff".to_owned());
    let surfaces = surfaces_from_groups(&[zone]);
    assert_eq!(surfaces[0].role, ZoneRole::Custom);
    assert_eq!(surfaces[0].color.as_deref(), Some("#e135ff"));
}

#[test]
fn only_custom_led_zones_are_deletable() {
    let surfaces = surfaces_from_groups(&[
        group("Default", ZoneRole::Primary, None),
        group("Case Fans", ZoneRole::Custom, None),
        group(
            "AIO Screen",
            ZoneRole::Display,
            Some(DisplayFaceTarget::new(DeviceId::new())),
        ),
    ]);
    // Primary is the permanent Default zone; a Screen is not a zone.
    assert!(!surfaces[0].is_deletable_zone());
    assert!(surfaces[1].is_deletable_zone());
    assert!(!surfaces[2].is_deletable_zone());
}

#[test]
fn led_zone_count_excludes_display_groups() {
    let groups = [
        group("Default", ZoneRole::Primary, None),
        group("Case Fans", ZoneRole::Custom, None),
        group(
            "AIO Screen",
            ZoneRole::Display,
            Some(DisplayFaceTarget::new(DeviceId::new())),
        ),
    ];
    assert_eq!(led_zone_count(&groups), 2);
}
