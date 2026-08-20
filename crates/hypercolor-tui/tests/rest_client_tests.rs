use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use hypercolor_tui::client::rest::DaemonClient;
use hypercolor_tui::state::{
    primary_zone, scene_is_multi_zone, zone_effect_controls, zone_effect_id, zone_effect_layer,
};
use hypercolor_types::api::scene::PatchControlsRequest;
use hypercolor_types::control::ControlValue as CanonicalControlValue;
use hypercolor_types::controls::ControlActionStatus;
use hypercolor_types::effect::{
    ControlBinding, ControlDefinition, ControlKind, ControlType, ControlValue, PresetTemplate,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

type CapturedControlPayloads = (Arc<Mutex<Option<Value>>>, Arc<Mutex<Option<Value>>>);

fn client_for(addr: SocketAddr) -> DaemonClient {
    DaemonClient::new("127.0.0.1", addr.port(), None)
}

async fn spawn_server(router: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener addr");

    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve test router");
    });

    addr
}

fn encoded_preview_bytes() -> Vec<u8> {
    let image = image::RgbImage::from_vec(2, 1, vec![255, 0, 0, 0, 255, 0])
        .expect("preview pixels should match dimensions");
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .expect("preview image should encode");
    bytes
}

#[tokio::test]
async fn get_effects_hydrates_the_catalog_in_one_round_trip() {
    let detail_hits = Arc::new(Mutex::new(0_usize));
    let seen_query = Arc::new(Mutex::new(None::<String>));

    let controls = vec![ControlDefinition {
        id: "speed".to_string(),
        name: "Speed".to_string(),
        kind: ControlKind::Number,
        control_type: ControlType::Slider,
        default_value: ControlValue::Float(0.25),
        min: Some(0.0),
        max: Some(1.0),
        step: Some(0.05),
        labels: Vec::new(),
        group: None,
        tooltip: None,
        aspect_lock: None,
        preview_source: None,
        binding: Some(ControlBinding {
            sensor: "cpu_temp".to_string(),
            sensor_min: 30.0,
            sensor_max: 100.0,
            target_min: 0.0,
            target_max: 1.0,
            deadband: 0.5,
            smoothing: 0.2,
        }),
    }];
    let presets = vec![PresetTemplate {
        id: hypercolor_types::library::PresetId::stable("Soft"),
        name: "Soft".to_string(),
        description: Some("Low energy".to_string()),
        controls: HashMap::from([("speed".to_string(), ControlValue::Float(0.4))]),
    }];

    let router = Router::new()
        .route(
            "/api/v1/effects",
            get({
                let seen_query = Arc::clone(&seen_query);
                move |uri: Uri| {
                    let seen_query = Arc::clone(&seen_query);
                    let controls = controls.clone();
                    let presets = presets.clone();
                    async move {
                        *seen_query.lock().await = uri.query().map(str::to_owned);
                        Json(json!({
                            "data": {
                                "items": [{
                                    "id": "rainbow",
                                    "name": "Rainbow Wave",
                                    "description": "Soft motion",
                                    "author": "hyperb1iss",
                                    "category": "ambient",
                                    "source": "native",
                                    "runnable": true,
                                    "tags": ["wave"],
                                    "version": "1.0.0",
                                    "audio_reactive": false,
                                    "controls": controls,
                                    "presets": presets
                                }],
                                "total": 1
                            }
                        }))
                    }
                }
            }),
        )
        // The per-effect detail route stays mounted as a tripwire: a
        // regression back to per-row hydration would light it up.
        .route(
            "/api/v1/effects/{id}",
            get({
                let detail_hits = Arc::clone(&detail_hits);
                move |Path(_id): Path<String>| {
                    let detail_hits = Arc::clone(&detail_hits);
                    async move {
                        *detail_hits.lock().await += 1;
                        Json(json!({ "data": {} }))
                    }
                }
            }),
        );

    let client = client_for(spawn_server(router).await);
    let effects = client.get_effects().await.expect("fetch effects");

    assert_eq!(
        seen_query.lock().await.as_deref(),
        Some("include=controls,presets"),
        "the catalog request asks the daemon to expand each summary"
    );
    assert_eq!(
        *detail_hits.lock().await,
        0,
        "hydrating the catalog costs no per-effect round trips"
    );

    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].id, "rainbow");
    assert_eq!(effects[0].controls.len(), 1);
    assert_eq!(effects[0].controls[0].id, "speed");
    assert_eq!(effects[0].controls[0].control_type, "slider");
    // True defaults are preserved — live values are per-zone and must NOT
    // be merged over `default_value`.
    assert_eq!(effects[0].controls[0].default_value.as_f32(), Some(0.25));
    assert_eq!(effects[0].presets.len(), 1);
    assert_eq!(
        effects[0].presets[0].id,
        hypercolor_types::library::PresetId::stable("Soft").to_string()
    );
}

