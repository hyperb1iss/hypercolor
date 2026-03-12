use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use hypercolor_tui::client::rest::DaemonClient;
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, ControlValue, PresetTemplate,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

fn client_for(addr: SocketAddr) -> DaemonClient {
    DaemonClient::new("127.0.0.1", addr.port())
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
    assert_eq!(effects[0].controls[0].default_value.as_f32(), Some(0.75));
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
                        "active_effect": "Rainbow Wave"
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
    assert_eq!(status.device_count, 3);
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
                            "backend": "wled",
                            "status": "connected",
                            "brightness": 100,
                            "firmware_version": null,
                            "network_ip": null,
                            "network_hostname": null,
                            "connection_label": null,
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
