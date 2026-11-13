//! Screens as outputs: lowering a screen's layers into the display zone the
//! engine renders, the kept face's precedence, the conversion of stored
//! display zones, and the views that stand for screens on the wire.

use std::collections::HashMap;

use hypercolor_core::scene::screens::{
    LegacyReason, ScreenStack, ScreenSurface, ScreenSurfaces, clear_screen_layers,
    convert_legacy_display_zones, is_screen_layer, remove_screen_from_layer, screen_stack,
    screen_view, screen_views, screen_zone_id, split_for_screen,
};
use hypercolor_core::scene::{SceneManager, ZoneMutationError, default_primary_zone, make_scene};
use hypercolor_core::spatial::DISPLAY_ZONE_NAME;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use hypercolor_types::layer::{BlendMode, LayerScope, LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{
    DisplayFaceTarget, Scene, SceneMutationMode, Zone, ZoneId, ZoneRole,
};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use uuid::Uuid;

const CANVAS: (u32, u32) = (320, 200);
const PUMP: &str = "usb:pump";
const PUMP_SCREEN: &str = "usb:pump-display";
const FAN: &str = "usb:fan";
const FAN_SCREEN: &str = "usb:fan-display";

fn output(id: &str, device: &str, segment: &str) -> Output {
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: device.to_owned(),
        zone_name: Some(segment.to_owned()),
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(0.2, 0.2),
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: if segment == DISPLAY_ZONE_NAME {
            LedTopology::Point
        } else {
            LedTopology::Strip {
                count: 4,
                direction: StripDirection::LeftToRight,
            }
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

fn screen(id: &str, device: &str) -> Output {
    output(id, device, DISPLAY_ZONE_NAME)
}

fn strip(id: &str) -> Output {
    output(id, "usb:keyboard", "main")
}

fn layout(outputs: Vec<Output>) -> SpatialLayout {
    SpatialLayout {
        id: "rig".to_owned(),
        name: "Rig".to_owned(),
        description: None,
        canvas_width: CANVAS.0,
        canvas_height: CANVAS.1,
        zones: outputs,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn sample_effect(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} description"),
        category: EffectCategory::Display,
        tags: Vec::new(),
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Native {
            path: format!("builtin/{name}").into(),
        },
        license: None,
    }
}

fn effect_layer() -> SceneLayer {
    SceneLayer::from_effect(
        SceneLayerId::new(),
        EffectId::new(Uuid::now_v7()),
        HashMap::new(),
        HashMap::new(),
        None,
    )
}

fn color_layer() -> SceneLayer {
    SceneLayer {
        source: LayerSource::ColorFill {
            rgba: [1.0, 0.0, 0.0, 1.0],
        },
        ..effect_layer()
    }
}

fn scoped(mut layer: SceneLayer, outputs: &[&str]) -> SceneLayer {
    layer.scope = Some(LayerScope {
        outputs: outputs.iter().map(|id| (*id).to_owned()).collect(),
    });
    layer
}

fn primary(outputs: Vec<Output>, layers: Vec<SceneLayer>) -> Zone {
    let mut zone = default_primary_zone(layout(outputs));
    zone.layers = layers;
    zone
}

fn custom(name: &str, outputs: Vec<Output>, layers: Vec<SceneLayer>) -> Zone {
    Zone {
        id: ZoneId::new(),
        name: name.to_owned(),
        role: ZoneRole::Custom,
        ..primary(outputs, layers)
    }
}

/// A display zone in the spelling builds before screen layers stored.
fn legacy_zone(layers: Vec<SceneLayer>, target: DisplayFaceTarget) -> Zone {
    Zone {
        id: ZoneId::new(),
        name: "Face".to_owned(),
        description: None,
        layers,
        layout: SpatialLayout {
            zones: Vec::new(),
            ..layout(Vec::new())
        },
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: Some(target),
        role: ZoneRole::Display,
        controls_version: 0,
        layers_version: 0,
    }
}

fn target(device_id: DeviceId, blend_mode: BlendMode, opacity: f32) -> DisplayFaceTarget {
    DisplayFaceTarget {
        device_id,
        blend_mode,
        opacity,
    }
}

fn surface(device_id: DeviceId, name: &str, size: (u32, u32)) -> ScreenSurface {
    ScreenSurface {
        device_id,
        connected: true,
        name: name.to_owned(),
        width: size.0,
        height: size.1,
    }
}

struct Rig {
    pump: DeviceId,
    fan: DeviceId,
    surfaces: ScreenSurfaces,
}

/// Two connected screens: a 480 square pump panel and a 400 square fan panel.
fn rig() -> Rig {
    let pump = DeviceId::new();
    let fan = DeviceId::new();
    let surfaces = ScreenSurfaces::from([
        (PUMP.to_owned(), surface(pump, "Pump", (480, 480))),
        (FAN.to_owned(), surface(fan, "Fan", (400, 400))),
    ]);
    Rig {
        pump,
        fan,
        surfaces,
    }
}

fn scene(zones: Vec<Zone>) -> Scene {
    let mut scene = make_scene("Studio Session");
    scene.zones = zones;
    scene
}

/// A manager with the scene active and the rig's screens connected.
fn active(rig: &Rig, scene: Scene) -> SceneManager {
    let mut manager = SceneManager::with_default();
    let id = scene.id;
    manager.create(scene).expect("scene should be created");
    manager.activate(&id, None).expect("scene should activate");
    manager.set_screen_surfaces(rig.surfaces.clone());
    manager
}

fn display_zones(manager: &SceneManager) -> Vec<Zone> {
    manager
        .resolved_zones()
        .iter()
        .filter(|zone| zone.display_target.is_some())
        .cloned()
        .collect()
}

fn lowered_for(manager: &SceneManager, device_id: DeviceId) -> Option<Zone> {
    display_zones(manager).into_iter().find(|zone| {
        zone.display_target
            .as_ref()
            .is_some_and(|target| target.device_id == device_id)
    })
}

/// A kept face for a device, as the preference store installs one.
fn kept_face(device_id: DeviceId) -> Zone {
    legacy_zone(vec![effect_layer()], DisplayFaceTarget::new(device_id))
}

// ── Screen layers ────────────────────────────────────────────────────────

#[test]
fn a_layer_is_a_screen_layer_only_when_every_live_output_in_its_scope_is_a_screen() {
    let zone = primary(
        vec![
            strip("keys"),
            screen(PUMP_SCREEN, PUMP),
            screen(FAN_SCREEN, FAN),
        ],
        Vec::new(),
    );

    assert!(is_screen_layer(
        &zone,
        &scoped(effect_layer(), &[PUMP_SCREEN])
    ));
    assert!(is_screen_layer(
        &zone,
        &scoped(effect_layer(), &[PUMP_SCREEN, FAN_SCREEN])
    ));
    assert!(
        is_screen_layer(&zone, &scoped(effect_layer(), &[PUMP_SCREEN, "gone"])),
        "an id that is not live does not decide the class"
    );
    assert!(
        !is_screen_layer(&zone, &scoped(effect_layer(), &["keys", PUMP_SCREEN])),
        "a layer on a keyboard and a screen is a canvas layer"
    );
    assert!(!is_screen_layer(&zone, &scoped(effect_layer(), &["gone"])));
    assert!(!is_screen_layer(&zone, &effect_layer()));
}

// ── Lowering ─────────────────────────────────────────────────────────────

#[test]
fn a_scene_with_no_scoped_layer_is_published_as_authored() {
    let rig = rig();
    let zone = primary(
        vec![strip("keys"), screen(PUMP_SCREEN, PUMP)],
        vec![effect_layer(), color_layer()],
    );
    let manager = active(&rig, scene(vec![zone.clone()]));

    assert_eq!(manager.resolved_zones().as_ref(), &[zone]);
}

#[test]
fn a_screen_layer_leaves_its_zone_and_renders_in_a_derived_display_zone() {
    let rig = rig();
    let base = effect_layer();
    let keys_only = scoped(color_layer(), &["keys"]);
    let mut face = scoped(effect_layer(), &[PUMP_SCREEN]);
    face.blend = BlendMode::Screen;
    face.opacity = 0.4;
    let mut overlay = scoped(color_layer(), &[PUMP_SCREEN]);
    overlay.blend = BlendMode::Add;
    overlay.opacity = 0.7;
    let zone = primary(
        vec![strip("keys"), screen(PUMP_SCREEN, PUMP)],
        vec![
            base.clone(),
            face.clone(),
            keys_only.clone(),
            overlay.clone(),
        ],
    );
    let authored = scene(vec![zone.clone()]);
    let scene_id = authored.id;
    let manager = active(&rig, authored);

    let resolved = manager.resolved_zones();
    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved[0].id, zone.id);
    assert_eq!(
        resolved[0].layers,
        vec![base, keys_only],
        "canvas layers stay, scopes intact"
    );

    let lowered = &resolved[1];
    assert_eq!(lowered.id, screen_zone_id(scene_id, rig.pump));
    assert_eq!(lowered.role, ZoneRole::Display);
    assert_eq!(lowered.name, "Pump");
    assert!(lowered.enabled);
    assert!((lowered.brightness - 1.0).abs() < f32::EPSILON);
    assert_eq!(
        (lowered.layout.canvas_width, lowered.layout.canvas_height),
        (480, 480)
    );
    assert!(lowered.layout.zones.is_empty());
    assert_eq!(
        lowered.display_target,
        Some(target(rig.pump, BlendMode::Screen, 0.4)),
        "the bottom layer's blend and opacity are how the stack meets its backdrop"
    );
    let mut opaque_base = face;
    opaque_base.scope = None;
    opaque_base.blend = BlendMode::Replace;
    opaque_base.opacity = 1.0;
    let mut upper = overlay;
    upper.scope = None;
    assert_eq!(
        lowered.layers,
        vec![opaque_base, upper],
        "ids are kept, scope is cleared, and the composition is consumed once"
    );
}

#[test]
fn the_scene_itself_keeps_its_screen_layers_when_they_are_lowered() {
    let rig = rig();
    let face = scoped(effect_layer(), &[PUMP_SCREEN]);
    let zone = primary(vec![screen(PUMP_SCREEN, PUMP)], vec![face.clone()]);
    let authored = scene(vec![zone]);
    let scene_id = authored.id;
    let manager = active(&rig, authored);

    let stored = manager.get(&scene_id).expect("scene");
    assert_eq!(stored.zones.len(), 1);
    assert_eq!(stored.zones[0].layers, vec![face]);
}

#[test]
fn the_bottom_enabled_screen_layer_is_the_base() {
    let rig = rig();
    let mut off = scoped(effect_layer(), &[PUMP_SCREEN]);
    off.enabled = false;
    off.blend = BlendMode::Add;
    let mut on = scoped(color_layer(), &[PUMP_SCREEN]);
    on.blend = BlendMode::Multiply;
    on.opacity = 0.5;
    let zone = primary(
        vec![screen(PUMP_SCREEN, PUMP)],
        vec![off.clone(), on.clone()],
    );
    let manager = active(&rig, scene(vec![zone]));

    let lowered = lowered_for(&manager, rig.pump).expect("derived zone");
    assert_eq!(
        lowered.display_target,
        Some(target(rig.pump, BlendMode::Multiply, 0.5))
    );
    assert_eq!(lowered.layers[0].id, off.id);
    assert_eq!(
        lowered.layers[0].blend,
        BlendMode::Add,
        "a disabled layer is left alone"
    );
    assert_eq!(lowered.layers[1].id, on.id);
    assert_eq!(lowered.layers[1].blend, BlendMode::Replace);
    assert!((lowered.layers[1].opacity - 1.0).abs() < f32::EPSILON);
}

#[test]
fn a_replacing_base_layer_lowers_to_a_target_at_full_opacity() {
    let rig = rig();
    let mut face = scoped(effect_layer(), &[PUMP_SCREEN]);
    face.blend = BlendMode::Replace;
    face.opacity = 0.3;
    let zone = primary(vec![screen(PUMP_SCREEN, PUMP)], vec![face]);
    let manager = active(&rig, scene(vec![zone]));

    assert_eq!(
        lowered_for(&manager, rig.pump).and_then(|zone| zone.display_target),
        Some(target(rig.pump, BlendMode::Replace, 1.0)),
        "a face that does not blend covers its backdrop outright"
    );
}

#[test]
fn a_screen_with_no_surface_lowers_to_nothing_and_returns_with_its_device() {
    let rig = rig();
    let face = scoped(effect_layer(), &[PUMP_SCREEN]);
    let zone = primary(vec![screen(PUMP_SCREEN, PUMP)], vec![face.clone()]);
    let authored = scene(vec![zone]);
    let scene_id = authored.id;
    let mut manager = active(&rig, authored);

    manager.set_screen_surfaces(ScreenSurfaces::new());
    assert!(display_zones(&manager).is_empty());
    assert_eq!(
        manager.get(&scene_id).expect("scene").zones[0].layers,
        vec![face],
        "the layers stay in the scene"
    );

    manager.set_screen_surfaces(rig.surfaces.clone());
    assert_eq!(
        lowered_for(&manager, rig.pump).map(|zone| zone.id),
        Some(screen_zone_id(scene_id, rig.pump))
    );
}

#[test]
fn pausing_a_zone_pauses_the_faces_of_the_screens_it_places() {
    let rig = rig();
    let base = effect_layer();
    let mut zone = primary(
        vec![screen(PUMP_SCREEN, PUMP)],
        vec![base.clone(), scoped(effect_layer(), &[PUMP_SCREEN])],
    );
    zone.enabled = false;
    let manager = active(&rig, scene(vec![zone]));

    assert!(display_zones(&manager).is_empty());
    assert_eq!(
        manager.resolved_zones()[0].layers,
        vec![base],
        "the paused zone still goes out without its screen layers"
    );
}

#[test]
fn one_layer_on_two_screens_lowers_once_per_panel_at_that_panels_size() {
    let rig = rig();
    let shared = scoped(effect_layer(), &[PUMP_SCREEN, FAN_SCREEN]);
    let zone = primary(
        vec![screen(PUMP_SCREEN, PUMP), screen(FAN_SCREEN, FAN)],
        vec![shared.clone()],
    );
    let manager = active(&rig, scene(vec![zone]));

    let pump = lowered_for(&manager, rig.pump).expect("pump zone");
    let fan = lowered_for(&manager, rig.fan).expect("fan zone");
    assert_ne!(pump.id, fan.id);
    assert_eq!(
        (pump.layout.canvas_width, pump.layout.canvas_height),
        (480, 480)
    );
    assert_eq!(
        (fan.layout.canvas_width, fan.layout.canvas_height),
        (400, 400)
    );
    assert_eq!(pump.layers[0].id, shared.id);
    assert_eq!(
        fan.layers[0].id, shared.id,
        "one set of controls, by one id"
    );
    assert_eq!(pump.layers[0].source, fan.layers[0].source);
}

#[test]
fn a_layer_on_a_keyboard_and_a_screen_stays_a_canvas_layer() {
    let rig = rig();
    let mixed = scoped(effect_layer(), &["keys", PUMP_SCREEN]);
    let zone = primary(
        vec![strip("keys"), screen(PUMP_SCREEN, PUMP)],
        vec![mixed.clone()],
    );
    let manager = active(&rig, scene(vec![zone]));

    assert!(display_zones(&manager).is_empty());
    assert_eq!(manager.resolved_zones()[0].layers, vec![mixed]);
}

#[test]
fn a_screens_zone_id_survives_a_move_between_zones() {
    let rig = rig();
    let face = scoped(effect_layer(), &[PUMP_SCREEN]);
    let first = primary(vec![screen(PUMP_SCREEN, PUMP)], vec![face.clone()]);
    let second = custom("Desk", vec![strip("keys")], Vec::new());
    let authored = scene(vec![first, second]);
    let scene_id = authored.id;
    let mut manager = active(&rig, authored);
    let before = lowered_for(&manager, rig.pump).expect("derived zone");

    let mut moved = manager.get(&scene_id).expect("scene").clone();
    let output = moved.zones[0].layout.zones.remove(0);
    let layer = moved.zones[0].layers.remove(0);
    moved.zones[1].layout.zones.push(output);
    moved.zones[1].layers.push(layer);
    manager.update(moved).expect("scene should update");

    let after = lowered_for(&manager, rig.pump).expect("derived zone");
    assert_eq!(after.id, before.id);
    assert_eq!(
        after.layers[0].id, face.id,
        "the effect slot key is unchanged"
    );
}

#[test]
fn a_stored_display_zone_passes_through_and_is_the_screens_whole_stack() {
    let rig = rig();
    let stored = legacy_zone(
        vec![effect_layer()],
        target(rig.pump, BlendMode::Screen, 0.8),
    );
    let zone = primary(
        vec![screen(PUMP_SCREEN, PUMP)],
        vec![scoped(color_layer(), &[PUMP_SCREEN])],
    );
    let mut locked = scene(vec![zone, stored.clone()]);
    locked.mutation_mode = SceneMutationMode::Snapshot;
    let manager = active(&rig, locked);

    let zones = display_zones(&manager);
    assert_eq!(zones.len(), 1, "the screen layer is not lowered beside it");
    assert_eq!(zones[0].id, stored.id);
    assert_eq!(zones[0].layers, stored.layers);
    assert_eq!(zones[0].display_target, stored.display_target);
    assert_eq!(
        (zones[0].layout.canvas_width, zones[0].layout.canvas_height),
        (480, 480),
        "its canvas is the panel the surface table knows"
    );
}

// ── The kept face ────────────────────────────────────────────────────────

/// Resolve what reaches the render list for the pump screen when the zone
/// that places it holds `layers` and a kept face is stored.
fn with_kept_face(zone_enabled: bool, layers: Vec<SceneLayer>) -> (SceneManager, Rig, Zone) {
    let rig = rig();
    let mut zone = primary(vec![strip("keys"), screen(PUMP_SCREEN, PUMP)], layers);
    zone.enabled = zone_enabled;
    let mut manager = active(&rig, scene(vec![zone]));
    let kept = kept_face(rig.pump);
    manager.set_default_display_zone(kept.clone());
    (manager, rig, kept)
}

#[test]
fn an_enabled_face_layer_renders_and_suppresses_the_kept_face() {
    let face = scoped(effect_layer(), &[PUMP_SCREEN]);
    let (manager, rig, kept) = with_kept_face(true, vec![face.clone()]);

    let zones = display_zones(&manager);
    assert_eq!(zones.len(), 1);
    assert_ne!(zones[0].id, kept.id);
    assert_eq!(zones[0].layers[0].id, face.id);
    let stack = manager.active_screen_stack(rig.pump).expect("stack");
    assert!(stack.assigned());
    assert!(stack.rendering());
}

#[test]
fn a_disabled_face_layer_shows_nothing_and_still_suppresses_the_kept_face() {
    let mut face = scoped(effect_layer(), &[PUMP_SCREEN]);
    face.enabled = false;
    let (manager, rig, _) = with_kept_face(true, vec![face]);

    assert!(
        display_zones(&manager).is_empty(),
        "disabling a face does not reveal the kept one"
    );
    let stack = manager.active_screen_stack(rig.pump).expect("stack");
    assert!(stack.assigned());
    assert!(!stack.rendering());
}

#[test]
fn a_face_layer_in_a_paused_zone_shows_nothing_and_suppresses_the_kept_face() {
    let face = scoped(effect_layer(), &[PUMP_SCREEN]);
    let (manager, rig, _) = with_kept_face(false, vec![face]);

    assert!(display_zones(&manager).is_empty());
    let stack = manager.active_screen_stack(rig.pump).expect("stack");
    assert!(stack.assigned());
    assert!(!stack.rendering());
}

#[test]
fn enabled_layers_of_other_sources_cover_the_kept_face() {
    let fill = scoped(color_layer(), &[PUMP_SCREEN]);
    let (manager, rig, kept) = with_kept_face(true, vec![fill.clone()]);

    let zones = display_zones(&manager);
    assert_eq!(zones.len(), 1, "exactly one binding reaches the screen");
    assert_ne!(zones[0].id, kept.id);
    assert_eq!(zones[0].layers[0].id, fill.id);
    let stack = manager.active_screen_stack(rig.pump).expect("stack");
    assert!(!stack.assigned(), "a color fill is not a face");
    assert!(stack.rendering());
}

#[test]
fn disabled_layers_of_other_sources_leave_the_kept_face_showing() {
    let mut fill = scoped(color_layer(), &[PUMP_SCREEN]);
    fill.enabled = false;
    let (manager, rig, kept) = with_kept_face(true, vec![fill]);

    let zones = display_zones(&manager);
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].id, kept.id);
    let stack = manager.active_screen_stack(rig.pump).expect("stack");
    assert!(!stack.assigned());
    assert!(!stack.rendering());
}