#[tokio::test]
async fn get_status_maps_the_system_response_without_an_active_effect_call() {
    let router = Router::new().route(
        "/api/v1/system",
        get(|| async {
            Json(json!({
                "data": { "status": {
                    "running": true,
                    "global_brightness": 42,
                    "device_count": 3,
                    "render_loop": {
                        "state": "running",
                        "fps_tier": "sixty",
                        "target_fps": 60,
                        "ceiling_fps": 60,
                        "capacity_fps": 59.8,
                        "delivered_fps": 59.4,
                        "actual_fps": 59.8,
                        "consecutive_misses": 0,
                        "total_frames": 12_000
                    }
                }}
            }))
        }),
    );

    let client = client_for(spawn_server(router).await);
    let status = client.get_status().await.expect("fetch status");

    assert!(status.running);
    assert_eq!(status.brightness, 42);
    assert_eq!(status.device_count, 3);
    // FPS lives under render_loop. A flat field here would read zero and
    // the status view would render a stalled render loop forever.
    assert_eq!(status.fps_target, 60.0);
    assert_eq!(status.fps_actual, 59.8);
}

/// A status refresh must not erase FPS.
///
/// Every daemon event triggers a REST status refresh, and the refreshed
/// state replaces the whole `DaemonState` a metrics tick just filled in.
/// While this mapping hardcoded zeros, FPS survived at most one metrics
/// interval before the next event blanked it, so the dashboard gauge and
/// title bar effectively never showed a number.
#[tokio::test]
async fn get_status_reads_fps_from_the_render_loop_block() {
    let router = Router::new().route(
        "/api/v1/system",
        get(|| async {
            Json(json!({
                "data": { "status": {
                    "running": true,
                    "global_brightness": 80,
                    "device_count": 0,
                    "active_effect": null,
                    "active_scene": null,
                    "render_loop": {
                        "state": "running",
                        "target_fps": 45,
                        "actual_fps": 44.2
                    }
                }}
            }))
        }),
    );

    let client = client_for(spawn_server(router).await);
    let status = client.get_status().await.expect("fetch status");

    assert_eq!(status.fps_target, 45.0);
    assert_eq!(status.fps_actual, 44.2);
}

/// An absent render_loop block is tolerated rather than fatal.
#[tokio::test]
async fn get_status_survives_a_status_payload_without_a_render_loop() {
    let router = Router::new().route(
        "/api/v1/system",
        get(|| async {
            Json(json!({
                "data": { "status": {
                    "running": false,
                    "global_brightness": 10,
                    "device_count": 0,
                    "active_effect": null,
                    "active_scene": null
                }}
            }))
        }),
    );

    let client = client_for(spawn_server(router).await);
    let status = client.get_status().await.expect("fetch status");

    assert_eq!(status.fps_target, 0.0);
    assert_eq!(status.fps_actual, 0.0);
}

#[tokio::test]
async fn rest_client_sends_bearer_token_when_configured() {
    let router = Router::new().route(
        "/api/v1/system",
        get(|headers: HeaderMap| async move {
            assert_eq!(
                headers
                    .get(header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok()),
                Some("Bearer hc_tui_test")
            );
            Json(json!({
                "data": { "status": {
                    "running": true,
                    "global_brightness": 1,
                    "device_count": 0
                }}
            }))
        }),
    );

    let addr = spawn_server(router).await;
    let client = DaemonClient::new("127.0.0.1", addr.port(), Some("hc_tui_test"));

    client.get_status().await.expect("fetch status");
}

