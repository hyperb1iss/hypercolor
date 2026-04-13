use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocketUpgrade};
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
}

#[tokio::test]
async fn active_scene_event_refreshes_daemon_status() {
    let status_calls = Arc::new(AtomicUsize::new(0));
    let state = TestState {
        status_calls: Arc::clone(&status_calls),
    };

    let router = Router::new()
        .route("/api/v1/status", get(status_handler))
        .route("/api/v1/effects", get(effects_handler))
        .route("/api/v1/devices", get(devices_handler))
        .route("/api/v1/library/favorites", get(favorites_handler))
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
            action_tx,
            bridge_cancel,
        )
        .await;
    });

    let updated = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Action::DaemonStateUpdated(state)) = action_rx.recv().await
                && state.scene_name.as_deref() == Some("Movie Night")
            {
                break (state.scene_name.clone(), state.scene_snapshot_locked);
            }
        }
    })
    .await
    .expect("timed out waiting for scene status refresh");

    assert_eq!(updated.0.as_deref(), Some("Movie Night"));
    assert!(updated.1);
    assert_eq!(status_calls.load(Ordering::SeqCst), 1);

    cancel.cancel();
    bridge.await.expect("bridge task should join");
    server.abort();
}

async fn status_handler(State(state): State<TestState>) -> Json<serde_json::Value> {
    let call = state.status_calls.fetch_add(1, Ordering::SeqCst);
    let (scene_name, snapshot_locked) = if call == 0 {
        ("Default", false)
    } else {
        ("Movie Night", true)
    };

    Json(serde_json::json!({
        "data": {
            "running": true,
            "global_brightness": 42,
            "device_count": 3,
            "active_effect": serde_json::Value::Null,
            "active_scene": scene_name,
            "active_scene_snapshot_locked": snapshot_locked
        }
    }))
}

async fn effects_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "data": {
            "items": [],
            "pagination": {
                "offset": 0,
                "limit": 50,
                "total": 0,
                "has_more": false
            }
        }
    }))
}

async fn devices_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "data": {
            "items": [],
            "pagination": {
                "offset": 0,
                "limit": 50,
                "total": 0,
                "has_more": false
            }
        }
    }))
}

async fn favorites_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "data": {
            "items": [],
            "pagination": {
                "offset": 0,
                "limit": 50,
                "total": 0,
                "has_more": false
            }
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
                    "actual": 59.8
                },
                "device_count": 3,
                "total_leds": 120
            }
        });
        socket
            .send(Message::Text(hello.to_string().into()))
            .await
            .expect("send hello");

        let _ = socket.recv().await;

        let event = serde_json::json!({
            "type": "event",
            "event": "active_scene_changed",
            "data": {
                "previous": "default",
                "current": "scene_movie_night",
                "current_name": "Movie Night",
                "current_snapshot_locked": true,
                "reason": "user_activate"
            }
        });
        socket
            .send(Message::Text(event.to_string().into()))
            .await
            .expect("send scene event");
    })
}
