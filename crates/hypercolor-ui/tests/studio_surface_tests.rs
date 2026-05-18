//! Contract tests for the Studio §8 surface model.
//!
//! `surface.rs` is leptos-free and `crate::`-free so it can be pulled in
//! directly; the test crate supplies `hypercolor_types`.

#[path = "../src/pages/studio/surface.rs"]
mod surface;

use std::collections::HashMap;

use hypercolor_types::device::DeviceId;
use hypercolor_types::scene::{DisplayFaceTarget, RenderGroup, RenderGroupId, RenderGroupRole};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

use surface::{SurfaceKind, surfaces_from_groups};

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

fn group(name: &str, role: RenderGroupRole, display_target: Option<DisplayFaceTarget>) -> RenderGroup {
    RenderGroup {
        id: RenderGroupId::new(),
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
fn single_primary_led_group_reads_as_all_lights() {
    let surfaces = surfaces_from_groups(&[group("Zone A", RenderGroupRole::Primary, None)]);

    assert_eq!(surfaces.len(), 1);
    let surface = &surfaces[0];
    assert_eq!(surface.name, "All Lights");
    assert_eq!(surface.kind, SurfaceKind::Light);
    assert_eq!(surface.display_device_id, None);
}

#[test]
fn multiple_led_groups_keep_their_authored_names() {
    let surfaces = surfaces_from_groups(&[
        group("Desk Zone", RenderGroupRole::Primary, None),
        group("Shelf Zone", RenderGroupRole::Custom, None),
    ]);

    // A second LED zone retires the "All Lights" relabel; both keep their
    // authored names, in scene order.
    let names: Vec<&str> = surfaces.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Desk Zone", "Shelf Zone"]);
    assert!(surfaces.iter().all(|s| s.kind == SurfaceKind::Light));
}

#[test]
fn display_group_becomes_a_screen_carrying_its_device_id() {
    let device_id = DeviceId::new();
    let target = DisplayFaceTarget::new(device_id);
    let surfaces = surfaces_from_groups(&[group(
        "Corsair LCD",
        RenderGroupRole::Display,
        Some(target),
    )]);

    assert_eq!(surfaces.len(), 1);
    let surface = &surfaces[0];
    assert_eq!(surface.kind, SurfaceKind::Screen);
    assert_eq!(surface.name, "Corsair LCD");
    assert_eq!(surface.display_device_id, Some(device_id.to_string()));
}

#[test]
fn display_group_without_a_target_has_no_preview_device() {
    let surfaces = surfaces_from_groups(&[group("Pending Face", RenderGroupRole::Display, None)]);

    let surface = &surfaces[0];
    assert_eq!(surface.kind, SurfaceKind::Screen);
    assert_eq!(surface.display_device_id, None);
}

#[test]
fn led_and_display_groups_split_into_lights_and_screens() {
    let surfaces = surfaces_from_groups(&[
        group("Zone A", RenderGroupRole::Primary, None),
        group(
            "AIO Screen",
            RenderGroupRole::Display,
            Some(DisplayFaceTarget::new(DeviceId::new())),
        ),
    ]);

    let lights = surfaces.iter().filter(|s| s.kind == SurfaceKind::Light).count();
    let screens = surfaces.iter().filter(|s| s.kind == SurfaceKind::Screen).count();
    assert_eq!((lights, screens), (1, 1));
    // A screen alongside an LED zone still leaves a lone LED group, so the
    // §9.2 "All Lights" relabel holds.
    assert_eq!(surfaces[0].name, "All Lights");
}