#[tokio::test]
async fn get_devices_and_favorites_parse_enveloped_lists() {
    let router = Router::new()
        .route(
            "/api/v1/devices",
            get(|| async {
                Json(json!({
                    "data": {
                        "items": [{
                            "id": "device-1",
                            "layout_device_id": "layout-1",
                            "name": "Desk Strip",
                            "origin": {
                                "driver_id": "wled",
                                "backend_id": "wled",
                                "transport": "network",
                                "protocol_id": null
                            },
                            "presentation": {
                                "label": "WLED",
                                "short_label": "WLED",
                                "accent_rgb": [255, 106, 193],
                                "secondary_rgb": [128, 255, 234],
                                "icon": "lightbulb",
                                "default_device_class": "controller"
                            },
                            "status": "connected",
                            "brightness": 100,
                            "firmware_version": null,
                            "connection": {
                                "transport": "network",
                                "label": null,
                                "endpoint": null,
                                "ip": null,
                                "hostname": null
                            },
                            "total_leds": 120,
                            "zones": []
                        }],
                        "total": 1
                    }
                }))
            }),
        )
        .route(
            "/api/v1/library/favorites",
            get(|| async {
                Json(json!({
                    "data": {
                        "items": [{
                            "effect_id": "rainbow",
                            "effect_name": "Rainbow Wave",
                            "added_at_ms": 1234
                        }],
                        "total": 1
                    }
                }))
            }),
        );

    let client = client_for(spawn_server(router).await);
    let devices = client.get_devices().await.expect("fetch devices");
    let favorites = client.get_favorites().await.expect("fetch favorites");

    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].family, "wled");
    assert_eq!(devices[0].state, "connected");
    assert_eq!(devices[0].led_count, 120);
    assert_eq!(favorites, vec!["rainbow".to_string()]);
}

#[tokio::test]
async fn control_surface_list_encodes_device_query() {
    let captured_uri = Arc::new(Mutex::new(None::<String>));
    let router = Router::new()
        .route(
            "/api/v1/control-surfaces",
            get(
                |State(captured_uri): State<Arc<Mutex<Option<String>>>>, uri: Uri| async move {
                    *captured_uri.lock().await = Some(uri.to_string());
                    Json(json!({
                        "data": {
                            "surfaces": [{
                                "surface_id": "device:Desk Strip",
                                "scope": {
                                    "device": {
                                        "device_id": "00000000-0000-0000-0000-000000000001",
                                        "driver_id": "wled"
                                    }
                                },
                                "schema_version": 1,
                                "revision": 4,
                                "groups": [],
                                "fields": [],
                                "actions": [],
                                "values": {},
                                "availability": {},
                                "action_availability": {}
                            }]
                        }
                    }))
                },
            ),
        )
        .with_state(Arc::clone(&captured_uri));

    let client = client_for(spawn_server(router).await);
    let surfaces = client
        .get_device_control_surfaces("Desk Strip", true)
        .await
        .expect("fetch device control surfaces");

    assert_eq!(surfaces.len(), 1);
    assert_eq!(surfaces[0].surface_id, "device:Desk Strip");
    assert_eq!(
        captured_uri.lock().await.as_deref(),
        Some("/api/v1/control-surfaces?device_id=Desk%20Strip&include_driver=true")
    );
}

#[tokio::test]
async fn control_surface_list_returns_empty_for_missing_device_surface() {
    let router = Router::new().route(
        "/api/v1/control-surfaces",
        get(|| async { StatusCode::NOT_FOUND }),
    );

    let client = client_for(spawn_server(router).await);
    let surfaces = client
        .get_device_control_surfaces("missing-device", true)
        .await
        .expect("missing device controls should be empty");

    assert!(surfaces.is_empty());
}

