use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::asset::{AssetTypeHint, AssetUploadOptions};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::engine::FpsTier;
use hypercolor_core::scene::make_scene;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use hypercolor_types::asset::AssetId;
use hypercolor_types::layer::{
    BlendMode, LayerAdjust, LayerSource, LayerTransform, MediaPlayback, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::{SceneId, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use serde_json::json;
use tower::ServiceExt;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn isolated_state_with_tempdir() -> (Arc<AppState>, tempfile::TempDir) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = AppState::new();
    ConfigManager::set_data_dir_override(None);
    (Arc::new(state), tempdir)
}

fn test_app_with_state(state: Arc<AppState>) -> axum::Router {
    api::build_router(state, None)
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    serde_json::from_slice(&bytes).expect("failed to parse JSON body")
}

async fn send(app: &axum::Router, request: Request<Body>) -> axum::response::Response {
    app.clone()
        .oneshot(request)
        .await
        .expect("request should succeed")
}

fn json_request(method: &str, uri: String, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request should build")
}

fn sample_layout(zone_id: &str) -> SpatialLayout {
    SpatialLayout {
        id: format!("layout-{zone_id}"),
        name: format!("Layout {zone_id}"),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![Output {
            id: zone_id.into(),
            name: zone_id.into(),
            device_id: "mock:device".into(),
            zone_name: None,
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(1.0, 1.0),
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
        spaces: None,
        version: 1,
    }
}

async fn insert_stream_asset(state: &Arc<AppState>, name: &str, url: &str) -> AssetId {
    let mut options = AssetUploadOptions::new(name);
    options.type_hint = Some(AssetTypeHint::Stream);
    let upsert = state
        .asset_library
        .write()
        .await
        .add_bytes(format!("{url}\n").as_bytes(), options)
        .expect("stream URL asset should upload");
    assert_eq!(
        upsert.record.mime_type,
        "application/vnd.hypercolor.stream-url"
    );
    upsert.record.id
}

async fn insert_mp4_asset(state: &Arc<AppState>, name: &str, seed: u8) -> AssetId {
    let mut bytes = b"\0\0\0\x18ftypisom\0\0\0\0isomiso2".to_vec();
    bytes.push(seed);
    let upsert = state
        .asset_library
        .write()
        .await
        .add_bytes(&bytes, AssetUploadOptions::new(name))
        .expect("mp4 asset should upload");
    assert_eq!(upsert.record.mime_type, "video/mp4");
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
        blend: BlendMode::Alpha,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    }
}

async fn install_media_scene(state: &Arc<AppState>, layers: Vec<SceneLayer>) -> SceneId {
    let mut scene = make_scene("Media Admission Scene");
    let scene_id = scene.id;
    scene.zones = vec![Zone {
        id: ZoneId::new(),
        name: "Media".to_owned(),
        description: None,
        layers,
        layout: sample_layout("media:main"),
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: ZoneRole::Primary,
        controls_version: 0,
        layers_version: 0,
    }];

    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation.create_scene(scene).expect("scene should create");
    hypercolor_daemon::domain::scene::commit_scene(&state.domains.scene, mutation)
        .await
        .expect("scene should commit");
    scene_id
}

#[tokio::test]
async fn activate_scene_rejects_video_media_cap() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let asset_a = insert_mp4_asset(&state, "a.mp4", 1).await;
    let asset_b = insert_mp4_asset(&state, "b.mp4", 2).await;
    let asset_c = insert_mp4_asset(&state, "c.mp4", 3).await;
    let scene_id = install_media_scene(
        &state,
        vec![
            media_layer(asset_a),
            media_layer(asset_b),
            media_layer(asset_c),
        ],
    )
    .await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = send(
        &app,
        json_request(
            "POST",
            format!("/api/v1/scenes/{scene_id}/activate"),
            json!({}),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("message should be a string")
            .contains("video producers 3/2")
    );
    assert_eq!(json["error"]["details"]["counts"]["video"], 3);
    assert_eq!(json["error"]["details"]["caps"]["video"], 2);
    // Pin the renamed per-layer detail keys, not just counts and prose.
    let first_layer = &json["error"]["details"]["layers"]["video"][0];
    assert!(
        first_layer["zone_id"].is_string(),
        "layer details carry zone_id"
    );
    assert!(
        first_layer["zone_name"].is_string(),
        "layer details carry zone_name"
    );
    assert_ne!(
        state
            .scene_manager
            .snapshot()
            .await
            .active_scene_id()
            .copied(),
        Some(scene_id)
    );
}

#[tokio::test]
async fn activate_scene_rejects_livestream_media_cap() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let asset_a =
        insert_stream_asset(&state, "camera-a.stream", "https://1.1.1.1/live-a.m3u8").await;
    let asset_b =
        insert_stream_asset(&state, "camera-b.stream", "https://8.8.8.8/live-b.m3u8").await;
    let scene_id =
        install_media_scene(&state, vec![media_layer(asset_a), media_layer(asset_b)]).await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = send(
        &app,
        json_request(
            "POST",
            format!("/api/v1/scenes/{scene_id}/activate"),
            json!({}),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("message should be a string")
            .contains("livestream producers 2/1")
    );
    assert_eq!(json["error"]["details"]["counts"]["livestream"], 2);
    assert_eq!(json["error"]["details"]["caps"]["livestream"], 1);
    assert_ne!(
        state
            .scene_manager
            .snapshot()
            .await
            .active_scene_id()
            .copied(),
        Some(scene_id)
    );
}

#[tokio::test]
async fn live_tree_create_rejects_a_second_livestream_without_mutation() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let asset_a =
        insert_stream_asset(&state, "camera-a.stream", "https://1.1.1.1/live-a.m3u8").await;
    let asset_b =
        insert_stream_asset(&state, "camera-b.stream", "https://8.8.8.8/live-b.m3u8").await;
    let original = media_layer(asset_a);
    let original_id = original.id;
    let scene_id = install_media_scene(&state, vec![original]).await;
    let app = test_app_with_state(Arc::clone(&state));
    assert_eq!(
        send(
            &app,
            json_request(
                "POST",
                format!("/api/v1/scenes/{scene_id}/activate"),
                json!({}),
            ),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let zone_id = state
        .scene_manager
        .snapshot()
        .await
        .active_scene()
        .and_then(hypercolor_types::scene::Scene::primary_zone)
        .expect("activated scene should have a primary zone")
        .id;

    let response = send(
        &app,
        json_request(
            "POST",
            format!("/api/v1/scene/zones/{zone_id}/layers"),
            json!({
                "source": { "type": "media", "asset_id": asset_b }
            }),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(body["error"]["details"]["counts"]["livestream"], 2);
    let manager = state.scene_manager.snapshot().await;
    let layers = &manager
        .active_scene()
        .and_then(hypercolor_types::scene::Scene::primary_zone)
        .expect("active zone should remain")
        .layers;
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0].id, original_id);
}

#[tokio::test]
async fn concurrent_livestream_creates_cannot_both_cross_the_cap() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let asset_a =
        insert_stream_asset(&state, "camera-a.stream", "https://1.1.1.1/live-a.m3u8").await;
    let asset_b =
        insert_stream_asset(&state, "camera-b.stream", "https://8.8.8.8/live-b.m3u8").await;
    let scene_id = install_media_scene(&state, Vec::new()).await;
    let app = test_app_with_state(Arc::clone(&state));
    assert_eq!(
        send(
            &app,
            json_request(
                "POST",
                format!("/api/v1/scenes/{scene_id}/activate"),
                json!({}),
            ),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let zone_id = state
        .scene_manager
        .snapshot()
        .await
        .active_scene()
        .and_then(hypercolor_types::scene::Scene::primary_zone)
        .expect("activated scene should have a primary zone")
        .id;
    let uri = format!("/api/v1/scene/zones/{zone_id}/layers");

    let request_a = send(
        &app,
        json_request(
            "POST",
            uri.clone(),
            json!({ "source": { "type": "media", "asset_id": asset_a } }),
        ),
    );
    let request_b = send(
        &app,
        json_request(
            "POST",
            uri,
            json!({ "source": { "type": "media", "asset_id": asset_b } }),
        ),
    );
    let (response_a, response_b) = tokio::join!(request_a, request_b);
    let statuses = [response_a.status(), response_b.status()];

    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CREATED)
            .count(),
        1
    );
    assert!(statuses.iter().any(|status| matches!(
        *status,
        StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY
    )));
    let manager = state.scene_manager.snapshot().await;
    assert_eq!(
        manager
            .active_scene()
            .and_then(hypercolor_types::scene::Scene::primary_zone)
            .expect("active zone should remain")
            .layers
            .len(),
        1
    );
}

#[tokio::test]
async fn live_tree_replace_subtracts_the_addressed_livestream_before_counting() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let asset_a =
        insert_stream_asset(&state, "camera-a.stream", "https://1.1.1.1/live-a.m3u8").await;
    let asset_b =
        insert_stream_asset(&state, "camera-b.stream", "https://8.8.8.8/live-b.m3u8").await;
    let original = media_layer(asset_a);
    let original_id = original.id;
    let scene_id = install_media_scene(&state, vec![original]).await;
    let app = test_app_with_state(Arc::clone(&state));
    assert_eq!(
        send(
            &app,
            json_request(
                "POST",
                format!("/api/v1/scenes/{scene_id}/activate"),
                json!({}),
            ),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let zone_id = state
        .scene_manager
        .snapshot()
        .await
        .active_scene()
        .and_then(hypercolor_types::scene::Scene::primary_zone)
        .expect("activated scene should have a primary zone")
        .id;

    let response = send(
        &app,
        json_request(
            "PUT",
            format!("/api/v1/scene/zones/{zone_id}/layers/{original_id}"),
            json!({
                "source": { "type": "media", "asset_id": asset_b }
            }),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let manager = state.scene_manager.snapshot().await;
    let layers = &manager
        .active_scene()
        .and_then(hypercolor_types::scene::Scene::primary_zone)
        .expect("active zone should remain")
        .layers;
    let [replacement] = layers.as_slice() else {
        panic!("replace should keep exactly one media layer");
    };
    assert_ne!(replacement.id, original_id);
    assert!(matches!(
        replacement.source,
        LayerSource::Media { asset_id, .. } if asset_id == asset_b
    ));
}

#[tokio::test]
async fn activate_scene_downshifts_when_media_cost_exceeds_soft_cap() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let asset_a = insert_mp4_asset(&state, "a.mp4", 1).await;
    let asset_b = insert_mp4_asset(&state, "b.mp4", 2).await;
    let stream_asset =
        insert_stream_asset(&state, "camera.stream", "https://1.1.1.1/live.m3u8").await;
    let scene_id = install_media_scene(
        &state,
        vec![
            media_layer(asset_a),
            media_layer(asset_b),
            media_layer(stream_asset),
        ],
    )
    .await;
    let app = test_app_with_state(Arc::clone(&state));

    assert_eq!(state.render_loop.read().await.stats().tier, FpsTier::Full);

    let response = send(
        &app,
        json_request(
            "POST",
            format!("/api/v1/scenes/{scene_id}/activate"),
            json!({}),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        state
            .scene_manager
            .snapshot()
            .await
            .active_scene_id()
            .copied(),
        Some(scene_id)
    );
    assert_eq!(state.render_loop.read().await.stats().tier, FpsTier::High);
}
