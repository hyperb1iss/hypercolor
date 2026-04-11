use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Body;
use http::{Method, Request};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_daemon::simulators::{
    SimulatedDisplayConfig, SimulatedDisplayStore, activate_simulated_displays,
    default_layout_device_id, logical_device_ids_for_simulator,
};
use hypercolor_types::device::{DeviceId, DeviceState};
use tower::ServiceExt;
use uuid::Uuid;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn simulator_config(enabled: bool) -> SimulatedDisplayConfig {
    SimulatedDisplayConfig {
        id: DeviceId::from_uuid(Uuid::now_v7()),
        name: "Wave 1 Simulator".to_owned(),
        width: 480,
        height: 480,
        circular: true,
        enabled,
    }
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    serde_json::from_slice(&bytes).expect("failed to parse JSON body")
}

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

#[test]
fn simulated_display_store_round_trips_configs() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let path = tempdir.path().join("simulated-displays.json");
    let config = simulator_config(true).normalized();

    let mut store = SimulatedDisplayStore::new(path.clone());
    store.upsert(config.clone());
    store.save().expect("simulated display store should save");

    let loaded = SimulatedDisplayStore::load(&path).expect("simulated display store should load");
    assert_eq!(loaded.list(), vec![config]);
}

#[tokio::test]
async fn activate_simulated_displays_registers_virtual_display_in_runtime_surfaces() {
    let (state, _tempdir) = isolated_state();
    let config = simulator_config(true).normalized();
    state
        .simulated_displays
        .write()
        .await
        .upsert(config.clone());

    let activated = activate_simulated_displays(
        &state.driver_host.discovery_runtime(),
        &state.simulated_displays,
    )
    .await
    .expect("simulated displays should activate");
    assert_eq!(activated, vec![config.id]);

    let tracked = state
        .device_registry
        .get(&config.id)
        .await
        .expect("simulated display should be tracked");
    assert!(tracked.state.is_renderable());

    let logical_ids = logical_device_ids_for_simulator(&state.logical_devices, config.id).await;
    assert_eq!(logical_ids, vec![default_layout_device_id(&config)]);

    {
        let mut manager = state.backend_manager.lock().await;
        manager
            .write_device_display_frame("simulator", config.id, &[1, 2, 3])
            .await
            .expect("simulated backend should accept display writes");
    }

    let app = api::build_router(Arc::clone(&state), None);

    let displays = body_json(
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/displays")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed"),
    )
    .await;
    let display_id = config.id.to_string();
    assert_eq!(displays["data"][0]["id"], display_id);
    assert_eq!(displays["data"][0]["name"], config.name);
    assert_eq!(displays["data"][0]["circular"], config.circular);

    let devices = body_json(
        app.oneshot(
            Request::builder()
                .uri("/api/v1/devices")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should succeed"),
    )
    .await;
    assert_eq!(devices["data"]["items"][0]["id"], display_id);
    assert_eq!(devices["data"]["items"][0]["backend"], "simulator");
}

#[tokio::test]
async fn activate_simulated_displays_keeps_disabled_simulator_non_renderable() {
    let (state, _tempdir) = isolated_state();
    let config = simulator_config(false).normalized();
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
    .expect("simulated displays should activate");

    let tracked = state
        .device_registry
        .get(&config.id)
        .await
        .expect("disabled simulated display should still be tracked");
    assert_eq!(tracked.state, DeviceState::Disabled);
}

#[tokio::test]
async fn simulated_display_crud_routes_update_runtime_state() {
    let (state, _tempdir) = isolated_state();
    let app = api::build_router(Arc::clone(&state), None);

    let created = body_json(
        app.clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/simulators/displays")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "name": "Desk Preview",
                            "width": 320,
                            "height": 240,
                            "circular": false,
                            "enabled": true
                        })
                        .to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed"),
    )
    .await;
    let device_id: DeviceId = created["data"]["id"]
        .as_str()
        .expect("created simulator should include an id")
        .parse()
        .expect("created simulator id should parse");

    let tracked = state
        .device_registry
        .get(&device_id)
        .await
        .expect("created simulator should be tracked");
    assert!(tracked.state.is_renderable());

    let patched = body_json(
        app.clone()
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri(format!("/api/v1/simulators/displays/{device_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "enabled": false,
                            "circular": true
                        })
                        .to_string(),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed"),
    )
    .await;
    assert_eq!(patched["data"]["enabled"], false);
    assert_eq!(patched["data"]["circular"], true);
    let tracked = state
        .device_registry
        .get(&device_id)
        .await
        .expect("patched simulator should still be tracked");
    assert_eq!(tracked.state, DeviceState::Disabled);

    let deleted = body_json(
        app.oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/api/v1/simulators/displays/{device_id}"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should succeed"),
    )
    .await;
    assert_eq!(deleted["data"]["deleted"], true);
    assert!(state.device_registry.get(&device_id).await.is_none());
    assert!(
        logical_device_ids_for_simulator(&state.logical_devices, device_id)
            .await
            .is_empty()
    );
    assert!(
        state
            .simulated_displays
            .read()
            .await
            .get(device_id)
            .is_none()
    );
}