#[tokio::test]
async fn get_control_surface_encodes_full_surface_id() {
    let captured_uri = Arc::new(Mutex::new(None::<String>));
    let router = Router::new()
        .route(
            "/api/v1/control-surfaces/{id}",
            get(
                |Path(surface_id): Path<String>,
                 State(captured_uri): State<Arc<Mutex<Option<String>>>>,
                 uri: Uri| async move {
                    assert_eq!(surface_id, "driver:wled:device:Desk Strip");
                    *captured_uri.lock().await = Some(uri.to_string());
                    Json(json!({
                        "data": {
                            "surface_id": "driver:wled:device:Desk Strip",
                            "scope": {
                                "device": {
                                    "device_id": "00000000-0000-0000-0000-000000000001",
                                    "driver_id": "wled"
                                }
                            },
                            "schema_version": 1,
                            "revision": 7,
                            "groups": [],
                            "fields": [],
                            "actions": [],
                            "values": {},
                            "availability": {},
                            "action_availability": {}
                        }
                    }))
                },
            ),
        )
        .with_state(Arc::clone(&captured_uri));

    let client = client_for(spawn_server(router).await);
    let surface = client
        .get_control_surface("driver:wled:device:Desk Strip")
        .await
        .expect("fetch control surface");

    assert_eq!(surface.surface_id, "driver:wled:device:Desk Strip");
    assert_eq!(
        captured_uri.lock().await.as_deref(),
        Some("/api/v1/control-surfaces/driver%3Awled%3Adevice%3ADesk%20Strip")
    );
}

#[tokio::test]
async fn control_surface_mutations_encode_path_ids_and_payloads() {
    let captured_patch = Arc::new(Mutex::new(None::<Value>));
    let captured_action = Arc::new(Mutex::new(None::<Value>));
    let router = Router::new()
        .route(
            "/api/v1/control-surfaces/{id}/values",
            patch(
                |Path(surface_id): Path<String>,
                 State((captured_patch, _captured_action)): State<CapturedControlPayloads>,
                 Json(payload): Json<Value>| async move {
                    assert_eq!(surface_id, "driver:wled:device:Desk Strip");
                    *captured_patch.lock().await = Some(payload);
                    Json(json!({
                        "data": {
                            "surface_id": "driver:wled:device:Desk Strip",
                            "previous_revision": 3,
                            "revision": 4,
                            "accepted": [],
                            "rejected": [],
                            "impacts": [],
                            "values": {}
                        }
                    }))
                },
            ),
        )
        .route(
            "/api/v1/control-surfaces/{id}/actions/{action}",
            post(
                |Path((surface_id, action_id)): Path<(String, String)>,
                 State((_captured_patch, captured_action)): State<CapturedControlPayloads>,
                 Json(payload): Json<Value>| async move {
                    assert_eq!(surface_id, "driver:wled:device:Desk Strip");
                    assert_eq!(action_id, "refresh topology");
                    *captured_action.lock().await = Some(payload);
                    Json(json!({
                        "data": {
                            "surface_id": "driver:wled:device:Desk Strip",
                            "action_id": "refresh topology",
                            "status": "completed",
                            "result": null,
                            "revision": 4
                        }
                    }))
                },
            ),
        )
        .with_state((Arc::clone(&captured_patch), Arc::clone(&captured_action)));

    let client = client_for(spawn_server(router).await);
    let request = PatchControlsRequest {
        values: BTreeMap::from([("enabled".to_string(), CanonicalControlValue::Bool(true))]),
        clear_bindings: Vec::new(),
    };
    let response = client
        .apply_control_changes("driver:wled:device:Desk Strip", &request)
        .await
        .expect("apply controls");
    let result = client
        .invoke_control_action(
            "driver:wled:device:Desk Strip",
            "refresh topology",
            BTreeMap::default(),
        )
        .await
        .expect("invoke action");

    assert_eq!(response.revision, 4);
    assert_eq!(result.status, ControlActionStatus::Completed);
    assert_eq!(
        captured_patch.lock().await.as_ref(),
        Some(&json!({
            "values": {
                "enabled": { "kind": "bool", "value": true }
            }
        }))
    );
    assert_eq!(
        captured_action.lock().await.as_ref(),
        Some(&json!({ "input": {} }))
    );
}

