use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use axum::body::Body;
use http::{Method, Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::scene::make_scene;
use hypercolor_daemon::api::{self, AppState};
use hypercolor_daemon::display_frames::DisplayFrameSnapshot;
use hypercolor_daemon::runtime_state;
use hypercolor_daemon::scene_transactions::apply_layout_update;
use hypercolor_daemon::simulators::{
    SimulatedDisplayConfig, SimulatedDisplayStore, activate_simulated_displays,
    default_layout_device_id, logical_device_ids_for_simulator,
};
use hypercolor_types::device::{DeviceId, DeviceState};
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
};
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

fn simulated_display_face_effect(name: &str) -> EffectMetadata {
    EffectMetadata {
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
    }
}

fn simulated_display_face_layout(id: &str) -> SpatialLayout {
    SpatialLayout {
        id: format!("display-face-layout-{id}"),
        name: format!("Display Face Layout {id}"),
        description: None,
        canvas_width: 320,
        canvas_height: 240,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    serde_json::from_slice(&bytes).expect("failed to parse JSON body")
}

async fn body_bytes(response: axum::response::Response) -> axum::body::Bytes {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body")
}

async fn publish_display_frame(
    state: &Arc<AppState>,
    config: &SimulatedDisplayConfig,
    jpeg: Vec<u8>,
) {
    state.display_frames.write().await.set_frame(
        config.id,
        DisplayFrameSnapshot {
            jpeg_data: Arc::new(jpeg),
            width: config.width,
            height: config.height,
            circular: config.circular,
            frame_number: 1,
            captured_at: SystemTime::now(),
        },
    );
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
async fn simulated_display_backend_reuses_owned_jpeg_payloads() {
    let (state, _tempdir) = isolated_state();
    let config = simulator_config(true).normalized();
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

    let jpeg = Arc::new(vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10]);
    {
        let mut manager = state.backend_manager.lock().await;
        manager
            .write_device_display_frame_owned("simulator", config.id, Arc::clone(&jpeg))
            .await
            .expect("simulated backend should retain owned display frames");
    }

    let stored = state
        .simulated_display_runtime
        .read()
        .await
        .frame(config.id)
        .expect("simulated runtime should capture the display frame");
    assert!(
        Arc::ptr_eq(&stored.jpeg_data, &jpeg),
        "simulated display runtime should reuse the owned JPEG payload",
    );
}

#[tokio::test]
async fn simulated_display_backend_ignores_empty_led_writes_but_rejects_real_led_payloads() {
    let (state, _tempdir) = isolated_state();
    let config = simulator_config(true).normalized();
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

    let mut manager = state.backend_manager.lock().await;
    manager
        .write_device_colors("simulator", config.id, &[])
        .await
        .expect("display-only simulators should ignore empty LED writes");

    let error = manager
        .write_device_colors("simulator", config.id, &[[1, 2, 3]])
        .await
        .expect_err("display-only simulators should reject non-empty LED writes");
    assert!(
        error.chain().any(|cause| {
            let message = cause.to_string();
            message.contains("failed to write 1 colors")
                || message.contains("does not accept LED color writes")
        }),
        "unexpected error: {error}"
    );
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
    let preview_config = SimulatedDisplayConfig {
        id: device_id,
        name: created["data"]["name"]
            .as_str()
            .expect("created simulator should include a name")
            .to_owned(),
        width: u32::try_from(
            created["data"]["width"]
                .as_u64()
                .expect("created simulator should include a width"),
        )
        .expect("created simulator width should fit in u32"),
        height: u32::try_from(
            created["data"]["height"]
                .as_u64()
                .expect("created simulator should include a height"),
        )
        .expect("created simulator height should fit in u32"),
        circular: created["data"]["circular"]
            .as_bool()
            .expect("created simulator should include circular state"),
        enabled: created["data"]["enabled"]
            .as_bool()
            .expect("created simulator should include enabled state"),
    };

    let tracked = state
        .device_registry
        .get(&device_id)
        .await
        .expect("created simulator should be tracked");
    assert!(tracked.state.is_renderable());

    let active_layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };
    let layout_device_id = default_layout_device_id(&preview_config);
    let zone_id = format!("zone_simulator_{device_id}");
    let mut active_layout_with_simulator = active_layout.clone();
    active_layout_with_simulator.zones.push(DeviceZone {
        id: zone_id.clone(),
        name: "Desk Preview Display".to_owned(),
        device_id: layout_device_id.clone(),
        zone_name: None,
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(1.0, 1.0),
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology: LedTopology::Point,
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        display_order: 0,
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    });
    {
        let mut layouts = state.layouts.write().await;
        layouts.insert(
            active_layout_with_simulator.id.clone(),
            active_layout_with_simulator.clone(),
        );
    }
    apply_layout_update(
        &state.spatial_engine,
        &state.scene_manager,
        &state.scene_transactions,
        active_layout_with_simulator.clone(),
    )
    .await;

    {
        let mut manager = state.backend_manager.lock().await;
        manager
            .write_device_display_frame("simulator", device_id, &[9, 8, 7])
            .await
            .expect("simulated backend should capture frame bytes");
    }
    publish_display_frame(&state, &preview_config, vec![7, 8, 9]).await;
    let frame = body_bytes(
        app.clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/simulators/displays/{device_id}/frame"))
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("request should succeed"),
    )
    .await;
    assert_eq!(frame.as_ref(), &[9, 8, 7]);

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
    assert!(state.display_frames.read().await.frame(device_id).is_none());
    assert!(
        state
            .layouts
            .read()
            .await
            .get(&active_layout_with_simulator.id)
            .expect("active layout should remain present")
            .zones
            .iter()
            .all(|zone| zone.device_id != layout_device_id)
    );
    assert!(
        state
            .spatial_engine
            .read()
            .await
            .layout()
            .zones
            .iter()
            .all(|zone| zone.id != zone_id)
    );
}