#[test]
fn a_screen_with_no_layers_shows_the_kept_face() {
    let (manager, _, kept) = with_kept_face(true, vec![effect_layer()]);

    let zones = display_zones(&manager);
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].id, kept.id);
}

#[test]
fn a_screen_with_no_layers_and_no_kept_face_shows_its_backdrop() {
    let rig = rig();
    let zone = primary(vec![screen(PUMP_SCREEN, PUMP)], vec![effect_layer()]);
    let manager = active(&rig, scene(vec![zone]));

    assert!(display_zones(&manager).is_empty());
    assert!(matches!(
        manager.active_screen_stack(rig.pump),
        Some(ScreenStack::Placed { .. })
    ));
}

#[test]
fn a_stored_display_zone_decides_the_kept_face_by_the_same_two_predicates() {
    let rig = rig();
    let mut face = effect_layer();
    face.enabled = false;
    let stored = legacy_zone(vec![face], DisplayFaceTarget::new(rig.pump));
    let mut manager = active(&rig, scene(vec![stored.clone()]));
    manager.set_default_display_zone(kept_face(rig.pump));

    let zones = display_zones(&manager);
    assert_eq!(zones.len(), 1, "assigned, so the kept face stays hidden");
    assert_eq!(zones[0].id, stored.id);
}

