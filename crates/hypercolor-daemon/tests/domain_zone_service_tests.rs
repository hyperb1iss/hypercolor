//! Service-level tests for the zone domain layer (Spec 76 §2.2, §2.3).
//!
//! These pin the canonical zone identity and metadata services that remain
//! after the stored-scene fine-grained API is retired.

use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_core::scene::ZoneMetaPatch;
use hypercolor_types::event::{HypercolorEvent, ZoneChangeKind};
use hypercolor_types::scene::{
    ColorInterpolation, EasingFunction, Scene, SceneId, SceneKind, SceneMutationMode,
    ScenePriority, SceneScope, TransitionSpec, UnassignedBehavior, ZoneId, ZoneRole,
};

use hypercolor_daemon::api::AppState;
use hypercolor_daemon::domain::commit::CommitDurability;
use hypercolor_daemon::domain::zone::{
    CreateZone, DeleteZone, UpdateZone, create_zone, delete_zone, update_zone,
};
use hypercolor_daemon::domain::{DomainError, MutationContext};
use hypercolor_daemon::zone_layout_preview::ZoneLayoutPreviewOwner;

// ── Harness ──────────────────────────────────────────────────────────────

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    (Arc::new(AppState::new_with_data_dir(data_dir)), tempdir)
}

fn named_scene(name: &str) -> Scene {
    Scene {
        id: SceneId::new(),
        name: name.to_owned(),
        description: None,
        scope: SceneScope::Full,
        zone_assignments: Vec::new(),
        groups: Vec::new(),
        groups_revision: 0,
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

fn blank_patch() -> ZoneMetaPatch {
    ZoneMetaPatch {
        name: None,
        description: None,
        color: None,
        brightness: None,
        enabled: None,
        make_primary: None,
    }
}

/// A scene with a Primary zone, which is what every structural zone
/// mutation needs underneath it.
async fn seeded_scene(state: &AppState) -> SceneId {
    let mut scene = named_scene("studio");
    let layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };
    scene
        .groups
        .push(hypercolor_core::scene::default_primary_group(layout));
    let scene_id = scene.id;
    let mut manager = state.scene_manager.write().await;
    manager.create(scene).expect("scene should be created");
    manager
        .activate(&scene_id, None)
        .expect("scene should activate");
    scene_id
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

fn create_command(scene_id: SceneId, name: &str) -> CreateZone {
    CreateZone {
        target: scene_id.into(),
        name: name.to_owned(),
        color: None,
        fallback_canvas: (640, 480),
        expected_revision: None,
    }
}

// ── create_zone ──────────────────────────────────────────────────────────

#[tokio::test]
async fn create_zone_adds_a_custom_zone_and_announces_it() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;
    let before = state.scene_commits.revision();
    let mut events = state.event_bus.subscribe_all();

    let written = create_zone(
        &state,
        create_command(scene_id, "Desk"),
        MutationContext::api(),
    )
    .await
    .expect("zone should be created");

    assert_eq!(written.zone.name, "Desk");
    assert_eq!(written.zone.role, ZoneRole::Custom);
    assert!(written.commit.revision() > before);
    assert_eq!(written.commit.durability(), CommitDurability::Written);

    let manager = state.scene_manager.read().await;
    let scene = manager.get(&scene_id).expect("scene should still exist");
    assert!(scene.groups.iter().any(|zone| zone.id == written.zone.id));
    drop(manager);

    let seen = drain_events(&mut events);
    assert!(
        seen.iter().any(|event| matches!(
            event,
            HypercolorEvent::ZoneChanged {
                kind: ZoneChangeKind::Created,
                ..
            }
        )),
        "the new zone must be announced: {seen:?}"
    );
}

#[tokio::test]
async fn create_zone_refuses_a_blank_name() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;

    let error = create_zone(
        &state,
        create_command(scene_id, "   "),
        MutationContext::api(),
    )
    .await
    .expect_err("a zone needs a name");
    match error {
        DomainError::Validation { field, .. } => assert_eq!(field.as_deref(), Some("name")),
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[tokio::test]
async fn create_zone_refuses_an_unknown_scene() {
    let (state, _tempdir) = isolated_state();
    let error = create_zone(
        &state,
        create_command(SceneId::new(), "Desk"),
        MutationContext::api(),
    )
    .await
    .expect_err("an unknown scene has nowhere to put a zone");
    assert!(
        matches!(error, DomainError::NotFound { .. }),
        "expected NotFound, got {error:?}"
    );
}

// ── scene revision preconditions ─────────────────────────────────────────

#[tokio::test]
async fn a_stale_scene_revision_is_refused_before_the_mutation() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;
    create_zone(
        &state,
        create_command(scene_id, "Desk"),
        MutationContext::api(),
    )
    .await
    .expect("first zone should be created");
    let current = state.scene_commits.revision();

    let mut command = create_command(scene_id, "Shelf");
    command.expected_revision = Some(current.saturating_sub(1));
    let error = create_zone(&state, command, MutationContext::api())
        .await
        .expect_err("a stale revision must not mutate");
    match error {
        DomainError::PreconditionFailed {
            expected,
            current: reported,
            ..
        } => {
            assert_eq!(expected, current.saturating_sub(1));
            assert_eq!(reported, current);
        }
        other => panic!("expected PreconditionFailed, got {other:?}"),
    }

    let manager = state.scene_manager.read().await;
    let scene = manager.get(&scene_id).expect("scene should still exist");
    assert!(
        !scene.groups.iter().any(|zone| zone.name == "Shelf"),
        "the refused mutation must not have landed"
    );
}

#[tokio::test]
async fn a_cosmetic_zone_patch_honors_the_scene_revision() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;
    let created = create_zone(
        &state,
        create_command(scene_id, "Desk"),
        MutationContext::api(),
    )
    .await
    .expect("zone should be created");

    let current = state.scene_commits.revision();
    let written = update_zone(
        &state,
        UpdateZone {
            target: scene_id.into(),
            zone_id: created.zone.id,
            patch: ZoneMetaPatch {
                name: Some("Desk Left".to_owned()),
                ..blank_patch()
            },
            expected_revision: Some(current),
        },
        MutationContext::api(),
    )
    .await
    .expect("a rename should accept the current scene revision");
    assert_eq!(written.zone.name, "Desk Left");
}

