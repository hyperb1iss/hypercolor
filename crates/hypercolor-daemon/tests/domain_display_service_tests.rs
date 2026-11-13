//! Service-level tests for the display-face domain layer
//! (Spec 76 §2.2, §2.3).
//!
//! A display carries its face on two layers, and the pair used to be
//! implemented twice — once for REST and once for MCP. What these pin is
//! the single contract: what the scene layer persists, what the default
//! layer deliberately does not, and that both commit rather than writing
//! through.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use hypercolor_core::effect::EffectEntry;
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceId, DeviceInfo,
    DeviceOrigin, DeviceTopologyHint, DisplayFrameFormat, SegmentInfo,
};
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, EffectCategory, EffectId, EffectMetadata,
    EffectSource, EffectState,
};
use hypercolor_types::event::{HypercolorEvent, ZoneChangeKind};
use hypercolor_types::layer::{BlendMode, LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{
    ColorInterpolation, DisplayFaceTarget, EasingFunction, Scene, SceneId, SceneKind,
    SceneMutationMode, ScenePriority, TransitionSpec, UnassignedBehavior, ZoneId, ZoneRole,
};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use uuid::Uuid;

use hypercolor_core::scene::screens::{ScreenStack, ScreenSurface, ScreenSurfaces};
use hypercolor_core::spatial::DISPLAY_ZONE_NAME;
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::domain::display::{
    PatchDisplayComposition, PatchDisplayFaceControls, SetDisplayFace, clear_display_face,
    patch_display_composition, patch_display_face_controls, prune_display_zones_for_device,
    remove_default_display_overlay, set_default_display_overlay, set_display_face,
    sync_screen_surfaces,
};
use hypercolor_daemon::domain::scene::commit_scene;
use hypercolor_types::event::LayerStackChangeKind;
use hypercolor_types::layer::LayerScope;
use hypercolor_types::scene::Zone;
use hypercolor_types::spatial::{LedTopology, NormalizedPosition, Output};

// ── Harness ──────────────────────────────────────────────────────────────

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    (Arc::new(AppState::new_with_data_dir(data_dir)), tempdir)
}

fn face_layout(device_id: DeviceId) -> SpatialLayout {
    SpatialLayout {
        id: format!("display-face:{device_id}"),
        name: "Face".to_owned(),
        description: None,
        canvas_width: 480,
        canvas_height: 480,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn face_effect(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} display face"),
        category: EffectCategory::Display,
        tags: vec!["face".to_owned()],
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Html {
            path: format!("/tmp/{name}.html").into(),
        },
        license: None,
    }
}

fn numeric_control(id: &str) -> ControlDefinition {
    ControlDefinition {
        id: id.to_owned(),
        name: id.to_owned(),
        kind: ControlKind::Number,
        control_type: ControlType::Slider,
        default_value: ControlValue::Float(0.5),
        min: Some(0.0),
        max: Some(1.0),
        step: None,
        labels: Vec::new(),
        group: None,
        tooltip: None,
        aspect_lock: None,
        preview_source: None,
        binding: None,
    }
}

async fn insert_effect(state: &AppState, metadata: &EffectMetadata) {
    let entry = EffectEntry {
        metadata: metadata.clone(),
        source_path: format!("/tmp/{}.html", metadata.name).into(),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    };
    let _ = state.domains.effects.register(entry).await;
}

