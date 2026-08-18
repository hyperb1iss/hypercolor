//! Service-level tests for the zone domain layer (Spec 76 §2.2, §2.3).
//!
//! Zones are the scene's output partition, so what these pin is the part
//! of the contract every zone endpoint shares: the `groups_revision`
//! precondition, the structural refusals, the events one mutation
//! publishes, and the fact that all seven transactions land through
//! `commit_scene` rather than through a hand-rolled admit-and-save.

use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_core::scene::{OutputPlacement, ZoneMetaPatch};
use hypercolor_types::api::zones::OutputAssignment;
use hypercolor_types::event::{HypercolorEvent, SceneSettingsChangeKind, ZoneChangeKind};
use hypercolor_types::scene::{
    ColorInterpolation, EasingFunction, Scene, SceneId, SceneKind, SceneMutationMode,
    ScenePriority, SceneScope, TransitionSpec, UnassignedBehavior, ZoneId, ZoneRole,
};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, StripDirection,
};

use hypercolor_daemon::api::AppState;
use hypercolor_daemon::domain::commit::CommitDurability;
use hypercolor_daemon::domain::zone::{
    AssignOutputs, CreateZone, DeleteZone, SetUnassignedBehavior, SetZoneLayout, UnassignOutput,
    UpdateZone, assign_outputs, create_zone, delete_zone, set_unassigned_behavior, set_zone_layout,
    unassign_output, update_zone,
};
use hypercolor_daemon::domain::{DomainError, MutationContext};

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

fn output(id: &str) -> Output {
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: format!("usb:{id}"),
        zone_name: None,
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(0.25, 0.1),
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 16,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: Some(SamplingMode::Bilinear),
        edge_behavior: Some(EdgeBehavior::Clamp),
        shape: None,
        shape_preset: None,
        display_order: 0,
        attachment: None,
        brightness: None,
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

async fn groups_revision(state: &AppState, scene_id: SceneId) -> u64 {
    state
        .scene_manager
        .read()
        .await
        .get(&scene_id)
        .map_or(0, |scene| scene.groups_revision)
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
        scene_id,
        name: name.to_owned(),
        color: None,
        fallback_canvas: (640, 480),
        expected_revision: None,
        expected_scene_revision: None,
    }
}

// ── create_zone ──────────────────────────────────────────────────────────

#[tokio::test]
async fn create_zone_adds_a_custom_zone_and_announces_it() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;
    let before = groups_revision(&state, scene_id).await;
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
    assert!(written.groups_revision > before);
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

// ── groups_revision preconditions ────────────────────────────────────────

#[tokio::test]
async fn a_stale_groups_revision_is_refused_before_the_mutation() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;
    create_zone(
        &state,
        create_command(scene_id, "Desk"),
        MutationContext::api(),
    )
    .await
    .expect("first zone should be created");
    let current = groups_revision(&state, scene_id).await;

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

