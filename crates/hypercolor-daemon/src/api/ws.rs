//! WebSocket handler — `/api/v1/ws`.
//!
//! Real-time event stream, binary frame data, and bidirectional commands.
//! Each connected client gets its own broadcast subscription with configurable
//! channel filtering. Backpressure is handled by bounded channels — slow
//! consumers get dropped frames rather than unbounded memory growth.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast};
use tracing::{debug, warn};

use crate::api::AppState;

/// Maximum number of events that can be buffered per WebSocket client.
const WS_BUFFER_SIZE: usize = 64;

// ── Subscription Types ───────────────────────────────────────────────────

/// Client-to-server subscription messages.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    /// Subscribe to one or more channels.
    Subscribe { channels: Vec<String> },
    /// Unsubscribe from one or more channels.
    Unsubscribe { channels: Vec<String> },
}

/// Server-to-client acknowledgment messages.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    /// Initial hello with state snapshot.
    Hello {
        version: String,
        capabilities: Vec<String>,
        subscriptions: Vec<String>,
    },
    /// Subscribe acknowledgment.
    Subscribed { channels: Vec<String> },
    /// Unsubscribe acknowledgment.
    Unsubscribed {
        channels: Vec<String>,
        remaining: Vec<String>,
    },
    /// Event relay from the bus.
    Event {
        event: String,
        timestamp: String,
        data: serde_json::Value,
    },
}

// ── Handler ──────────────────────────────────────────────────────────────

/// `GET /api/v1/ws` — Upgrade to WebSocket.
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.protocols(["hypercolor-v1"])
        .on_upgrade(move |socket| handle_socket(socket, state))
}

/// Process a single WebSocket connection.
async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    // Default subscriptions: events only.
    // Wrapped in Arc<RwLock<_>> so the relay task sees subscription changes.
    let subscriptions = Arc::new(RwLock::new({
        let mut set = HashSet::new();
        set.insert("events".to_owned());
        set
    }));

    // Send hello message.
    let hello = {
        let subs = subscriptions.read().await;
        ServerMessage::Hello {
            version: "1.0".to_owned(),
            capabilities: vec![
                "frames".to_owned(),
                "spectrum".to_owned(),
                "events".to_owned(),
                "canvas".to_owned(),
                "metrics".to_owned(),
            ],
            subscriptions: subs.iter().cloned().collect(),
        }
    };
    if send_json(&mut socket, &hello).await.is_err() {
        return;
    }

    // Subscribe to the event bus.
    let event_rx = state.event_bus.subscribe_all();

    // Bounded relay channel for backpressure.
    let (relay_tx, mut relay_rx) = tokio::sync::mpsc::channel::<String>(WS_BUFFER_SIZE);

    // Spawn event relay task — shares the subscription set via Arc<RwLock<_>>.
    let relay_subs = Arc::clone(&subscriptions);
    let relay_handle = tokio::spawn(relay_events(event_rx, relay_tx, relay_subs));

    // Main loop: multiplex between incoming client messages and outbound events.
    loop {
        tokio::select! {
            // Outbound: relay events to the client.
            relay_msg = relay_rx.recv() => {
                match relay_msg {
                    Some(msg) => {
                        if socket.send(Message::Text(msg.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break, // Relay channel closed.
                }
            }

            // Inbound: process client messages.
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_client_message(&text, &subscriptions, &mut socket).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        warn!("WebSocket recv error: {e}");
                        break;
                    }
                    _ => {} // Ignore binary/ping/pong.
                }
            }
        }
    }

    relay_handle.abort();
    debug!("WebSocket client disconnected");
}

/// Relay events from the broadcast bus to a bounded mpsc channel.
/// Drops events when the consumer is slow (backpressure).
async fn relay_events(
    mut event_rx: broadcast::Receiver<hypercolor_core::bus::TimestampedEvent>,
    relay_tx: tokio::sync::mpsc::Sender<String>,
    subscriptions: Arc<RwLock<HashSet<String>>>,
) {
    loop {
        match event_rx.recv().await {
            Ok(timestamped) => {
                let has_events = subscriptions.read().await.contains("events");
                if !has_events {
                    continue;
                }
                let event_type = format!("{:?}", timestamped.event.category()).to_lowercase();
                let msg = ServerMessage::Event {
                    event: event_type,
                    timestamp: timestamped.timestamp.clone(),
                    data: serde_json::to_value(&timestamped.event).unwrap_or_default(),
                };
                let Ok(json) = serde_json::to_string(&msg) else {
                    continue;
                };
                // If the channel is full, drop the event (backpressure).
                if relay_tx.try_send(json).is_err() {
                    debug!("Dropping event for slow WebSocket consumer");
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!("WebSocket consumer lagged by {n} events");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Process a client subscription/unsubscription message.
async fn handle_client_message(
    text: &str,
    subscriptions: &Arc<RwLock<HashSet<String>>>,
    socket: &mut WebSocket,
) {
    let Ok(msg) = serde_json::from_str::<ClientMessage>(text) else {
        debug!("Ignoring unparseable WebSocket message");
        return;
    };

    match msg {
        ClientMessage::Subscribe { channels } => {
            {
                let mut subs = subscriptions.write().await;
                for ch in &channels {
                    subs.insert(ch.clone());
                }
            }
            let ack = ServerMessage::Subscribed {
                channels: channels.clone(),
            };
            let _ = send_json(socket, &ack).await;
        }
        ClientMessage::Unsubscribe { channels } => {
            let remaining = {
                let mut subs = subscriptions.write().await;
                for ch in &channels {
                    subs.remove(ch);
                }
                subs.iter().cloned().collect()
            };
            let ack = ServerMessage::Unsubscribed {
                channels: channels.clone(),
                remaining,
            };
            let _ = send_json(socket, &ack).await;
        }
    }
}

/// Serialize and send a JSON message over the WebSocket.
async fn send_json(socket: &mut WebSocket, msg: &impl Serialize) -> Result<(), axum::Error> {
    let json = serde_json::to_string(msg).unwrap_or_default();
    socket.send(Message::Text(json.into())).await.map_err(|e| {
        debug!("WebSocket send error: {e}");
        e
    })
}
