//! Spec 69 §3.6 — per-display default face persistence and precedence.

use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use axum::body::Body;
use http::{Method, Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::effect::EffectEntry;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_daemon::display_preferences::{DisplayPreference, DisplayPreferencesStore};
use hypercolor_daemon::simulators::{SimulatedDisplayConfig, activate_simulated_displays};
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{
    EffectCategory, EffectId, EffectMetadata, EffectSource, EffectState,
};
use tower::ServiceExt;
use uuid::Uuid;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = Arc::new(AppState::new());
    ConfigManager::set_data_dir_override(None);
    (state, tempdir)
}

fn display_config(name: &str) -> SimulatedDisplayConfig {
    SimulatedDisplayConfig {
        id: DeviceId::from_uuid(Uuid::now_v7()),
        name: name.to_owned(),
        width: 480,
        height: 480,
        circular: true,
        enabled: true,
    }
}

async fn register_display(state: &Arc<AppState>, name: &str) -> DeviceId {
    let config = display_config(name).normalized();
    state
        .simulated_displays
        .write()
        .await
        .upsert(config.clone());
    activate_simulated_displays(
        &state.driver_host.discovery_runtime(),
        &state.simulated_displays,
    )
    .await
    .expect("simulated display should activate");
    config.id
}

async fn register_face_effect(state: &Arc<AppState>, name: &str) -> EffectId {
    let metadata = EffectMetadata {
        id: EffectId::from(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} face"),
        category: EffectCategory::Display,
        tags: Vec::new(),
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Html {
            path: format!("/tmp/{name}.html").into(),
        },
        license: None,
    };
    let effect_id = metadata.id;
    let entry = EffectEntry {
        metadata,
        source_path: format!("/tmp/{name}.html").into(),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    };
    let _ = state.effect_registry.write().await.register(entry);
    effect_id
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    serde_json::from_slice(&bytes).expect("failed to parse JSON body")
}

fn json_request(method: Method, uri: String, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request should build")
}

fn get_request(uri: String) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .expect("request should build")
}

fn delete_request(uri: String) -> Request<Body> {
    Request::builder()
        .method(Method::DELETE)
        .uri(uri)
        .body(Body::empty())
        .expect("request should build")
}

async fn send(app: &axum::Router, request: Request<Body>) -> axum::response::Response {
    app.clone()
        .oneshot(request)
        .await
        .expect("request should succeed")
}

