#![allow(clippy::float_cmp)]

use hypercolor_types::device::DeviceId;
use hypercolor_types::portable_setup::*;
use hypercolor_types::scene::{
    Scene, SceneId, SceneKind, SceneMutationMode, ScenePriority, TransitionSpec,
    UnassignedBehavior, Zone, ZoneId, ZoneRole,
};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

fn layout() -> SpatialLayout {
    SpatialLayout {
        id: "case".into(),
        name: "Case".into(),
        description: None,
        canvas_width: 640,
        canvas_height: 630,
        zones: vec![Output {
            id: "front".into(),
            name: "Front".into(),
            device_id: "usb:/dev/hidraw8".into(),
            zone_name: None,
            position: NormalizedPosition::new(0.5, 0.4),
            size: NormalizedPosition::new(0.2, 0.8),
            rotation: 0.0,
            scale: 1.0,
            display_order: 0,
            orientation: None,
            topology: LedTopology::Point,
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            attachment: None,
            brightness: Some(0.75),
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn setup() -> PortableSetupV1 {
    PortableSetupV1::new(
        Uuid::now_v7(),
        "Studio".into(),
        PortableOutputState {
            canvas_width: 640,
            canvas_height: 480,
            brightness: 0.8,
            enabled: true,
            paused: false,
        },
    )
}

fn blob() -> PortableBlob {
    PortableBlob {
        sha256: "a".repeat(64),
        byte_len: 100,
    }
}

#[test]
fn layout_cross_os_round_trip_preserves_geometry_and_destination_resolution() {
    let binding = Uuid::now_v7();
    let source = SetupBindings {
        outputs: HashMap::from([("usb:/dev/hidraw8".into(), binding)]),
        ..Default::default()
    };
    let destination = SetupBindings {
        outputs: HashMap::from([("usb:windows-controller".into(), binding)]),
        ..Default::default()
    };
    let original = layout();
    let portable = PortableLayout::project(&original, &source).expect("valid portable fixture");
    let wire = serde_json::to_string(&portable).expect("valid portable fixture");
    assert!(!wire.contains("hidraw"));
    assert!(!wire.contains("canvas_width"));
    assert!(!wire.contains("canvas_height"));
    let decoded: PortableLayout = serde_json::from_str(&wire).expect("valid portable fixture");
    let restored = decoded.materialize(&destination, (1920, 1080));
    assert_eq!(restored.canvas_width, 1920);
    assert_eq!(restored.canvas_height, 1080);
    assert_eq!(restored.zones[0].device_id, "usb:windows-controller");
    assert_eq!(restored.zones[0].position, original.zones[0].position);
    assert_eq!(restored.zones[0].size, original.zones[0].size);
    assert_eq!(restored.zones[0].brightness, Some(0.75));
}

#[test]
fn projection_refuses_unassociated_local_paths() {
    assert!(PortableLayout::project(&layout(), &SetupBindings::default()).is_err());
}

#[test]
fn ambiguous_and_missing_hardware_remains_offline_without_guessing() {
    let binding = Uuid::now_v7();
    let a = DeviceId::new();
    let b = DeviceId::new();
    let ambiguous = SetupBindings {
        outputs: HashMap::from([("a".into(), binding), ("b".into(), binding)]),
        devices: HashMap::from([(a, binding), (b, binding)]),
    };
    for bindings in [SetupBindings::default(), ambiguous] {
        assert_eq!(
            bindings.local_output(binding),
            format!("portable:{binding}")
        );
        assert_eq!(bindings.local_device(binding), None);
    }
}

#[test]
fn scene_round_trip_keeps_authoring_identity_but_discards_run_counters() {
    let binding = Uuid::now_v7();
    let bindings = SetupBindings {
        outputs: HashMap::from([("usb:/dev/hidraw8".into(), binding)]),
        ..Default::default()
    };
    let original = Scene {
        id: SceneId::new(),
        name: "Studio".into(),
        description: None,
        zones: vec![Zone {
            id: ZoneId::new(),
            name: "Case".into(),
            description: None,
            layers: Vec::new(),
            layout: layout(),
            brightness: 0.8,
            enabled: true,
            color: None,
            display_target: None,
            role: ZoneRole::Primary,
            controls_version: 13,
            layers_version: 7,
        }],
        zones_revision: 99,
        transition: TransitionSpec {
            duration_ms: 500,
            easing: hypercolor_types::scene::EasingFunction::EaseInOut,
            color_interpolation: hypercolor_types::scene::ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: true,
        metadata: HashMap::new(),
        unassigned_behavior: UnassignedBehavior::Off,
        layout_id: None,
        activation_brightness: None,
        kind: SceneKind::Named,
        mutation_mode: SceneMutationMode::Live,
    };
    let portable = PortableScene::project(&original, &bindings).expect("valid portable fixture");
    let wire = serde_json::to_string(&portable).expect("valid portable fixture");
    for forbidden in [
        "zones_revision",
        "controls_version",
        "layers_version",
        "canvas_width",
        "hidraw",
    ] {
        assert!(!wire.contains(forbidden), "{forbidden}");
    }
    let restored = portable.materialize(original.id, &bindings, (800, 600));
    assert_eq!(restored.id, original.id);
    assert_eq!(restored.zones[0].id, original.zones[0].id);
    assert_eq!(restored.zones_revision, 0);
    assert_eq!(restored.zones[0].controls_version, 0);
    assert_eq!(restored.zones[0].layout.canvas_height, 600);
}

#[test]
fn manifest_rejects_unknown_version_and_machine_configuration() {
    let original = setup();
    original.validate().expect("valid portable fixture");
    let mut value = serde_json::to_value(&original).expect("valid portable fixture");
    value["drivers"] = serde_json::json!({"usb": true});
    assert!(serde_json::from_value::<PortableSetupV1>(value).is_err());
    let mut future = original;
    future.schema_version = 2;
    assert!(future.validate().is_err());
}

#[test]
fn effect_files_reject_cross_os_traversal_and_case_collisions() {
    for path in [
        "../bad",
        "/tmp/effect",
        "C:/effect",
        "bad\\effect",
        "NUL.html",
        "bad.",
        "a//b",
    ] {
        assert!(!is_portable_bundle_path(path), "{path}");
    }
    assert!(is_portable_bundle_path("assets/texture.png"));
    let mut manifest = setup();
    manifest.effects.insert(
        Uuid::now_v7(),
        PortableEffect {
            name: "Effect".into(),
            entrypoint: "index.html".into(),
            files: BTreeMap::from([("index.html".into(), blob()), ("INDEX.HTML".into(), blob())]),
        },
    );
    assert!(manifest.validate().is_err());
}

#[test]
fn blob_inventory_deduplicates_and_rejects_conflicting_sizes() {
    let mut manifest = setup();
    let effect_id = Uuid::now_v7();
    manifest.effects.insert(
        effect_id,
        PortableEffect {
            name: "Effect".into(),
            entrypoint: "index.html".into(),
            files: BTreeMap::from([("index.html".into(), blob()), ("other.html".into(), blob())]),
        },
    );
    assert_eq!(manifest.blobs().expect("valid portable fixture").len(), 1);
    manifest
        .effects
        .get_mut(&effect_id)
        .expect("valid portable fixture")
        .files
        .get_mut("other.html")
        .expect("valid portable fixture")
        .byte_len += 1;
    assert!(manifest.blobs().is_err());
}

#[test]
fn missing_selection_and_invalid_render_values_fail_before_restore() {
    let mut manifest = setup();
    manifest.selection.active_scene_id = Some(SceneId::new());
    assert!(manifest.validate().is_err());
    manifest.selection.active_scene_id = None;
    manifest.output.brightness = f32::NAN;
    assert!(manifest.validate().is_err());
    manifest.output.brightness = 1.0;
    manifest.output.canvas_width = 0;
    assert!(manifest.validate().is_err());
}

#[test]
fn bundle_rejects_reserved_metadata_and_file_directory_collisions() {
    for files in [
        BTreeMap::from([
            ("index.html".into(), blob()),
            ("HyPeRcOlOr-EfFeCt.JsOn".into(), blob()),
        ]),
        BTreeMap::from([
            ("index.html".into(), blob()),
            ("resources/.HyPeRcOlOr-EfFeCt-PeNdInG".into(), blob()),
        ]),
        BTreeMap::from([
            ("index.html".into(), blob()),
            ("textures".into(), blob()),
            ("textures/image.png".into(), blob()),
        ]),
    ] {
        let mut manifest = setup();
        manifest.effects.insert(
            Uuid::now_v7(),
            PortableEffect {
                name: "Effect".into(),
                entrypoint: "index.html".into(),
                files,
            },
        );
        assert!(manifest.validate().is_err());
    }
}

#[test]
fn builtin_presets_are_checked_against_the_destination_catalog() {
    use hypercolor_types::effect::EffectId;
    use hypercolor_types::library::{
        EffectPlaylist, PlaylistId, PlaylistItem, PlaylistItemId, PlaylistItemTarget, PresetId,
    };
    let mut manifest = setup();
    let preset = PresetId::new();
    let playlist = PlaylistId::new();
    let effect = EffectId::new(Uuid::now_v7());
    manifest.playlists.insert(
        playlist.0,
        EffectPlaylist {
            id: playlist,
            name: "Playlist".into(),
            description: None,
            items: vec![PlaylistItem {
                id: PlaylistItemId::new(),
                target: PlaylistItemTarget::Preset { preset_id: preset },
                duration_ms: None,
                transition_ms: None,
            }],
            loop_enabled: true,
            created_at_ms: 0,
            updated_at_ms: 0,
        },
    );
    assert!(
        manifest
            .validate_with_catalog(&PortableReferenceCatalog::default())
            .is_err()
    );
    let mut catalog = PortableReferenceCatalog::default();
    catalog.presets.insert(preset, effect);
    catalog.effects.insert(effect);
    manifest
        .validate_with_catalog(&catalog)
        .expect("builtin catalog satisfies reference");
}

#[test]
fn unresolved_display_zone_is_inert_and_materializes_after_explicit_binding() {
    let manifest: PortableSetupV1 =
        serde_json::from_str(include_str!("fixtures/portable_setup_v1.json"))
            .expect("portable wire fixture");
    let (id, portable) = manifest
        .scenes
        .iter()
        .find(|(_, scene)| scene.zones.iter().any(|zone| zone.display_target.is_some()))
        .expect("fixture includes a display scene");
    let target = portable
        .zones
        .iter()
        .find_map(|zone| zone.display_target.as_ref())
        .expect("display target");
    let missing = portable.materialize(SceneId(*id), &SetupBindings::default(), (800, 600));
    assert!(
        missing
            .zones
            .iter()
            .all(|zone| zone.role != ZoneRole::Display)
    );
    missing
        .validate()
        .expect("missing hardware leaves a valid native scene");
    let local = DeviceId::new();
    let mut bindings = SetupBindings::default();
    bindings.devices.insert(local, target.device);
    let restored = portable.materialize(SceneId(*id), &bindings, (800, 600));
    assert!(restored.zones.iter().any(|zone| {
        zone.display_target
            .as_ref()
            .is_some_and(|target| target.device_id == local)
    }));
    restored
        .validate()
        .expect("explicit binding restores a valid native display zone");
}

#[test]
fn portable_output_requires_one_representable_power_mode() {
    let mut manifest = setup();
    manifest.output.enabled = false;
    manifest.output.paused = true;
    assert!(manifest.validate().is_err());
    manifest.output.paused = false;
    manifest
        .validate()
        .expect("stopped output is representable");
}

#[test]
fn default_scene_identity_requires_its_durable_scene_kind() {
    let mut manifest: PortableSetupV1 =
        serde_json::from_str(include_str!("fixtures/portable_setup_v1.json"))
            .expect("portable wire fixture");
    let mut scene = manifest
        .scenes
        .values()
        .next()
        .expect("fixture scene")
        .clone();
    manifest.scenes.clear();
    manifest.selection.active_scene_id = None;
    manifest.selection.default_scene_id = None;
    manifest.scenes.insert(Uuid::nil(), scene.clone());
    assert!(manifest.validate().is_err());
    scene.kind = SceneKind::Ephemeral;
    manifest.scenes.insert(Uuid::nil(), scene);
    manifest
        .validate()
        .expect("default scene has its durable owner");
}

#[test]
fn unavailable_face_deferral_preserves_structural_and_bound_resource_validation() {
    let mut manifest = setup();
    let target = Uuid::now_v7();
    manifest.devices.insert(
        target,
        PortableDevice {
            name: "Offline display".into(),
            portable_key: None,
            driver_id: "test".into(),
            vendor_id: None,
            product_id: None,
            model: None,
            led_count: 0,
            binding_hint: None,
        },
    );
    manifest.display_faces.insert(
        target,
        PortableDisplayFace {
            effect_id: hypercolor_types::effect::EffectId::new(Uuid::now_v7()),
            controls: HashMap::new(),
            blend_mode: hypercolor_types::layer::BlendMode::Alpha,
            opacity: 1.0,
        },
    );
    let catalog = PortableReferenceCatalog::default();
    assert!(manifest.validate_with_catalog(&catalog).is_err());
    manifest
        .validate_materializable_references(&catalog, &SetupBindings::default())
        .expect("only missing unbound default-face resources may be deferred");
    let bindings = SetupBindings {
        devices: HashMap::from([(DeviceId::new(), target)]),
        outputs: HashMap::new(),
    };
    assert!(
        manifest
            .validate_materializable_references(&catalog, &bindings)
            .is_err()
    );
    manifest
        .display_faces
        .get_mut(&target)
        .expect("face")
        .opacity = 1.5;
    assert!(
        manifest
            .validate_materializable_references(&catalog, &SetupBindings::default())
            .is_err()
    );
    manifest
        .display_faces
        .get_mut(&target)
        .expect("face")
        .opacity = 1.0;
    let original = manifest.display_faces[&target].effect_id;
    manifest
        .display_faces
        .get_mut(&target)
        .expect("face")
        .effect_id = hypercolor_types::effect::EffectId::new(Uuid::nil());
    assert!(
        manifest
            .validate_materializable_references(&catalog, &SetupBindings::default())
            .is_err()
    );
    manifest
        .display_faces
        .get_mut(&target)
        .expect("face")
        .effect_id = original;
    manifest.devices.clear();
    assert!(
        manifest
            .validate_materializable_references(&catalog, &SetupBindings::default())
            .is_err()
    );
}
