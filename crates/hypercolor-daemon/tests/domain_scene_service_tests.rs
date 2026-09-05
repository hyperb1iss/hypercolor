//! Service-level tests for the scene domain layer (Spec 76 §2.3).
//!
//! These exercise `apply_effect` and `activate_scene` through the
//! service surface rather than through a transport, so what they pin is
//! the contract both REST and MCP now share: the compare-and-swap that
//! guards the short lock scopes, the durability receipt, and the
//! ordering the commit sequencer imposes on publication.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use hypercolor_core::asset::{AssetTypeHint, AssetUploadOptions};
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::effect::EffectEntry;
use hypercolor_core::scene::SceneManager;
use hypercolor_types::asset::AssetId;
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceState, DeviceTopologyHint,
    DisplayFrameFormat, SegmentInfo,
};
use hypercolor_types::effect::{
    EffectCategory, EffectId, EffectMetadata, EffectSource, EffectState,
};
use hypercolor_types::event::{HypercolorEvent, SceneLibraryChangeKind, Severity, ZoneChangeKind};
use hypercolor_types::identity::LayoutId;
use hypercolor_types::layer::{
    BlendMode, LayerAdjust, LayerSource, LayerTransform, MediaPlayback, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::{
    ColorInterpolation, DisplayFaceTarget, EasingFunction, Scene, SceneId, SceneKind,
    SceneMutationMode, ScenePriority, TransitionSpec, UnassignedBehavior, Zone, ZoneId, ZoneRole,
};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use uuid::Uuid;

use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::domain::commit::CommitDurability;
use hypercolor_daemon::domain::effect::{ApplyEffect, RequestedTransition, apply_effect};
use hypercolor_daemon::domain::scene::{
    ActivateScene, CreateScene, SceneService, SnapshotScene, activate_scene, commit_scene,
    create_scene, deactivate_scene, delete_scene, snapshot_scene,
};
use hypercolor_daemon::domain::scene_tree::{
    ClearScene, PatchLayerControls, clear_scene, patch_layer_controls, read_document,
};
use hypercolor_daemon::domain::{DomainError, DomainErrorDetails, MutationContext};
use hypercolor_daemon::scene_store;
use hypercolor_daemon::zone_layout_preview::ZoneLayoutPreviewOwner;

// ── Harness ──────────────────────────────────────────────────────────────

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    (Arc::new(AppState::new_with_data_dir(data_dir)), tempdir)
}

async fn seed_scene(state: &Arc<AppState>, scene: Scene) {
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .create_scene(scene)
        .expect("test scene should be created");
    commit_scene(&state.domains.scene, mutation)
        .await
        .expect("test scene should commit");
}

async fn seed_active_scene(state: &Arc<AppState>, scene: Scene) {
    let scene_id = scene.id;
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .create_scene(scene)
        .expect("test scene should be created");
    mutation
        .activate(
            scene_id,
            None,
            hypercolor_types::event::SceneChangeReason::UserActivate,
        )
        .expect("test scene should activate");
    commit_scene(&state.domains.scene, mutation)
        .await
        .expect("test scene should commit");
}

async fn await_with_layout_publication<T>(
    state: &AppState,
    workflow: impl std::future::Future<Output = T>,
) -> T {
    tokio::pin!(workflow);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            tokio::select! {
                result = &mut workflow => break result,
                () = tokio::time::sleep(Duration::from_millis(1)) => {
                    state
                        .layout_publication_test_executor()
                        .execute_next_layout_publication()
                        .await
                        .expect("layout publication should succeed");
                }
            }
        }
    })
    .await
    .expect("layout workflow should not deadlock")
}

#[tokio::test]
async fn scene_service_returns_owned_snapshots_and_lock_free_plans() {
    let service =
        SceneService::in_memory(SceneManager::with_default(), Arc::new(HypercolorBus::new()));
    let sibling = service.clone();
    let mut snapshot = service.snapshot().await;
    snapshot.deactivate_current();

    assert_eq!(
        sibling.snapshot().await.active_scene_id(),
        Some(&SceneId::DEFAULT)
    );
    let plan = sibling.plan_reader().load();
    assert_eq!(plan.generation, 0);
    assert_eq!(plan.active_scene_id, Some(SceneId::DEFAULT));
}

#[tokio::test]
async fn scene_service_clones_share_one_commit_revision() {
    let service =
        SceneService::in_memory(SceneManager::with_default(), Arc::new(HypercolorBus::new()));

    assert_eq!(service.revision(), service.clone().revision());
    assert_eq!(
        service.begin_mutation().await.base_revision(),
        service.revision()
    );
}

fn test_effect_metadata(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} html effect"),
        category: EffectCategory::Ambient,
        tags: vec!["test".to_owned()],
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

