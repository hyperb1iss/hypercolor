#![cfg(feature = "persistence-test-hooks")]

use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::domain::scene::commit_scene;
use hypercolor_daemon::domain::scene_activation::{
    BrightnessDurability, ObservedScene, ProjectionDurability, SelectedSceneFields,
    SelectedSceneOutcome, activate_selected_fields,
};
use hypercolor_types::api::layouts::CreateLayoutRequest;
use hypercolor_types::identity::LayoutId;
use hypercolor_types::scene::{SceneId, SceneKind};
use hypercolor_types::spatial::SpatialLayout;
use std::sync::Arc;
use std::time::Duration;

async fn fixture() -> (
    Arc<AppState>,
    tempfile::TempDir,
    ObservedScene,
    SpatialLayout,
) {
    let dir = tempfile::tempdir().expect("fixture directory");
    let state = Arc::new(AppState::new_with_data_dir(dir.path().join("data")));
    let created = state
        .domains
        .layout
        .create(CreateLayoutRequest {
            name: "Selected layout".into(),
            canvas_width: Some(800),
            canvas_height: Some(450),
            ..Default::default()
        })
        .await
        .expect("layout fixture");
    let layout = hypercolor_daemon::layout_store::load(&dir.path().join("data/layouts.json"))
        .expect("persisted catalog")
        .get(&created.id)
        .expect("created layout")
        .clone();
    let mut scene = state
        .scene_manager
        .snapshot()
        .await
        .active_scene()
        .expect("default scene")
        .clone();
    scene.id = SceneId::new();
    scene.name = "Selected scene".into();
    scene.kind = SceneKind::Named;
    scene.layout_id = Some(LayoutId::new(layout.id.clone()).expect("layout identity"));
    scene.activation_brightness = Some(0.31);
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation.create_scene(scene.clone()).expect("scene fixture");
    let commit = commit_scene(&state.domains.scene, mutation)
        .await
        .expect("scene commit");
    (
        state,
        dir,
        ObservedScene {
            scene,
            revision: commit.revision(),
        },
        layout,
    )
}

fn command(expected: &ObservedScene, layout: &SpatialLayout, mask: u8) -> SelectedSceneFields {
    SelectedSceneFields {
        expected: expected.clone(),
        context: (mask & 1 != 0).then(|| expected.scene.transition.clone()),
        layout: (mask & 2 != 0).then(|| layout.clone()),
        brightness: (mask & 4 != 0).then_some(0.31),
    }
}

async fn execute(state: &AppState, command: SelectedSceneFields) -> SelectedSceneOutcome {
    let workflow = activate_selected_fields(&state.domains.scene_library, command);
    tokio::pin!(workflow);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = &mut workflow => break result.expect("selected activation"),
                () = tokio::task::yield_now() => {
                    state.layout_publication_test_executor().execute_next_layout_publication().await.expect("renderer publication");
                }
            }
        }
    }).await.expect("shared locks must not deadlock")
}

#[tokio::test]
async fn all_seven_field_selections_preserve_omitted_fields_and_output_power() {
    for mask in 1..8 {
        let (state, _dir, expected, layout) = fixture().await;
        let old_scene = state
            .scene_manager
            .snapshot()
            .await
            .active_scene_id()
            .copied();
        let old_layout = state.spatial_engine.snapshot().layout().as_ref().clone();
        let old_output = hypercolor_daemon::domain::output::get_output(&state.domains.output);
        let result = execute(&state, command(&expected, &layout, mask)).await;
        assert_eq!(result.context.is_some(), mask & 1 != 0);
        assert_eq!(
            state
                .scene_manager
                .snapshot()
                .await
                .active_scene_id()
                .copied(),
            if mask & 1 != 0 {
                Some(expected.scene.id)
            } else {
                old_scene
            }
        );
        assert_eq!(
            state.spatial_engine.snapshot().layout().as_ref(),
            if mask & 2 != 0 { &layout } else { &old_layout }
        );
        let output = hypercolor_daemon::domain::output::get_output(&state.domains.output);
        assert_eq!(
            output.brightness,
            if mask & 4 != 0 {
                0.31
            } else {
                old_output.brightness
            }
        );
        assert_eq!(output.power, old_output.power);
        if let Some(brightness) = result.brightness {
            assert!(matches!(
                brightness.expect("brightness admission").durability,
                BrightnessDurability::Written
            ));
        }
        if let Some(layout) = result.layout {
            assert!(layout.publication.is_ok(), "{layout:?}");
            assert!(matches!(
                layout.writes[0].durability,
                ProjectionDurability::Written
            ));
            assert!(layout.admitted_scene.is_some());
        }
    }
}

