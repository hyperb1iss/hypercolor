//! Contract tests for the Studio §8 surface model.

use std::collections::HashMap;

use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::scene::{DisplayFaceTarget, ZoneId, ZoneRole};
use uuid::Uuid;

use hypercolor_ui::api::ZoneResource;
use hypercolor_ui::pages::studio::surface::{SurfaceKind, led_zone_count, surfaces_from_zones};

fn zone_resource(
    name: &str,
    role: ZoneRole,
    display_target: Option<DisplayFaceTarget>,
) -> ZoneResource {
    ZoneResource {
        id: ZoneId::new(),
        name: name.to_owned(),
        description: None,
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target,
        role,
        members: Vec::new(),
        layout: None,
        layers: Vec::new(),
    }
}

#[test]
fn a_named_primary_zone_shows_its_authored_name() {
    let surfaces = surfaces_from_zones(&[zone_resource("Zone A", ZoneRole::Primary, None)]);

    assert_eq!(surfaces.len(), 1);
    let surface = &surfaces[0];
    assert_eq!(surface.name, "Zone A");
    assert_eq!(surface.kind, SurfaceKind::Light);
    assert_eq!(surface.display_device_id, None);
}

#[test]
fn multiple_led_zones_keep_their_authored_names() {
    let surfaces = surfaces_from_zones(&[
        zone_resource("Desk Zone", ZoneRole::Primary, None),
        zone_resource("Shelf Zone", ZoneRole::Custom, None),
    ]);

    // Every LED zone keeps its authored name, in scene order.
    let names: Vec<&str> = surfaces.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Desk Zone", "Shelf Zone"]);
    assert!(surfaces.iter().all(|s| s.kind == SurfaceKind::Light));
}

#[test]
fn display_zone_becomes_a_screen_carrying_its_device_id() {
    let device_id = DeviceId::new();
    let target = DisplayFaceTarget::new(device_id);
    let surfaces = surfaces_from_zones(&[zone_resource(
        "Corsair LCD",
        ZoneRole::Display,
        Some(target),
    )]);

    assert_eq!(surfaces.len(), 1);
    let surface = &surfaces[0];
    assert_eq!(surface.kind, SurfaceKind::Screen);
    assert_eq!(surface.name, "Corsair LCD");
    assert_eq!(surface.display_device_id, Some(device_id.to_string()));
}

#[test]
fn display_zone_without_a_target_has_no_preview_device() {
    let surfaces = surfaces_from_zones(&[zone_resource("Pending Face", ZoneRole::Display, None)]);

    let surface = &surfaces[0];
    assert_eq!(surface.kind, SurfaceKind::Screen);
    assert_eq!(surface.display_device_id, None);
}

#[test]
fn a_surface_carries_backing_zone_live_layer_ids() {
    let mut zone = zone_resource("Zone A", ZoneRole::Primary, None);
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

    // The surface mirrors the zone's live layer ids in stack order, the
    // set the degraded check filters streamed health against.
    let surfaces = surfaces_from_zones(&[zone]);
    assert_eq!(surfaces[0].layer_ids, expected);
}

#[test]
fn led_and_display_zones_split_into_lights_and_screens() {
    let surfaces = surfaces_from_zones(&[
        zone_resource("Zone A", ZoneRole::Primary, None),
        zone_resource(
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
    let surfaces = surfaces_from_zones(&[
        zone_resource("Desk Strips", ZoneRole::Primary, None),
        zone_resource("Case Fans", ZoneRole::Custom, None),
    ]);
    // A multi-zone Primary zone keeps the user's typed name.
    assert_eq!(surfaces[0].name, "Desk Strips");
}

#[test]
fn an_unnamed_primary_zone_reads_as_default_zone() {
    // The daemon seeds the Default zone as "Primary"; until renamed, the
    // rail shows "Default zone" rather than leaking that internal label.
    let surfaces = surfaces_from_zones(&[
        zone_resource("Primary", ZoneRole::Primary, None),
        zone_resource("Case Fans", ZoneRole::Custom, None),
    ]);
    assert_eq!(surfaces[0].name, "Default zone");
    // The relabel holds at every scale — a solo unnamed zone reads the same.
    let solo = surfaces_from_zones(&[zone_resource("Primary", ZoneRole::Primary, None)]);
    assert_eq!(solo[0].name, "Default zone");
}

#[test]
fn a_surface_carries_backing_zone_role_and_accent_color() {
    let mut zone = zone_resource("Case Fans", ZoneRole::Custom, None);
    zone.color = Some("#e135ff".to_owned());
    let surfaces = surfaces_from_zones(&[zone]);
    assert_eq!(surfaces[0].role, ZoneRole::Custom);
    assert_eq!(surfaces[0].color.as_deref(), Some("#e135ff"));
}

#[test]
fn only_custom_led_zones_are_deletable() {
    let surfaces = surfaces_from_zones(&[
        zone_resource("Default", ZoneRole::Primary, None),
        zone_resource("Case Fans", ZoneRole::Custom, None),
        zone_resource(
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
fn led_zone_count_excludes_display_zones() {
    let zones = [
        zone_resource("Default", ZoneRole::Primary, None),
        zone_resource("Case Fans", ZoneRole::Custom, None),
        zone_resource(
            "AIO Screen",
            ZoneRole::Display,
            Some(DisplayFaceTarget::new(DeviceId::new())),
        ),
    ];
    assert_eq!(led_zone_count(&zones), 2);
}
