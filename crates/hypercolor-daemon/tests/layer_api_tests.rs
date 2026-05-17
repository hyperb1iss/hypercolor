use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::effect::EffectEntry;
use hypercolor_core::scene::make_scene;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, ControlValue, EffectCategory, EffectId,
    EffectMetadata, EffectSource, EffectState,
};
use hypercolor_types::event::{HypercolorEvent, LayerStackChangeKind};
use hypercolor_types::layer::{
    LayerAdjust, LayerBlendMode, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::{RenderGroup, RenderGroupId, RenderGroupRole, SceneId};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
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
        zones: vec![DeviceZone {
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
) -> (SceneId, RenderGroupId) {
    let mut scene = make_scene("Layered Scene");
    let group = RenderGroup {
        id: RenderGroupId::new(),
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
        role: RenderGroupRole::Primary,
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