// ── Conversion ───────────────────────────────────────────────────────────

fn convert(scene: &mut Scene, rig: &Rig) -> hypercolor_core::scene::screens::ScreenConversion {
    convert_legacy_display_zones(scene, &rig.surfaces)
}

/// The display zone lowering produces for a device, from a bare scene.
fn lower(rig: &Rig, scene: &Scene, device_id: DeviceId) -> Option<Zone> {
    let manager = active(rig, scene.clone());
    lowered_for(&manager, device_id)
}

#[test]
fn a_convertible_zone_moves_onto_the_zone_that_places_its_screen() {
    let rig = rig();
    let base = effect_layer();
    let mut first = effect_layer();
    first.blend = BlendMode::Add;
    let mut second = color_layer();
    second.blend = BlendMode::Multiply;
    second.opacity = 0.6;
    let mut third = color_layer();
    third.enabled = false;
    let stored = legacy_zone(
        vec![first.clone(), second.clone(), third.clone()],
        target(rig.pump, BlendMode::Screen, 0.8),
    );
    let owner = primary(
        vec![strip("keys"), screen(PUMP_SCREEN, PUMP)],
        vec![base.clone()],
    );
    let owner_id = owner.id;
    let mut converted = scene(vec![owner, stored.clone()]);
    let before = lower(&rig, &converted, rig.pump).expect("legacy zone renders");

    let report = convert(&mut converted, &rig);

    assert_eq!(report.converted, vec![(stored.clone(), owner_id)]);
    assert!(report.legacy.is_empty());
    assert_eq!(converted.zones.len(), 1, "the display zone is removed");
    let layers = &converted.zones[0].layers;
    assert_eq!(layers.len(), 4, "the stack lands at the top, in order");
    assert_eq!(layers[0], base);
    let mut expected_first = scoped(first, &[PUMP_SCREEN]);
    expected_first.blend = BlendMode::Screen;
    expected_first.opacity = 0.8;
    assert_eq!(
        layers[1], expected_first,
        "the first layer takes the target"
    );
    assert_eq!(layers[2], scoped(second, &[PUMP_SCREEN]));
    assert_eq!(layers[3], scoped(third, &[PUMP_SCREEN]));

    // Lowering rebuilds the zone that was stored: same target, same opaque
    // base, same upper layers. The bottom layer's stored blend never
    // rendered, because a display zone's bottom layer always replaces.
    let after = lower(&rig, &converted, rig.pump).expect("screen layers render");
    assert_eq!(after.display_target, before.display_target);
    assert_eq!(after.layout, before.layout);
    assert_eq!(after.layers[1..], before.layers[1..]);
    let mut stored_base = before.layers[0].clone();
    stored_base.blend = BlendMode::Replace;
    assert_eq!(after.layers[0], stored_base);
    assert_eq!(after.enabled, before.enabled);
    assert!((after.brightness - before.brightness).abs() < f32::EPSILON);
}

