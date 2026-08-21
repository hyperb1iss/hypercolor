//! Candidate-scoped targeting for live `/scene` mutations.

use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_core::scene::{ZoneMetaPatch, default_primary_group};
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::domain::DomainError;
use hypercolor_daemon::domain::layer::{insert_layer, remove_layer, reorder_layers};
use hypercolor_daemon::domain::zone::{
    CreateZone, DeleteZone, UpdateZone, create_zone, delete_zone, update_zone,
};
use hypercolor_types::layer::{
    LayerAdjust, LayerBlendMode, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::{
    ColorInterpolation, EasingFunction, Scene, SceneId, SceneKind, SceneMutationMode,
    ScenePriority, TransitionSpec, UnassignedBehavior, Zone, ZoneId, ZoneRole,
};

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    (Arc::new(AppState::new_with_data_dir(data_dir)), tempdir)
}

fn color_layer(id: SceneLayerId, channel: usize) -> SceneLayer {
    let mut rgba = [0.0, 0.0, 0.0, 1.0];
    rgba[channel] = 1.0;
    SceneLayer {
        id,
        name: None,
        source: LayerSource::ColorFill { rgba },
        blend: LayerBlendMode::Replace,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }
}

async fn scene_template(
    state: &AppState,
    id: SceneId,
    name: &str,
    zone_id: ZoneId,
    layer_ids: [SceneLayerId; 2],
    mutation_mode: SceneMutationMode,
) -> Scene {
    let layout = state.spatial_engine.snapshot().layout().as_ref().clone();
    let mut primary = default_primary_group(layout.clone());
    primary.name = format!("{name} primary");
    let zone = Zone {
        id: zone_id,
        name: format!("{name} shared zone"),
        description: None,
        layers: vec![color_layer(layer_ids[0], 0), color_layer(layer_ids[1], 1)],
        layout,
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: ZoneRole::Custom,
        controls_version: 0,
        layers_version: 0,
    };
    Scene {
        id,
        name: name.to_owned(),
        description: None,
        zones: vec![primary, zone],
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
        mutation_mode,
    }
}

fn rename_patch(name: &str) -> ZoneMetaPatch {
    ZoneMetaPatch {
        name: Some(name.to_owned()),
        description: None,
        color: None,
        brightness: None,
        enabled: None,
        make_primary: None,
    }
}

fn assert_conflict<T: std::fmt::Debug>(result: Result<T, DomainError>) {
    assert!(
        matches!(result, Err(DomainError::Conflict { .. })),
        "snapshot mutation should return conflict, got {result:?}"
    );
}