#[tokio::test]
async fn get_simulated_displays_and_frame_decode_preview_image() {
    let frame_bytes = encoded_preview_bytes();
    let router = Router::new()
        .route(
            "/api/v1/simulators/displays",
            get(|| async {
                Json(json!({
                    "data": [{
                        "id": "sim-1",
                        "name": "Desk Preview",
                        "width": 480,
                        "height": 480,
                        "circular": true,
                        "enabled": true
                    }]
                }))
            }),
        )
        .route(
            "/api/v1/simulators/displays/{id}/frame",
            get(move |Path(id): Path<String>| {
                let bytes = frame_bytes.clone();
                async move {
                    assert_eq!(id, "sim-1");
                    (StatusCode::OK, bytes)
                }
            }),
        );

    let client = client_for(spawn_server(router).await);
    let simulators = client
        .get_simulated_displays()
        .await
        .expect("fetch simulators");
    let frame = client
        .get_simulated_display_frame("sim-1")
        .await
        .expect("fetch simulator frame")
        .expect("simulator frame should exist");

    assert_eq!(simulators.len(), 1);
    assert_eq!(simulators[0].id, "sim-1");
    assert_eq!(simulators[0].name, "Desk Preview");
    assert_eq!(frame.width, 2);
    assert_eq!(frame.height, 1);
    assert_eq!(frame.pixels.as_ref(), &[255, 0, 0, 0, 255, 0]);
}

#[tokio::test]
async fn get_simulated_display_frame_returns_none_for_missing_frame() {
    let router = Router::new().route(
        "/api/v1/simulators/displays/{id}/frame",
        get(|Path(id): Path<String>| async move {
            assert_eq!(id, "sim-missing");
            StatusCode::NOT_FOUND
        }),
    );

    let client = client_for(spawn_server(router).await);
    let frame = client
        .get_simulated_display_frame("sim-missing")
        .await
        .expect("missing simulator frame should not error");

    assert!(frame.is_none());
}

const SCENE_ID: &str = "0198c5b6-1111-7000-8000-000000000001";
const ZONE_A: &str = "0198c5b6-1111-7000-8000-000000000002";
const ZONE_B: &str = "0198c5b6-1111-7000-8000-000000000003";
const LAYER_ID: &str = "0198c5b6-1111-7000-8000-000000000004";
const EFFECT_RAINBOW: &str = "0198c5b6-1111-7000-8000-000000000005";

fn scene_document() -> Value {
    json!({
        "id": SCENE_ID,
        "name": "Desk",
        "kind": "named",
        "is_default": false,
        "mutation_mode": "snapshot",
        "unassigned_behavior": "off",
        "layout_id": null,
        "revision": 42,
        "zones": [
            {
                "id": ZONE_A,
                "name": "Primary",
                "role": "primary",
                "enabled": true,
                "brightness": 0.8,
                "color": null,
                "display_target": null,
                "members": [],
                "layout": null,
                "layers": [{
                    "id": LAYER_ID,
                    "source": {
                        "type": "effect",
                        "effect_id": EFFECT_RAINBOW,
                        "controls": {"speed": {"float": 0.6}}
                    },
                    "blend": "replace",
                    "opacity": 1.0
                }]
            },
            {
                "id": ZONE_B,
                "name": "Shelf",
                "role": "custom",
                "enabled": false,
                "brightness": 1.0,
                "color": null,
                "display_target": null,
                "members": [],
                "layout": null,
                "layers": []
            }
        ]
    })
}

