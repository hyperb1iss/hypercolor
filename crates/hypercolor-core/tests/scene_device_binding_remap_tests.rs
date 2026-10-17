//! Display-zone collision handling when a device identity remap retargets
//! a display zone onto a display that already has one.

use std::collections::HashMap;

use hypercolor_core::scene::{SceneManager, make_scene};
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::{DeviceFingerprint, DeviceId};
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::scene::{DisplayFaceTarget, Scene, SceneId, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use uuid::Uuid;

const LEGACY_PUSH2: &str = "usb:2982:1967:001-12";
const CURRENT_PUSH2: &str = "usb:push2:2982:1967:001-12";
const LEGACY_LCD: &str = "usb:1b1c:0c4e:A6B9T350013K7T";
const CURRENT_LCD: &str = "usb:corsair:1b1c:0c4e:A6B9T350013K7T";

fn physical_id(fingerprint: &str) -> DeviceId {
    DeviceFingerprint::from_persisted(fingerprint).stable_device_id()
}

fn empty_layout(zone_name: &str) -> SpatialLayout {
    SpatialLayout {
        id: format!("zone-{zone_name}"),
        name: "Zone Layout".to_owned(),
        description: None,
        canvas_width: 480,
        canvas_height: 480,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn display_zone(name: &str, device_id: DeviceId, layers: Vec<SceneLayer>) -> Zone {
    Zone {
        id: ZoneId::new(),
        name: name.to_owned(),
        description: Some(format!("Screen surface for {name}")),
        layers,
        layout: empty_layout(name),
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: Some(DisplayFaceTarget::new(device_id)),
        role: ZoneRole::Display,
        controls_version: 0,
        layers_version: 0,
    }
}

fn face_layer(layer_id: Uuid, font: &str) -> SceneLayer {
    SceneLayer::from_effect(
        SceneLayerId::from_uuid(layer_id),
        EffectId::new(Uuid::from_u128(0x15b2_f828_b3f7_4712_9c52_3966_e2e1_9614)),
        HashMap::from([("heroFont".to_owned(), ControlValue::Enum(font.to_owned()))]),
        HashMap::new(),
        None,
    )
}

fn v7_at(millis: u64) -> Uuid {
    uuid::Builder::from_unix_timestamp_millis(millis, &[7; 10]).into_uuid()
}

fn identity_remaps() -> HashMap<DeviceId, DeviceId> {
    HashMap::from([
        (physical_id(LEGACY_PUSH2), physical_id(CURRENT_PUSH2)),
        (physical_id(LEGACY_LCD), physical_id(CURRENT_LCD)),
    ])
}

fn activate(scene: Scene) -> (SceneManager, SceneId) {
    let scene_id = scene.id;
    let mut manager = SceneManager::with_default();
    manager
        .create(scene)
        .expect("fixture scene should be valid");
    manager
        .activate(&scene_id, None)
        .expect("fixture scene should activate");
    (manager, scene_id)
}

fn display_zones_for(scene: &Scene, device_id: DeviceId) -> Vec<&Zone> {
    scene
        .zones
        .iter()
        .filter(|zone| {
            zone.display_target
                .as_ref()
                .is_some_and(|target| target.device_id == device_id)
        })
        .collect()
}

#[test]
fn remapped_legacy_face_absorbs_the_empty_surface_minted_for_the_new_identity() {
    let face = face_layer(v7_at(1_782_000_000_000), "Space Grotesk");
    let face_id = face.id;
    let mut scene = make_scene("Studio Session");
    scene.zones = vec![
        display_zone("Ableton Push 2", physical_id(LEGACY_PUSH2), Vec::new()),
        display_zone("Corsair iCUE LINK LCD", physical_id(LEGACY_LCD), vec![face]),
        display_zone("Ableton Push 2", physical_id(CURRENT_PUSH2), Vec::new()),
        display_zone(
            "Corsair iCUE LINK LCD",
            physical_id(CURRENT_LCD),
            Vec::new(),
        ),
    ];
    let legacy_push_zone = scene.zones[0].id;
    let legacy_lcd_zone = scene.zones[1].id;
    let (mut manager, scene_id) = activate(scene);
    let revision_before = manager
        .get(&scene_id)
        .expect("fixture scene")
        .zones_revision;

    let migrated = manager.remap_device_bindings(&HashMap::new(), &identity_remaps());

    assert_eq!(migrated, 4, "two retargets plus two collapsed duplicates");
    let scene = manager.get(&scene_id).expect("migrated scene");
    assert!(scene.validate().is_ok(), "{:?}", scene.validate());
    assert_eq!(scene.zones.len(), 2);
    assert!(display_zones_for(scene, physical_id(LEGACY_LCD)).is_empty());
    assert!(display_zones_for(scene, physical_id(LEGACY_PUSH2)).is_empty());

    let [lcd] = display_zones_for(scene, physical_id(CURRENT_LCD))[..] else {
        panic!("the LCD should keep exactly one display zone");
    };
    assert_eq!(
        lcd.id, legacy_lcd_zone,
        "the zone carrying the face survives"
    );
    assert_eq!(lcd.layers.len(), 1);
    assert_eq!(lcd.layers[0].id, face_id);

    let [push] = display_zones_for(scene, physical_id(CURRENT_PUSH2))[..] else {
        panic!("the Push 2 should keep exactly one display zone");
    };
    assert_eq!(
        push.id, legacy_push_zone,
        "with no layers on either side the remapped zone survives"
    );
    assert!(scene.zones_revision > revision_before);

    let resolved = manager.resolved_zones();
    assert_eq!(
        resolved
            .iter()
            .filter(|zone| zone.role == ZoneRole::Display)
            .count(),
        2,
        "the render zones follow the collapsed scene"
    );

    let settled = manager.get(&scene_id).expect("settled scene").clone();
    assert_eq!(
        manager.remap_device_bindings(&HashMap::new(), &identity_remaps()),
        0,
        "a second remap finds nothing left to move"
    );
    assert_eq!(manager.get(&scene_id), Some(&settled));
}

#[test]
fn most_recently_assigned_layers_win_when_both_zones_carry_layers() {
    let older = face_layer(v7_at(1_780_000_000_000), "Legacy");
    let newer = face_layer(v7_at(1_790_000_000_000), "Reassigned");
    let newer_id = newer.id;
    let mut scene = make_scene("Desk");
    scene.zones = vec![
        display_zone("LCD", physical_id(LEGACY_LCD), vec![older]),
        display_zone("LCD", physical_id(CURRENT_LCD), vec![newer]),
    ];
    let current_zone = scene.zones[1].id;
    let (mut manager, scene_id) = activate(scene);

    manager.remap_device_bindings(&HashMap::new(), &identity_remaps());

    let scene = manager.get(&scene_id).expect("migrated scene");
    let [lcd] = display_zones_for(scene, physical_id(CURRENT_LCD))[..] else {
        panic!("the LCD should keep exactly one display zone");
    };
    assert_eq!(lcd.id, current_zone);
    assert_eq!(lcd.layers[0].id, newer_id);
}

#[test]
fn remapped_zone_wins_when_layer_assignment_time_is_unknown() {
    let legacy = face_layer(
        Uuid::from_u128(0x0123_4567_89ab_4def_8123_4567_89ab_cdef),
        "Legacy",
    );
    let legacy_id = legacy.id;
    let current = face_layer(v7_at(1_790_000_000_000), "Current");
    let mut scene = make_scene("Desk");
    scene.zones = vec![
        display_zone("LCD", physical_id(CURRENT_LCD), vec![current]),
        display_zone("LCD", physical_id(LEGACY_LCD), vec![legacy]),
    ];
    let legacy_zone = scene.zones[1].id;
    let (mut manager, scene_id) = activate(scene);

    manager.remap_device_bindings(&HashMap::new(), &identity_remaps());

    let scene = manager.get(&scene_id).expect("migrated scene");
    let [lcd] = display_zones_for(scene, physical_id(CURRENT_LCD))[..] else {
        panic!("the LCD should keep exactly one display zone");
    };
    assert_eq!(lcd.id, legacy_zone);
    assert_eq!(lcd.layers[0].id, legacy_id);
}

#[test]
fn retargeted_default_face_yields_to_the_default_already_registered() {
    let mut manager = SceneManager::with_default();
    let canonical = display_zone(
        "LCD",
        physical_id(CURRENT_LCD),
        vec![face_layer(v7_at(1_790_000_000_000), "Canonical")],
    );
    let canonical_zone = canonical.id;
    manager.set_default_display_zone(display_zone(
        "LCD",
        physical_id(LEGACY_LCD),
        vec![face_layer(v7_at(1_780_000_000_000), "Legacy")],
    ));
    manager.set_default_display_zone(canonical);

    manager.remap_device_bindings(&HashMap::new(), &identity_remaps());

    let defaults = manager.default_display_zones();
    assert_eq!(defaults.len(), 1);
    assert_eq!(defaults[0].id, canonical_zone);
}
