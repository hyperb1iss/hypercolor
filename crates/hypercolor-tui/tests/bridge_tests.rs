use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use hypercolor_tui::action::Action;
use hypercolor_tui::bridge::spawn_data_bridge;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct TestState {
    status_calls: Arc<AtomicUsize>,
    scene_calls: Arc<AtomicUsize>,
    control_surface_calls: Arc<AtomicUsize>,
}

async fn assert_canonical_subscription(socket: &mut WebSocket) {
    let Some(Ok(Message::Text(message))) = socket.recv().await else {
        panic!("expected a text subscription request");
    };
    let subscription: serde_json::Value =
        serde_json::from_str(&message).expect("subscription request should be JSON");
    assert_eq!(
        subscription.get("type").and_then(serde_json::Value::as_str),
        Some("subscribe")
    );
    assert!(subscription.get("preview_transport").is_none());
}

#[tokio::test]
async fn active_scene_event_refreshes_the_canonical_document() {
    let status_calls = Arc::new(AtomicUsize::new(0));
    let scene_calls = Arc::new(AtomicUsize::new(0));
    let state = TestState {
        status_calls: Arc::clone(&status_calls),
        scene_calls: Arc::clone(&scene_calls),
        control_surface_calls: Arc::new(AtomicUsize::new(0)),
    };

    let router = Router::new()
        .route("/api/v1/system", get(status_handler))
        .route("/api/v1/effects", get(effects_handler))
        .route("/api/v1/devices", get(devices_handler))
        .route("/api/v1/library/favorites", get(favorites_handler))
        .route("/api/v1/scene", get(scene_handler))
        .route("/api/v1/ws", get(ws_handler))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve test router");
    });

    let cancel = CancellationToken::new();
    let (action_tx, mut action_rx) = mpsc::unbounded_channel();
    let bridge_cancel = cancel.clone();
    let bridge = tokio::spawn(async move {
        spawn_data_bridge(
            "127.0.0.1".to_string(),
            addr.port(),
            None,
            action_tx,
            bridge_cancel,
        )
        .await;
    });

    let updated = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Action::ActiveSceneUpdated(scene)) = action_rx.recv().await
                && scene.name == "Movie Night"
            {
                break (scene.name.clone(), scene.mutation_mode);
            }
        }
    })
    .await
    .expect("timed out waiting for scene status refresh");

    assert_eq!(updated.0, "Movie Night");
    assert_eq!(
        updated.1,
        hypercolor_types::scene::SceneMutationMode::Snapshot
    );
    assert_eq!(status_calls.load(Ordering::SeqCst), 2);
    assert_eq!(scene_calls.load(Ordering::SeqCst), 3);

    cancel.cancel();
    bridge.await.expect("bridge task should join");
    server.abort();
}

#[tokio::test]
async fn control_surface_event_refreshes_device_surface() {
    let control_surface_calls = Arc::new(AtomicUsize::new(0));
    let state = TestState {
        status_calls: Arc::new(AtomicUsize::new(0)),
        scene_calls: Arc::new(AtomicUsize::new(0)),
        control_surface_calls: Arc::clone(&control_surface_calls),
    };

    let router = Router::new()
        .route("/api/v1/system", get(status_handler))
        .route("/api/v1/effects", get(effects_handler))
        .route("/api/v1/devices", get(devices_handler))
        .route("/api/v1/library/favorites", get(favorites_handler))
        .route(
            "/api/v1/control-surfaces/{id}",
            get(control_surface_handler),
        )
        .route("/api/v1/ws", get(control_surface_ws_handler))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve test router");
    });

    let cancel = CancellationToken::new();
    let (action_tx, mut action_rx) = mpsc::unbounded_channel();
    let bridge_cancel = cancel.clone();
    let bridge = tokio::spawn(async move {
        spawn_data_bridge(
            "127.0.0.1".to_string(),
            addr.port(),
            None,
            action_tx,
            bridge_cancel,
        )
        .await;
    });

    let refreshed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Action::DeviceControlSurfaceRefreshed { device_id, surface }) =
                action_rx.recv().await
            {
                break (device_id, surface.surface_id.clone(), surface.revision);
            }
        }
    })
    .await
    .expect("timed out waiting for control surface refresh");

    assert_eq!(refreshed.0, test_device_id());
    assert_eq!(refreshed.1, test_surface_id());
    assert_eq!(refreshed.2, 8);
    assert_eq!(control_surface_calls.load(Ordering::SeqCst), 1);

    cancel.cancel();
    bridge.await.expect("bridge task should join");
    server.abort();
}