#[tokio::test]
async fn active_targets_follow_the_candidate_scene_for_every_deferred_service() {
    let (state, _tempdir) = isolated_state();
    let scene_a_id = SceneId::new();
    let scene_b_id = SceneId::new();
    let shared_zone_id = ZoneId::new();
    let shared_layer_ids = [SceneLayerId::new(), SceneLayerId::new()];
    let scene_a = scene_template(
        &state,
        scene_a_id,
        "A",
        shared_zone_id,
        shared_layer_ids,
        SceneMutationMode::Live,
    )
    .await;
    let scene_b = scene_template(
        &state,
        scene_b_id,
        "B",
        shared_zone_id,
        shared_layer_ids,
        SceneMutationMode::Live,
    )
    .await;
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .create_scene(scene_a.clone())
        .expect("scene A should create");
    mutation
        .create_scene(scene_b)
        .expect("scene B should create");
    mutation
        .activate(
            scene_a_id,
            None,
            hypercolor_types::event::SceneChangeReason::UserActivate,
        )
        .expect("scene A should activate");
    hypercolor_daemon::domain::scene::commit_scene(&state.scene, mutation)
        .await
        .expect("scene A should commit");

    let create = CreateZone {
        name: "candidate zone".to_owned(),
        color: None,
        fallback_canvas: (640, 480),
        expected_revision: None,
    };
    let update = UpdateZone {
        zone_id: shared_zone_id,
        patch: rename_patch("candidate zone renamed"),
        expected_revision: None,
    };
    let delete = DeleteZone {
        zone_id: shared_zone_id,
        expected_revision: None,
    };
    let inserted_layer_id = SceneLayerId::new();
    let inserted_layer = color_layer(inserted_layer_id, 2);

    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .activate(
            scene_b_id,
            None,
            hypercolor_types::event::SceneChangeReason::UserActivate,
        )
        .expect("scene B should activate before candidates are opened");
    hypercolor_daemon::domain::scene::commit_scene(&state.scene, mutation)
        .await
        .expect("scene B should commit");

    let created = create_zone(&state.scene, create)
        .await
        .expect("zone creation should follow scene B");
    let updated = update_zone(&state.scene, update)
        .await
        .expect("zone update should follow scene B");
    assert_eq!(updated.zone.name, "candidate zone renamed");

    let inserted = insert_layer(&state.scene, shared_zone_id, inserted_layer, None, None)
        .await
        .expect("layer insertion should follow scene B")
        .expect("layer insertion should be admitted");
    assert!(
        inserted
            .zone()
            .layers
            .iter()
            .any(|layer| layer.id == inserted_layer_id)
    );

    let reordered = reorder_layers(
        &state.scene,
        shared_zone_id,
        vec![inserted_layer_id, shared_layer_ids[1], shared_layer_ids[0]],
        None,
    )
    .await
    .expect("layer reorder should follow scene B")
    .expect("layer reorder should be admitted");
    assert_eq!(
        reordered
            .zone()
            .layers
            .iter()
            .map(|layer| layer.id)
            .collect::<Vec<_>>(),
        vec![inserted_layer_id, shared_layer_ids[1], shared_layer_ids[0]]
    );

    let removed = remove_layer(&state.scene, shared_zone_id, shared_layer_ids[1], None)
        .await
        .expect("layer deletion should follow scene B")
        .expect("layer deletion should be admitted");
    assert!(
        removed
            .zone()
            .layers
            .iter()
            .all(|layer| layer.id != shared_layer_ids[1])
    );

    delete_zone(&state.scene, delete)
        .await
        .expect("zone deletion should follow scene B");

    let manager = state.scene_manager.snapshot().await;
    assert_eq!(
        manager.get(&scene_a_id),
        Some(&scene_a),
        "commands created while A was active must never mutate stale A"
    );
    let scene_b = manager.get(&scene_b_id).expect("scene B should remain");
    assert!(scene_b.zones.iter().any(|zone| zone.id == created.zone.id));
    assert!(
        scene_b.zones.iter().all(|zone| zone.id != shared_zone_id),
        "the shared custom zone should be deleted only from active B"
    );
}

#[tokio::test]
async fn active_targets_refuse_every_deferred_service_in_snapshot_mode() {
    let (state, _tempdir) = isolated_state();
    let scene_id = SceneId::new();
    let zone_id = ZoneId::new();
    let layer_ids = [SceneLayerId::new(), SceneLayerId::new()];
    let scene = scene_template(
        &state,
        scene_id,
        "snapshot",
        zone_id,
        layer_ids,
        SceneMutationMode::Snapshot,
    )
    .await;
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .create_scene(scene.clone())
        .expect("snapshot scene should create");
    mutation
        .activate(
            scene_id,
            None,
            hypercolor_types::event::SceneChangeReason::UserActivate,
        )
        .expect("snapshot scene should activate");
    hypercolor_daemon::domain::scene::commit_scene(&state.scene, mutation)
        .await
        .expect("snapshot scene should commit");
    let revision = state.scene_manager.revision();

    assert_conflict(
        create_zone(
            &state.scene,
            CreateZone {
                name: "blocked".to_owned(),
                color: None,
                fallback_canvas: (640, 480),
                expected_revision: Some(revision),
            },
        )
        .await,
    );
    assert_conflict(
        update_zone(
            &state.scene,
            UpdateZone {
                zone_id,
                patch: rename_patch("blocked"),
                expected_revision: Some(revision),
            },
        )
        .await,
    );
    assert_conflict(
        delete_zone(
            &state.scene,
            DeleteZone {
                zone_id,
                expected_revision: Some(revision),
            },
        )
        .await,
    );
    assert_conflict(
        insert_layer(
            &state.scene,
            zone_id,
            color_layer(SceneLayerId::new(), 2),
            None,
            Some(revision),
        )
        .await,
    );
    assert_conflict(
        reorder_layers(
            &state.scene,
            zone_id,
            layer_ids.into_iter().rev().collect(),
            Some(revision),
        )
        .await,
    );
    assert_conflict(remove_layer(&state.scene, zone_id, layer_ids[0], Some(revision)).await);

    let manager = state.scene_manager.snapshot().await;
    assert_eq!(manager.get(&scene_id), Some(&scene));
    assert_eq!(state.scene_manager.revision(), revision);
}