async fn put_face(
    app: &axum::Router,
    device_id: DeviceId,
    effect_id: EffectId,
    scope: &str,
) -> serde_json::Value {
    let response = send(
        app,
        json_request(
            Method::PUT,
            format!("/api/v1/displays/{device_id}/face"),
            serde_json::json!({ "effect_id": effect_id.to_string(), "scope": scope }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

async fn get_face(app: &axum::Router, device_id: DeviceId) -> serde_json::Value {
    let response = send(
        app,
        get_request(format!("/api/v1/displays/{device_id}/face")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

// ── Precedence matrix ───────────────────────────────────────────────────

#[tokio::test]
async fn neither_layer_reports_no_assignment() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state, "Bare Display").await;
    let app = api::build_router(Arc::clone(&state), None);

    let payload = get_face(&app, device_id).await;
    assert!(payload["data"].is_null());
}

#[tokio::test]
async fn default_only_is_live_on_the_default_layer() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state, "Default Display").await;
    let effect_id = register_face_effect(&state, "Default Face").await;
    let app = api::build_router(Arc::clone(&state), None);

    let put = put_face(&app, device_id, effect_id, "default").await;
    assert_eq!(put["data"]["live_scope"], "default");
    assert_eq!(put["data"]["default_assigned"], true);
    assert_eq!(put["data"]["scene_assigned"], false);

    let payload = get_face(&app, device_id).await;
    assert_eq!(payload["data"]["live_scope"], "default");
    assert_eq!(payload["data"]["effect"]["id"], effect_id.to_string());

    // The default zone reaches the render groups.
    let scene_manager = state.scene_manager.read().await;
    assert!(scene_manager.active_render_groups().iter().any(|zone| {
        zone.display_target
            .as_ref()
            .is_some_and(|target| target.device_id == device_id)
            && zone.effect_id == Some(effect_id)
    }));
}

#[tokio::test]
async fn scene_only_is_live_on_the_scene_layer() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state, "Scene Display").await;
    let effect_id = register_face_effect(&state, "Scene Face").await;
    let app = api::build_router(Arc::clone(&state), None);

    let put = put_face(&app, device_id, effect_id, "scene").await;
    assert_eq!(put["data"]["live_scope"], "scene");
    assert_eq!(put["data"]["scene_assigned"], true);
    assert_eq!(put["data"]["default_assigned"], false);

    let payload = get_face(&app, device_id).await;
    assert_eq!(payload["data"]["live_scope"], "scene");
}

#[tokio::test]
async fn scene_layer_wins_when_both_are_assigned() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state, "Both Display").await;
    let default_effect = register_face_effect(&state, "Default Face").await;
    let scene_effect = register_face_effect(&state, "Scene Face").await;
    let app = api::build_router(Arc::clone(&state), None);

    put_face(&app, device_id, default_effect, "default").await;
    put_face(&app, device_id, scene_effect, "scene").await;

    let payload = get_face(&app, device_id).await;
    assert_eq!(payload["data"]["live_scope"], "scene");
    assert_eq!(payload["data"]["scene_assigned"], true);
    assert_eq!(payload["data"]["default_assigned"], true);
    assert_eq!(payload["data"]["effect"]["id"], scene_effect.to_string());

    // Only the scene zone renders for the device — the overlay is suppressed.
    let scene_manager = state.scene_manager.read().await;
    let groups_for_device = scene_manager
        .active_render_groups()
        .iter()
        .filter(|zone| {
            zone.display_target
                .as_ref()
                .is_some_and(|target| target.device_id == device_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(groups_for_device.len(), 1);
    assert_eq!(groups_for_device[0].effect_id, Some(scene_effect));
}

// ── Delete semantics ────────────────────────────────────────────────────

#[tokio::test]
async fn deleting_the_scene_layer_falls_back_to_the_default() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state, "Fallback Display").await;
    let default_effect = register_face_effect(&state, "Default Face").await;
    let scene_effect = register_face_effect(&state, "Scene Face").await;
    let app = api::build_router(Arc::clone(&state), None);

    put_face(&app, device_id, default_effect, "default").await;
    put_face(&app, device_id, scene_effect, "scene").await;

    let response = send(
        &app,
        delete_request(format!("/api/v1/displays/{device_id}/face?scope=scene")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let payload = get_face(&app, device_id).await;
    assert_eq!(payload["data"]["live_scope"], "default");
    assert_eq!(payload["data"]["effect"]["id"], default_effect.to_string());
}

#[tokio::test]
async fn deleting_the_default_under_a_scene_override_changes_nothing_live() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state, "Shadowed Display").await;
    let default_effect = register_face_effect(&state, "Default Face").await;
    let scene_effect = register_face_effect(&state, "Scene Face").await;
    let app = api::build_router(Arc::clone(&state), None);

    put_face(&app, device_id, default_effect, "default").await;
    put_face(&app, device_id, scene_effect, "scene").await;

    let response = send(
        &app,
        delete_request(format!("/api/v1/displays/{device_id}/face?scope=default")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let payload = body_json(response).await;
    assert_eq!(payload["data"]["deleted"], true);
    assert_eq!(payload["data"]["scope"], "default");

    let live = get_face(&app, device_id).await;
    assert_eq!(live["data"]["live_scope"], "scene");
    assert_eq!(live["data"]["default_assigned"], false);
}

#[tokio::test]
async fn delete_defaults_to_the_default_scope() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state, "Plain Delete Display").await;
    let effect_id = register_face_effect(&state, "Default Face").await;
    let app = api::build_router(Arc::clone(&state), None);

    put_face(&app, device_id, effect_id, "default").await;
    let response = send(
        &app,
        delete_request(format!("/api/v1/displays/{device_id}/face")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let payload = get_face(&app, device_id).await;
    assert!(payload["data"].is_null());
}

// ── Controls routing ────────────────────────────────────────────────────

#[tokio::test]
async fn control_patches_route_to_the_default_layer_when_it_is_live() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state, "Controls Display").await;
    let effect_id = register_face_effect(&state, "Default Face").await;
    let app = api::build_router(Arc::clone(&state), None);

    put_face(&app, device_id, effect_id, "default").await;
    let response = send(
        &app,
        json_request(
            Method::PATCH,
            format!("/api/v1/displays/{device_id}/face/controls"),
            serde_json::json!({ "controls": { "accent": "#e135ff" } }),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let payload = body_json(response).await;
    assert_eq!(payload["data"]["live_scope"], "default");

    // The stored preference carries the merged control.
    let store = state.display_preferences.read().await;
    let preference = store.get(device_id).expect("preference should exist");
    assert!(preference.controls.contains_key("accent"));
}

// ── Store round-trip ────────────────────────────────────────────────────

#[test]
fn store_round_trips_preferences_to_disk() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let path = tempdir.path().join("display-preferences.json");
    let device_id = DeviceId::from_uuid(Uuid::now_v7());
    let preference = DisplayPreference {
        effect_id: EffectId::from(Uuid::now_v7()),
        controls: std::collections::HashMap::new(),
        blend_mode: hypercolor_types::scene::DisplayFaceBlendMode::Alpha,
        opacity: 0.8,
    };

    let mut store = DisplayPreferencesStore::new(path.clone());
    store.set(device_id, preference.clone());
    store.save().expect("store should save");

    let reloaded = DisplayPreferencesStore::load(&path).expect("store should load");
    assert_eq!(reloaded.get(device_id), Some(&preference));
}

// ── Scene-switch survival ───────────────────────────────────────────────

#[tokio::test]
async fn default_face_survives_scene_switches() {
    let (state, _tempdir) = isolated_state();
    let device_id = register_display(&state, "Survivor Display").await;
    let effect_id = register_face_effect(&state, "Default Face").await;
    let app = api::build_router(Arc::clone(&state), None);

    put_face(&app, device_id, effect_id, "default").await;

    for name in ["Scene One", "Scene Two"] {
        let response = send(
            &app,
            json_request(
                Method::POST,
                "/api/v1/scenes".to_owned(),
                serde_json::json!({ "name": name }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let scene_id = body_json(response).await["data"]["id"]
            .as_str()
            .expect("scene id should be a string")
            .to_owned();
        let response = send(
            &app,
            json_request(
                Method::POST,
                format!("/api/v1/scenes/{scene_id}/activate"),
                serde_json::json!({}),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let payload = get_face(&app, device_id).await;
        assert_eq!(
            payload["data"]["live_scope"], "default",
            "after activating {name}"
        );
        let scene_manager = state.scene_manager.read().await;
        assert!(
            scene_manager.active_render_groups().iter().any(|zone| {
                zone.display_target
                    .as_ref()
                    .is_some_and(|target| target.device_id == device_id)
                    && zone.effect_id == Some(effect_id)
            }),
            "default face should render after activating {name}"
        );
    }
}
