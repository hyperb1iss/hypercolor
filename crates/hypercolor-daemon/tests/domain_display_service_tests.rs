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
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{
    ControlValue, EffectCategory, EffectId, EffectMetadata, EffectSource, EffectState,
};
use hypercolor_types::event::{HypercolorEvent, ZoneChangeKind};
use hypercolor_types::layer::{LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{
    ColorInterpolation, DisplayFaceBlendMode, DisplayFaceTarget, EasingFunction, Scene, SceneId,
    SceneKind, SceneMutationMode, ScenePriority, TransitionSpec, UnassignedBehavior, ZoneId,
    ZoneRole,
};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use uuid::Uuid;

use hypercolor_daemon::api::AppState;
use hypercolor_daemon::domain::display::{
    ClearDisplayFace, PatchDisplayComposition, PatchDisplayFaceControls, SetDisplayFace,
    clear_display_face, patch_display_composition, patch_display_face_controls,
    prune_display_zones_for_device, remove_default_display_overlay, set_default_display_overlay,
    set_display_face, sync_display_surfaces,
};

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
        spaces: None,
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

async fn insert_effect(state: &AppState, metadata: &EffectMetadata) {
    let entry = EffectEntry {
        metadata: metadata.clone(),
        source_path: format!("/tmp/{}.html", metadata.name).into(),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    };
    let _ = state.effect_registry.write().await.register(entry);
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

fn assign_command(device_id: DeviceId, effect: &EffectMetadata) -> SetDisplayFace {
    SetDisplayFace {
        device_id,
        device_name: "Kraken".to_owned(),
        effect: effect.clone(),
        controls: HashMap::new(),
        layout: face_layout(device_id),
        target: DisplayFaceTarget {
            blend_mode: DisplayFaceBlendMode::Alpha,
            device_id,
            opacity: 1.0,
        },
    }
}

/// A runtime overlay zone, as the preference store materializes one.
fn overlay_zone(device_id: DeviceId, effect_id: EffectId) -> hypercolor_types::scene::Zone {
    let mut zone = hypercolor_core::scene::default_primary_group(face_layout(device_id));
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

// ── Scene layer ──────────────────────────────────────────────────────────

#[tokio::test]
async fn set_display_face_creates_the_zone_then_updates_it() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let first = face_effect("clock");
    let second = face_effect("weather");
    insert_effect(&state, &first).await;
    insert_effect(&state, &second).await;
    let mut events = state.event_bus.subscribe_all();

    let created = set_display_face(&state.scene, assign_command(device_id, &first))
        .await
        .expect("the face should be assigned");
    assert_eq!(created.change, ZoneChangeKind::Created);
    assert_eq!(created.zone.effect_ids().next(), Some(first.id));
    assert_eq!(created.zone.role, ZoneRole::Display);

    let updated = set_display_face(&state.scene, assign_command(device_id, &second))
        .await
        .expect("the face should be replaced");
    assert_eq!(updated.change, ZoneChangeKind::Updated);
    assert_eq!(updated.zone.effect_ids().next(), Some(second.id));
    assert_eq!(updated.zone.id, created.zone.id, "the zone is reused");

    let seen = drain_events(&mut events);
    let kinds = seen
        .iter()
        .filter_map(|event| match event {
            HypercolorEvent::ZoneChanged { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![ZoneChangeKind::Created, ZoneChangeKind::Updated],
        "both transports announce the same two changes: {seen:?}"
    );
}

/// The upsert seeds the target as Replace, which would black out the
/// effect beneath the face. The caller's composition has to survive.
#[tokio::test]
async fn set_display_face_applies_the_requested_composition() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;

    let mut command = assign_command(device_id, &effect);
    command.target = DisplayFaceTarget {
        blend_mode: DisplayFaceBlendMode::Alpha,
        device_id,
        opacity: 0.4,
    };
    let written = set_display_face(&state.scene, command)
        .await
        .expect("the face should be assigned");

    let target = written
        .zone
        .display_target
        .expect("a display zone carries a target");
    assert_eq!(target.blend_mode, DisplayFaceBlendMode::Alpha);
    assert!((target.opacity - 0.4).abs() < f32::EPSILON);
}

#[tokio::test]
async fn clear_display_face_keeps_the_zone_and_drops_the_effect() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;
    let created = set_display_face(&state.scene, assign_command(device_id, &effect))
        .await
        .expect("the face should be assigned");

    let cleared = clear_display_face(
        &state.scene,
        ClearDisplayFace {
            device_id,
            device_name: "Kraken".to_owned(),
            layout: face_layout(device_id),
        },
    )
    .await
    .expect("the face should be cleared");

    assert_eq!(cleared.zone.id, created.zone.id);
    assert!(cleared.zone.effect_ids().next().is_none());
    assert_eq!(cleared.change, ZoneChangeKind::Updated);
}

#[tokio::test]
async fn display_face_mutations_conflict_on_a_snapshot_locked_scene() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;

    let mut scene = named_scene("frozen");
    scene.mutation_mode = SceneMutationMode::Snapshot;
    let scene_id = scene.id;
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .create_scene(scene)
        .expect("scene should be created");
    mutation
        .activate(
            scene_id,
            None,
            hypercolor_types::event::SceneChangeReason::UserActivate,
        )
        .expect("scene should activate");
    hypercolor_daemon::domain::scene::commit_scene(&state, mutation)
        .await
        .expect("scene should commit");

    let error = set_display_face(&state.scene, assign_command(device_id, &effect))
        .await
        .expect_err("a snapshot scene refuses runtime rewriting");
    assert!(
        matches!(
            error,
            hypercolor_daemon::domain::DomainError::Conflict { .. }
        ),
        "expected Conflict, got {error:?}"
    );
}

#[tokio::test]
async fn patching_composition_and_controls_reports_a_missing_zone() {
    let (state, _tempdir) = isolated_state();

    assert!(
        patch_display_composition(
            &state.scene,
            PatchDisplayComposition {
                zone_id: ZoneId::new(),
                blend_mode: Some(DisplayFaceBlendMode::Replace),
                opacity: None,
            },
        )
        .await
        .expect("a missing zone is a not-found, not a failure")
        .is_none()
    );

    assert!(
        patch_display_face_controls(
            &state.scene,
            PatchDisplayFaceControls {
                zone_id: ZoneId::new(),
                controls: HashMap::from([("accent".to_owned(), ControlValue::Float(0.5))]),
            },
        )
        .await
        .expect("a missing zone is a not-found, not a failure")
        .is_none()
    );
}

#[tokio::test]
async fn patch_display_face_controls_merges_onto_the_zone() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;
    let created = set_display_face(&state.scene, assign_command(device_id, &effect))
        .await
        .expect("the face should be assigned");

    let written = patch_display_face_controls(
        &state.scene,
        PatchDisplayFaceControls {
            zone_id: created.zone.id,
            controls: HashMap::from([("accent".to_owned(), ControlValue::Float(0.5))]),
        },
    )
    .await
    .expect("the patch should not fail")
    .expect("the zone exists");

    assert_eq!(
        zone_controls(&written.zone).and_then(|controls| controls.get("accent")),
        Some(&ControlValue::Float(0.5))
    );
    assert_eq!(written.change, ZoneChangeKind::ControlsPatched);
}