#[test]
fn a_first_layer_that_is_not_opaque_stays_in_the_legacy_spelling() {
    let rig = rig();
    let mut first = effect_layer();
    first.opacity = 0.5;
    let stored = legacy_zone(vec![first], DisplayFaceTarget::new(rig.pump));
    let authored = scene(vec![
        primary(vec![screen(PUMP_SCREEN, PUMP)], Vec::new()),
        stored.clone(),
    ]);
    let mut converted = authored.clone();

    let report = convert(&mut converted, &rig);

    assert_eq!(
        report.legacy,
        vec![(stored.id, LegacyReason::FirstLayerOpacity)]
    );
    assert!(!report.changed());
    assert_eq!(converted, authored);
}

#[test]
fn a_disabled_first_layer_stays_in_the_legacy_spelling() {
    let rig = rig();
    let mut first = effect_layer();
    first.enabled = false;
    let stored = legacy_zone(vec![first, color_layer()], DisplayFaceTarget::new(rig.pump));
    let authored = scene(vec![
        primary(vec![screen(PUMP_SCREEN, PUMP)], Vec::new()),
        stored.clone(),
    ]);
    let mut converted = authored.clone();

    let report = convert(&mut converted, &rig);

    assert_eq!(
        report.legacy,
        vec![(stored.id, LegacyReason::FirstLayerDisabled)]
    );
    assert_eq!(converted, authored);
}

#[test]
fn a_disabled_zone_and_a_dimmed_zone_stay_in_the_legacy_spelling() {
    let rig = rig();
    let mut disabled = legacy_zone(vec![effect_layer()], DisplayFaceTarget::new(rig.pump));
    disabled.enabled = false;
    let mut dimmed = legacy_zone(vec![effect_layer()], DisplayFaceTarget::new(rig.fan));
    dimmed.brightness = 0.5;
    let authored = scene(vec![
        primary(
            vec![screen(PUMP_SCREEN, PUMP), screen(FAN_SCREEN, FAN)],
            Vec::new(),
        ),
        disabled.clone(),
        dimmed.clone(),
    ]);
    let mut converted = authored.clone();

    let report = convert(&mut converted, &rig);

    assert_eq!(
        report.legacy,
        vec![
            (disabled.id, LegacyReason::ZoneDisabled),
            (dimmed.id, LegacyReason::ZoneBrightness),
        ]
    );
    assert_eq!(converted, authored);
}

