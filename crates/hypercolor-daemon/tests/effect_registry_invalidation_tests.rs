use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use hypercolor_types::event::HypercolorEvent;
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use tower::ServiceExt;
use uuid::Uuid;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn isolated_state_with_tempdir() -> (AppState, tempfile::TempDir) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = AppState::new();
    ConfigManager::set_data_dir_override(None);
    (state, tempdir)
}

fn multipart_upload_request(file_name: &str, html: &str) -> Request<Body> {
    let boundary = "hypercolor-upload-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: text/html\r\n\r\n{html}\r\n--{boundary}--\r\n"
    );

    Request::builder()
        .method("POST")
        .uri("/api/v1/effects/install")
        .header(
            http::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .expect("failed to build multipart request")
}

fn rescan_request() -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/effects/rescan")
        .body(Body::empty())
        .expect("failed to build rescan request")
}

fn sample_effect_metadata(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".into(),
        version: "0.1.0".into(),
        description: format!("{name} effect"),
        category: EffectCategory::Ambient,
        tags: Vec::new(),
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from(format!("native/{name}.wgsl")),
        },
        license: None,
    }
}

fn sample_layout() -> SpatialLayout {
    SpatialLayout {
        id: "effect-registry-invalidation".into(),
        name: "Effect Registry Invalidation".into(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

#[tokio::test]
async fn install_effect_invalidates_resolved_zone_revision() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let app = api::build_router(Arc::clone(&state), None);
    let metadata = sample_effect_metadata("seed");

    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .upsert_primary_zone(
            &metadata,
            HashMap::new(),
            None,
            sample_layout(),
            hypercolor_types::event::ChangeTrigger::System,
            None,
        )
        .expect("primary zone should be created");
    hypercolor_daemon::domain::scene::commit_scene(&state.domains.scene, mutation)
        .await
        .expect("primary zone should commit");

    let revision_before = {
        let scene_manager = state.scene_manager.snapshot().await;
        scene_manager.resolved_zones_revision()
    };

    let html = r#"<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta name="hypercolor-version" content="1" />
    <title>Aurora</title>
    <meta description="Northern lights" />
    <meta publisher="Hypercolor" />
    <meta property="speed" label="Speed" type="number" default="5" min="1" max="10" />
    <meta preset="Default" preset-controls='{"speed":5}' />
  </head>
  <body>
    <canvas id="exCanvas"></canvas>
    <script>console.log("ok")</script>
  </body>
</html>"#;

    let response = app
        .oneshot(multipart_upload_request("aurora.html", html))
        .await
        .expect("upload request should succeed");

    assert_eq!(response.status(), StatusCode::CREATED);

    let revision_after = {
        let scene_manager = state.scene_manager.snapshot().await;
        scene_manager.resolved_zones_revision()
    };

    assert!(
        revision_after > revision_before,
        "effect registry updates should invalidate resolved-zone caches"
    );
}

#[tokio::test]
async fn rescan_effects_invalidates_resolved_zone_revision() {
    let (state, tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let app = api::build_router(Arc::clone(&state), None);
    let metadata = sample_effect_metadata("seed");

    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .upsert_primary_zone(
            &metadata,
            HashMap::new(),
            None,
            sample_layout(),
            hypercolor_types::event::ChangeTrigger::System,
            None,
        )
        .expect("primary zone should be created");
    hypercolor_daemon::domain::scene::commit_scene(&state.domains.scene, mutation)
        .await
        .expect("primary zone should commit");

    let revision_before = {
        let scene_manager = state.scene_manager.snapshot().await;
        scene_manager.resolved_zones_revision()
    };

    let user_effects_dir = tempdir.path().join("data/effects");
    std::fs::create_dir_all(&user_effects_dir).expect("user effects dir should be created");
    std::fs::write(
        user_effects_dir.join("rescan.html"),
        r#"<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta name="hypercolor-version" content="1" />
    <title>Rescan Effect</title>
    <meta description="Manual rescan test" />
    <meta publisher="Hypercolor" />
  </head>
  <body>
    <canvas id="exCanvas"></canvas>
  </body>
</html>"#,
    )
    .expect("effect file should be written");

    let mut events = state.event_bus.subscribe_all();
    let response = app
        .oneshot(rescan_request())
        .await
        .expect("rescan request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("rescan response body should read");
    let response: serde_json::Value =
        serde_json::from_slice(&body).expect("rescan response should be JSON");
    let expected = (
        response["data"]["added"]
            .as_u64()
            .expect("added count should be present") as usize,
        response["data"]["removed"]
            .as_u64()
            .expect("removed count should be present") as usize,
        response["data"]["updated"]
            .as_u64()
            .expect("updated count should be present") as usize,
    );
    let registry_updates = std::iter::from_fn(|| events.try_recv().ok())
        .filter_map(|event| match event.event {
            HypercolorEvent::EffectRegistryUpdated {
                added,
                removed,
                updated,
            } => Some((added, removed, updated)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(registry_updates, vec![expected]);

    let revision_after = {
        let scene_manager = state.scene_manager.snapshot().await;
        scene_manager.resolved_zones_revision()
    };

    assert!(
        revision_after > revision_before,
        "manual rescans should invalidate resolved-zone caches"
    );
}