#[tokio::test]
async fn deleting_simulated_display_prunes_scene_display_groups_and_persists_cleanup() {
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

    let face = simulated_display_face_effect("Desk Clock");
    let named_scene_id = {
        let mut manager = state.scene_manager.write().await;
        manager
            .upsert_display_group(
                device_id,
                "Desk Preview",
                &face,
                HashMap::new(),
                simulated_display_face_layout("default"),
            )
            .expect("default simulator face should be assigned");

        let named_scene = make_scene("Display Scene");
        let named_scene_id = named_scene.id;
        manager
            .create(named_scene)
            .expect("named scene should be created");
        manager
            .activate(&named_scene_id, None)
            .expect("named scene should activate");
        manager
            .upsert_display_group(
                device_id,
                "Desk Preview",
                &face,
                HashMap::new(),
                simulated_display_face_layout("named"),
            )
            .expect("named simulator face should be assigned");
        manager.deactivate_current();
        named_scene_id
    };

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

    {
        let manager = state.scene_manager.read().await;
        let default_scene = manager
            .active_scene()
            .expect("default scene should remain active");
        assert!(default_scene.display_group_for(device_id).is_none());
        let named_scene = manager
            .get(&named_scene_id)
            .expect("named scene should remain present");
        assert!(named_scene.display_group_for(device_id).is_none());
    }

    let persisted =
        runtime_state::load(&state.runtime_state_path).expect("runtime state should load");
    let persisted = persisted.expect("runtime state should exist");
    assert!(
        persisted.default_scene_groups.iter().all(|group| {
            group
                .display_target
                .as_ref()
                .is_none_or(|target| target.device_id != device_id)
        }),
        "deleted simulator should not survive in the persisted default scene"
    );

    let scene_store = state.scene_store.read().await;
    let named_scene = scene_store
        .list()
        .find(|scene| scene.id == named_scene_id)
        .expect("named scene should be persisted");
    assert!(
        named_scene.groups.iter().all(|group| {
            group
                .display_target
                .as_ref()
                .is_none_or(|target| target.device_id != device_id)
        }),
        "deleted simulator should not survive in persisted named scenes"
    );
}

#[tokio::test]
async fn simulated_display_frame_route_falls_back_to_display_preview_cache() {
    let (state, _tempdir) = isolated_state();
    let config = simulator_config(true).normalized();
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

    let jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F'];
    publish_display_frame(&state, &config, jpeg.clone()).await;

    let app = api::build_router(Arc::clone(&state), None);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/simulators/displays/{}/frame", config.id))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/jpeg")
    );
    let body = body_bytes(response).await;
    assert_eq!(body.as_ref(), jpeg.as_slice());
}

#[tokio::test]
async fn simulated_display_frame_route_rejects_non_simulator_display_cache_entries() {
    let (state, _tempdir) = isolated_state();
    let config = simulator_config(true).normalized();
    publish_display_frame(&state, &config, vec![1, 2, 3, 4]).await;

    let app = api::build_router(Arc::clone(&state), None);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/simulators/displays/{}/frame", config.id))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
