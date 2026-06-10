use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use hypercolor_tui::client::rest::DaemonClient;
use hypercolor_types::controls::{
    ApplyControlChangesRequest, ControlActionStatus, ControlChange,
    ControlValue as SurfaceControlValue,
};
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
async fn get_effects_enriches_summaries_with_detail_controls() {
    let router = Router::new()
        .route(
            "/api/v1/effects",
            get(|| async {
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
                            "audio_reactive": false
                        }],
                        "pagination": {
                            "offset": 0,
                            "limit": 50,
                            "total": 1,
                            "has_more": false
                        }
                    }
                }))
            }),
        )
        .route(
            "/api/v1/effects/{id}",
            get(|Path(id): Path<String>| async move {
                assert_eq!(id, "rainbow");

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
                    name: "Soft".to_string(),
                    description: Some("Low energy".to_string()),
                    controls: HashMap::from([("speed".to_string(), ControlValue::Float(0.4))]),
                }];
                let active_control_values =
                    HashMap::from([("speed".to_string(), ControlValue::Float(0.75))]);

                Json(json!({
                    "data": {
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
                        "presets": presets,
                        "active_control_values": active_control_values
                    }
                }))
            }),
        );

    let client = client_for(spawn_server(router).await);
    let effects = client.get_effects().await.expect("fetch effects");

    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].id, "rainbow");
    assert_eq!(effects[0].controls.len(), 1);
    assert_eq!(effects[0].controls[0].id, "speed");
    assert_eq!(effects[0].controls[0].control_type, "slider");
    // True defaults are preserved — live values are per-zone and must NOT
    // be merged over `default_value` (the daemon's active_control_values
    // reflect only the primary zone).
    assert_eq!(effects[0].controls[0].default_value.as_f32(), Some(0.25));
    assert_eq!(effects[0].presets.len(), 1);
}

#[tokio::test]
async fn get_status_maps_system_and_active_effect_responses() {
    let router = Router::new()
        .route(
            "/api/v1/status",
            get(|| async {
                Json(json!({
                    "data": {
                        "running": true,
                        "global_brightness": 42,
                        "device_count": 3,
                        "active_effect": "Rainbow Wave",
                        "active_scene": "Focus",
                        "active_scene_snapshot_locked": true
                    }
                }))
            }),
        )
        .route(
            "/api/v1/effects/active",
            get(|| async {
                Json(json!({
                    "data": {
                        "id": "rainbow",
                        "name": "Rainbow Wave",
                        "state": "running",
                        "controls": [],
                        "control_values": {},
                        "active_preset_id": null
                    }
                }))
            }),
        );

    let client = client_for(spawn_server(router).await);
    let status = client.get_status().await.expect("fetch status");

    assert!(status.running);
    assert_eq!(status.brightness, 42);
    assert_eq!(status.effect_id.as_deref(), Some("rainbow"));
    assert_eq!(status.effect_name.as_deref(), Some("Rainbow Wave"));
    assert_eq!(status.scene_name.as_deref(), Some("Focus"));
    assert!(status.scene_snapshot_locked);
    assert_eq!(status.device_count, 3);
}