#[tokio::test]
async fn stale_definition_invalid_fields_and_missing_layout_refuse_before_mutation() {
    for case in 0..7 {
        let (state, _dir, expected, layout) = fixture().await;
        let old_scene = state.scene_manager.snapshot().await;
        let old_layout = state.spatial_engine.snapshot().layout().as_ref().clone();
        let old_output = hypercolor_daemon::domain::output::get_output(&state.domains.output);
        let mut request = command(&expected, &layout, 7);
        match case {
            0 => request.expected.revision += 1,
            1 => request.expected.scene.description = Some("unobserved".into()),
            2 => request.brightness = Some(f32::NAN),
            3 => request.layout.as_mut().expect("layout").name = "changed".into(),
            4 => request.expected.scene.id = SceneId::new(),
            5 => {
                request.context = None;
                request.layout = None;
                request.brightness = None;
            }
            _ => {
                request.expected.scene.transition.duration_ms = request
                    .expected
                    .scene
                    .transition
                    .duration_ms
                    .saturating_add(1);
            }
        }
        assert!(
            activate_selected_fields(&state.domains.scene_library, request)
                .await
                .is_err()
        );
        assert_eq!(
            state.scene_manager.snapshot().await.active_scene_id(),
            old_scene.active_scene_id()
        );
        assert_eq!(
            state.spatial_engine.snapshot().layout().as_ref(),
            &old_layout
        );
        assert_eq!(
            hypercolor_daemon::domain::output::get_output(&state.domains.output).brightness,
            old_output.brightness
        );
    }
}

#[tokio::test]
async fn definition_edit_after_prewrite_rejects_publication_and_persists_exact_rollback() {
    let (state, _dir, expected, layout) = fixture().await;
    let original_layout = state.spatial_engine.snapshot().layout().id.clone();
    let workflow =
        activate_selected_fields(&state.domains.scene_library, command(&expected, &layout, 6));
    tokio::pin!(workflow);
    let mut changed = false;
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = &mut workflow => break result.expect("partial result"),
                () = tokio::task::yield_now(), if !changed => {
                    let executor = state.layout_publication_test_executor();
                    if executor.pending_layout_publications() == 0 { continue; }
                    let publication = executor.execute_next_layout_publication_with_hook(|| async {
                        let on_disk = hypercolor_daemon::runtime_state::load(&state.runtime_state_path).expect("prewrite read").expect("prewrite");
                        assert_eq!(on_disk.active_layout_id.as_deref(), Some(layout.id.as_str()));
                        let source_zones_revision = state.scene_manager.snapshot().await.resolved_zones_revision();
                        let mut mutation = state.scene_manager.begin_mutation().await;
                        let mut replacement = expected.scene.clone();
                        replacement.description = Some("concurrent API edit".into());
                        mutation.update_scene(replacement).expect("authored edit");
                        commit_scene(&state.domains.scene, mutation).await.expect("concurrent commit");
                        assert_eq!(state.scene_manager.snapshot().await.resolved_zones_revision(), source_zones_revision, "authored fence catches an edit invisible to the old layout guard");
                    }).await;
                    assert!(publication.is_err());
                    changed = true;
                }
            }
        }
    }).await.expect("publication and rollback cannot deadlock");
    let layout_result = result.layout.expect("selected layout");
    assert!(layout_result.publication.is_err());
    assert!(layout_result.admitted_scene.is_none());
    assert_eq!(layout_result.writes.len(), 2);
    assert!(
        layout_result
            .writes
            .iter()
            .all(|write| matches!(write.durability, ProjectionDurability::Written))
    );
    assert_eq!(
        layout_result.writes[0]
            .projection
            .as_ref()
            .expect("prewrite")
            .active_layout_id
            .as_deref(),
        Some(layout.id.as_str())
    );
    assert_eq!(
        layout_result.writes[1]
            .projection
            .as_ref()
            .expect("rollback")
            .active_layout_id
            .as_deref(),
        Some(original_layout.as_str())
    );
    let restarted = hypercolor_daemon::runtime_state::load(&state.runtime_state_path)
        .expect("restart read")
        .expect("rollback snapshot");
    assert_eq!(
        restarted.active_layout_id.as_deref(),
        Some(original_layout.as_str())
    );
    assert!(result.brightness.expect("selected brightness").is_err());
}

#[tokio::test]
async fn brightness_retry_and_refusal_preserve_original_context_receipt() {
    use hypercolor_daemon::persistence::AtomicFileWriter;
    for after_replacement in [false, true] {
        let (state, _dir, expected, layout) = fixture().await;
        let path = state
            .runtime_state_path
            .parent()
            .expect("state directory")
            .join("device-settings.json");
        let writer = AtomicFileWriter::new(&path).expect("settings writer");
        if after_replacement {
            writer.set_injected_directory_sync_failures(usize::MAX);
        } else {
            writer.set_injected_replace_failures(usize::MAX);
        }
        let result = execute(&state, command(&expected, &layout, 5)).await;
        writer.set_injected_replace_failures(0);
        writer.set_injected_directory_sync_failures(0);
        assert!(
            result.context.is_some(),
            "later brightness failure cannot erase context"
        );
        let brightness = result.brightness.expect("selected brightness");
        if after_replacement {
            assert!(matches!(
                brightness.expect("admitted settings").durability,
                BrightnessDurability::Retrying(_)
            ));
            assert_eq!(
                hypercolor_daemon::domain::output::get_output(&state.domains.output).brightness,
                0.31
            );
        } else {
            assert!(brightness.is_err());
            assert_eq!(
                hypercolor_daemon::domain::output::get_output(&state.domains.output).brightness,
                1.0
            );
        }
    }
}