#[test]
fn a_layerless_display_zone_is_removed_and_the_screen_keeps_its_view() {
    let rig = rig();
    let bare = legacy_zone(Vec::new(), DisplayFaceTarget::new(rig.pump));
    let owner = primary(vec![screen(PUMP_SCREEN, PUMP)], vec![effect_layer()]);
    let authored_owner = owner.clone();
    let mut converted = scene(vec![owner, bare.clone()]);

    let report = convert(&mut converted, &rig);

    assert_eq!(report.removed, vec![bare]);
    assert!(report.converted.is_empty());
    assert_eq!(converted.zones, vec![authored_owner]);
    let view = screen_view(&converted, &rig.surfaces, rig.pump).expect("view");
    assert!(view.zone.layers.is_empty());
    assert_eq!(view.zone.role, ZoneRole::Display);
}

#[test]
fn a_zone_whose_screen_no_zone_places_stays_stored_until_the_screen_is_placed() {
    let rig = rig();
    let face = effect_layer();
    let stored = legacy_zone(vec![face.clone()], DisplayFaceTarget::new(rig.pump));
    // The pump's ring is in the zone, its screen is not: the render thread
    // finds the screen through its device, and a box of the scene
    // manager's choosing would move its backdrop.
    let owner = primary(
        vec![strip("keys"), output("pump-ring", PUMP, "Ring")],
        vec![effect_layer()],
    );
    let authored = scene(vec![owner, stored.clone()]);
    let mut converted = authored.clone();

    let report = convert(&mut converted, &rig);

    assert_eq!(
        report.legacy,
        vec![(stored.id, LegacyReason::ScreenUnplaced)]
    );
    assert!(!report.changed());
    assert_eq!(
        converted, authored,
        "no output is added on the screen's behalf"
    );
    assert_eq!(
        lower(&rig, &converted, rig.pump).map(|zone| zone.id),
        Some(stored.id),
        "the stored zone keeps rendering as it does"
    );

    converted.zones[0]
        .layout
        .zones
        .push(screen(PUMP_SCREEN, PUMP));
    let report = convert(&mut converted, &rig);

    assert_eq!(
        report.converted.len(),
        1,
        "placing the screen offers it again"
    );
    assert!(converted.display_zone_for(rig.pump).is_none());
    assert_eq!(converted.zones[0].layers[1].id, face.id);
}

#[test]
fn a_scene_that_holds_only_a_display_zone_keeps_it() {
    let rig = rig();
    let stored = legacy_zone(vec![effect_layer()], DisplayFaceTarget::new(rig.pump));
    let authored = scene(vec![stored.clone()]);
    let mut converted = authored.clone();

    let report = convert(&mut converted, &rig);

    assert_eq!(
        report.legacy,
        vec![(stored.id, LegacyReason::ScreenUnplaced)]
    );
    assert_eq!(
        converted, authored,
        "no zone is created to place the screen"
    );
}

#[test]
fn a_zone_whose_owner_already_holds_layers_for_the_screen_stays_stored() {
    let rig = rig();
    let stored = legacy_zone(vec![effect_layer()], DisplayFaceTarget::new(rig.pump));
    let authored = scene(vec![
        primary(
            vec![screen(PUMP_SCREEN, PUMP)],
            vec![scoped(color_layer(), &[PUMP_SCREEN])],
        ),
        stored.clone(),
    ]);
    let mut converted = authored.clone();

    let report = convert(&mut converted, &rig);

    assert_eq!(
        report.legacy,
        vec![(stored.id, LegacyReason::OwnerHoldsScreenLayers)]
    );
    assert_eq!(
        converted, authored,
        "the covered layer would become the bottom of the stack and change the frames"
    );
}

#[test]
fn a_zone_for_an_unknown_device_waits_until_the_device_binds() {
    let rig = rig();
    let stranger = DeviceId::new();
    let stored = legacy_zone(vec![effect_layer()], DisplayFaceTarget::new(stranger));
    let authored = scene(vec![
        primary(
            vec![strip("keys"), screen("usb:late-display", "usb:late")],
            Vec::new(),
        ),
        stored.clone(),
    ]);
    let mut converted = authored.clone();

    let report = convert(&mut converted, &rig);
    assert_eq!(
        report.legacy,
        vec![(stored.id, LegacyReason::DeviceUnknown)]
    );
    assert_eq!(converted, authored);

    let mut bound = rig.surfaces.clone();
    bound.insert("usb:late".to_owned(), surface(stranger, "Late", (320, 320)));
    let report = convert_legacy_display_zones(&mut converted, &bound);
    assert_eq!(report.converted.len(), 1);
    assert!(converted.display_zone_for(stranger).is_none());
}

#[test]
fn a_snapshot_locked_scene_keeps_its_display_zones_until_it_is_unlocked() {
    let rig = rig();
    let stored = legacy_zone(vec![effect_layer()], DisplayFaceTarget::new(rig.pump));
    let mut authored = scene(vec![
        primary(vec![screen(PUMP_SCREEN, PUMP)], Vec::new()),
        stored.clone(),
    ]);
    authored.mutation_mode = SceneMutationMode::Snapshot;
    let mut converted = authored.clone();

    let report = convert(&mut converted, &rig);
    assert_eq!(report.legacy, vec![(stored.id, LegacyReason::SceneLocked)]);
    assert_eq!(converted, authored);

    converted.mutation_mode = SceneMutationMode::Live;
    assert_eq!(convert(&mut converted, &rig).converted.len(), 1);
}

#[test]
fn conversion_leaves_the_kept_face_where_it_is_stored() {
    let rig = rig();
    let stored = legacy_zone(vec![effect_layer()], DisplayFaceTarget::new(rig.pump));
    let authored = scene(vec![
        primary(vec![screen(PUMP_SCREEN, PUMP)], Vec::new()),
        stored,
    ]);
    let scene_id = authored.id;
    let mut manager = active(&rig, authored);
    let kept = kept_face(rig.fan);
    manager.set_default_display_zone(kept.clone());

    let report = manager.convert_legacy_display_zones(&scene_id);

    assert_eq!(report.converted.len(), 1);
    assert_eq!(manager.default_display_zones(), std::slice::from_ref(&kept));
    assert_eq!(
        lowered_for(&manager, rig.fan).map(|zone| zone.id),
        Some(kept.id),
        "a screen the scene leaves alone still shows its kept face"
    );
}

