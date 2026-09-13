//! Exact native lighting targets and evidence from the owning persistence path.

use hypercolor_color::{LinearRgba, Rgb};
use hypercolor_core::scene::default_primary_zone;
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::domain::DomainError;
use hypercolor_daemon::domain::commit::CommitDurability;
use hypercolor_daemon::domain::context::RuntimeSessionSaveOutcome;
use hypercolor_daemon::domain::runtime_zone::{RuntimeZoneTarget, set_color};
use hypercolor_daemon::domain::scene::commit_scene;
#[cfg(feature = "persistence-test-hooks")]
use hypercolor_daemon::persistence::AtomicFileWriter;
use hypercolor_daemon::persistence::AtomicWriteOutcome;
use hypercolor_types::event::{HypercolorEvent, SceneChangeReason};
use hypercolor_types::layer::{
    BlendMode, LayerAdjust, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::{Scene, SceneId, SceneKind, SceneMutationMode, ZoneId, ZoneRole};

fn layer() -> SceneLayer {
    SceneLayer {
        id: SceneLayerId::new(),
        name: Some("old effect state".to_owned()),
        source: LayerSource::ColorFill {
            rgba: [0.25, 0.5, 0.75, 0.4],
        },
        blend: BlendMode::Replace,
        opacity: 0.6,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }
}

async fn fixture() -> (AppState, tempfile::TempDir, Scene, RuntimeZoneTarget) {
    let dir = tempfile::tempdir().expect("fixture directory");
    let state = AppState::new_with_data_dir(dir.path().join("data"));
    let mut scene = state
        .scene_manager
        .snapshot()
        .await
        .active_scene()
        .expect("default scene")
        .clone();
    scene.id = SceneId::new();
    scene.kind = SceneKind::Named;
    scene.mutation_mode = SceneMutationMode::Live;
    let mut primary =
        default_primary_zone(state.spatial_engine.snapshot().layout().as_ref().clone());
    "retain primary metadata".clone_into(&mut primary.name);
    primary.description = Some("do not replace this zone".to_owned());
    primary.brightness = 0.37;
    primary.enabled = false;
    primary.layers = vec![layer(), layer()];
    primary
        .layout
        .zones
        .push(hypercolor_types::spatial::Output {
            id: "retained-membership".to_owned(),
            name: "desk strip".to_owned(),
            device_id: "mock:desk".into(),
            zone_name: Some("left".to_owned()),
            position: hypercolor_types::spatial::NormalizedPosition::new(0.3, 0.7),
            size: hypercolor_types::spatial::NormalizedPosition::new(0.6, 0.2),
            rotation: 27.0,
            scale: 0.8,
            display_order: 3,
            orientation: None,
            topology: hypercolor_types::spatial::LedTopology::Strip {
                count: 5,
                direction: hypercolor_types::spatial::StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            attachment: None,
            brightness: Some(0.65),
        });
    let mut independent = primary.clone();
    independent.id = ZoneId::new();
    independent.layout.zones.clear();
    independent.role = ZoneRole::Custom;
    independent.layers = vec![layer()];
    scene.zones = vec![primary, independent];
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation.create_scene(scene.clone()).expect("seed scene");
    mutation
        .activate(scene.id, None, SceneChangeReason::UserActivate)
        .expect("activate fixture");
    let commit = commit_scene(&state.domains.scene, mutation)
        .await
        .expect("seed commit");
    let target = RuntimeZoneTarget {
        scene_id: scene.id,
        zone_id: scene.zones[0].id,
        revision: commit.revision(),
    };
    (state, dir, scene, target)
}

#[tokio::test]
async fn color_changes_only_layers_and_preserves_paused_output_and_primary_metadata() {
    let (state, _dir, before, target) = fixture().await;
    state
        .output_power
        .set_global_brightness(&state.event_bus, 0.42)
        .await
        .expect("brightness");
    state
        .output_power
        .set_manual_pause(&state.event_bus, true, [1, 2, 3])
        .await;
    let power = state.output_power.snapshot();
    let mut events = state.event_bus.subscribe_all();
    let color = Rgb::new(34, 129, 207);
    let result = set_color(&state.domains.scene, target, color)
        .await
        .expect("exact color");
    assert_eq!(result.commit.durability(), CommitDurability::Written);
    assert!(result.has_written_layers());
    assert!(matches!(
        result.runtime_session,
        RuntimeSessionSaveOutcome::Attempted {
            scene_store: Ok(Some(AtomicWriteOutcome::Written)),
            snapshot: Ok(AtomicWriteOutcome::Written),
            ..
        }
    ));
    let mut after = state
        .scene_manager
        .snapshot()
        .await
        .get(&target.scene_id)
        .expect("scene retained")
        .clone();
    assert_eq!(result.zone, after.zones[0]);
    assert_eq!(after.zones[0].layers.len(), 1);
    let new_layer = &after.zones[0].layers[0];
    assert!(
        before.zones[0]
            .layers
            .iter()
            .all(|old| old.id != new_layer.id)
    );
    let LayerSource::ColorFill { rgba } = new_layer.source else {
        panic!("constant color")
    };
    assert_eq!(
        LinearRgba::new(rgba[0], rgba[1], rgba[2], rgba[3]).to_encoded(),
        color.to_rgba()
    );
    assert!(rgba[1] < 0.3, "encoded middle gray must be linearized");
    after.zones[0].layers = before.zones[0].layers.clone();
    after.zones[0].layers_version = before.zones[0].layers_version;
    assert_eq!(
        after, before,
        "all other scene and zone fields are preserved"
    );
    assert_eq!(state.output_power.snapshot(), power);
    let mut layer_events = 0;
    let mut zone_events = 0;
    while let Ok(event) = events.try_recv() {
        match event.event {
            HypercolorEvent::LayerStackChanged { .. } => layer_events += 1,
            HypercolorEvent::ZoneChanged { .. } => zone_events += 1,
            other => panic!("unexpected side-effect event {other:?}"),
        }
    }
    assert_eq!((layer_events, zone_events), (1, 1));
}

#[tokio::test]
async fn wrong_scene_stale_revision_and_missing_zone_leave_the_scene_unchanged() {
    let (state, _dir, before, target) = fixture().await;
    assert!(matches!(
        set_color(
            &state.domains.scene,
            RuntimeZoneTarget {
                scene_id: SceneId::new(),
                ..target
            },
            Rgb::WHITE
        )
        .await,
        Err(DomainError::Conflict { .. })
    ));
    assert!(matches!(
        set_color(
            &state.domains.scene,
            RuntimeZoneTarget {
                revision: target.revision + 1,
                ..target
            },
            Rgb::WHITE
        )
        .await,
        Err(DomainError::PreconditionFailed { .. })
    ));
    assert!(matches!(
        set_color(
            &state.domains.scene,
            RuntimeZoneTarget {
                zone_id: ZoneId::new(),
                ..target
            },
            Rgb::WHITE
        )
        .await,
        Err(DomainError::NotFound { .. })
    ));
    assert_eq!(
        state.scene_manager.snapshot().await.get(&target.scene_id),
        Some(&before)
    );
    assert_eq!(state.scene_manager.revision(), target.revision);
}

#[tokio::test]
async fn display_and_snapshot_guards_are_checked_on_the_owned_candidate() {
    let (state, _dir, before, target) = fixture().await;
    let mut mutation = state.scene_manager.begin_mutation().await;
    let mut changed = before.clone();
    changed.mutation_mode = SceneMutationMode::Snapshot;
    mutation.update_scene(changed).expect("snapshot fixture");
    let commit = commit_scene(&state.domains.scene, mutation)
        .await
        .expect("snapshot commit");
    assert!(matches!(
        set_color(
            &state.domains.scene,
            RuntimeZoneTarget {
                revision: commit.revision(),
                ..target
            },
            Rgb::WHITE
        )
        .await,
        Err(DomainError::Conflict { .. })
    ));
    let mut changed = before;
    changed.zones[0].role = ZoneRole::Display;
    changed.zones[0].display_target = Some(hypercolor_types::scene::DisplayFaceTarget {
        device_id: hypercolor_types::device::DeviceId::new(),
        blend_mode: BlendMode::Replace,
        opacity: 1.0,
    });
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .update_scene(changed.clone())
        .expect("display fixture");
    let commit = commit_scene(&state.domains.scene, mutation)
        .await
        .expect("display commit");
    assert!(matches!(
        set_color(
            &state.domains.scene,
            RuntimeZoneTarget {
                revision: commit.revision(),
                ..target
            },
            Rgb::WHITE
        )
        .await,
        Err(DomainError::Validation { .. })
    ));
    assert_eq!(
        state.scene_manager.snapshot().await.get(&target.scene_id),
        Some(&changed)
    );
}

#[tokio::test]
async fn a_concurrent_scene_activation_rejects_an_already_prepared_color_candidate() {
    let (state, _dir, before, target) = fixture().await;
    let mut color = state.scene_manager.begin_mutation().await;
    color
        .set_runtime_zone_color(target, Rgb::WHITE)
        .expect("prepare before activation");
    let mut activation = state.scene_manager.begin_mutation().await;
    activation
        .activate(SceneId::DEFAULT, None, SceneChangeReason::UserActivate)
        .expect("activate default");
    commit_scene(&state.domains.scene, activation)
        .await
        .expect("competing commit");
    assert!(matches!(
        commit_scene(&state.domains.scene, color).await,
        Err(DomainError::Conflict { .. })
    ));
    assert_eq!(
        state.scene_manager.snapshot().await.get(&target.scene_id),
        Some(&before)
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn scene_reservation_failure_does_not_admit_the_color() {
    use hypercolor_daemon::persistence::set_injected_serialization_failures;
    let (state, _dir, before, target) = fixture().await;
    set_injected_serialization_failures(1);
    let result = set_color(&state.domains.scene, target, Rgb::WHITE).await;
    set_injected_serialization_failures(0);
    assert!(matches!(result, Err(DomainError::Internal(_))));
    assert_eq!(state.scene_manager.revision(), target.revision);
    assert_eq!(
        state.scene_manager.snapshot().await.get(&target.scene_id),
        Some(&before)
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn an_admitted_failed_write_remains_retrying_without_a_success_event() {
    let (state, _dir, _before, target) = fixture().await;
    let writer = AtomicFileWriter::new(&state.data_dir.join("scenes.json")).expect("scene writer");
    writer.set_injected_replace_failures(usize::MAX);
    let mut events = state.event_bus.subscribe_all();
    let result = set_color(&state.domains.scene, target, Rgb::WHITE).await;
    writer.set_injected_replace_failures(0);
    writer.kick();
    let result = result.expect("admitted result is retained");
    assert_eq!(result.commit.durability(), CommitDurability::Retrying);
    assert!(result.commit.retry_error().is_some());
    assert!(matches!(
        result.runtime_session,
        RuntimeSessionSaveOutcome::Attempted {
            scene_store: Err(_),
            ..
        }
    ));
    assert_eq!(state.scene_manager.revision(), result.commit.revision());
    assert!(
        events.try_recv().is_err(),
        "no unproven applied announcement"
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn runtime_write_failure_does_not_erase_original_scene_commit_evidence() {
    let (state, _dir, _before, target) = fixture().await;
    let writer = AtomicFileWriter::new(&state.runtime_state_path).expect("runtime writer");
    writer.set_injected_replace_failures(usize::MAX);
    let result = set_color(&state.domains.scene, target, Rgb::WHITE).await;
    writer.set_injected_replace_failures(0);
    writer.kick();
    let result = result.expect("scene admitted");
    assert_eq!(result.commit.durability(), CommitDurability::Written);
    assert!(matches!(
        result.runtime_session,
        RuntimeSessionSaveOutcome::Attempted {
            snapshot: Err(_),
            ..
        }
    ));
}

async fn default_target(state: &AppState, scene: &Scene) -> RuntimeZoneTarget {
    let mut default = state
        .scene_manager
        .snapshot()
        .await
        .get(&SceneId::DEFAULT)
        .expect("default")
        .clone();
    default.zones = scene.zones.clone();
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation.update_scene(default).expect("seed default zones");
    mutation
        .activate(SceneId::DEFAULT, None, SceneChangeReason::UserActivate)
        .expect("default active");
    let commit = commit_scene(&state.domains.scene, mutation)
        .await
        .expect("default fixture");
    RuntimeZoneTarget {
        scene_id: SceneId::DEFAULT,
        zone_id: scene.zones[0].id,
        revision: commit.revision(),
    }
}

#[tokio::test]
async fn default_scene_color_requires_the_written_runtime_payload() {
    let (state, _dir, before, _) = fixture().await;
    let target = default_target(&state, &before).await;
    let result = set_color(&state.domains.scene, target, Rgb::new(10, 20, 30))
        .await
        .expect("default color");
    assert!(result.has_written_layers());
    let disk = hypercolor_daemon::runtime_state::load(&state.runtime_state_path)
        .expect("read runtime")
        .expect("saved projection");
    assert_eq!(
        disk.default_scene_zones
            .iter()
            .find(|z| z.id == target.zone_id)
            .expect("persisted zone")
            .layers,
        result.zone.layers
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn default_scene_does_not_claim_durability_from_a_named_scene_store_write() {
    let (state, _dir, before, _) = fixture().await;
    let target = default_target(&state, &before).await;
    let writer = AtomicFileWriter::new(&state.runtime_state_path).expect("runtime writer");
    writer.set_injected_replace_failures(usize::MAX);
    let result = set_color(&state.domains.scene, target, Rgb::WHITE).await;
    writer.set_injected_replace_failures(0);
    writer.kick();
    let result = result.expect("default color admitted");
    assert_eq!(result.commit.durability(), CommitDurability::Written);
    assert!(!result.has_written_layers());
}

#[tokio::test]
async fn overwritten_default_layers_are_not_proven_by_a_later_written_projection() {
    use hypercolor_daemon::domain::runtime_zone::RuntimeZoneColorOutcome;
    let (state, _dir, before, _) = fixture().await;
    let target = default_target(&state, &before).await;
    let mut first = state.scene_manager.begin_mutation().await;
    let zone = first
        .set_runtime_zone_color(target, Rgb::WHITE)
        .expect("first color");
    let scene_kind = first.scenes().get(&target.scene_id).expect("default").kind;
    let commit = commit_scene(&state.domains.scene, first)
        .await
        .expect("first commit");
    let second = set_color(
        &state.domains.scene,
        RuntimeZoneTarget {
            revision: commit.revision(),
            ..target
        },
        Rgb::BLACK,
    )
    .await
    .expect("overwrite before first save");
    assert!(second.has_written_layers());
    let runtime_session = state.domains.runtime_session.save_with_outcome().await;
    let first = RuntimeZoneColorOutcome {
        target,
        scene_kind,
        zone,
        commit,
        runtime_session,
    };
    assert!(
        !first.has_written_layers(),
        "later Written contains different layer identities and content"
    );
}

#[tokio::test]
async fn nondefault_ephemeral_scene_has_no_named_store_durability_claim() {
    let (state, _dir, mut before, target) = fixture().await;
    before.kind = SceneKind::Ephemeral;
    before.id = SceneId::new();
    let target = RuntimeZoneTarget {
        scene_id: before.id,
        ..target
    };
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation.create_scene(before).expect("ephemeral fixture");
    mutation
        .activate(target.scene_id, None, SceneChangeReason::UserActivate)
        .expect("ephemeral active");
    let commit = commit_scene(&state.domains.scene, mutation)
        .await
        .expect("ephemeral commit");
    let result = set_color(
        &state.domains.scene,
        RuntimeZoneTarget {
            revision: commit.revision(),
            ..target
        },
        Rgb::WHITE,
    )
    .await
    .expect("volatile color still applies");
    assert_eq!(result.commit.durability(), CommitDurability::Written);
    assert!(
        !result.has_written_layers(),
        "ephemeral non-default content is not in either durable store"
    );
}

#[tokio::test]
async fn an_exact_but_superseded_runtime_payload_is_not_a_written_receipt() {
    use hypercolor_daemon::domain::runtime_zone::RuntimeZoneColorOutcome;
    use hypercolor_daemon::runtime_state;
    let (state, _dir, before, _) = fixture().await;
    let target = default_target(&state, &before).await;
    let mut mutation = state.scene_manager.begin_mutation().await;
    let zone = mutation
        .set_runtime_zone_color(target, Rgb::WHITE)
        .expect("candidate");
    let scene_kind = mutation
        .scenes()
        .get(&target.scene_id)
        .expect("default")
        .kind;
    let commit = commit_scene(&state.domains.scene, mutation)
        .await
        .expect("scene admission");
    let pending =
        runtime_state::reserve_save(&state.runtime_state_path).expect("older reservation");
    let projection = state.domains.runtime_session.snapshot().await;
    let newer = state.domains.runtime_session.save_with_outcome().await;
    assert!(matches!(
        newer,
        RuntimeSessionSaveOutcome::Attempted {
            snapshot: Ok(AtomicWriteOutcome::Written),
            ..
        }
    ));
    let snapshot = runtime_state::save_reserved(pending, &projection);
    assert!(matches!(snapshot, Ok(AtomicWriteOutcome::Superseded)));
    let result = RuntimeZoneColorOutcome {
        target,
        scene_kind,
        zone,
        commit,
        runtime_session: RuntimeSessionSaveOutcome::Attempted {
            scene_store: Ok(Some(AtomicWriteOutcome::Written)),
            projection,
            snapshot,
        },
    };
    assert!(
        !result.has_written_layers(),
        "even matching content needs the original writer's Written evidence"
    );
}

#[tokio::test]
async fn runtime_reservation_failure_is_returned_without_attempting_either_save() {
    use hypercolor_daemon::app_state::AppStateBuilder;
    let dir = tempfile::tempdir().expect("fixture directory");
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, b"not a directory").expect("block runtime destination");
    let state = AppStateBuilder::new(dir.path().join("data"))
        .with_runtime_state_path(blocked.join("runtime.json"))
        .build();
    assert!(matches!(
        state.domains.runtime_session.save_with_outcome().await,
        RuntimeSessionSaveOutcome::BeforeAdmission { .. }
    ));
}

#[tokio::test]
async fn core_stack_replacement_validates_the_whole_input_before_mutating() {
    use hypercolor_core::scene::LayerMutationError;
    let (state, _dir, before, target) = fixture().await;
    let mut manager = state.scene_manager.snapshot().await;
    let valid = layer();
    let mut invalid = layer();
    invalid.opacity = f32::NAN;
    assert!(matches!(
        manager.replace_zone_layer_stack(
            target.scene_id,
            target.zone_id,
            vec![valid.clone(), invalid]
        ),
        Err(LayerMutationError::InvalidLayer { .. })
    ));
    assert!(matches!(
        manager.replace_zone_layer_stack(
            target.scene_id,
            target.zone_id,
            vec![valid.clone(), valid.clone()]
        ),
        Err(LayerMutationError::DuplicateLayer { .. })
    ));
    assert!(matches!(
        manager.replace_zone_layer_stack(SceneId::new(), target.zone_id, vec![valid.clone()]),
        Err(LayerMutationError::SceneMissing)
    ));
    assert!(matches!(
        manager.replace_zone_layer_stack(target.scene_id, ZoneId::new(), vec![valid]),
        Err(LayerMutationError::ZoneMissing)
    ));
    assert_eq!(manager.get(&target.scene_id), Some(&before));
    let (zone, _) = manager
        .replace_zone_layer_stack(target.scene_id, target.zone_id, Vec::new())
        .expect("explicit empty stack");
    assert!(zone.layers.is_empty());
    let mut restored = manager.get(&target.scene_id).expect("scene").clone();
    restored.zones[0].layers = before.zones[0].layers.clone();
    restored.zones[0].layers_version = before.zones[0].layers_version;
    assert_eq!(restored, before);
}