#[tokio::test]
async fn update_control_targets_the_real_scene_layer() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let router = Router::new()
        .route(
            "/api/v1/scene",
            get(|| async { Json(json!({"data": scene_document()})) }),
        )
        .route(
            "/api/v1/scene/zones/{zone}/layers/{layer}/controls",
            patch(
                |State(captured): State<Arc<Mutex<Option<Value>>>>,
                 Path((zone, layer)): Path<(String, String)>,
                 Json(payload): Json<Value>| async move {
                    assert_eq!(zone, ZONE_A);
                    assert_eq!(layer, LAYER_ID);
                    *captured.lock().await = Some(payload);
                    Json(json!({"data": {}}))
                },
            ),
        )
        .with_state(Arc::clone(&captured));

    let client = client_for(spawn_server(router).await);
    client
        .update_control("speed", &json!(0.5))
        .await
        .expect("update control");

    assert_eq!(
        captured.lock().await.clone().expect("captured payload"),
        json!({"values": {"speed": {"kind": "float", "value": 0.5}}})
    );
}

#[tokio::test]
async fn toggle_favorite_uses_effect_field_and_checks_errors() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let ok_router =
        Router::new()
            .route(
                "/api/v1/library/favorites",
                post(
                    |State(captured): State<Arc<Mutex<Option<Value>>>>,
                     Json(payload): Json<Value>| async move {
                        *captured.lock().await = Some(payload);
                        Json(json!({"data": {"created": true}}))
                    },
                ),
            )
            .with_state(Arc::clone(&captured));

    let client = client_for(spawn_server(ok_router).await);
    client
        .toggle_favorite("rainbow", false)
        .await
        .expect("add favorite");
    assert_eq!(
        captured
            .lock()
            .await
            .clone()
            .expect("captured favorite payload"),
        json!({"effect": "rainbow"})
    );

    let error_router = Router::new().route(
        "/api/v1/library/favorites",
        post(|| async {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": {
                        "code": "validation_error",
                        "message": "invalid favorite payload"
                    },
                    "meta": {
                        "api_version": "1.0",
                        "request_id": "req_test",
                        "timestamp": "2026-08-16T00:00:00.000Z"
                    }
                })),
            )
        }),
    );
    let failing_client = client_for(spawn_server(error_router).await);
    assert!(
        failing_client
            .toggle_favorite("rainbow", false)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn get_active_scene_returns_the_canonical_document_without_a_scene_list_fetch() {
    let router = Router::new().route(
        "/api/v1/scene",
        get(|| async { Json(json!({"data": scene_document()})) }),
    );

    let client = client_for(spawn_server(router).await);
    let scene = client.get_active_scene().await.expect("fetch active scene");

    assert_eq!(scene.id.to_string(), SCENE_ID);
    assert_eq!(
        scene.mutation_mode,
        hypercolor_types::scene::SceneMutationMode::Snapshot
    );
    assert_eq!(scene.revision, 42);
    assert!(scene_is_multi_zone(&scene));

    let primary = primary_zone(&scene).expect("primary zone");
    assert_eq!(
        zone_effect_layer(primary).map(|layer| layer.id.to_string()),
        Some(LAYER_ID.to_owned())
    );
    assert_eq!(zone_effect_id(primary).as_deref(), Some(EFFECT_RAINBOW));
    assert_eq!(
        zone_effect_controls(primary)
            .get("speed")
            .and_then(hypercolor_tui::state::ControlValue::as_f32),
        Some(0.6)
    );
}

#[tokio::test]
async fn apply_effect_uses_canonical_zone_and_control_values() {
    let captured: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/effects/{id}/apply",
            post(
                |State(captured): State<Arc<Mutex<Option<Value>>>>,
                 Path(id): Path<String>,
                 Json(body): Json<Value>| async move {
                    assert_eq!(id, "rainbow");
                    *captured.lock().await = Some(body);
                    Json(json!({"data": {}}))
                },
            ),
        )
        .with_state(Arc::clone(&captured));

    let client = client_for(spawn_server(router).await);
    client
        .apply_effect("rainbow", Some(&json!({"speed": 0.5})), Some(ZONE_B))
        .await
        .expect("apply effect");

    assert_eq!(
        captured.lock().await.clone().expect("captured apply body"),
        json!({
            "zone": ZONE_B,
            "controls": {"speed": {"kind": "float", "value": 0.5}}
        })
    );
}