#[tokio::test]
async fn promoting_a_zone_to_primary_honors_the_revision_precondition() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;
    let created = create_zone(
        &state,
        create_command(scene_id, "Desk"),
        MutationContext::api(),
    )
    .await
    .expect("zone should be created");

    let error = update_zone(
        &state,
        UpdateZone {
            target: scene_id.into(),
            zone_id: created.zone.id,
            patch: ZoneMetaPatch {
                make_primary: Some(true),
                ..blank_patch()
            },
            expected_revision: Some(0),
        },
        MutationContext::api(),
    )
    .await
    .expect_err("a structural patch must respect the revision the caller saw");
    assert!(
        matches!(error, DomainError::PreconditionFailed { .. }),
        "expected PreconditionFailed, got {error:?}"
    );
}

// ── delete_zone ──────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_zone_removes_it_and_announces_the_removal() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;
    let created = create_zone(
        &state,
        create_command(scene_id, "Desk"),
        MutationContext::api(),
    )
    .await
    .expect("zone should be created");
    let preview_layout = state.spatial_engine.read().await.layout().as_ref().clone();
    state
        .zone_layout_previews
        .set(
            ZoneLayoutPreviewOwner::new(),
            scene_id,
            created.zone.id,
            preview_layout,
        )
        .await;
    let mut events = state.event_bus.subscribe_all();

    let removed = delete_zone(
        &state,
        DeleteZone {
            target: scene_id.into(),
            zone_id: created.zone.id,
            expected_revision: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("zone should be removed");

    assert_eq!(removed.zone.id, created.zone.id);
    let manager = state.scene_manager.read().await;
    let scene = manager.get(&scene_id).expect("scene should still exist");
    assert!(!scene.groups.iter().any(|zone| zone.id == created.zone.id));
    drop(manager);
    assert!(
        state
            .zone_layout_previews
            .scene_overrides(scene_id)
            .await
            .is_empty(),
        "deleting a zone must retire its transient layout preview"
    );

    let seen = drain_events(&mut events);
    assert!(
        seen.iter().any(|event| matches!(
            event,
            HypercolorEvent::ZoneChanged {
                kind: ZoneChangeKind::Removed,
                ..
            }
        )),
        "the removal must be announced: {seen:?}"
    );
}

#[tokio::test]
async fn delete_zone_refuses_the_primary_zone() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;
    let primary_id = {
        let manager = state.scene_manager.read().await;
        manager
            .get(&scene_id)
            .and_then(Scene::primary_group)
            .expect("the seeded scene has a primary zone")
            .id
    };

    let error = delete_zone(
        &state,
        DeleteZone {
            target: scene_id.into(),
            zone_id: primary_id,
            expected_revision: None,
        },
        MutationContext::api(),
    )
    .await
    .expect_err("the primary zone is not deletable through this path");
    assert!(
        matches!(error, DomainError::Conflict { .. }),
        "expected Conflict, got {error:?}"
    );
}

#[tokio::test]
async fn delete_zone_refuses_an_unknown_zone() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;

    let error = delete_zone(
        &state,
        DeleteZone {
            target: scene_id.into(),
            zone_id: ZoneId::new(),
            expected_revision: None,
        },
        MutationContext::api(),
    )
    .await
    .expect_err("an unknown zone has nothing to delete");
    assert!(
        matches!(error, DomainError::NotFound { .. }),
        "expected NotFound, got {error:?}"
    );
}