#[tokio::test]
async fn rest_client_sends_bearer_token_when_configured() {
    let router = Router::new().route(
        "/api/v1/status",
        get(|headers: HeaderMap| async move {
            assert_eq!(
                headers
                    .get(header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok()),
                Some("Bearer hc_tui_test")
            );
            Json(json!({
                "data": {
                    "running": true,
                    "global_brightness": 1,
                    "device_count": 0,
                    "active_effect": null,
                    "active_scene": null,
                    "active_scene_snapshot_locked": false
                }
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
                        "pagination": {
                            "offset": 0,
                            "limit": 50,
                            "total": 1,
                            "has_more": false
                        }
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
                        "pagination": {
                            "offset": 0,
                            "limit": 50,
                            "total": 1,
                            "has_more": false
                        }
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
            "/api/v1/control-surfaces/{surface_id}",
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
            "/api/v1/control-surfaces/{surface_id}/values",
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
            "/api/v1/control-surfaces/{surface_id}/actions/{action_id}",
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
    let request = ApplyControlChangesRequest {
        surface_id: "driver:wled:device:Desk Strip".to_string(),
        expected_revision: Some(3),
        changes: vec![ControlChange {
            field_id: "enabled".to_string(),
            value: SurfaceControlValue::Bool(true),
        }],
        dry_run: false,
    };
    let response = client
        .apply_control_changes(&request)
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
            "surface_id": "driver:wled:device:Desk Strip",
            "expected_revision": 3,
            "changes": [{
                "field_id": "enabled",
                "value": { "kind": "bool", "value": true }
            }],
            "dry_run": false
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

#[tokio::test]
async fn update_control_wraps_payload_under_controls() {
    let captured = Arc::new(Mutex::new(None::<Value>));
    let router =
        Router::new()
            .route(
                "/api/v1/effects/current/controls",
                patch(
                    |State(captured): State<Arc<Mutex<Option<Value>>>>,
                     Json(payload): Json<Value>| async move {
                        *captured.lock().await = Some(payload);
                        Json(json!({ "data": { "applied": { "speed": { "float": 0.5 } } } }))
                    },
                ),
            )
            .with_state(Arc::clone(&captured));

    let client = client_for(spawn_server(router).await);
    client
        .update_control("speed", &json!(0.5))
        .await
        .expect("update control");

    let payload = captured.lock().await.clone().expect("captured payload");
    assert_eq!(payload, json!({ "controls": { "speed": 0.5 } }));
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
                        Json(json!({ "data": { "created": true } }))
                    },
                ),
            )
            .with_state(Arc::clone(&captured));

    let client = client_for(spawn_server(ok_router).await);
    client
        .toggle_favorite("rainbow", false)
        .await
        .expect("add favorite");

    let payload = captured
        .lock()
        .await
        .clone()
        .expect("captured favorite payload");
    assert_eq!(payload, json!({ "effect": "rainbow" }));

    let error_router = Router::new().route(
        "/api/v1/library/favorites",
        post(|| async {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid favorite payload" })),
            )
        }),
    );
    let failing_client = client_for(spawn_server(error_router).await);

    let error = failing_client.toggle_favorite("rainbow", false).await;
    assert!(error.is_err());
}

// ── Scenes & zones ──────────────────────────────────────────────────

const ZONE_A: &str = "11111111-1111-1111-1111-111111111111";
const ZONE_B: &str = "22222222-2222-2222-2222-222222222222";
// Zone.effect_id is an EffectId (UUID) on the wire — the same UUID string
// the REST effect list exposes as `EffectSummary.id`.
const EFFECT_RAINBOW: &str = "33333333-3333-3333-3333-333333333333";

fn zone_json(id: &str, name: &str, role: &str, effect: Option<&str>, enabled: bool) -> Value {
    json!({
        "id": id,
        "name": name,
        "description": null,
        "effect_id": effect,
        "controls": { "speed": { "float": 0.6 } },
        "layout": {
            "id": "layout",
            "name": "layout",
            "description": null,
            "canvas_width": 320,
            "canvas_height": 200,
            "zones": [],
            "spaces": null,
            "version": 1
        },
        "color": null,
        "role": role,
        "enabled": enabled,
        "brightness": 0.8,
        "controls_version": 3,
        "layers_version": 7
    })
}

#[tokio::test]
async fn get_active_scene_maps_zones_and_lock_state() {
    let router = Router::new().route(
        "/api/v1/scenes/active",
        get(|| async {
            Json(json!({
                "data": {
                    "id": "scene-1",
                    "name": "Desk",
                    "description": null,
                    "enabled": true,
                    "priority": 0,
                    "kind": "named",
                    "mutation_mode": "snapshot",
                    "groups": [
                        zone_json(ZONE_A, "Primary", "primary", Some(EFFECT_RAINBOW), true),
                        zone_json(ZONE_B, "Shelf", "custom", None, false),
                    ],
                    "groups_revision": 42,
                    "unassigned_behavior": "off"
                }
            }))
        }),
    );

    let client = client_for(spawn_server(router).await);
    let scene = client
        .get_active_scene()
        .await
        .expect("fetch active scene")
        .expect("scene should be present");

    assert_eq!(scene.id, "scene-1");
    assert_eq!(scene.name, "Desk");
    assert!(scene.snapshot_locked);
    assert_eq!(scene.groups_revision, 42);
    assert!(scene.multi_zone());
    assert_eq!(scene.zones.len(), 2);

    let primary = scene.primary().expect("primary zone");
    assert_eq!(primary.id, ZONE_A);
    assert!(primary.is_primary);
    assert_eq!(primary.effect_id.as_deref(), Some(EFFECT_RAINBOW));
    assert!(primary.enabled);
    assert_eq!(primary.controls_version, 3);
    assert_eq!(primary.layers_version, 7);
    assert!((primary.brightness - 0.8).abs() < f32::EPSILON);
    assert_eq!(
        primary
            .controls
            .get("speed")
            .and_then(hypercolor_tui::state::ControlValue::as_f32),
        Some(0.6)
    );

    let shelf = scene.zone(ZONE_B).expect("shelf zone");
    assert!(!shelf.is_primary);
    assert!(!shelf.enabled);
    assert_eq!(shelf.effect_id, None);
}

#[tokio::test]
async fn get_active_scene_returns_none_without_active_scene() {
    let router = Router::new().route(
        "/api/v1/scenes/active",
        get(|| async {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "No active scene" })),
            )
        }),
    );

    let client = client_for(spawn_server(router).await);
    let scene = client.get_active_scene().await.expect("fetch active scene");
    assert!(scene.is_none());
}

