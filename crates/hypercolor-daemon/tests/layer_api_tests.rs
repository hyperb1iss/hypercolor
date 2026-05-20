use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::asset::{AssetTypeHint, AssetUploadOptions};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::effect::EffectEntry;
use hypercolor_core::engine::FpsTier;
use hypercolor_core::scene::make_scene;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_types::asset::AssetId;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, ControlValue, EffectCategory, EffectId,
    EffectMetadata, EffectSource, EffectState,
};
use hypercolor_types::event::{HypercolorEvent, LayerStackChangeKind};
use hypercolor_types::layer::{
    LayerAdjust, LayerBlendMode, LayerSource, LayerTransform, MediaPlayback, SceneLayer,
    SceneLayerId,
};
use hypercolor_types::scene::{DisplayFaceTarget, SceneId, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use tower::ServiceExt;
use uuid::Uuid;

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

fn empty_request(method: &str, uri: String) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .expect("request should build")
}

fn if_match(mut request: Request<Body>, version: u64) -> Request<Body> {
    request.headers_mut().insert(
        http::header::IF_MATCH,
        http::HeaderValue::from_str(&format!("\"{version}\"")).expect("valid etag"),
    );
    request
}

fn response_etag(response: &axum::response::Response) -> String {
    response
        .headers()
        .get(http::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .expect("response should include ETag")
        .to_owned()
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

fn sample_effect(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} description"),
        category: EffectCategory::Ambient,
        tags: vec!["test".to_owned()],
        controls: vec![ControlDefinition {
            id: "speed".to_owned(),
            name: "Speed".to_owned(),
            kind: ControlKind::Number,
            control_type: ControlType::Slider,
            default_value: ControlValue::Float(0.5),
            min: Some(0.0),
            max: Some(100.0),
            step: Some(0.5),
            labels: Vec::new(),
            group: Some("General".to_owned()),
            tooltip: None,
            aspect_lock: None,
            preview_source: None,
            binding: None,
        }],
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: format!("builtin/{name}").into(),
        },
        license: None,
    }
}

async fn insert_effect(state: &Arc<AppState>, name: &str) -> EffectMetadata {
    let metadata = sample_effect(name);
    let entry = EffectEntry {
        metadata: metadata.clone(),
        source_path: format!("/tmp/{name}.rs").into(),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    };
    let mut registry = state.effect_registry.write().await;
    assert!(registry.register(entry).is_none());
    metadata
}

fn effect_layer(effect_id: EffectId, speed: f32) -> SceneLayer {
    SceneLayer::from_effect(
        SceneLayerId::new(),
        effect_id,
        HashMap::from([("speed".into(), ControlValue::Float(speed))]),
        HashMap::new(),
        None,
    )
}

async fn install_scene(
    state: &Arc<AppState>,
    effect_id: EffectId,
    layers: Vec<SceneLayer>,
) -> (SceneId, ZoneId) {
    let mut scene = make_scene("Layered Scene");
    let group = Zone {
        id: ZoneId::new(),
        name: "Primary".to_owned(),
        description: None,
        effect_id: Some(effect_id),
        controls: HashMap::from([("speed".into(), ControlValue::Float(0.5))]),
        control_bindings: HashMap::new(),
        preset_id: None,
        layers,
        layout: sample_layout("desk:main"),
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: ZoneRole::Primary,
        controls_version: 0,
        layers_version: 0,
    };
    let scene_id = scene.id;
    let group_id = group.id;
    scene.groups = vec![group];

    let mut manager = state.scene_manager.write().await;
    manager.create(scene).expect("scene should create");
    manager
        .activate(&scene_id, None)
        .expect("scene should activate");
    (scene_id, group_id)
}

async fn install_scene_with_two_groups(
    state: &Arc<AppState>,
    effect_id: EffectId,
) -> (SceneId, ZoneId, ZoneId) {
    let mut scene = make_scene("Broadcast Scene");
    let primary = Zone {
        id: ZoneId::new(),
        name: "Primary".to_owned(),
        description: None,
        effect_id: Some(effect_id),
        controls: HashMap::from([("speed".into(), ControlValue::Float(0.5))]),
        control_bindings: HashMap::new(),
        preset_id: None,
        layers: Vec::new(),
        layout: sample_layout("desk:main"),
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: ZoneRole::Primary,
        controls_version: 0,
        layers_version: 0,
    };
    let display = Zone {
        id: ZoneId::new(),
        name: "AIO Display".to_owned(),
        description: None,
        effect_id: Some(effect_id),
        controls: HashMap::from([("speed".into(), ControlValue::Float(0.25))]),
        control_bindings: HashMap::new(),
        preset_id: None,
        layers: Vec::new(),
        layout: sample_layout("display:aio"),
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: Some(DisplayFaceTarget::new(DeviceId::new())),
        role: ZoneRole::Display,
        controls_version: 0,
        layers_version: 0,
    };
    let scene_id = scene.id;
    let primary_id = primary.id;
    let display_id = display.id;
    scene.groups = vec![primary, display];

    let mut manager = state.scene_manager.write().await;
    manager.create(scene).expect("scene should create");
    manager
        .activate(&scene_id, None)
        .expect("scene should activate");
    (scene_id, primary_id, display_id)
}