// ── Surface sync and pruning ─────────────────────────────────────────────

/// Surface sync runs on every scene activation and display listing, so a
/// snapshot scene must make it a no-op rather than an error.
#[tokio::test]
async fn sync_display_surfaces_skips_a_snapshot_locked_scene() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();

    let mut scene = named_scene("frozen");
    scene.mutation_mode = SceneMutationMode::Snapshot;
    let scene_id = scene.id;
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .create_scene(scene)
        .expect("scene should be created");
    mutation
        .activate(
            scene_id,
            None,
            hypercolor_types::event::SceneChangeReason::UserActivate,
        )
        .expect("scene should activate");
    hypercolor_daemon::domain::scene::commit_scene(&state, mutation)
        .await
        .expect("scene should commit");

    let changed = sync_display_surfaces(
        &state.scene,
        vec![(device_id, "Kraken".to_owned(), face_layout(device_id))],
    )
    .await
    .expect("surface sync should not fail on a locked scene");
    assert!(!changed);
}

#[tokio::test]
async fn sync_display_surfaces_reports_whether_anything_moved() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let displays = vec![(device_id, "Kraken".to_owned(), face_layout(device_id))];

    assert!(
        sync_display_surfaces(&state.scene, displays.clone())
            .await
            .expect("the first sync installs a surface"),
        "installing a surface is a change"
    );
    assert!(
        !sync_display_surfaces(&state.scene, displays)
            .await
            .expect("the second sync is idempotent"),
        "an unchanged surface must not commit"
    );
}

#[tokio::test]
async fn prune_display_zones_removes_both_layers_for_a_deleted_device() {
    let (state, _tempdir) = isolated_state();
    let device_id = DeviceId::new();
    let effect = face_effect("clock");
    insert_effect(&state, &effect).await;
    set_display_face(&state.scene, assign_command(device_id, &effect))
        .await
        .expect("the face should be assigned");
    set_default_display_overlay(&state.scene, device_id, overlay_zone(device_id, effect.id))
        .await
        .expect("the overlay should install");

    let pruned = prune_display_zones_for_device(&state.scene, device_id)
        .await
        .expect("pruning should succeed");

    assert_eq!(pruned.removed_zones.len(), 1);
    assert!(pruned.removed_default.is_some());
    assert!(pruned.commit.is_some());

    let manager = state.scene_manager.snapshot().await;
    assert!(manager.default_display_group_for(device_id).is_none());
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

    let installed =
        set_default_display_overlay(&state.scene, device_id, overlay_zone(device_id, effect.id))
            .await
            .expect("the overlay should install")
            .expect("the overlay is readable back");
    assert_eq!(installed.effect_ids().next(), Some(effect.id));

    let removed = remove_default_display_overlay(&state.scene, device_id)
        .await
        .expect("the overlay should retract")
        .expect("the retraction reports what it removed");
    assert_eq!(removed.id, installed.id);

    let manager = state.scene_manager.snapshot().await;
    assert!(manager.default_display_group_for(device_id).is_none());
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
    set_default_display_overlay(&state.scene, device_id, zone.clone())
        .await
        .expect("the first install lands");
    let after_install = state.scene_manager.revision();

    for _ in 0..3 {
        let mut refresh = zone.clone();
        refresh.layers[0].id = SceneLayerId::new();
        let installed = set_default_display_overlay(&state.scene, device_id, refresh)
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
        remove_default_display_overlay(&state.scene, DeviceId::new())
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

    set_default_display_overlay(&state.scene, device_id, overlay_zone(device_id, first.id))
        .await
        .expect("the first install lands");
    let after_first = state.scene_manager.revision();

    let installed =
        set_default_display_overlay(&state.scene, device_id, overlay_zone(device_id, second.id))
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

    let pruned = prune_display_zones_for_device(&state.scene, DeviceId::new())
        .await
        .expect("pruning nothing succeeds");

    assert!(pruned.removed_zones.is_empty());
    assert!(pruned.removed_default.is_none());
    assert!(pruned.commit.is_none());
    assert_eq!(state.scene_manager.revision(), before);
}