#[tokio::test]
async fn get_scenes_maps_summaries() {
    let router = Router::new().route(
        "/api/v1/scenes",
        get(|| async {
            Json(json!({
                "data": {
                    "items": [
                        {
                            "id": "scene-1",
                            "name": "Desk",
                            "description": "two zones",
                            "enabled": true,
                            "priority": 0,
                            "mutation_mode": "live"
                        },
                        {
                            "id": "scene-2",
                            "name": "Party",
                            "description": null,
                            "enabled": true,
                            "priority": 1,
                            "mutation_mode": "snapshot"
                        }
                    ],
                    "pagination": { "offset": 0, "limit": 50, "total": 2, "has_more": false }
                }
            }))
        }),
    );

    let client = client_for(spawn_server(router).await);
    let scenes = client.get_scenes().await.expect("fetch scenes");

    assert_eq!(scenes.len(), 2);
    assert_eq!(scenes[0].id, "scene-1");
    assert_eq!(scenes[1].name, "Party");
    assert_eq!(
        scenes[1].mutation_mode,
        hypercolor_types::scene::SceneMutationMode::Snapshot
    );
}

#[tokio::test]
async fn apply_effect_sends_render_group_and_controls() {
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
                    Json(json!({ "data": {} }))
                },
            ),
        )
        .with_state(Arc::clone(&captured));

    let client = client_for(spawn_server(router).await);
    client
        .apply_effect("rainbow", Some(&json!({ "speed": 0.5 })), Some(ZONE_B))
        .await
        .expect("apply effect");

    let body = captured.lock().await.clone().expect("captured apply body");
    assert_eq!(body["render_group"], json!(ZONE_B));
    assert_eq!(body["controls"], json!({ "speed": 0.5 }));
}

#[tokio::test]
async fn apply_effect_omits_render_group_for_primary() {
    let captured: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/effects/{id}/apply",
            post(
                |State(captured): State<Arc<Mutex<Option<Value>>>>,
                 Json(body): Json<Value>| async move {
                    *captured.lock().await = Some(body);
                    Json(json!({ "data": {} }))
                },
            ),
        )
        .with_state(Arc::clone(&captured));

    let client = client_for(spawn_server(router).await);
    client
        .apply_effect("rainbow", None, None)
        .await
        .expect("apply effect");

    let body = captured.lock().await.clone().expect("captured apply body");
    assert_eq!(body, json!({}));
}

#[tokio::test]
async fn update_zone_sends_if_match_revision() {
    let router = Router::new().route(
        "/api/v1/scenes/{id}/zones/{zone_id}",
        patch(
            |Path((scene_id, zone_id)): Path<(String, String)>,
             headers: HeaderMap,
             Json(body): Json<Value>| async move {
                assert_eq!(scene_id, "scene-1");
                assert_eq!(zone_id, ZONE_B);
                assert_eq!(
                    headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()),
                    Some("42")
                );
                assert_eq!(body, json!({ "enabled": false }));
                Json(json!({ "data": {} }))
            },
        ),
    );

    let client = client_for(spawn_server(router).await);
    client
        .update_zone("scene-1", ZONE_B, 42, Some(false), None)
        .await
        .expect("update zone");
}

#[tokio::test]
async fn patch_zone_controls_targets_legacy_layer_without_if_match() {
    let router = Router::new().route(
        "/api/v1/scenes/{id}/groups/{group_id}/layers/{layer_id}/controls",
        patch(
            |Path((scene_id, group_id, layer_id)): Path<(String, String, String)>,
             headers: HeaderMap,
             Json(body): Json<Value>| async move {
                assert_eq!(scene_id, "scene-1");
                // Legacy layer id == zone id
                assert_eq!(group_id, ZONE_B);
                assert_eq!(layer_id, ZONE_B);
                assert!(headers.get(header::IF_MATCH).is_none());
                assert_eq!(body, json!({ "controls": { "speed": 0.9 } }));
                Json(json!({ "data": {} }))
            },
        ),
    );

    let client = client_for(spawn_server(router).await);
    client
        .patch_zone_controls("scene-1", ZONE_B, &json!({ "speed": 0.9 }))
        .await
        .expect("patch zone controls");
}

#[tokio::test]
async fn activate_and_deactivate_scene_hit_expected_routes() {
    let router = Router::new()
        .route(
            "/api/v1/scenes/{id}/activate",
            post(|Path(id): Path<String>| async move {
                assert_eq!(id, "scene-2");
                Json(json!({ "data": {} }))
            }),
        )
        .route(
            "/api/v1/scenes/deactivate",
            post(|| async { Json(json!({ "data": {} })) }),
        );

    let client = client_for(spawn_server(router).await);
    client.activate_scene("scene-2").await.expect("activate");
    client.deactivate_scene().await.expect("deactivate");
}

#[tokio::test]
async fn reset_controls_scopes_to_render_group() {
    let captured: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/effects/current/reset",
            post(
                |State(captured): State<Arc<Mutex<Option<Value>>>>,
                 Json(body): Json<Value>| async move {
                    *captured.lock().await = Some(body);
                    Json(json!({ "data": {} }))
                },
            ),
        )
        .with_state(Arc::clone(&captured));

    let client = client_for(spawn_server(router).await);

    client
        .reset_controls(Some(ZONE_B))
        .await
        .expect("zone-scoped reset");
    assert_eq!(
        captured.lock().await.clone().expect("captured body"),
        json!({ "render_group": ZONE_B })
    );

    client.reset_controls(None).await.expect("primary reset");
    assert_eq!(
        captured.lock().await.clone().expect("captured body"),
        json!({})
    );
}