#[tokio::test]
async fn rollback_failure_retains_escaped_candidate_write_without_an_applied_claim() {
    use hypercolor_daemon::persistence::AtomicFileWriter;
    let (state, _dir, expected, layout) = fixture().await;
    let writer = AtomicFileWriter::new(&state.runtime_state_path).expect("runtime writer");
    let workflow =
        activate_selected_fields(&state.domains.scene_library, command(&expected, &layout, 2));
    tokio::pin!(workflow);
    let mut changed = false;
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = &mut workflow => break result.expect("partial result"),
                () = tokio::task::yield_now(), if !changed => {
                    let executor = state.layout_publication_test_executor();
                    if executor.pending_layout_publications() == 0 { continue; }
                    let publication = executor.execute_next_layout_publication_with_hook(|| async {
                        writer.set_injected_replace_failures(usize::MAX);
                        let mut mutation = state.scene_manager.begin_mutation().await;
                        let mut replacement = expected.scene.clone();
                        replacement.description = Some("concurrent edit before renderer publication".into());
                        mutation.update_scene(replacement).expect("authored edit");
                        commit_scene(&state.domains.scene, mutation).await.expect("concurrent commit");
                    }).await;
                    assert!(publication.is_err());
                    changed = true;
                }
            }
        }
    }).await.expect("failed rollback returns evidence");
    writer.set_injected_replace_failures(0);
    let result = result.layout.expect("layout result");
    assert!(result.publication.is_err());
    assert!(result.admitted_scene.is_none());
    assert_eq!(result.writes.len(), 2);
    assert!(matches!(
        result.writes[0].durability,
        ProjectionDurability::Written
    ));
    assert!(matches!(
        result.writes[1].durability,
        ProjectionDurability::Retrying(_)
    ));
    assert_eq!(
        result.writes[0]
            .projection
            .as_ref()
            .expect("escaped payload")
            .active_layout_id
            .as_deref(),
        Some(layout.id.as_str())
    );
}

#[tokio::test]
async fn selected_context_enforces_media_caps_but_brightness_does_not_activate_media() {
    use hypercolor_core::asset::{AssetTypeHint, AssetUploadOptions};
    use hypercolor_types::layer::{
        BlendMode, LayerAdjust, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
    };
    let (state, _dir, mut expected, layout) = fixture().await;
    let mut zone = hypercolor_core::scene::default_primary_zone(layout.clone());
    for (name, url) in [
        ("one.stream", "https://1.1.1.1/one.m3u8"),
        ("two.stream", "https://8.8.8.8/two.m3u8"),
    ] {
        let mut options = AssetUploadOptions::new(name);
        options.type_hint = Some(AssetTypeHint::Stream);
        let asset_id = state
            .asset_library
            .write()
            .await
            .add_bytes(url.as_bytes(), options)
            .expect("stream descriptor only")
            .record
            .id;
        zone.layers.push(SceneLayer {
            id: SceneLayerId::new(),
            name: None,
            source: LayerSource::Media {
                asset_id,
                playback: hypercolor_types::layer::MediaPlayback::default(),
            },
            blend: BlendMode::default(),
            opacity: 1.0,
            transform: LayerTransform::default(),
            adjust: LayerAdjust::default(),
            bindings: Vec::new(),
            enabled: true,
        });
    }
    expected.scene.zones = vec![zone];
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .update_scene(expected.scene.clone())
        .expect("authored scene");
    expected.revision = commit_scene(&state.domains.scene, mutation)
        .await
        .expect("fixture commit")
        .revision();
    let old_scene = state
        .scene_manager
        .snapshot()
        .await
        .active_scene_id()
        .copied();
    assert!(
        activate_selected_fields(&state.domains.scene_library, command(&expected, &layout, 1))
            .await
            .is_err()
    );
    assert_eq!(
        state
            .scene_manager
            .snapshot()
            .await
            .active_scene_id()
            .copied(),
        old_scene
    );
    let output = execute(&state, command(&expected, &layout, 4)).await;
    assert!(output.context.is_none());
    assert!(output.runtime_session.is_none());
    assert!(output.brightness.expect("brightness only").is_ok());
    assert_eq!(
        state
            .scene_manager
            .snapshot()
            .await
            .active_scene_id()
            .copied(),
        old_scene
    );
}