async fn status_handler(State(state): State<TestState>) -> Json<serde_json::Value> {
    state.status_calls.fetch_add(1, Ordering::SeqCst);

    Json(serde_json::json!({
        "data": {
          "status": {
            "running": true,
            "global_brightness": 42,
            "device_count": 3,
            "active_effect": serde_json::Value::Null
          }
        }
    }))
}

async fn scene_handler(State(state): State<TestState>) -> Json<serde_json::Value> {
    let call = state.scene_calls.fetch_add(1, Ordering::SeqCst);
    let (name, mutation_mode) = if call <= 1 {
        ("Default", "live")
    } else {
        ("Movie Night", "snapshot")
    };
    Json(serde_json::json!({
        "data": {
            "id": "0198c5b6-1111-7000-8000-000000000001",
            "name": name,
            "kind": "named",
            "is_default": false,
            "mutation_mode": mutation_mode,
            "revision": call + 1,
            "zones": []
        }
    }))
}

async fn effects_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "data": {
            "items": [],
            "total": 0,
            "page": {
                "offset": 0,
                "limit": 50,
                "has_more": false
            }
        }
    }))
}

async fn devices_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "data": {
            "items": [],
            "total": 0,
            "page": {
                "offset": 0,
                "limit": 50,
                "has_more": false
            }
        }
    }))
}

async fn favorites_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "data": {
            "items": [],
            "total": 0
        }
    }))
}

async fn control_surface_handler(
    Path(surface_id): Path<String>,
    State(state): State<TestState>,
) -> Json<serde_json::Value> {
    assert_eq!(surface_id, test_surface_id());
    state.control_surface_calls.fetch_add(1, Ordering::SeqCst);

    Json(serde_json::json!({
        "data": {
            "surface_id": test_surface_id(),
            "scope": {
                "device": {
                    "device_id": test_device_id(),
                    "driver_id": "wled"
                }
            },
            "schema_version": 1,
            "revision": 8,
            "groups": [],
            "fields": [],
            "actions": [],
            "values": {},
            "availability": {},
            "action_availability": {}
        }
    }))
}

async fn ws_handler(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(|mut socket| async move {
        let hello = serde_json::json!({
            "type": "hello",
            "state": {
                "running": true,
                "paused": false,
                "brightness": 42,
                "fps": {
                    "target": 60,
                    "delivered": 59.8
                },
                "device_count": 3,
                "total_leds": 120
            }
        });
        socket
            .send(Message::Text(hello.to_string().into()))
            .await
            .expect("send hello");

        assert_canonical_subscription(&mut socket).await;
        let subscribed = serde_json::json!({
            "type": "subscribed",
            "topics": []
        });
        socket
            .send(Message::Text(subscribed.to_string().into()))
            .await
            .expect("send subscription acknowledgment");

        let event = serde_json::json!({
            "type": "event",
            "event": "active_scene_changed",
            "data": {
                "previous": "default",
                "current": "scene_movie_night",
                "reason": "user_activate"
            }
        });
        socket
            .send(Message::Text(event.to_string().into()))
            .await
            .expect("send scene event");
    })
}

async fn control_surface_ws_handler(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(|mut socket| async move {
        let hello = serde_json::json!({
            "type": "hello",
            "state": {
                "running": true,
                "paused": false,
                "brightness": 42,
                "fps": {
                    "target": 60,
                    "delivered": 59.8
                },
                "device_count": 1,
                "total_leds": 225
            }
        });
        socket
            .send(Message::Text(hello.to_string().into()))
            .await
            .expect("send hello");

        assert_canonical_subscription(&mut socket).await;
        let subscribed = serde_json::json!({
            "type": "subscribed",
            "topics": []
        });
        socket
            .send(Message::Text(subscribed.to_string().into()))
            .await
            .expect("send subscription acknowledgment");

        let event = serde_json::json!({
            "type": "event",
            "event": "control_surface_changed",
            "data": {
                "kind": "values_changed",
                "surface_id": test_surface_id(),
                "revision": 8
            }
        });
        socket
            .send(Message::Text(event.to_string().into()))
            .await
            .expect("send control surface event");
    })
}

fn test_device_id() -> &'static str {
    "00000000-0000-0000-0000-000000000001"
}

fn test_surface_id() -> &'static str {
    "driver:wled:device:00000000-0000-0000-0000-000000000001"
}