#[tokio::test]
async fn zone_mutations_use_live_scene_routes() {
    let router = Router::new()
        .route(
            "/api/v1/scene/zones/{zone}",
            patch(
                |Path(zone): Path<String>, headers: HeaderMap, Json(body): Json<Value>| async move {
                    assert_eq!(zone, ZONE_B);
                    assert_eq!(
                        headers
                            .get(header::IF_MATCH)
                            .and_then(|value| value.to_str().ok()),
                        Some("42")
                    );
                    assert_eq!(body, json!({"enabled": false}));
                    Json(json!({"data": {}}))
                },
            ),
        )
        .route(
            "/api/v1/scene/zones/{zone}/layers/{layer}/controls",
            patch(
                |Path((zone, layer)): Path<(String, String)>,
                 headers: HeaderMap,
                 Json(body): Json<Value>| async move {
                    assert_eq!(zone, ZONE_B);
                    assert_eq!(layer, LAYER_ID);
                    assert!(headers.get(header::IF_MATCH).is_none());
                    assert_eq!(
                        body,
                        json!({"values": {"speed": {"kind": "float", "value": 0.9_f32}}})
                    );
                    Json(json!({"data": {}}))
                },
            ),
        );

    let client = client_for(spawn_server(router).await);
    client
        .update_zone(ZONE_B, 42, Some(false), None)
        .await
        .expect("update zone");
    client
        .patch_zone_controls(ZONE_B, LAYER_ID, &json!({"speed": 0.9}))
        .await
        .expect("patch controls");
}

#[tokio::test]
async fn activate_and_deactivate_scene_hit_expected_routes() {
    let router = Router::new()
        .route(
            "/api/v1/scenes/{id}/activate",
            post(|Path(id): Path<String>| async move {
                assert_eq!(id, "scene-2");
                Json(json!({"data": {}}))
            }),
        )
        .route(
            "/api/v1/scene/deactivate",
            post(|| async { Json(json!({"data": {}})) }),
        );

    let client = client_for(spawn_server(router).await);
    client.activate_scene("scene-2").await.expect("activate");
    client.deactivate_scene().await.expect("deactivate");
}

#[tokio::test]
async fn reset_controls_replaces_the_layer_without_apply_side_effects() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let router = Router::new()
        .route(
            "/api/v1/scene",
            get(|| async { Json(json!({"data": scene_document()})) }),
        )
        .route(
            "/api/v1/effects/{id}",
            get(|Path(id): Path<String>| async move {
                assert_eq!(id, EFFECT_RAINBOW);
                Json(json!({
                    "data": {
                        "id": EFFECT_RAINBOW,
                        "name": "Rainbow",
                        "description": "test",
                        "author": "test",
                        "category": "ambient",
                        "source": "native",
                        "runnable": true,
                        "tags": [],
                        "version": "1",
                        "audio_reactive": false,
                        "controls": []
                    }
                }))
            }),
        )
        .route(
            "/api/v1/scene/zones/{zone}/layers/{layer}",
            put(
                |State(captured): State<Arc<Mutex<Option<Value>>>>,
                 Path((zone, layer)): Path<(String, String)>,
                 Json(body): Json<Value>| async move {
                    assert_eq!(zone, ZONE_A);
                    assert_eq!(layer, LAYER_ID);
                    *captured.lock().await = Some(body);
                    Json(json!({"data": {}}))
                },
            ),
        )
        .with_state(Arc::clone(&captured));

    let client = client_for(spawn_server(router).await);
    client.reset_controls(None).await.expect("reset controls");
    let body = captured.lock().await.clone().expect("captured reset body");
    assert_eq!(body["source"]["type"], "effect");
    assert_eq!(body["source"]["effect_id"], EFFECT_RAINBOW);
    assert_eq!(body["source"]["controls"], json!({}));
    assert!(body["source"].get("preset_id").is_none());
    assert_eq!(body["blend"], "replace");
    assert_eq!(body["enabled"], true);
}