#[tokio::test]
async fn clearing_a_zone_retires_its_transient_layout_preview() {
    let (state, _tempdir) = isolated_state();
    let (zone_id, layout) = {
        let manager = state.scene_manager.snapshot().await;
        let zone = manager
            .active_scene()
            .and_then(Scene::primary_zone)
            .expect("default scene should have a primary zone");
        (zone.id, zone.layout.clone())
    };
    state
        .zone_layout_previews
        .set(
            ZoneLayoutPreviewOwner::new(),
            SceneId::DEFAULT,
            zone_id,
            layout,
        )
        .await;

    clear_scene(
        &state.domains.scene_tree,
        ClearScene {
            zone: Some(zone_id),
            expected_revision: None,
        },
    )
    .await
    .expect("zone clear should commit");

    assert!(
        state
            .zone_layout_previews
            .scene_overrides(SceneId::DEFAULT)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn control_patch_refuses_a_revision_resolved_before_a_scene_switch() {
    let (state, _tempdir) = isolated_state();
    let stale = read_document(&state.domains.scene_tree)
        .await
        .expect("default scene should be readable");
    let next_scene = named_scene("next");
    let next_scene_id = next_scene.id;
    seed_scene(&state, next_scene).await;
    activate_scene(
        &state.domains.scene_library,
        ActivateScene {
            scene_id: next_scene_id,
            transition_ms: None,
        },
    )
    .await
    .expect("next scene should activate");

    let error = patch_layer_controls(
        &state.domains.scene_tree,
        PatchLayerControls {
            zone_id: stale.zones[0].id,
            layer_id: SceneLayerId::new(),
            values: HashMap::from([("speed".to_owned(), ControlValue::Float(1.0))]),
            clear_bindings: Vec::new(),
            expected_revision: Some(stale.revision),
        },
        MutationContext::mcp(),
    )
    .await
    .expect_err("the selector snapshot must not target the replacement scene");

    assert!(
        matches!(
            error,
            DomainError::PreconditionFailed {
                expected,
                current,
                ..
            } if expected == stale.revision && current > expected
        ),
        "scene revision must fail before zone or layer lookup: {error:?}"
    );
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

fn preview_layout() -> SpatialLayout {
    SpatialLayout {
        id: "preview".to_owned(),
        name: "preview".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn display_device_info(device_id: DeviceId, width: u32, height: u32) -> DeviceInfo {
    DeviceInfo {
        id: device_id,
        name: "Panel".to_owned(),
        vendor: "test".to_owned(),
        family: DeviceFamily::new_static("test", "Test"),
        model: Some("Panel".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("test", "usb", ConnectionType::Usb),
        segments: vec![SegmentInfo {
            name: "Display".to_owned(),
            led_count: width.saturating_mul(height),
            topology: DeviceTopologyHint::Display {
                width,
                height,
                circular: false,
                format: DisplayFrameFormat::Jpeg,
            },
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: width.saturating_mul(height),
            supports_direct: true,
            supports_brightness: true,
            has_display: true,
            display_resolution: Some((width, height)),
            max_fps: 30,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

fn auto_layout_device_info(device_id: DeviceId) -> DeviceInfo {
    DeviceInfo {
        id: device_id,
        name: "Repair Target".to_owned(),
        vendor: "test".to_owned(),
        family: DeviceFamily::new_static("test", "Test"),
        model: Some("Repair Target".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("test", "usb", ConnectionType::Usb),
        segments: vec![SegmentInfo {
            name: "Main".to_owned(),
            led_count: 16,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: 16,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

fn stale_auto_layout(layout_device_id: &str) -> SpatialLayout {
    SpatialLayout {
        id: "default".to_owned(),
        name: "Default Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![Output {
            id: format!("auto-{}-main", layout_device_id.replace(':', "-")),
            name: "Stale Repair Target".to_owned(),
            device_id: layout_device_id.to_owned(),
            zone_name: Some("Main".to_owned()),
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(0.1, 0.1),
            rotation: 0.0,
            scale: 1.0,
            display_order: 0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 1,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: Some(SamplingMode::Bilinear),
            edge_behavior: Some(EdgeBehavior::Clamp),
            shape: None,
            shape_preset: None,
            attachment: None,
            brightness: None,
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn imported_display_zone(device_id: DeviceId) -> Zone {
    Zone {
        id: ZoneId::new(),
        name: "Imported display".to_owned(),
        description: None,
        layers: Vec::new(),
        layout: SpatialLayout {
            canvas_width: 1,
            canvas_height: 1,
            ..preview_layout()
        },
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: Some(DisplayFaceTarget::new(device_id)),
        role: ZoneRole::Display,
        controls_version: 0,
        layers_version: 0,
    }
}

async fn insert_lottie_asset(state: &AppState, name: &str, seed: u8) -> AssetId {
    let mut options = AssetUploadOptions::new(name);
    options.type_hint = Some(AssetTypeHint::Lottie);
    let upsert = state
        .asset_library
        .write()
        .await
        .add_bytes(
            format!(r#"{{"v":"5.7.4","fr":{seed},"layers":[]}}"#).as_bytes(),
            options,
        )
        .expect("lottie asset should upload");
    upsert.record.id
}

fn media_layer(asset_id: AssetId) -> SceneLayer {
    SceneLayer {
        id: SceneLayerId::new(),
        name: None,
        source: LayerSource::Media {
            asset_id,
            playback: MediaPlayback::default(),
        },
        blend: BlendMode::default(),
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }
}

async fn apply_command(state: &AppState, effect: &EffectMetadata) -> ApplyEffect {
    ApplyEffect {
        effect: state
            .domains
            .effects
            .metadata_for_mutation(effect.id)
            .await
            .expect("registered effect should resolve"),
        controls: HashMap::new(),
        preset_id: None,
        target_zone: None,
        expected_revision: None,
        transition: RequestedTransition::cut(),
        wake_output: true,
    }
}

// ── apply_effect ─────────────────────────────────────────────────────────

#[tokio::test]
async fn apply_effect_loads_the_primary_zone_and_commits_durably() {
    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    insert_effect(&state, &metadata).await;

    let applied = apply_effect(
        &state.domains.effects,
        apply_command(&state, &metadata).await,
        MutationContext::api(),
    )
    .await
    .expect("apply should succeed");

    assert_eq!(applied.effect.id, metadata.id.to_string());
    assert_eq!(applied.effect.name, "aurora");
    assert!(applied.previous_effect.is_none());
    assert_eq!(applied.transition.style, "cut");
    assert_eq!(applied.transition.duration_ms, 0);
    assert_eq!(applied.commit.durability(), CommitDurability::Written);
    assert!(applied.commit.retry_error().is_none());

    let manager = state.scene_manager.snapshot().await;
    let primary = manager
        .active_scene()
        .and_then(Scene::primary_zone)
        .expect("the active scene should have a primary zone");
    assert!(primary.has_effect(metadata.id));
}

#[tokio::test]
async fn apply_effect_reports_the_outgoing_effect_of_the_target_zone() {
    let (state, _tempdir) = isolated_state();
    let first = test_effect_metadata("aurora");
    let second = test_effect_metadata("nebula");
    insert_effect(&state, &first).await;
    insert_effect(&state, &second).await;

    apply_effect(
        &state.domains.effects,
        apply_command(&state, &first).await,
        MutationContext::api(),
    )
    .await
    .expect("first apply should succeed");
    let applied = apply_effect(
        &state.domains.effects,
        apply_command(&state, &second).await,
        MutationContext::api(),
    )
    .await
    .expect("second apply should succeed");

    let previous = applied
        .previous_effect
        .expect("the second apply should report the first effect");
    assert_eq!(previous.id, first.id.to_string());
    assert_eq!(applied.zone_change, ZoneChangeKind::Updated);
}

#[tokio::test]
async fn apply_effect_refuses_a_display_face() {
    let (state, _tempdir) = isolated_state();
    let mut metadata = test_effect_metadata("clock-face");
    metadata.category = EffectCategory::Display;
    insert_effect(&state, &metadata).await;

    let error = apply_effect(
        &state.domains.effects,
        apply_command(&state, &metadata).await,
        MutationContext::api(),
    )
    .await
    .expect_err("a display face should not reach the LED pipeline");
    assert!(
        matches!(error, DomainError::Validation { .. }),
        "expected Validation, got {error:?}"
    );
}

#[tokio::test]
async fn apply_effect_refuses_an_unimplemented_transition_from_either_transport() {
    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    insert_effect(&state, &metadata).await;

    // This is the divergence the unified surface closes: MCP used to
    // accept a duration here, echo it back, and never apply it.
    for trigger in [MutationContext::api(), MutationContext::mcp()] {
        let mut command = apply_command(&state, &metadata).await;
        command.transition = RequestedTransition::of_duration(500);
        let error = apply_effect(&state.domains.effects, command, trigger)
            .await
            .expect_err("a non-zero transition is not implemented");
        match error {
            DomainError::Validation { message, .. } => assert!(
                message.contains("not implemented yet"),
                "unexpected message: {message}"
            ),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    // A zero-duration cut is the one request the daemon can honor.
    let applied = apply_effect(
        &state.domains.effects,
        ApplyEffect {
            transition: RequestedTransition::of_duration(0),
            ..apply_command(&state, &metadata).await
        },
        MutationContext::mcp(),
    )
    .await
    .expect("an immediate cut applies");
    assert_eq!(applied.transition.duration_ms, 0);
}

#[tokio::test]
async fn apply_effect_conflicts_when_the_active_scene_is_snapshot_locked() {
    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    insert_effect(&state, &metadata).await;

    let mut scene = named_scene("frozen");
    scene.mutation_mode = SceneMutationMode::Snapshot;
    seed_active_scene(&state, scene).await;

    let error = apply_effect(
        &state.domains.effects,
        apply_command(&state, &metadata).await,
        MutationContext::mcp(),
    )
    .await
    .expect_err("a snapshot scene refuses runtime rewriting");
    assert!(
        matches!(error, DomainError::Conflict { .. }),
        "expected Conflict, got {error:?}"
    );
}

// ── activate_scene ───────────────────────────────────────────────────────

#[tokio::test]
async fn activate_scene_switches_the_current_scene_and_publishes_once() {
    let (state, _tempdir) = isolated_state();
    let scene = named_scene("evening");
    let scene_id = scene.id;
    seed_scene(&state, scene).await;
    let mut events = state.event_bus.subscribe_all();

    let activated = activate_scene(
        &state.domains.scene_library,
        ActivateScene {
            scene_id,
            transition_ms: None,
        },
    )
    .await
    .expect("activation should succeed");

    assert_eq!(activated.scene_id, scene_id);
    assert_eq!(activated.scene_name, "evening");
    assert_eq!(activated.commit.durability(), CommitDurability::Written);
    assert!(!activated.layout.applied);
    assert_eq!(activated.layout.layout_id, None);
    assert!(!activated.brightness.applied);

    let mut active_changed = 0;
    while let Ok(timestamped) = events.try_recv() {
        if let HypercolorEvent::ActiveSceneChanged { current, .. } = timestamped.event {
            assert_eq!(current, scene_id);
            active_changed += 1;
        }
    }
    assert_eq!(active_changed, 1, "exactly one activation announcement");

    let manager = state.scene_manager.snapshot().await;
    assert_eq!(manager.active_scene_id().copied(), Some(scene_id));
}

#[tokio::test]
async fn activation_persists_zones_after_auto_layout_convergence() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let info = auto_layout_device_info(device_id);
    let fingerprint = DeviceFingerprint::from_persisted("usb:repair-target".to_owned());
    state
        .device_registry
        .add_with_fingerprint(info.clone(), fingerprint.clone())
        .await;
    assert!(
        state
            .device_registry
            .set_state(&device_id, DeviceState::Connected)
            .await
    );
    let layout_device_id = {
        let discovery = state.driver_host().discovery_runtime();
        let mut lifecycle = discovery.lifecycle_manager.lock().await;
        let _ = lifecycle.on_discovered(device_id, &info, Some(&fingerprint));
        lifecycle
            .on_connected(device_id)
            .expect("repair target should enter the connected state");
        lifecycle
            .layout_device_id_for(device_id)
            .map(ToOwned::to_owned)
            .expect("repair target should have a canonical layout id")
    };
    let stale_layout = stale_auto_layout(&layout_device_id);
    await_with_layout_publication(
        &state,
        state
            .domains
            .layout
            .test_workflows()
            .publish(stale_layout.clone()),
    )
    .await
    .expect("stale layout should publish");

    let mut scene = named_scene("repair scene");
    scene.zones = vec![hypercolor_core::scene::default_primary_zone(stale_layout)];
    let scene_id = scene.id;
    seed_scene(&state, scene).await;

    await_with_layout_publication(
        &state,
        activate_scene(
            &state.domains.scene_library,
            ActivateScene {
                scene_id,
                transition_ms: None,
            },
        ),
    )
    .await
    .expect("repair scene should activate");

    let scene_store = scene_store::load(&state.data_dir.join("scenes.json"))
        .expect("durable scene store should load");
    let durable_scene = scene_store
        .list()
        .find(|scene| scene.id == scene_id)
        .expect("activated scene should remain durable");
    let durable_output = durable_scene
        .primary_zone()
        .and_then(|zone| zone.layout.zones.first())
        .expect("durable primary zone should contain the repaired output");
    assert_eq!(durable_output.name, info.name);
    assert_eq!(
        durable_output.topology,
        LedTopology::Strip {
            count: 16,
            direction: StripDirection::LeftToRight,
        }
    );

    let runtime = hypercolor_daemon::runtime_state::load(&state.runtime_state_path)
        .expect("runtime state should load")
        .expect("activation should write runtime state");
    assert_eq!(
        runtime.active_scene_id.as_deref(),
        Some(scene_id.to_string().as_str())
    );
}

#[tokio::test]
async fn activation_hydrates_only_existing_connected_display_zones() {
    let (state, _tempdir) = isolated_state();
    let assigned_device = DeviceId::new();
    let unassigned_device = DeviceId::new();
    for device_id in [assigned_device, unassigned_device] {
        state
            .device_registry
            .add(display_device_info(device_id, 320, 200))
            .await;
        assert!(
            state
                .device_registry
                .set_state(&device_id, DeviceState::Connected)
                .await
        );
    }

    let mut scene = named_scene("imported");
    scene.mutation_mode = SceneMutationMode::Snapshot;
    scene.zones.push(imported_display_zone(assigned_device));
    let scene_id = scene.id;
    seed_scene(&state, scene).await;

    activate_scene(
        &state.domains.scene_library,
        ActivateScene {
            scene_id,
            transition_ms: None,
        },
    )
    .await
    .expect("snapshot activation should hydrate derived geometry");

    let manager = state.scene_manager.snapshot().await;
    let active = manager.active_scene().expect("scene should be active");
    let assigned = active
        .display_zone_for(assigned_device)
        .expect("assigned display zone should remain");
    assert_eq!(assigned.layout.canvas_width, 320);
    assert_eq!(assigned.layout.canvas_height, 200);
    assert!(active.display_zone_for(unassigned_device).is_none());
}

#[tokio::test]
async fn reconnect_hydrates_an_active_snapshot_without_adding_surfaces() {
    let (state, _tempdir) = isolated_state();
    let assigned_device = DeviceId::new();
    let unassigned_device = DeviceId::new();
    for device_id in [assigned_device, unassigned_device] {
        state
            .device_registry
            .add(display_device_info(device_id, 320, 200))
            .await;
        assert!(
            state
                .device_registry
                .set_state(&device_id, DeviceState::Connected)
                .await
        );
    }

    let mut scene = named_scene("restored snapshot");
    scene.mutation_mode = SceneMutationMode::Snapshot;
    scene.zones.push(imported_display_zone(assigned_device));
    seed_active_scene(&state, scene).await;

    state.domains.display.sync_connected_surfaces().await;

    let manager = state.scene_manager.snapshot().await;
    let active = manager
        .active_scene()
        .expect("snapshot should remain active");
    let assigned = active
        .display_zone_for(assigned_device)
        .expect("the authored display surface should remain");
    assert_eq!(assigned.layout.canvas_width, 320);
    assert_eq!(assigned.layout.canvas_height, 200);
    assert!(
        active.display_zone_for(unassigned_device).is_none(),
        "reconnect must not add authored content to a snapshot"
    );
}

#[tokio::test]
async fn connected_displays_get_editable_surfaces_across_scene_switches() {
    let (state, _tempdir) = isolated_state();
    let connected_device = DeviceId::new();
    let known_device = DeviceId::new();
    for device_id in [connected_device, known_device] {
        state
            .device_registry
            .add(display_device_info(device_id, 320, 200))
            .await;
    }
    assert!(
        state
            .device_registry
            .set_state(&connected_device, DeviceState::Connected)
            .await
    );

    state.domains.display.sync_connected_surfaces().await;
    {
        let manager = state.scene_manager.snapshot().await;
        let active = manager
            .active_scene()
            .expect("default scene should be active");
        assert!(active.display_zone_for(connected_device).is_some());
        assert!(active.display_zone_for(known_device).is_none());
    }

    let effect_id = EffectId::new(Uuid::now_v7());
    let mut primary = hypercolor_core::scene::default_primary_zone(preview_layout());
    primary.layers.push(SceneLayer::from_effect(
        SceneLayerId::new(),
        effect_id,
        HashMap::new(),
        HashMap::new(),
        None,
    ));
    let primary_id = primary.id;
    let mut scene = named_scene("display scene");
    scene.zones.push(primary);
    let scene_id = scene.id;
    seed_scene(&state, scene).await;

    activate_scene(
        &state.domains.scene_library,
        ActivateScene {
            scene_id,
            transition_ms: None,
        },
    )
    .await
    .expect("scene activation should install connected display surfaces");

    let manager = state.scene_manager.snapshot().await;
    let active = manager
        .active_scene()
        .expect("display scene should be active");
    let screen = active
        .display_zone_for(connected_device)
        .expect("connected display should have an editable screen surface");
    assert_eq!(screen.role, ZoneRole::Display);
    assert_eq!(
        screen
            .display_target
            .as_ref()
            .map(|target| target.device_id),
        Some(connected_device)
    );
    assert!(screen.layers.is_empty());
    assert!(active.display_zone_for(known_device).is_none());
    let primary = active
        .zones
        .iter()
        .find(|zone| zone.id == primary_id)
        .expect("the authored primary zone should remain");
    assert!(primary.has_effect(effect_id));
}

#[tokio::test]
async fn activation_commits_before_layout_failure_and_still_applies_brightness() {
    let (state, _tempdir) = isolated_state();
    let mut scene = named_scene("evening");
    scene.layout_id = Some(LayoutId::new("missing-layout").expect("valid layout id"));
    scene.activation_brightness = Some(0.42);
    let scene_id = scene.id;
    seed_scene(&state, scene).await;
    let mut events = state.event_bus.subscribe_all();

    let activated = activate_scene(
        &state.domains.scene_library,
        ActivateScene {
            scene_id,
            transition_ms: None,
        },
    )
    .await
    .expect("post-commit side effects must not turn activation into an error");

    assert_eq!(
        state
            .scene_manager
            .snapshot()
            .await
            .active_scene_id()
            .copied(),
        Some(scene_id)
    );
    assert_eq!(
        activated.layout.layout_id.as_ref().map(LayoutId::as_str),
        Some("missing-layout")
    );
    assert!(!activated.layout.applied);
    assert!(
        activated
            .layout
            .message
            .as_deref()
            .is_some_and(|message| message.contains("not available"))
    );
    assert!(activated.brightness.applied);
    assert_eq!(
        hypercolor_daemon::domain::output::get_output(&state.domains.output).brightness,
        0.42
    );

    let mut warned = false;
    while let Ok(timestamped) = events.try_recv() {
        warned |= matches!(
            timestamped.event,
            HypercolorEvent::Error {
                ref code,
                severity: Severity::Warning,
                ..
            } if code == "scene_layout_unavailable"
        );
    }
    assert!(warned);
}

#[tokio::test]
async fn activation_applies_a_named_layout_without_reentering_its_guard() {
    let (state, _tempdir) = isolated_state();
    let created = state
        .domains
        .layout
        .create(hypercolor_types::api::layouts::CreateLayoutRequest {
            name: "Activation Layout".to_owned(),
            ..Default::default()
        })
        .await
        .expect("activation layout should create");
    let layout_id = LayoutId::new(created.id).expect("valid layout id");

    let mut scene = named_scene("layout scene");
    scene.layout_id = Some(layout_id.clone());
    let scene_id = scene.id;
    seed_scene(&state, scene).await;

    let activation = activate_scene(
        &state.domains.scene_library,
        ActivateScene {
            scene_id,
            transition_ms: None,
        },
    );
    tokio::pin!(activation);
    let activated = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            tokio::select! {
                result = &mut activation => break result,
                () = tokio::time::sleep(Duration::from_millis(1)) => {
                    state
                        .layout_publication_test_executor()
                        .execute_next_layout_publication()
                        .await
                        .expect("layout publication should succeed");
                }
            }
        }
    })
    .await
    .expect("layout activation should not deadlock")
    .expect("layout activation should succeed");

    assert!(activated.layout.applied);
    assert_eq!(activated.layout.layout_id, Some(layout_id.clone()));
    assert_eq!(
        state.spatial_engine.snapshot().layout().id,
        layout_id.as_str()
    );
}

#[tokio::test]
async fn activate_scene_honors_a_transition_override() {
    let (state, _tempdir) = isolated_state();
    let first = named_scene("evening");
    let second = named_scene("night");
    let (first_id, second_id) = (first.id, second.id);
    seed_scene(&state, first).await;
    seed_scene(&state, second).await;

    activate_scene(
        &state.domains.scene_library,
        ActivateScene {
            scene_id: first_id,
            transition_ms: None,
        },
    )
    .await
    .expect("first activation should succeed");

    // MCP passes a duration override; the service applies it rather
    // than echoing it, which is why it is a command field and not an
    // adapter detail.
    activate_scene(
        &state.domains.scene_library,
        ActivateScene {
            scene_id: second_id,
            transition_ms: Some(2_500),
        },
    )
    .await
    .expect("second activation should succeed");

    let manager = state.scene_manager.snapshot().await;
    let transition = manager
        .transition_plan()
        .expect("a non-zero override should start a transition");
    assert_eq!(transition.spec.duration_ms, 2_500);
}

#[tokio::test]
async fn activating_another_scene_retires_transient_layout_previews() {
    let (state, _tempdir) = isolated_state();
    let first = named_scene("evening");
    let second = named_scene("night");
    let (first_id, second_id) = (first.id, second.id);
    seed_scene(&state, first).await;
    seed_scene(&state, second).await;

    activate_scene(
        &state.domains.scene_library,
        ActivateScene {
            scene_id: first_id,
            transition_ms: None,
        },
    )
    .await
    .expect("first activation should succeed");

    let zone_id = ZoneId::new();
    state
        .zone_layout_previews
        .set(
            ZoneLayoutPreviewOwner::new(),
            first_id,
            zone_id,
            preview_layout(),
        )
        .await;

    activate_scene(
        &state.domains.scene_library,
        ActivateScene {
            scene_id: second_id,
            transition_ms: None,
        },
    )
    .await
    .expect("second activation should succeed");
    activate_scene(
        &state.domains.scene_library,
        ActivateScene {
            scene_id: first_id,
            transition_ms: None,
        },
    )
    .await
    .expect("first scene should reactivate");

    assert!(
        state
            .zone_layout_previews
            .scene_overrides(first_id)
            .await
            .is_empty(),
        "reactivating a scene must not revive transient previews from its previous activation"
    );
}

#[tokio::test]
async fn activate_scene_refuses_an_unknown_scene() {
    let (state, _tempdir) = isolated_state();
    let error = activate_scene(
        &state.domains.scene_library,
        ActivateScene {
            scene_id: SceneId::new(),
            transition_ms: None,
        },
    )
    .await
    .expect_err("an unknown scene should not activate");
    assert!(
        matches!(error, DomainError::NotFound { .. }),
        "expected NotFound, got {error:?}"
    );
}

/// MCP scene activation used to skip the soft admission REST performs,
/// so a media-heavy scene could activate over MCP without the
/// preemptive render downshift that protects frame budget. Driving the
/// MCP tool proves the check now runs on that path too.
#[tokio::test]
async fn mcp_scene_activation_applies_media_soft_admission() {
    let (state, _tempdir) = isolated_state();

    // Eight distinct Lottie producers cost 64ms of frame budget against
    // a 60ms soft cap. Lottie carries no hard cap, so this exercises the
    // soft path without tripping the hard one.
    let mut zone = {
        let spatial = state.spatial_engine.snapshot();
        hypercolor_core::scene::default_primary_zone(spatial.layout().as_ref().clone())
    };
    for index in 0..8u8 {
        let asset_id = insert_lottie_asset(&state, &format!("sparkle-{index}.json"), index).await;
        zone.layers.push(media_layer(asset_id));
    }

    let mut scene = named_scene("cinema");
    scene.zones = vec![zone];
    let scene_id = scene.id;
    seed_scene(&state, scene).await;

    let tier_before = state.render_loop.read().await.stats().tier;
    assert!(
        tier_before.downshift().is_some(),
        "the harness must start above the minimum tier for this to prove anything"
    );

    let result = hypercolor_daemon::mcp::tools::execute_tool_with_state(
        "activate_scene",
        &serde_json::json!({ "name": "cinema" }),
        &state,
    )
    .await
    .expect("activation should succeed");
    assert_eq!(result["activated"], true);

    let tier_after = state.render_loop.read().await.stats().tier;
    assert_eq!(
        Some(tier_after),
        tier_before.downshift(),
        "an over-budget scene downshifts the render tier on activation"
    );

    let manager = state.scene_manager.snapshot().await;
    assert_eq!(manager.active_scene_id().copied(), Some(scene_id));
}

// ── Commit contract ──────────────────────────────────────────────────────

#[tokio::test]
async fn a_stale_base_revision_is_rejected_before_admission() {
    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    insert_effect(&state, &metadata).await;
    let layout = {
        let spatial = state.spatial_engine.snapshot();
        spatial.layout().as_ref().clone()
    };

    // Two candidates from the same base revision. The first wins.
    let mut stale = state.scene_manager.begin_mutation().await;
    let mut winner = state.scene_manager.begin_mutation().await;
    assert_eq!(stale.base_revision(), winner.base_revision());

    winner
        .upsert_primary_zone(
            &metadata,
            HashMap::new(),
            None,
            layout.clone(),
            hypercolor_types::event::ChangeTrigger::System,
            None,
        )
        .expect("candidate mutation should apply");
    let commit = commit_scene(&state.domains.scene, winner)
        .await
        .expect("the first commit wins");
    assert_eq!(commit.durability(), CommitDurability::Written);

    stale
        .upsert_primary_zone(
            &metadata,
            HashMap::new(),
            None,
            layout,
            hypercolor_types::event::ChangeTrigger::System,
            None,
        )
        .expect("candidate mutation should apply");
    let error = commit_scene(&state.domains.scene, stale)
        .await
        .expect_err("a candidate built from a dead revision must not land");
    // Losing the commit compare-and-swap is a conflict, not a failed
    // caller precondition: no request carries a scene commit revision, so
    // a 412 here would be indistinguishable from the `If-Match` failures
    // the zone and layer routes serve.
    match error {
        DomainError::Conflict {
            details:
                Some(DomainErrorDetails::SceneCommitSuperseded {
                    expected_revision,
                    current_revision,
                }),
            ..
        } => {
            assert_eq!(expected_revision, commit.revision() - 1);
            assert_eq!(current_revision, commit.revision());
        }
        other => panic!("expected a superseded-commit Conflict, got {other:?}"),
    }
}

/// The temporary 2.3b bridge. Until every scene writer routes through
/// `commit_scene`, the revision cannot see a direct `scene_manager`
/// write, and the whole-manager install would discard it silently.
/// Divergence against the pristine base turns that into a retryable
/// conflict, and the direct write survives.
#[tokio::test]
async fn a_rejected_candidate_leaves_the_live_state_untouched() {
    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    let other = test_effect_metadata("nebula");
    insert_effect(&state, &metadata).await;
    insert_effect(&state, &other).await;
    let layout = {
        let spatial = state.spatial_engine.snapshot();
        spatial.layout().as_ref().clone()
    };

    let mut stale = state.scene_manager.begin_mutation().await;
    stale
        .upsert_primary_zone(
            &other,
            HashMap::new(),
            None,
            layout,
            hypercolor_types::event::ChangeTrigger::System,
            None,
        )
        .expect("candidate mutation should apply");

    apply_effect(
        &state.domains.effects,
        apply_command(&state, &metadata).await,
        MutationContext::api(),
    )
    .await
    .expect("the winning apply should succeed");

    let mut events = state.event_bus.subscribe_all();
    commit_scene(&state.domains.scene, stale)
        .await
        .expect_err("the stale candidate must be refused");

    // Nothing about the rejected candidate reached the world.
    let manager = state.scene_manager.snapshot().await;
    let primary = manager
        .active_scene()
        .and_then(Scene::primary_zone)
        .expect("the active scene should have a primary zone");
    assert!(primary.has_effect(metadata.id));
    assert!(
        events.try_recv().is_err(),
        "a rejected candidate publishes nothing"
    );
}

#[tokio::test]
async fn dropping_an_uncommitted_mutation_is_a_no_op() {
    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    insert_effect(&state, &metadata).await;
    let layout = {
        let spatial = state.spatial_engine.snapshot();
        spatial.layout().as_ref().clone()
    };

    let revision_before = {
        let mut abandoned = state.scene_manager.begin_mutation().await;
        abandoned
            .upsert_primary_zone(
                &metadata,
                HashMap::new(),
                None,
                layout,
                hypercolor_types::event::ChangeTrigger::System,
                None,
            )
            .expect("candidate mutation should apply");
        abandoned.base_revision()
    };

    // A later candidate still sees the original revision, so the drop
    // consumed no generation and moved no state.
    let next = state.scene_manager.begin_mutation().await;
    assert_eq!(next.base_revision(), revision_before);
    let manager = state.scene_manager.snapshot().await;
    assert!(
        manager
            .active_scene()
            .and_then(Scene::primary_zone)
            .and_then(|zone| zone.effect_ids().next())
            .is_none(),
        "an abandoned candidate never touches live state"
    );
}

/// A write that does not prove durable is not a rejection: the payload
/// stays the destination's newest admitted intent, the receipt says so,
/// and the announcement is withheld to match the failure the v1 wire
/// has always reported for one.
#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn a_non_durable_write_reports_retrying_and_publishes_nothing() {
    use hypercolor_daemon::persistence::AtomicFileWriter;

    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    insert_effect(&state, &metadata).await;

    // Destinations are keyed by canonical path, so a second writer for
    // the scenes file drives the same generation coordinator the store
    // uses.
    let scenes_path = state.data_dir.join("scenes.json");
    let writer = AtomicFileWriter::new(&scenes_path).expect("atomic writer");
    writer.set_injected_replace_failures(usize::MAX);

    let mut events = state.event_bus.subscribe_all();
    let applied = apply_effect(
        &state.domains.effects,
        apply_command(&state, &metadata).await,
        MutationContext::api(),
    )
    .await
    .expect("a non-durable write is not a rejection");

    assert_eq!(applied.commit.durability(), CommitDurability::Retrying);
    assert!(applied.commit.retry_error().is_some());
    assert!(
        events.try_recv().is_err(),
        "a commit whose write has not proven durable announces nothing"
    );

    writer.set_injected_replace_failures(0);
    writer.kick();
}

#[tokio::test]
async fn commit_generations_advance_the_scene_revision_in_order() {
    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    insert_effect(&state, &metadata).await;

    let mut generations = Vec::new();
    for _ in 0..3 {
        let applied = apply_effect(
            &state.domains.effects,
            apply_command(&state, &metadata).await,
            MutationContext::api(),
        )
        .await
        .expect("apply should succeed");
        generations.push(applied.commit.generation());
        assert_eq!(applied.commit.generation(), applied.commit.revision());
    }

    assert!(
        generations.windows(2).all(|pair| pair[0] < pair[1]),
        "generations must be strictly increasing: {generations:?}"
    );
}

// ── Scene library CRUD ───────────────────────────────────────────────────

fn create_command(name: &str) -> CreateScene {
    CreateScene {
        name: name.to_owned(),
        description: Some(format!("{name} scene")),
        enabled: None,
        mutation_mode: None,
        metadata: HashMap::new(),
    }
}

#[tokio::test]
async fn create_scene_seeds_a_default_zone_and_announces_the_scene() {
    let (state, _tempdir) = isolated_state();
    let mut events = state.event_bus.subscribe_all();

    let created = create_scene(&state.domains.scene_library, create_command("evening"))
        .await
        .expect("scene should be created");

    assert_eq!(created.scene.name, "evening");
    assert!(created.scene.enabled);
    assert_eq!(created.scene.mutation_mode, SceneMutationMode::Live);
    assert_eq!(
        created.scene.zones.len(),
        1,
        "every scene is born with a Default zone to select"
    );
    assert_eq!(created.scene.zones[0].role, ZoneRole::Primary);
    assert_eq!(created.commit.durability(), CommitDurability::Written);

    let manager = state.scene_manager.snapshot().await;
    assert!(manager.get(&created.scene.id).is_some());
    drop(manager);

    assert_eq!(
        library_events(&mut events),
        vec![(created.scene.id, SceneLibraryChangeKind::Created)]
    );
}

#[tokio::test]
async fn snapshot_scene_preserves_the_live_tree_and_captures_the_active_layout() {
    let (state, _tempdir) = isolated_state();
    let metadata = test_effect_metadata("aurora");
    insert_effect(&state, &metadata).await;
    apply_effect(
        &state.domains.effects,
        apply_command(&state, &metadata).await,
        MutationContext::api(),
    )
    .await
    .expect("the live scene should contain one effect layer");
    let active = state
        .scene_manager
        .snapshot()
        .await
        .active_scene()
        .cloned()
        .expect("the default scene should be active");
    let active_layout_id = state.spatial_engine.snapshot().layout().id.clone();

    let snapshot = snapshot_scene(
        &state.domains.scene_library,
        SnapshotScene {
            name: "Captured desk".to_owned(),
            description: Some("Runtime state".to_owned()),
        },
    )
    .await
    .expect("snapshot should commit");

    assert_ne!(snapshot.scene.id, active.id);
    assert_eq!(snapshot.scene.name, "Captured desk");
    assert_eq!(snapshot.scene.description.as_deref(), Some("Runtime state"));
    assert_eq!(snapshot.scene.kind, SceneKind::Named);
    assert_eq!(snapshot.scene.mutation_mode, SceneMutationMode::Snapshot);
    assert_eq!(snapshot.scene.zones, active.zones);
    assert_eq!(
        snapshot.scene.layout_id.as_ref().map(LayoutId::as_str),
        Some(active_layout_id.as_str())
    );
    assert_eq!(snapshot.scene.activation_brightness, None);
    assert_eq!(snapshot.commit.durability(), CommitDurability::Written);

    let manager = state.scene_manager.snapshot().await;
    assert_eq!(manager.active_scene_id(), Some(&active.id));
    assert_eq!(manager.get(&snapshot.scene.id), Some(&snapshot.scene));
}

/// MCP used to mint scenes with no zones and announce nothing, so a
/// scene created through the tool was unselectable in Studio and
/// invisible to every event-driven client until a refetch. One service
/// means one behavior.
#[tokio::test]
async fn create_scene_behaves_identically_for_both_transports() {
    let (state, _tempdir) = isolated_state();

    let via_api = create_scene(&state.domains.scene_library, create_command("api-made"))
        .await
        .expect("api create should succeed");
    let via_mcp = create_scene(&state.domains.scene_library, create_command("mcp-made"))
        .await
        .expect("mcp create should succeed");

    assert_eq!(via_api.scene.zones.len(), via_mcp.scene.zones.len());
    assert_eq!(via_api.scene.kind, via_mcp.scene.kind);
    assert_eq!(via_api.scene.priority, via_mcp.scene.priority);
    assert_eq!(
        via_api.scene.transition.duration_ms,
        via_mcp.scene.transition.duration_ms
    );
}

#[tokio::test]
async fn create_scene_carries_adapter_metadata_onto_the_scene() {
    let (state, _tempdir) = isolated_state();
    let mut command = create_command("triggered");
    command.metadata = HashMap::from([("trigger_type".to_owned(), "sunset".to_owned())]);
    command.mutation_mode = Some(SceneMutationMode::Snapshot);
    command.enabled = Some(false);

    let created = create_scene(&state.domains.scene_library, command)
        .await
        .expect("scene should be created");

    assert_eq!(
        created
            .scene
            .metadata
            .get("trigger_type")
            .map(String::as_str),
        Some("sunset")
    );
    assert_eq!(created.scene.mutation_mode, SceneMutationMode::Snapshot);
    assert!(!created.scene.enabled);
}

#[tokio::test]
async fn delete_scene_deactivates_it_and_announces_both_changes() {
    let (state, _tempdir) = isolated_state();
    let created = create_scene(&state.domains.scene_library, create_command("evening"))
        .await
        .expect("scene should be created");
    activate_scene(
        &state.domains.scene_library,
        ActivateScene {
            scene_id: created.scene.id,
            transition_ms: None,
        },
    )
    .await
    .expect("scene should activate");
    state
        .zone_layout_previews
        .set(
            ZoneLayoutPreviewOwner::new(),
            created.scene.id,
            ZoneId::new(),
            preview_layout(),
        )
        .await;
    let mut events = state.event_bus.subscribe_all();

    let deleted = delete_scene(&state.domains.scene_library, created.scene.id)
        .await
        .expect("scene should be deleted");

    assert_eq!(deleted.scene.id, created.scene.id);
    assert_eq!(deleted.previous_scene_id, Some(created.scene.id));
    assert_eq!(
        deleted.current_scene.as_ref().map(|scene| scene.id),
        Some(SceneId::DEFAULT),
        "deleting the active scene must fall back to Default"
    );

    let manager = state.scene_manager.snapshot().await;
    assert!(manager.get(&created.scene.id).is_none());
    drop(manager);
    assert!(
        state
            .zone_layout_previews
            .scene_overrides(created.scene.id)
            .await
            .is_empty(),
        "deleting a scene must retire its transient layout previews"
    );

    let seen = drain_events(&mut events);
    assert!(
        seen.iter()
            .any(|event| matches!(event, HypercolorEvent::ActiveSceneChanged { .. })),
        "the fallback to Default must be announced: {seen:?}"
    );
    assert!(
        seen.iter().any(|event| matches!(
            event,
            HypercolorEvent::SceneLibraryChanged {
                kind: SceneLibraryChangeKind::Deleted,
                ..
            }
        )),
        "the removal must be announced: {seen:?}"
    );
}

#[tokio::test]
async fn delete_scene_refuses_the_default_scene() {
    let (state, _tempdir) = isolated_state();
    let error = delete_scene(&state.domains.scene_library, SceneId::DEFAULT)
        .await
        .expect_err("the default scene is not deletable");
    assert!(
        matches!(error, DomainError::Conflict { .. }),
        "expected Conflict, got {error:?}"
    );
}

#[tokio::test]
async fn deactivate_scene_returns_to_default_and_reports_both_ends() {
    let (state, _tempdir) = isolated_state();
    let created = create_scene(&state.domains.scene_library, create_command("evening"))
        .await
        .expect("scene should be created");
    activate_scene(
        &state.domains.scene_library,
        ActivateScene {
            scene_id: created.scene.id,
            transition_ms: None,
        },
    )
    .await
    .expect("scene should activate");

    let deactivated = deactivate_scene(&state.domains.scene_library)
        .await
        .expect("deactivation should succeed");

    assert_eq!(
        deactivated.previous_scene.map(|scene| scene.id),
        Some(created.scene.id)
    );
    assert_eq!(
        deactivated.current_scene.map(|scene| scene.id),
        Some(SceneId::DEFAULT)
    );
    let manager = state.scene_manager.snapshot().await;
    assert_eq!(manager.active_scene_id().copied(), Some(SceneId::DEFAULT));
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

fn library_events(
    receiver: &mut tokio::sync::broadcast::Receiver<hypercolor_core::bus::TimestampedEvent>,
) -> Vec<(SceneId, SceneLibraryChangeKind)> {
    drain_events(receiver)
        .into_iter()
        .filter_map(|event| match event {
            HypercolorEvent::SceneLibraryChanged { scene_id, kind, .. } => Some((scene_id, kind)),
            _ => None,
        })
        .collect()
}

// ── Scene ownership fences ──────────────────────────────────────────────

/// Production callers operate through `SceneService`; none can take its
/// scene lock directly and bypass commit admission or ordered events.
#[test]
fn no_scene_writer_lives_outside_the_commit_path() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found: Vec<(String, usize)> = Vec::new();
    let mut pending = vec![src.clone()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("source directory reads") {
            let path = entry.expect("source entry reads").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let relative = path
                .strip_prefix(&src)
                .expect("paths are under src")
                .to_string_lossy()
                .replace('\\', "/");
            // `#[cfg(test)] mod tests` is the tail of every file that has
            // one, and a test that drives the manager directly is not a
            // production writer. `scene_transactions/tests.rs` is such a
            // module in its own file.
            if relative.ends_with("/tests.rs") || relative == "tests.rs" {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("source file reads");
            let count = scene_write_lock_sites(&source);
            if count > 0 {
                found.push((relative, count));
            }
        }
    }
    found.sort();

    assert_eq!(
        found,
        Vec::<(String, usize)>::new(),
        "scene callers must use SceneService intents rather than its lock"
    );
}

/// The owning service is the only production type allowed to hold the lock.
#[test]
fn the_scene_manager_handle_stays_where_it_is() {
    const EXPECTED: [&str; 1] = ["domain/scene.rs"];

    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    let mut pending = vec![src.clone()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("source directory reads") {
            let path = entry.expect("source entry reads").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            if std::fs::read_to_string(&path)
                .expect("source file reads")
                .contains("RwLock<SceneManager>")
            {
                found.push(
                    path.strip_prefix(&src)
                        .expect("paths are under src")
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    found.sort();

    let mut expected = EXPECTED.map(ToOwned::to_owned).to_vec();
    expected.sort();
    assert_eq!(
        found, expected,
        "only SceneService may own the scene-manager lock"
    );
}

/// Count `scene_manager.write()` acquisitions in one file's production
/// source, ignoring comments and tolerating the rustfmt line breaks that
/// split the receiver from the call.
///
/// Inline test modules are dropped before scanning production source.
fn scene_write_lock_sites(source: &str) -> usize {
    let lines = source.lines().collect::<Vec<_>>();
    let mut production: Vec<&str> = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let is_test_module = line.trim() == "#[cfg(test)]"
            && lines
                .get(index + 1)
                .is_some_and(|next| next.trim_start().starts_with("mod "));
        if !is_test_module {
            production.push(line);
            index += 1;
            continue;
        }

        let declaration = lines[index + 1];
        if declaration.trim_end().ends_with(';') {
            // `mod tests;` — the block lives in its own file.
            index += 2;
            continue;
        }
        let indent = &line[..line.len() - line.trim_start().len()];
        let closing = format!("{indent}}}");
        index += 2;
        while index < lines.len() && lines[index] != closing {
            index += 1;
        }
        index += 1;
    }

    let code = production
        .iter()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    let collapsed = code.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.matches("scene_manager.write()").count()
        + collapsed.matches("scene_manager .write()").count()
}