/// Renaming a zone races with nothing, so it deliberately ignores the
/// header a structural edit would honor.
#[tokio::test]
async fn a_cosmetic_zone_patch_ignores_the_revision_precondition() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;
    let created = create_zone(
        &state,
        create_command(scene_id, "Desk"),
        MutationContext::api(),
    )
    .await
    .expect("zone should be created");

    let written = update_zone(
        &state,
        UpdateZone {
            scene_id,
            zone_id: created.zone.id,
            patch: ZoneMetaPatch {
                name: Some("Desk Left".to_owned()),
                ..blank_patch()
            },
            expected_revision: Some(0),
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("a rename should not be gated on the structural revision");
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
            scene_id,
            zone_id: created.zone.id,
            patch: ZoneMetaPatch {
                make_primary: Some(true),
                ..blank_patch()
            },
            expected_revision: Some(0),
            expected_scene_revision: None,
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
    let mut events = state.event_bus.subscribe_all();

    let removed = delete_zone(
        &state,
        DeleteZone {
            scene_id,
            zone_id: created.zone.id,
            expected_revision: None,
            expected_scene_revision: None,
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
            scene_id,
            zone_id: primary_id,
            expected_revision: None,
            expected_scene_revision: None,
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
            scene_id,
            zone_id: ZoneId::new(),
            expected_revision: None,
            expected_scene_revision: None,
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

// ── Output assignment ────────────────────────────────────────────────────

#[tokio::test]
async fn assign_outputs_moves_them_into_the_target_zone() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;
    let created = create_zone(
        &state,
        create_command(scene_id, "Desk"),
        MutationContext::api(),
    )
    .await
    .expect("zone should be created");

    let written = assign_outputs(
        &state,
        AssignOutputs {
            scene_id,
            zone_id: created.zone.id,
            assignments: vec![
                OutputAssignment::New(Box::new(output("strimer"))),
                OutputAssignment::New(Box::new(output("fan-1"))),
            ],
            placement: OutputPlacement::AutoGrid,
            expected_revision: None,
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("outputs should be assigned");

    assert_eq!(written.target_zone.layout.zones.len(), 2);
    assert!(
        written.zones.iter().any(|zone| zone.id == created.zone.id),
        "the partition response must include the target zone"
    );
}

#[tokio::test]
async fn unassign_output_drops_it_out_of_the_zone() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;
    let created = create_zone(
        &state,
        create_command(scene_id, "Desk"),
        MutationContext::api(),
    )
    .await
    .expect("zone should be created");
    assign_outputs(
        &state,
        AssignOutputs {
            scene_id,
            zone_id: created.zone.id,
            assignments: vec![OutputAssignment::New(Box::new(output("strimer")))],
            placement: OutputPlacement::AutoGrid,
            expected_revision: None,
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("output should be assigned");

    let written = unassign_output(
        &state,
        UnassignOutput {
            scene_id,
            zone_id: created.zone.id,
            output_id: "strimer".to_owned(),
            expected_revision: None,
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("output should be unassigned");

    assert!(written.target_zone.layout.zones.is_empty());
}

#[tokio::test]
async fn unassign_output_refuses_one_the_zone_does_not_hold() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;
    let created = create_zone(
        &state,
        create_command(scene_id, "Desk"),
        MutationContext::api(),
    )
    .await
    .expect("zone should be created");

    let error = unassign_output(
        &state,
        UnassignOutput {
            scene_id,
            zone_id: created.zone.id,
            output_id: "ghost".to_owned(),
            expected_revision: None,
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    .expect_err("an output the zone does not hold cannot be dropped");
    assert!(
        matches!(error, DomainError::NotFound { .. }),
        "expected NotFound, got {error:?}"
    );
}

// ── Layout and scene settings ────────────────────────────────────────────

#[tokio::test]
async fn set_zone_layout_refuses_a_layout_that_changes_the_output_set() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;
    let created = create_zone(
        &state,
        create_command(scene_id, "Desk"),
        MutationContext::api(),
    )
    .await
    .expect("zone should be created");
    let mut layout = created.zone.layout.clone();
    layout.zones.push(output("smuggled"));

    let error = set_zone_layout(
        &state,
        SetZoneLayout {
            scene_id,
            zone_id: created.zone.id,
            layout,
            expected_revision: None,
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    .expect_err("adds and drops route through the device endpoints");
    assert!(
        matches!(error, DomainError::Validation { .. }),
        "expected Validation, got {error:?}"
    );
}

#[tokio::test]
async fn set_zone_layout_repositions_the_outputs_the_zone_owns() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;
    let created = create_zone(
        &state,
        create_command(scene_id, "Desk"),
        MutationContext::api(),
    )
    .await
    .expect("zone should be created");
    let assigned = assign_outputs(
        &state,
        AssignOutputs {
            scene_id,
            zone_id: created.zone.id,
            assignments: vec![OutputAssignment::New(Box::new(output("strimer")))],
            placement: OutputPlacement::AutoGrid,
            expected_revision: None,
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("output should be assigned");

    let mut layout = assigned.target_zone.layout.clone();
    layout.zones[0].position = NormalizedPosition::new(0.25, 0.75);

    let written = set_zone_layout(
        &state,
        SetZoneLayout {
            scene_id,
            zone_id: created.zone.id,
            layout,
            expected_revision: None,
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("a placement-only edit should apply");
    assert_eq!(
        written.zone.layout.zones[0].position,
        NormalizedPosition::new(0.25, 0.75)
    );
}

#[tokio::test]
async fn set_unassigned_behavior_announces_a_scene_settings_change() {
    let (state, _tempdir) = isolated_state();
    let scene_id = seeded_scene(&state).await;
    let mut events = state.event_bus.subscribe_all();

    let written = set_unassigned_behavior(
        &state,
        SetUnassignedBehavior {
            scene_id,
            behavior: UnassignedBehavior::Hold,
            expected_revision: None,
            expected_scene_revision: None,
        },
        MutationContext::api(),
    )
    .await
    .expect("behavior should be set");

    assert_eq!(written.behavior, UnassignedBehavior::Hold);

    let seen = drain_events(&mut events);
    assert!(
        seen.iter().any(|event| matches!(
            event,
            HypercolorEvent::SceneSettingsChanged {
                kind: SceneSettingsChangeKind::UnassignedBehavior,
                ..
            }
        )),
        "the settings change must be announced: {seen:?}"
    );
}