async fn insert_lottie_asset(state: &Arc<AppState>) -> hypercolor_types::asset::AssetId {
    let mut options = AssetUploadOptions::new("sparkle.json");
    options.type_hint = Some(AssetTypeHint::Lottie);
    let upsert = state
        .asset_library
        .write()
        .await
        .add_bytes(br#"{"v":"5.7.4","layers":[]}"#, options)
        .expect("lottie asset should upload");
    upsert.record.id
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
        blend: LayerBlendMode::Alpha,
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
    scene.groups = vec![Zone {
        id: ZoneId::new(),
        name: "Media".to_owned(),
        description: None,
        effect_id: None,
        controls: HashMap::new(),
        control_bindings: HashMap::new(),
        preset_id: None,
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

    let mut manager = state.scene_manager.write().await;
    manager.create(scene).expect("scene should create");
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
        empty_request("POST", format!("/api/v1/scenes/{scene_id}/activate")),
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
    assert_ne!(
        state.scene_manager.read().await.active_scene_id().copied(),
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
        empty_request("POST", format!("/api/v1/scenes/{scene_id}/activate")),
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
        state.scene_manager.read().await.active_scene_id().copied(),
        Some(scene_id)
    );
}

#[tokio::test]
async fn active_scene_broadcast_enforces_livestream_cap() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let effect = insert_effect(&state, "stream-admission").await;
    let first_stream =
        insert_stream_asset(&state, "camera-a.stream", "https://1.1.1.1/live-a.m3u8").await;
    let second_stream =
        insert_stream_asset(&state, "camera-b.stream", "https://8.8.8.8/live-b.m3u8").await;
    let (scene_id, group_id) =
        install_scene(&state, effect.id, vec![media_layer(first_stream)]).await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = send(
        &app,
        json_request(
            "POST",
            format!("/api/v1/scenes/{scene_id}/layers/broadcast-media"),
            serde_json::json!({
                "asset_id": second_stream,
                "targets": [{ "group_id": group_id }]
            }),
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
}

#[tokio::test]
async fn inactive_scene_broadcast_skips_livestream_cap() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let effect = insert_effect(&state, "stream-admission-inactive").await;
    let first_stream =
        insert_stream_asset(&state, "camera-a.stream", "https://1.1.1.1/live-a.m3u8").await;
    let second_stream =
        insert_stream_asset(&state, "camera-b.stream", "https://8.8.8.8/live-b.m3u8").await;
    let (target_scene, group_id) =
        install_scene(&state, effect.id, vec![media_layer(first_stream)]).await;
    // Installing a second scene activates it, leaving target_scene inactive.
    let _active = install_scene(&state, effect.id, Vec::new()).await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = send(
        &app,
        json_request(
            "POST",
            format!("/api/v1/scenes/{target_scene}/layers/broadcast-media"),
            serde_json::json!({
                "asset_id": second_stream,
                "targets": [{ "group_id": group_id }]
            }),
        ),
    )
    .await;

    assert_ne!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
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
        empty_request("POST", format!("/api/v1/scenes/{scene_id}/activate")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        state.scene_manager.read().await.active_scene_id().copied(),
        Some(scene_id)
    );
    assert_eq!(state.render_loop.read().await.stats().tier, FpsTier::High);
}

#[tokio::test]
async fn layer_crud_returns_etags_and_stale_versions() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let effect = insert_effect(&state, "aurora").await;
    let (scene_id, group_id) = install_scene(&state, effect.id, Vec::new()).await;
    let app = test_app_with_state(Arc::clone(&state));
    let base_uri = format!("/api/v1/scenes/{scene_id}/groups/{group_id}/layers");
    let mut events = state.event_bus.subscribe_all();

    let list_response = send(&app, empty_request("GET", base_uri.clone())).await;
    assert_eq!(list_response.status(), StatusCode::OK);
    assert_eq!(response_etag(&list_response), "\"0\"");
    let list_json = body_json(list_response).await;
    assert_eq!(
        list_json["data"]["items"].as_array().expect("items").len(),
        1
    );

    let create_body = serde_json::json!({
        "source": { "type": "color_fill", "rgba": [1.0, 0.0, 0.5, 1.0] },
        "blend": "alpha",
        "opacity": 0.75
    });
    let create_response = send(
        &app,
        if_match(
            json_request("POST", base_uri.clone(), create_body.clone()),
            0,
        ),
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::CREATED);
    assert_eq!(response_etag(&create_response), "\"1\"");
    let event = events.recv().await.expect("layer stack changed event");
    assert!(matches!(
        event.event,
        HypercolorEvent::RenderGroupChanged { .. }
    ));
    let event = events.recv().await.expect("layer stack changed event");
    assert!(matches!(
        event.event,
        HypercolorEvent::LayerStackChanged {
            scene_id: event_scene_id,
            group_id: event_group_id,
            layers_version: 1,
            kind: LayerStackChangeKind::Created,
        } if event_scene_id == scene_id && event_group_id == group_id
    ));
    let create_json = body_json(create_response).await;
    assert_eq!(create_json["data"]["layers_version"], 1);
    assert_eq!(
        create_json["data"]["items"]
            .as_array()
            .expect("items")
            .len(),
        2
    );

    let stale_response = send(
        &app,
        if_match(json_request("POST", base_uri, create_body), 0),
    )
    .await;
    assert_eq!(stale_response.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(response_etag(&stale_response), "\"1\"");
    let stale_json = body_json(stale_response).await;
    assert_eq!(stale_json["current"], 1);
}

#[tokio::test]
async fn scene_wide_media_broadcast_creates_layers_per_group() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let effect = insert_effect(&state, "broadcast").await;
    let asset_id = insert_lottie_asset(&state).await;
    let (scene_id, primary_id, display_id) = install_scene_with_two_groups(&state, effect.id).await;
    let app = test_app_with_state(Arc::clone(&state));
    let uri = format!("/api/v1/scenes/{scene_id}/layers/broadcast-media");
    let mut events = state.event_bus.subscribe_all();

    let response = send(
        &app,
        json_request(
            "POST",
            uri.clone(),
            serde_json::json!({
                "name": "Sparkle",
                "asset_id": asset_id,
                "blend": "screen",
                "opacity": 0.6,
                "targets": [
                    {
                        "group_id": primary_id,
                        "expected_layers_version": 0,
                        "transform": {
                            "anchor": { "x": 0.25, "y": 0.5 },
                            "scale": [1.0, 1.0],
                            "rotation": 0.0,
                            "fit": "contain"
                        }
                    },
                    {
                        "group_id": display_id,
                        "expected_layers_version": 0,
                        "transform": {
                            "anchor": { "x": 0.75, "y": 0.5 },
                            "scale": [0.8, 0.8],
                            "rotation": 0.0,
                            "fit": "cover"
                        }
                    }
                ]
            }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    let groups = json["data"]["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0]["group_id"], primary_id.to_string());
    assert_eq!(groups[0]["layers_version"], 1);
    assert_eq!(groups[0]["items"].as_array().expect("items").len(), 2);
    assert_eq!(groups[0]["items"][1]["source"]["type"], "media");
    assert_eq!(
        groups[0]["items"][1]["source"]["asset_id"],
        asset_id.to_string()
    );
    assert_eq!(groups[0]["items"][1]["transform"]["anchor"]["x"], 0.25);
    assert_eq!(groups[1]["group_id"], display_id.to_string());
    assert_eq!(groups[1]["layers_version"], 1);
    assert_eq!(groups[1]["items"][1]["transform"]["anchor"]["x"], 0.75);

    let mut changed_groups = Vec::new();
    while changed_groups.len() < 2 {
        let event = events.recv().await.expect("layer event");
        if let HypercolorEvent::LayerStackChanged {
            scene_id: event_scene_id,
            group_id,
            layers_version: 1,
            kind: LayerStackChangeKind::Created,
        } = event.event
        {
            assert_eq!(event_scene_id, scene_id);
            changed_groups.push(group_id);
        }
    }
    assert!(changed_groups.contains(&primary_id));
    assert!(changed_groups.contains(&display_id));

    let stale = send(
        &app,
        json_request(
            "POST",
            uri,
            serde_json::json!({
                "name": "Stale Sparkle",
                "asset_id": asset_id,
                "targets": [
                    {
                        "group_id": primary_id,
                        "expected_layers_version": 0
                    }
                ]
            }),
        ),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(response_etag(&stale), "\"1\"");
}

#[tokio::test]
async fn scene_wide_media_broadcast_rejects_missing_group() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let effect = insert_effect(&state, "broadcast-missing-group").await;
    let asset_id = insert_lottie_asset(&state).await;
    let (scene_id, primary_id, _display_id) =
        install_scene_with_two_groups(&state, effect.id).await;
    let app = test_app_with_state(Arc::clone(&state));
    let uri = format!("/api/v1/scenes/{scene_id}/layers/broadcast-media");

    let missing_group_id = ZoneId::new();
    let response = send(
        &app,
        json_request(
            "POST",
            uri,
            serde_json::json!({
                "asset_id": asset_id,
                "targets": [
                    { "group_id": primary_id },
                    { "group_id": missing_group_id }
                ]
            }),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("message should be a string")
            .contains("Zone not found")
    );
}

#[tokio::test]
async fn layer_reorder_rejects_bad_membership_and_returns_next_version() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let effect = insert_effect(&state, "pulse").await;
    let base = effect_layer(effect.id, 0.5);
    let overlay = SceneLayer {
        id: SceneLayerId::new(),
        name: None,
        source: LayerSource::ColorFill {
            rgba: [0.0, 0.0, 1.0, 1.0],
        },
        blend: LayerBlendMode::Alpha,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    };
    let base_id = base.id;
    let overlay_id = overlay.id;
    let (scene_id, group_id) = install_scene(&state, effect.id, vec![base, overlay]).await;
    let app = test_app_with_state(Arc::clone(&state));
    let uri = format!("/api/v1/scenes/{scene_id}/groups/{group_id}/layers/order");

    let bad_response = send(
        &app,
        if_match(
            json_request(
                "PATCH",
                uri.clone(),
                serde_json::json!({ "layer_ids": [overlay_id] }),
            ),
            0,
        ),
    )
    .await;
    assert_eq!(bad_response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let reorder_response = send(
        &app,
        if_match(
            json_request(
                "PATCH",
                uri,
                serde_json::json!({ "layer_ids": [overlay_id, base_id] }),
            ),
            0,
        ),
    )
    .await;
    assert_eq!(reorder_response.status(), StatusCode::OK);
    assert_eq!(response_etag(&reorder_response), "\"1\"");
    let reorder_json = body_json(reorder_response).await;
    assert_eq!(reorder_json["data"]["layers_version"], 1);
    assert_eq!(
        reorder_json["data"]["items"][0]["id"],
        overlay_id.to_string()
    );
}

#[tokio::test]
async fn layer_controls_patch_uses_layers_version() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let effect = insert_effect(&state, "controls").await;
    let layer = effect_layer(effect.id, 0.5);
    let layer_id = layer.id;
    let (scene_id, group_id) = install_scene(&state, effect.id, vec![layer]).await;
    let app = test_app_with_state(Arc::clone(&state));
    let uri = format!("/api/v1/scenes/{scene_id}/groups/{group_id}/layers/{layer_id}/controls");

    let response = send(
        &app,
        if_match(
            json_request(
                "PATCH",
                uri.clone(),
                serde_json::json!({ "controls": { "speed": 1.5 } }),
            ),
            0,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_etag(&response), "\"1\"");
    let json = body_json(response).await;
    assert_eq!(json["data"]["layers_version"], 1);
    assert_eq!(
        json["data"]["items"][0]["source"]["controls"]["speed"]["float"],
        1.5
    );

    let stale = send(
        &app,
        if_match(
            json_request(
                "PATCH",
                uri,
                serde_json::json!({ "controls": { "speed": 2.0 } }),
            ),
            0,
        ),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn top_level_current_controls_rejects_multiple_effect_layers() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let effect = insert_effect(&state, "multi").await;
    let layer_a = effect_layer(effect.id, 0.5);
    let layer_b = effect_layer(effect.id, 1.0);
    install_scene(&state, effect.id, vec![layer_a, layer_b]).await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = send(
        &app,
        json_request(
            "PATCH",
            "/api/v1/effects/current/controls".to_owned(),
            serde_json::json!({ "controls": { "speed": 2.0 } }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("message should be a string")
            .contains("layer controls endpoint")
    );
}