#[test]
fn conversion_run_twice_changes_nothing_the_second_time() {
    let rig = rig();
    let mut partial = effect_layer();
    partial.opacity = 0.5;
    let convertible = legacy_zone(
        vec![effect_layer(), color_layer()],
        target(rig.pump, BlendMode::Screen, 0.8),
    );
    let stays = legacy_zone(vec![partial], DisplayFaceTarget::new(rig.fan));
    let mut converted = scene(vec![
        primary(
            vec![screen(PUMP_SCREEN, PUMP), screen(FAN_SCREEN, FAN)],
            Vec::new(),
        ),
        convertible,
        stays.clone(),
    ]);

    let first = convert(&mut converted, &rig);
    assert!(first.changed());
    let settled = converted.clone();

    let second = convert(&mut converted, &rig);

    assert!(!second.changed());
    assert!(second.converted.is_empty());
    assert_eq!(
        second.legacy,
        vec![(stays.id, LegacyReason::FirstLayerOpacity)]
    );
    assert_eq!(converted, settled, "not even a revision moves");
}

#[test]
fn a_target_the_renderer_would_clamp_stays_in_the_legacy_spelling() {
    let rig = rig();
    let stored = legacy_zone(
        vec![effect_layer()],
        target(rig.pump, BlendMode::Replace, 0.5),
    );
    let authored = scene(vec![
        primary(vec![screen(PUMP_SCREEN, PUMP)], Vec::new()),
        stored.clone(),
    ]);
    let mut converted = authored.clone();

    let report = convert(&mut converted, &rig);

    assert_eq!(
        report.legacy,
        vec![(stored.id, LegacyReason::TargetNotNormalized)]
    );
    assert_eq!(converted, authored);
}

#[test]
fn a_zone_whose_screen_sits_in_a_paused_zone_stays_in_the_legacy_spelling() {
    let rig = rig();
    let stored = legacy_zone(vec![effect_layer()], DisplayFaceTarget::new(rig.pump));
    let mut owner = primary(vec![screen(PUMP_SCREEN, PUMP)], Vec::new());
    owner.enabled = false;
    let authored = scene(vec![owner, stored.clone()]);
    let mut converted = authored.clone();

    let report = convert(&mut converted, &rig);

    assert_eq!(
        report.legacy,
        vec![(stored.id, LegacyReason::OwnerDisabled)]
    );
    assert_eq!(
        converted, authored,
        "the face keeps running as it does today"
    );
}

#[test]
fn a_zone_whose_layer_id_the_owner_already_uses_stays_in_the_legacy_spelling() {
    let rig = rig();
    let taken = effect_layer();
    let stored = legacy_zone(vec![taken.clone()], DisplayFaceTarget::new(rig.pump));
    let authored = scene(vec![
        primary(vec![screen(PUMP_SCREEN, PUMP)], vec![taken]),
        stored.clone(),
    ]);
    let mut converted = authored.clone();

    let report = convert(&mut converted, &rig);

    assert_eq!(report.legacy, vec![(stored.id, LegacyReason::LayerIdTaken)]);
    assert_eq!(converted, authored);
}

#[test]
fn a_face_for_a_screen_no_zone_places_is_stored_as_a_display_zone() {
    let rig = rig();
    let owner = primary(vec![strip("keys")], vec![effect_layer()]);
    let authored = scene(vec![owner.clone()]);
    let scene_id = authored.id;
    let mut manager = active(&rig, authored);
    let clock = sample_effect("clock");
    let weather = sample_effect("weather");

    let written = manager
        .write_display_zone_face(
            &scene_id,
            rig.pump,
            &clock,
            HashMap::new(),
            target(rig.pump, BlendMode::Replace, 0.4),
        )
        .expect("the face should be stored")
        .clone();

    assert_eq!(
        written.id,
        screen_zone_id(scene_id, rig.pump),
        "the stored zone takes the id the screen's empty view had"
    );
    assert_eq!(written.role, ZoneRole::Display);
    assert_eq!(written.name, "Pump Face");
    assert_eq!(
        written.display_target,
        Some(target(rig.pump, BlendMode::Replace, 1.0)),
        "a replacing face is stored at full opacity"
    );
    assert_eq!(
        (written.layout.canvas_width, written.layout.canvas_height),
        (480, 480)
    );
    assert_eq!(written.effect_ids().collect::<Vec<_>>(), vec![clock.id]);
    let stored = manager.get(&scene_id).expect("scene");
    assert_eq!(
        stored.zones[0], owner,
        "no zone gains an output for the screen"
    );
    assert_eq!(lowered_for(&manager, rig.pump), Some(written.clone()));
    assert!(matches!(
        manager.active_screen_stack(rig.pump),
        Some(ScreenStack::Legacy(_))
    ));

    let again = manager
        .write_display_zone_face(
            &scene_id,
            rig.pump,
            &weather,
            HashMap::new(),
            DisplayFaceTarget::new(rig.pump),
        )
        .expect("the face should be replaced")
        .clone();
    assert_eq!(again.id, written.id, "the zone is updated in place");
    assert_eq!(again.effect_ids().collect::<Vec<_>>(), vec![weather.id]);
    assert_eq!(display_zones(&manager).len(), 1);

    // Placing the screen converts the stored face, and the face keeps its
    // effect slot: the derived zone has the id the stored one had.
    let before = effect_slots(&manager);
    let mut placed = manager.get(&scene_id).expect("scene").clone();
    placed.zones[0].layout.zones.push(screen(PUMP_SCREEN, PUMP));
    manager.update(placed).expect("scene should update");
    let report = manager.convert_legacy_display_zones(&scene_id);
    assert_eq!(report.converted.len(), 1);
    assert_eq!(effect_slots(&manager), before);
}

#[test]
fn a_display_zone_is_stored_only_for_a_known_screen_in_a_live_scene() {
    let rig = rig();
    let mut locked = scene(vec![primary(vec![strip("keys")], Vec::new())]);
    locked.mutation_mode = SceneMutationMode::Snapshot;
    let locked_id = locked.id;
    let mut manager = active(&rig, locked);
    assert_eq!(
        manager.ensure_display_zone(&locked_id, rig.pump),
        Err(ZoneMutationError::SnapshotLocked)
    );

    let live = scene(vec![primary(vec![strip("keys")], Vec::new())]);
    let live_id = live.id;
    manager.create(live).expect("scene should be created");
    assert_eq!(
        manager.ensure_display_zone(&live_id, DeviceId::new()),
        Err(ZoneMutationError::OutputMissing)
    );
    let first = manager
        .ensure_display_zone(&live_id, rig.pump)
        .expect("a bare display zone");
    assert_eq!(manager.ensure_display_zone(&live_id, rig.pump), Ok(first));
    assert_eq!(
        manager.get(&live_id).expect("scene").zones.len(),
        2,
        "asking again adds nothing"
    );
}