fn named_scene(name: &str) -> Scene {
    Scene {
        id: SceneId::new(),
        name: name.to_owned(),
        description: None,
        zones: Vec::new(),
        zones_revision: 0,
        transition: TransitionSpec {
            duration_ms: 1000,
            easing: EasingFunction::Linear,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: true,
        metadata: HashMap::new(),
        unassigned_behavior: UnassignedBehavior::Off,
        layout_id: None,
        activation_brightness: None,
        kind: SceneKind::Named,
        mutation_mode: SceneMutationMode::Live,
    }
}

async fn assign_command(
    state: &AppState,
    device_id: DeviceId,
    effect: &EffectMetadata,
) -> SetDisplayFace {
    SetDisplayFace {
        device_id,
        effect: state
            .domains
            .effects
            .metadata_for_mutation(effect.id)
            .await
            .expect("registered effect should resolve"),
        controls: HashMap::new(),
        target: DisplayFaceTarget {
            blend_mode: BlendMode::Alpha,
            device_id,
            opacity: 1.0,
        },
    }
}

/// A runtime overlay zone, as the preference store materializes one.
fn overlay_zone(device_id: DeviceId, effect_id: EffectId) -> hypercolor_types::scene::Zone {
    let mut zone = hypercolor_core::scene::default_primary_zone(face_layout(device_id));
    zone.id = ZoneId::new();
    "Kraken Face".clone_into(&mut zone.name);
    zone.role = ZoneRole::Display;
    zone.layers = vec![SceneLayer::from_effect(
        SceneLayerId::new(),
        effect_id,
        HashMap::new(),
        HashMap::new(),
        None,
    )];
    zone.display_target = Some(DisplayFaceTarget::new(device_id));
    zone
}

fn zone_controls(zone: &hypercolor_types::scene::Zone) -> Option<&HashMap<String, ControlValue>> {
    zone.layers.iter().find_map(|layer| match &layer.source {
        LayerSource::Effect { controls, .. } => Some(controls),
        _ => None,
    })
}

fn drain_events(
    receiver: &mut tokio::sync::broadcast::Receiver<hypercolor_core::bus::TimestampedEvent>,
) -> Vec<HypercolorEvent> {
    let mut seen = Vec::new();
    while let Ok(timestamped) = receiver.try_recv() {
        seen.push(timestamped.event);
    }
    seen
}

// ── Screens ──────────────────────────────────────────────────────────────

const LAYOUT_ID: &str = "usb:kraken";
const SCREEN: &str = "usb:kraken-display";

fn screen_output(id: &str, layout_id: &str) -> Output {
    Output {
        id: id.to_owned(),
        name: "Kraken LCD".to_owned(),
        device_id: layout_id.to_owned(),
        zone_name: Some(DISPLAY_ZONE_NAME.to_owned()),
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(0.2, 0.2),
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
        brightness: None,
    }
}

fn surface(device_id: DeviceId) -> ScreenSurfaces {
    ScreenSurfaces::from([(
        LAYOUT_ID.to_owned(),
        ScreenSurface {
            device_id,
            connected: true,
            name: "Kraken".to_owned(),
            width: 480,
            height: 480,
        },
    )])
}

/// Register a display the way discovery does, tell the scene manager, and
/// optionally make its screen an output of the primary zone. Returns the
/// device and the id of its screen output.
async fn connect_screen(state: &AppState, name: &str, placed: bool) -> (DeviceId, String) {
    let device_id = DeviceId::new();
    let mut info = DeviceInfo {
        id: device_id,
        name: name.to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::new_static("lianli", "Lian Li"),
        model: None,
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("lianli", "usb", ConnectionType::Usb),
        segments: vec![SegmentInfo {
            name: DISPLAY_ZONE_NAME.to_owned(),
            led_count: 0,
            topology: DeviceTopologyHint::Display {
                width: 480,
                height: 480,
                circular: true,
                format: DisplayFrameFormat::Jpeg,
            },
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    };
    info.sync_display_capabilities();
    let _ = state.device_registry.add(info).await;
    state.domains.display.sync_connected_surfaces().await;
    let layout_id = state
        .scene_manager
        .snapshot()
        .await
        .screen_surfaces()
        .iter()
        .find(|(_, surface)| surface.device_id == device_id)
        .map(|(layout_id, _)| layout_id.clone())
        .expect("a registered display is a known screen");
    let screen = format!("{layout_id}-display");
    if placed {
        place_output(state, screen_output(&screen, &layout_id)).await;
    }
    (device_id, screen)
}

/// Give the primary zone an output, the way a client places a device.
async fn place_output(state: &AppState, output: Output) {
    let mut mutation = state.scene_manager.begin_mutation().await;
    let mut scene = mutation
        .scenes()
        .active_scene()
        .cloned()
        .expect("active scene");
    scene
        .primary_zone_mut()
        .expect("the default scene has a primary zone")
        .layout
        .zones
        .push(output);
    mutation.update_scene(scene).expect("place the output");
    commit_scene(&state.domains.scene, mutation)
        .await
        .expect("the placement should commit");
}

/// A stored display zone, the spelling builds before screen layers wrote.
fn legacy_zone(device_id: DeviceId, effect_id: EffectId) -> Zone {
    let mut zone = overlay_zone(device_id, effect_id);
    zone.display_target = Some(DisplayFaceTarget {
        device_id,
        blend_mode: BlendMode::Screen,
        opacity: 0.5,
    });
    zone
}

async fn store_zone(state: &AppState, zone: Zone) {
    let mut mutation = state.scene_manager.begin_mutation().await;
    let mut scene = mutation
        .scenes()
        .active_scene()
        .cloned()
        .expect("active scene");
    scene.zones.push(zone);
    mutation.update_scene(scene).expect("store the zone");
    commit_scene(&state.domains.scene, mutation)
        .await
        .expect("the zone should commit");
}

async fn active_scene(state: &AppState) -> Scene {
    state
        .scene_manager
        .snapshot()
        .await
        .active_scene()
        .cloned()
        .expect("active scene")
}

/// The layers scoped to the screen in the zone that places it.
async fn screen_layers(state: &AppState, screen: &str) -> Vec<SceneLayer> {
    active_scene(state)
        .await
        .primary_zone()
        .map(|zone| {
            zone.layers
                .iter()
                .filter(|layer| {
                    layer
                        .scope
                        .as_ref()
                        .is_some_and(|scope| scope.contains(screen))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn setting_a_face_writes_one_screen_layer_and_answers_with_the_view() {
    let (state, _tempdir) = isolated_state();
    let first = face_effect("clock");
    let second = face_effect("weather");
    insert_effect(&state, &first).await;
    insert_effect(&state, &second).await;
    let (device_id, screen) = connect_screen(&state, "Kraken", true).await;
    let mut events = state.event_bus.subscribe_all();

    let created = set_display_face(
        &state.domains.effects,
        assign_command(&state, device_id, &first).await,
    )
    .await
    .expect("the face should be assigned");

    assert_eq!(created.change, ZoneChangeKind::Created);
    assert_eq!(created.zone.role, ZoneRole::Display);
    assert_eq!(created.zone.effect_ids().next(), Some(first.id));
    assert!(
        created
            .zone
            .layers
            .iter()
            .all(|layer| layer.scope.is_none()),
        "a view lists its layers without their scope"
    );
    let scene = active_scene(&state).await;
    assert!(
        scene.display_zone_for(device_id).is_none(),
        "no display zone is written into the scene"
    );
    let primary = scene.primary_zone().expect("primary zone");
    assert_eq!(created.owner, Some((primary.id, screen.clone())));
    let layers = screen_layers(&state, &screen).await;
    assert_eq!(layers.len(), 1);
    assert_eq!(
        layers[0].scope,
        Some(LayerScope {
            outputs: vec![screen.clone()]
        })
    );
    let seen = drain_events(&mut events);
    assert!(seen.iter().any(|event| matches!(
        event,
        HypercolorEvent::ZoneChanged { zone_id, role: ZoneRole::Display, .. }
            if *zone_id == created.zone.id
    )));
    assert!(seen.iter().any(|event| matches!(
        event,
        HypercolorEvent::LayerStackChanged { zone_id, kind: LayerStackChangeKind::Updated, .. }
            if *zone_id == primary.id
    )));

    let updated = set_display_face(
        &state.domains.effects,
        assign_command(&state, device_id, &second).await,
    )
    .await
    .expect("the face should be replaced");

    assert_eq!(updated.change, ZoneChangeKind::Updated);
    assert_eq!(
        updated.zone.id, created.zone.id,
        "a screen's view id is stable"
    );
    let layers = screen_layers(&state, &screen).await;
    assert_eq!(layers.len(), 1, "the stack becomes the one new layer");
    assert!(matches!(
        layers[0].source,
        LayerSource::Effect { effect_id, .. } if effect_id == second.id
    ));
}

#[tokio::test]
async fn setting_a_face_for_a_screen_no_zone_places_stores_a_display_zone() {
    let (state, _tempdir) = isolated_state();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;
    let (device_id, screen) = connect_screen(&state, "Kraken", false).await;
    let zones_before = active_scene(&state).await.zones;
    let mut command = assign_command(&state, device_id, &effect).await;
    command.target.blend_mode = BlendMode::Screen;
    command.target.opacity = 0.4;

    let written = set_display_face(&state.domains.effects, command)
        .await
        .expect("the face should be assigned");

    assert_eq!(written.change, ZoneChangeKind::Created);
    assert_eq!(written.owner, None, "no zone places the screen");
    assert_eq!(
        written.zone.id,
        hypercolor_core::scene::screens::screen_zone_id(SceneId::DEFAULT, device_id),
        "the stored zone answers to the id the screen's empty view had"
    );
    let scene = active_scene(&state).await;
    let stored = scene
        .display_zone_for(device_id)
        .expect("the face is stored as a display zone");
    assert_eq!(stored.effect_ids().next(), Some(effect.id));
    let target = stored.display_target.as_ref().expect("target");
    assert_eq!(target.blend_mode, BlendMode::Screen);
    assert!((target.opacity - 0.4).abs() < f32::EPSILON);
    assert_eq!(
        scene
            .zones
            .iter()
            .filter(|zone| zone.role != ZoneRole::Display)
            .cloned()
            .collect::<Vec<_>>(),
        zones_before,
        "no zone gains an output or a layer on the screen's behalf"
    );
    assert!(screen_layers(&state, &screen).await.is_empty());

    // The second write updates the stored zone in place.
    let again = set_display_face(
        &state.domains.effects,
        assign_command(&state, device_id, &effect).await,
    )
    .await
    .expect("the face should be assigned again");
    assert_eq!(again.change, ZoneChangeKind::Updated);
    assert_eq!(again.zone.id, written.zone.id);
    assert_eq!(
        active_scene(&state)
            .await
            .zones
            .iter()
            .filter(|zone| zone.role == ZoneRole::Display)
            .count(),
        1
    );
}

#[tokio::test]
async fn setting_a_face_on_a_display_the_rig_does_not_know_is_refused() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;
    let revision = state.scene_manager.revision();

    let refused = set_display_face(
        &state.domains.effects,
        assign_command(&state, device_id, &effect).await,
    )
    .await
    .expect_err("a display that is not registered has no screen to place");

    assert!(matches!(
        refused,
        hypercolor_daemon::domain::DomainError::NotFound { .. }
    ));
    assert_eq!(state.scene_manager.revision(), revision);
}

#[tokio::test]
async fn setting_a_face_applies_the_composition_to_the_layer_and_the_view() {
    let (state, _tempdir) = isolated_state();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;
    let (device_id, screen) = connect_screen(&state, "Kraken", true).await;
    let mut command = assign_command(&state, device_id, &effect).await;
    command.target.blend_mode = BlendMode::Screen;
    command.target.opacity = 0.4;

    let written = set_display_face(&state.domains.effects, command)
        .await
        .expect("the face should be assigned");

    let target = written.zone.display_target.expect("view target");
    assert_eq!(target.blend_mode, BlendMode::Screen);
    assert!((target.opacity - 0.4).abs() < f32::EPSILON);
    let layer = &screen_layers(&state, &screen).await[0];
    assert_eq!(layer.blend, BlendMode::Screen);
    assert!((layer.opacity - 0.4).abs() < f32::EPSILON);
}

#[tokio::test]
async fn setting_a_face_replaces_a_stored_display_zone_in_the_same_commit() {
    let (state, _tempdir) = isolated_state();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;
    let (device_id, screen) = connect_screen(&state, "Kraken", true).await;
    // A first layer that is not opaque keeps the zone from converting.
    let mut zone = legacy_zone(device_id, effect.id);
    zone.layers[0].opacity = 0.5;
    store_zone(&state, zone).await;
    assert!(
        active_scene(&state)
            .await
            .display_zone_for(device_id)
            .is_some()
    );

    set_display_face(
        &state.domains.effects,
        assign_command(&state, device_id, &effect).await,
    )
    .await
    .expect("the face should be assigned");

    assert!(
        active_scene(&state)
            .await
            .display_zone_for(device_id)
            .is_none()
    );
    assert_eq!(screen_layers(&state, &screen).await.len(), 1);
}

#[tokio::test]
async fn setting_the_same_face_again_carries_its_input_bindings() {
    let (state, _tempdir) = isolated_state();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;
    let (device_id, screen) = connect_screen(&state, "Kraken", true).await;
    set_display_face(
        &state.domains.effects,
        assign_command(&state, device_id, &effect).await,
    )
    .await
    .expect("the face should be assigned");
    let layer = screen_layers(&state, &screen).await.remove(0);
    let mut bound = layer.clone();
    if let LayerSource::Effect {
        control_bindings, ..
    } = &mut bound.source
    {
        control_bindings.insert(
            "speed".to_owned(),
            hypercolor_types::effect::ControlBinding {
                sensor: "cpu".to_owned(),
                sensor_min: 0.0,
                sensor_max: 100.0,
                target_min: 0.0,
                target_max: 1.0,
                deadband: 0.0,
                smoothing: 0.0,
            },
        );
    }
    let mut mutation = state.scene_manager.begin_mutation().await;
    let owner = mutation
        .scenes()
        .active_scene()
        .and_then(Scene::primary_zone)
        .map(|zone| zone.id)
        .expect("primary zone");
    mutation
        .replace_layer(SceneId::DEFAULT, owner, layer.id, bound)
        .expect("bind the face");
    commit_scene(&state.domains.scene, mutation)
        .await
        .expect("binding should commit");

    set_display_face(
        &state.domains.effects,
        assign_command(&state, device_id, &effect).await,
    )
    .await
    .expect("the same face is assigned again");

    let LayerSource::Effect {
        control_bindings, ..
    } = &screen_layers(&state, &screen).await[0].source
    else {
        panic!("the face is an effect layer");
    };
    assert!(control_bindings.contains_key("speed"));
}

#[tokio::test]
async fn set_display_face_rejects_controls_inside_the_domain() {
    let (state, _tempdir) = isolated_state();
    let mut effect = face_effect("clock");
    effect.controls.push(numeric_control("accent"));
    insert_effect(&state, &effect).await;
    let (device_id, screen) = connect_screen(&state, "Kraken", true).await;
    let mut command = assign_command(&state, device_id, &effect).await;
    command
        .controls
        .insert("accent".to_owned(), ControlValue::Bool(true));
    let before = state.domains.scene.revision();

    let error = set_display_face(&state.domains.effects, command)
        .await
        .expect_err("a direct display-domain caller cannot bypass control admission");

    assert!(error.to_string().contains("control values were rejected"));
    assert_eq!(state.domains.scene.revision(), before);
    assert!(screen_layers(&state, &screen).await.is_empty());
}

#[tokio::test]
async fn clearing_a_face_removes_the_screens_own_layers_and_leaves_shared_ones() {
    let (state, _tempdir) = isolated_state();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;
    // Two screens in one zone, one layer scoped to both, one to the first.
    let (device_id, screen) = connect_screen(&state, "Kraken", true).await;
    let (_, other_screen) = connect_screen(&state, "Other", true).await;
    let mut mutation = state.scene_manager.begin_mutation().await;
    let owner = mutation
        .scenes()
        .active_scene()
        .and_then(Scene::primary_zone)
        .map(|zone| zone.id)
        .expect("primary zone");
    let scoped = |outputs: &[&str]| {
        let mut layer = SceneLayer::from_effect(
            SceneLayerId::new(),
            effect.id,
            HashMap::new(),
            HashMap::new(),
            None,
        );
        layer.scope = Some(LayerScope {
            outputs: outputs.iter().map(|id| (*id).to_owned()).collect(),
        });
        layer
    };
    let shared = scoped(&[screen.as_str(), other_screen.as_str()]);
    let shared_id = shared.id;
    for layer in [shared, scoped(&[screen.as_str()])] {
        mutation
            .insert_layer(SceneId::DEFAULT, owner, layer, None, None)
            .expect("screen layer");
    }
    commit_scene(&state.domains.scene, mutation)
        .await
        .expect("seed commit");

    let cleared = clear_display_face(&state.domains.scene, device_id)
        .await
        .expect("the stack should clear");

    let view = cleared.view.expect("a connected screen keeps its view");
    assert!(view.zone.layers.is_empty(), "the view is now empty");
    let scene = active_scene(&state).await;
    let primary = scene.primary_zone().expect("primary zone");
    assert_eq!(primary.layers.len(), 1, "the shared layer stays");
    assert_eq!(primary.layers[0].id, shared_id);
    assert_eq!(
        primary.layers[0].scope,
        Some(LayerScope {
            outputs: vec![other_screen.clone()]
        }),
        "this screen left the shared layer's scope"
    );
}

#[tokio::test]
async fn display_face_mutations_conflict_on_a_snapshot_locked_scene() {
    let (state, _tempdir) = isolated_state();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;
    let (device_id, _) = connect_screen(&state, "Kraken", true).await;
    let mut locked = named_scene("Locked");
    locked.mutation_mode = SceneMutationMode::Snapshot;
    let locked_id = locked.id;
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation.create_scene(locked).expect("create locked scene");
    mutation
        .activate(
            locked_id,
            None,
            hypercolor_types::event::SceneChangeReason::UserActivate,
        )
        .expect("activate locked scene");
    commit_scene(&state.domains.scene, mutation)
        .await
        .expect("activation should commit");

    let refused = set_display_face(
        &state.domains.effects,
        assign_command(&state, device_id, &effect).await,
    )
    .await;
    assert!(matches!(
        refused,
        Err(hypercolor_daemon::domain::DomainError::Conflict { .. })
    ));
    assert!(matches!(
        clear_display_face(&state.domains.scene, device_id).await,
        Err(hypercolor_daemon::domain::DomainError::Conflict { .. })
    ));
}

#[tokio::test]
async fn patching_composition_and_controls_reports_a_screen_with_no_stack() {
    let (state, _tempdir) = isolated_state();
    let (device_id, _) = connect_screen(&state, "Kraken", true).await;

    let composition = patch_display_composition(
        &state.domains.scene,
        PatchDisplayComposition {
            device_id,
            blend_mode: Some(BlendMode::Screen),
            opacity: None,
        },
    )
    .await
    .expect("the patch runs");
    assert!(composition.is_none());

    let controls = patch_display_face_controls(
        &state.domains.effects,
        PatchDisplayFaceControls {
            device_id,
            controls: HashMap::from([("speed".to_owned(), ControlValue::Float(0.5))]),
        },
    )
    .await
    .expect("the patch runs");
    assert!(controls.is_none());
}

#[tokio::test]
async fn patching_composition_and_controls_edits_the_screens_layer() {
    let (state, _tempdir) = isolated_state();
    let mut effect = face_effect("clock");
    effect.controls = vec![numeric_control("speed")];
    insert_effect(&state, &effect).await;
    let (device_id, screen) = connect_screen(&state, "Kraken", true).await;
    set_display_face(
        &state.domains.effects,
        assign_command(&state, device_id, &effect).await,
    )
    .await
    .expect("the face should be assigned");

    let composed = patch_display_composition(
        &state.domains.scene,
        PatchDisplayComposition {
            device_id,
            blend_mode: Some(BlendMode::Add),
            opacity: Some(0.25),
        },
    )
    .await
    .expect("the patch runs")
    .expect("the screen has a stack");
    let target = composed.zone.display_target.expect("view target");
    assert_eq!(target.blend_mode, BlendMode::Add);
    assert!((target.opacity - 0.25).abs() < f32::EPSILON);

    let patched = patch_display_face_controls(
        &state.domains.effects,
        PatchDisplayFaceControls {
            device_id,
            controls: HashMap::from([("speed".to_owned(), ControlValue::Float(0.75))]),
        },
    )
    .await
    .expect("the patch runs")
    .expect("the screen has one face");
    assert_eq!(
        zone_controls(&patched.written.zone).and_then(|controls| controls.get("speed")),
        Some(&ControlValue::Float(0.75))
    );
    let layer = &screen_layers(&state, &screen).await[0];
    assert_eq!(layer.blend, BlendMode::Add);
    assert!(matches!(
        &layer.source,
        LayerSource::Effect { controls, .. }
            if controls.get("speed") == Some(&ControlValue::Float(0.75))
    ));
}

#[tokio::test]
async fn syncing_screens_converts_a_stored_display_zone_once() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;
    place_output(&state, screen_output(SCREEN, LAYOUT_ID)).await;
    store_zone(&state, legacy_zone(device_id, effect.id)).await;
    // The screen's surface is not known yet, so nothing can convert.
    let stack = state
        .scene_manager
        .snapshot()
        .await
        .active_screen_stack(device_id)
        .is_some_and(|stack| matches!(stack, ScreenStack::Legacy(_)));
    assert!(stack);

    assert!(
        sync_screen_surfaces(&state.domains.scene, surface(device_id))
            .await
            .expect("the sync should commit")
    );

    let scene = active_scene(&state).await;
    assert!(scene.display_zone_for(device_id).is_none());
    let layers = screen_layers(&state, SCREEN).await;
    assert_eq!(layers.len(), 1);
    assert_eq!(
        layers[0].blend,
        BlendMode::Screen,
        "the target moved onto the layer"
    );
    assert!((layers[0].opacity - 0.5).abs() < f32::EPSILON);

    let revision = state.scene_manager.revision();
    assert!(
        !sync_screen_surfaces(&state.domains.scene, surface(device_id))
            .await
            .expect("the second sync runs"),
        "a second sync changes nothing"
    );
    assert_eq!(state.scene_manager.revision(), revision);
}

#[tokio::test]
async fn syncing_screens_saves_the_converted_default_scene() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;
    place_output(&state, screen_output(SCREEN, LAYOUT_ID)).await;
    store_zone(&state, legacy_zone(device_id, effect.id)).await;

    assert!(
        sync_screen_surfaces(&state.domains.scene, surface(device_id))
            .await
            .expect("the sync should commit")
    );

    hypercolor_daemon::persistence::AtomicFileWriter::new(&state.runtime_state_path)
        .expect("runtime state writer should open")
        .flush(std::time::Duration::from_secs(5))
        .expect("the runtime session write should land");
    let saved = hypercolor_daemon::runtime_state::load(&state.runtime_state_path)
        .expect("runtime state should load")
        .expect("the sync should write the runtime session");
    assert!(
        saved
            .default_scene_zones
            .iter()
            .all(|zone| zone.role != ZoneRole::Display),
        "the converted default scene is what a restart would load"
    );
    assert_eq!(
        saved
            .default_scene_zones
            .iter()
            .flat_map(|zone| zone.layers.iter())
            .filter(|layer| layer.scope.is_some())
            .count(),
        1
    );
}

#[tokio::test]
async fn syncing_screens_leaves_a_snapshot_locked_scene_as_stored() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;
    let mut locked = named_scene("Locked");
    locked.mutation_mode = SceneMutationMode::Snapshot;
    locked.zones = vec![legacy_zone(device_id, effect.id)];
    let locked_id = locked.id;
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .create_scene(locked.clone())
        .expect("create locked scene");
    mutation
        .activate(
            locked_id,
            None,
            hypercolor_types::event::SceneChangeReason::UserActivate,
        )
        .expect("activate locked scene");
    commit_scene(&state.domains.scene, mutation)
        .await
        .expect("activation should commit");

    sync_screen_surfaces(&state.domains.scene, surface(device_id))
        .await
        .expect("the sync should commit");

    assert_eq!(
        active_scene(&state).await.zones,
        locked.zones,
        "a locked scene keeps its display zone"
    );
}

#[tokio::test]
async fn prune_display_zones_removes_both_layers_for_a_deleted_device() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;
    store_zone(&state, legacy_zone(device_id, effect.id)).await;
    set_default_display_overlay(
        &state.domains.scene,
        device_id,
        overlay_zone(device_id, effect.id),
    )
    .await
    .expect("the overlay should install");

    let pruned = prune_display_zones_for_device(&state.domains.scene, device_id)
        .await
        .expect("pruning should succeed");

    assert_eq!(pruned.removed_zones.len(), 1);
    assert!(pruned.removed_default.is_some());
    assert!(pruned.commit.is_some());

    let manager = state.scene_manager.snapshot().await;
    assert!(manager.default_display_zone_for(device_id).is_none());
    assert!(
        manager
            .active_scene()
            .and_then(|scene| scene.display_zone_for(device_id))
            .is_none()
    );
}

// ── Default layer ────────────────────────────────────────────────────────

/// The default overlay is materialized from the preference store on every
/// run, so it is runtime state and must not arm a scene-store write.
#[tokio::test]
async fn the_default_overlay_installs_and_retracts_without_persisting() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;

    let installed = set_default_display_overlay(
        &state.domains.scene,
        device_id,
        overlay_zone(device_id, effect.id),
    )
    .await
    .expect("the overlay should install")
    .expect("the overlay is readable back");
    assert_eq!(installed.effect_ids().next(), Some(effect.id));

    let removed = remove_default_display_overlay(&state.domains.scene, device_id)
        .await
        .expect("the overlay should retract")
        .expect("the retraction reports what it removed");
    assert_eq!(removed.id, installed.id);

    let manager = state.scene_manager.snapshot().await;
    assert!(manager.default_display_zone_for(device_id).is_none());
    assert!(
        manager
            .list()
            .iter()
            .all(|scene| scene.display_zone_for(device_id).is_none()),
        "the default layer never writes into a stored scene"
    );
}

// ── Read paths must not burn the scene revision ──────────────────────────

/// Re-materializing an unchanged preference used to commit, and a commit
/// mints a scene revision that invalidates every in-flight candidate.
/// `GET /api/v1/scene` walks this path once per stored
/// preference, so an unguarded install turned a plain read into the thing
/// that fails a user's zone edit.
#[tokio::test]
async fn reinstalling_an_unchanged_default_overlay_mints_no_revision() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;

    let zone = overlay_zone(device_id, effect.id);
    set_default_display_overlay(&state.domains.scene, device_id, zone.clone())
        .await
        .expect("the first install lands");
    let after_install = state.scene_manager.revision();

    for _ in 0..3 {
        let mut refresh = zone.clone();
        refresh.layers[0].id = SceneLayerId::new();
        let installed = set_default_display_overlay(&state.domains.scene, device_id, refresh)
            .await
            .expect("a repeat install succeeds")
            .expect("it reports the installed overlay");
        assert_eq!(installed.effect_ids().next(), Some(effect.id));
    }

    assert_eq!(
        state.scene_manager.revision(),
        after_install,
        "an unchanged overlay must not advance the scene revision"
    );
}

#[tokio::test]
async fn retracting_an_absent_default_overlay_mints_no_revision() {
    let (state, _tempdir) = isolated_state();
    let before = state.scene_manager.revision();

    assert!(
        remove_default_display_overlay(&state.domains.scene, DeviceId::new())
            .await
            .expect("retracting nothing succeeds")
            .is_none()
    );
    assert_eq!(state.scene_manager.revision(), before);
}

/// A changed preference still has to land, or the display keeps
/// rendering the face the user just replaced.
#[tokio::test]
async fn a_changed_default_overlay_still_commits() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let first = face_effect("clock");
    let second = face_effect("weather");
    insert_effect(&state, &first).await;
    insert_effect(&state, &second).await;

    set_default_display_overlay(
        &state.domains.scene,
        device_id,
        overlay_zone(device_id, first.id),
    )
    .await
    .expect("the first install lands");
    let after_first = state.scene_manager.revision();

    let installed = set_default_display_overlay(
        &state.domains.scene,
        device_id,
        overlay_zone(device_id, second.id),
    )
    .await
    .expect("the replacement lands")
    .expect("it reports the installed overlay");

    assert_eq!(installed.effect_ids().next(), Some(second.id));
    assert!(
        state.scene_manager.revision() > after_first,
        "a real change must advance the revision"
    );
}

#[tokio::test]
async fn pruning_a_device_that_owns_no_display_zones_mints_no_revision() {
    let (state, _tempdir) = isolated_state();
    let before = state.scene_manager.revision();

    let pruned = prune_display_zones_for_device(&state.domains.scene, DeviceId::new())
        .await
        .expect("pruning nothing succeeds");

    assert!(pruned.removed_zones.is_empty());
    assert!(pruned.removed_default.is_none());
    assert!(pruned.commit.is_none());
    assert_eq!(state.scene_manager.revision(), before);
}