// ── Renderers ────────────────────────────────────────────────────────────

/// The effect slots a render list asks for. A slot is keyed by zone and
/// layer, so a key that persists keeps its renderer and a new key starts one.
fn effect_slots(manager: &SceneManager) -> Vec<(ZoneId, SceneLayerId)> {
    let mut slots = manager
        .resolved_zones()
        .iter()
        .filter(|zone| zone.display_target.is_some())
        .flat_map(|zone| {
            zone.layers
                .iter()
                .filter(|layer| matches!(layer.source, LayerSource::Effect { .. }))
                .map(|layer| (zone.id, layer.id))
        })
        .collect::<Vec<_>>();
    slots.sort_by_key(|(zone, layer)| (zone.0, layer.0));
    slots
}

#[test]
fn conversion_starts_a_faces_renderer_once_and_nothing_restarts_it_afterwards() {
    let rig = rig();
    let face = effect_layer();
    let stored = legacy_zone(vec![face.clone()], DisplayFaceTarget::new(rig.pump));
    let owner = primary(vec![strip("keys"), screen(PUMP_SCREEN, PUMP)], Vec::new());
    let desk = custom("Desk", vec![strip("desk")], Vec::new());
    let authored = scene(vec![owner, desk, stored.clone()]);
    let scene_id = authored.id;
    let mut manager = active(&rig, authored);
    assert_eq!(effect_slots(&manager), vec![(stored.id, face.id)]);

    manager.convert_legacy_display_zones(&scene_id);
    let converted = vec![(screen_zone_id(scene_id, rig.pump), face.id)];
    assert_eq!(
        effect_slots(&manager),
        converted,
        "one slot before and one after: the face restarts once, under its new zone id"
    );

    // Converting again, the device table arriving again, and an unrelated
    // edit all leave the slot where it is.
    manager.convert_legacy_display_zones(&scene_id);
    manager.set_screen_surfaces(rig.surfaces.clone());
    manager.invalidate_resolved_zones();
    assert_eq!(effect_slots(&manager), converted);

    // So does moving the screen, with its face, to another zone.
    let mut moved = manager.get(&scene_id).expect("scene").clone();
    let position = moved.zones[0]
        .layout
        .zones
        .iter()
        .position(|output| output.id == PUMP_SCREEN)
        .expect("the screen's output");
    let output = moved.zones[0].layout.zones.remove(position);
    let layer = moved.zones[0].layers.remove(0);
    moved.zones[1].layout.zones.push(output);
    moved.zones[1].layers.push(layer);
    manager.update(moved).expect("scene should update");
    assert_eq!(effect_slots(&manager), converted);
    assert_eq!(
        display_zones(&manager).len(),
        1,
        "the screen has one binding at every step"
    );
}

#[test]
fn one_layer_on_three_panels_asks_for_as_many_renderers_as_three_display_zones() {
    let mut rig = rig();
    let third = DeviceId::new();
    rig.surfaces
        .insert("usb:top".to_owned(), surface(third, "Top", (320, 320)));
    let outputs = vec![
        screen(PUMP_SCREEN, PUMP),
        screen(FAN_SCREEN, FAN),
        screen("usb:top-display", "usb:top"),
    ];
    let shared = scoped(
        effect_layer(),
        &[PUMP_SCREEN, FAN_SCREEN, "usb:top-display"],
    );
    let layered = active(&rig, scene(vec![primary(outputs.clone(), vec![shared])]));

    let mut zones = vec![primary(outputs, Vec::new())];
    for device_id in [rig.pump, rig.fan, third] {
        zones.push(legacy_zone(
            vec![effect_layer()],
            DisplayFaceTarget::new(device_id),
        ));
    }
    let mut stored = scene(zones);
    stored.mutation_mode = SceneMutationMode::Snapshot;
    let zoned = active(&rig, stored);

    assert_eq!(effect_slots(&zoned).len(), 3);
    assert_eq!(
        effect_slots(&layered).len(),
        effect_slots(&zoned).len(),
        "one renderer per panel either way"
    );
}

// ── Views ────────────────────────────────────────────────────────────────

#[test]
fn every_connected_screen_has_a_view_whether_or_not_it_has_a_stack() {
    let rig = rig();
    let mut face = scoped(effect_layer(), &[PUMP_SCREEN]);
    face.blend = BlendMode::Screen;
    face.opacity = 0.4;
    let overlay = scoped(color_layer(), &[PUMP_SCREEN]);
    let owner = primary(
        vec![strip("keys"), screen(PUMP_SCREEN, PUMP)],
        vec![effect_layer(), face.clone(), overlay.clone()],
    );
    let authored = scene(vec![owner.clone()]);

    let views = screen_views(&authored, &rig.surfaces);
    assert_eq!(views.len(), 2);
    let pump = views
        .iter()
        .find(|view| view.device_id == rig.pump)
        .expect("pump view");
    assert_eq!(pump.zone.id, screen_zone_id(authored.id, rig.pump));
    assert_eq!(pump.zone.name, "Pump");
    assert_eq!(pump.zone.role, ZoneRole::Display);
    assert!(pump.zone.enabled);
    assert_eq!(
        pump.zone.display_target,
        Some(target(rig.pump, BlendMode::Screen, 0.4))
    );
    let mut listed_face = face;
    listed_face.scope = None;
    let mut listed_overlay = overlay;
    listed_overlay.scope = None;
    assert_eq!(
        pump.zone.layers,
        vec![listed_face, listed_overlay],
        "as authored, with the scope omitted"
    );
    assert!(pump.zone.layout.zones.is_empty());
    assert_eq!(pump.owner, Some((owner.id, PUMP_SCREEN.to_owned())));

    let fan = views
        .iter()
        .find(|view| view.device_id == rig.fan)
        .expect("fan view");
    assert_eq!(fan.zone.id, screen_zone_id(authored.id, rig.fan));
    assert!(fan.zone.layers.is_empty());
    assert_eq!(
        fan.zone.display_target,
        Some(DisplayFaceTarget::new(rig.fan)),
        "an empty stack reads as a fresh target"
    );
    assert_eq!(fan.owner, None, "no zone places the fan screen");
    assert!(matches!(
        screen_stack(&authored, &rig.surfaces, rig.fan),
        ScreenStack::Unplaced
    ));
}

#[test]
fn a_stored_display_zone_is_listed_as_itself_and_not_as_a_view() {
    let rig = rig();
    let mut stored = legacy_zone(vec![effect_layer()], target(rig.pump, BlendMode::Add, 0.5));
    stored.enabled = false;
    let authored = scene(vec![
        primary(vec![screen(PUMP_SCREEN, PUMP)], Vec::new()),
        stored.clone(),
    ]);

    let views = screen_views(&authored, &rig.surfaces);
    assert_eq!(views.len(), 1, "the legacy zone is a zone of the scene");
    assert_eq!(views[0].device_id, rig.fan);

    let resolved = screen_view(&authored, &rig.surfaces, rig.pump).expect("legacy view");
    assert_eq!(resolved.zone.id, stored.id);
    assert!(!resolved.zone.enabled, "its own flag");
    assert_eq!(resolved.zone.display_target, stored.display_target);
    assert_eq!(resolved.owner, None);
}

#[test]
fn a_screen_that_is_away_is_listed_only_while_it_has_a_stack() {
    let mut rig = rig();
    for surface in rig.surfaces.values_mut() {
        surface.connected = false;
    }
    let face = scoped(effect_layer(), &[PUMP_SCREEN]);
    let authored = scene(vec![primary(
        vec![screen(PUMP_SCREEN, PUMP), screen(FAN_SCREEN, FAN)],
        vec![face.clone()],
    )]);

    let views = screen_views(&authored, &rig.surfaces);
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].device_id, rig.pump);
    assert_eq!(views[0].zone.layers[0].id, face.id);
    assert!(
        screen_view(&authored, &rig.surfaces, rig.fan).is_some(),
        "its view still resolves, so a first face can be written for it"
    );
    assert!(
        lower(&rig, &authored, rig.pump).is_some(),
        "its stack still lowers, as a stored display zone always rendered"
    );
}

#[test]
fn a_screen_the_rig_does_not_know_has_no_view() {
    let rig = rig();
    let authored = scene(vec![primary(
        vec![screen("usb:gone-display", "usb:gone")],
        vec![scoped(effect_layer(), &["usb:gone-display"])],
    )]);

    assert!(screen_view(&authored, &rig.surfaces, DeviceId::new()).is_none());
    assert!(
        screen_views(&authored, &rig.surfaces)
            .iter()
            .all(|view| view.zone.layers.is_empty())
    );
}

// ── Writes about one screen ──────────────────────────────────────────────

#[test]
fn splitting_a_shared_layer_gives_one_screen_a_copy_directly_above() {
    let shared = scoped(effect_layer(), &[FAN_SCREEN, PUMP_SCREEN]);
    let top = effect_layer();
    let mut zone = primary(
        vec![screen(PUMP_SCREEN, PUMP), screen(FAN_SCREEN, FAN)],
        vec![shared.clone(), top.clone()],
    );

    let index = split_for_screen(&mut zone, shared.id, PUMP_SCREEN).expect("split");

    assert_eq!(index, 1);
    assert_eq!(zone.layers.len(), 3);
    assert_eq!(zone.layers[0].id, shared.id);
    assert_eq!(
        zone.layers[0].scope,
        Some(LayerScope {
            outputs: vec![FAN_SCREEN.to_owned()]
        }),
        "the other screen keeps the original"
    );
    assert_ne!(
        zone.layers[1].id, shared.id,
        "the copy has an id of its own"
    );
    assert_eq!(zone.layers[1].source, shared.source);
    assert_eq!(
        zone.layers[1].scope,
        Some(LayerScope {
            outputs: vec![PUMP_SCREEN.to_owned()]
        })
    );
    assert_eq!(zone.layers[2], top);
}

#[test]
fn splitting_a_sole_layer_changes_nothing() {
    let sole = scoped(effect_layer(), &[PUMP_SCREEN]);
    let mut zone = primary(vec![screen(PUMP_SCREEN, PUMP)], vec![sole.clone()]);
    let before = zone.clone();

    assert_eq!(split_for_screen(&mut zone, sole.id, PUMP_SCREEN), Some(0));
    assert_eq!(zone, before);
    assert_eq!(split_for_screen(&mut zone, sole.id, FAN_SCREEN), None);
    assert_eq!(
        split_for_screen(&mut zone, SceneLayerId::new(), PUMP_SCREEN),
        None
    );
}

#[test]
fn removing_a_screen_deletes_a_sole_layer_and_narrows_a_shared_one() {
    let sole = scoped(effect_layer(), &[PUMP_SCREEN]);
    let shared = scoped(color_layer(), &[FAN_SCREEN, PUMP_SCREEN]);
    let mut zone = primary(
        vec![screen(PUMP_SCREEN, PUMP), screen(FAN_SCREEN, FAN)],
        vec![sole.clone(), shared.clone()],
    );

    assert!(remove_screen_from_layer(&mut zone, shared.id, PUMP_SCREEN));
    assert_eq!(
        zone.layers[1].scope,
        Some(LayerScope {
            outputs: vec![FAN_SCREEN.to_owned()]
        })
    );
    assert!(remove_screen_from_layer(&mut zone, sole.id, PUMP_SCREEN));
    assert_eq!(zone.layers.len(), 1);
    assert_eq!(zone.layers[0].id, shared.id);
    assert!(!remove_screen_from_layer(&mut zone, shared.id, PUMP_SCREEN));
}

#[test]
fn clearing_a_screen_leaves_canvas_layers_and_other_screens_alone() {
    let base = effect_layer();
    let mixed = scoped(effect_layer(), &["keys", PUMP_SCREEN]);
    let sole = scoped(effect_layer(), &[PUMP_SCREEN]);
    let shared = scoped(color_layer(), &[FAN_SCREEN, PUMP_SCREEN]);
    let other = scoped(color_layer(), &[FAN_SCREEN]);
    let mut zone = primary(
        vec![
            strip("keys"),
            screen(PUMP_SCREEN, PUMP),
            screen(FAN_SCREEN, FAN),
        ],
        vec![
            base.clone(),
            mixed.clone(),
            sole,
            shared.clone(),
            other.clone(),
        ],
    );

    assert!(clear_screen_layers(&mut zone, PUMP_SCREEN));

    assert_eq!(
        zone.layers,
        vec![base, mixed, scoped(shared, &[FAN_SCREEN]), other]
    );
    assert!(!clear_screen_layers(&mut zone, PUMP_SCREEN));
}

#[test]
fn clearing_a_zones_lighting_keeps_a_face_and_drops_one_whose_screen_left() {
    let rig = rig();
    let face = scoped(effect_layer(), &[PUMP_SCREEN]);
    let stranded = scoped(effect_layer(), &["usb:gone-display"]);
    let zone = primary(
        vec![strip("keys"), screen(PUMP_SCREEN, PUMP)],
        vec![
            effect_layer(),
            scoped(color_layer(), &["keys"]),
            face.clone(),
            stranded,
        ],
    );
    let zone_id = zone.id;
    let mut manager = active(&rig, scene(vec![zone]));

    let cleared = manager.clear_zone_effect(zone_id).expect("zone").clone();

    assert_eq!(
        cleared.layers,
        vec![face.clone()],
        "stopping the lighting leaves the screen's face, as it left a display zone's"
    );
    assert_eq!(
        lowered_for(&manager, rig.pump).map(|zone| zone.layers[0].id),
        Some(face.id)
    );
}
