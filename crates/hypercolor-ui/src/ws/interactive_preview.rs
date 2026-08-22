//! Connection-scoped interactive preview control messages.

use std::collections::{HashMap, VecDeque};

use hypercolor_leptos_ext::ws::INTERACTIVE_PREVIEW_ID_MAX_BYTES;
use serde_json::Value;

use super::input::InputInjectEdge;
use super::transport::{WebSocketConnection, send_json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractivePreviewRequest {
    pub preview_id: String,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractivePreviewLifecycle {
    Requested,
    Opened { publication_id: u64 },
    Closing,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractivePreviewServerUpdate {
    Opened {
        preview_id: String,
        publication_id: u64,
    },
    Closed {
        preview_id: String,
    },
    Rejected {
        preview_id: String,
    },
}

impl InteractivePreviewServerUpdate {
    #[must_use]
    pub fn preview_id(&self) -> &str {
        match self {
            Self::Opened { preview_id, .. }
            | Self::Closed { preview_id }
            | Self::Rejected { preview_id } => preview_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingOperation {
    Open,
    Close,
}

#[derive(Debug, Default)]
struct PreviewLifecycleState {
    pending: VecDeque<PendingOperation>,
    publication_id: Option<u64>,
    rejected: bool,
}

#[derive(Debug, Default)]
pub struct InteractivePreviewLifecycleTracker {
    previews: HashMap<String, PreviewLifecycleState>,
}

impl InteractivePreviewLifecycleTracker {
    pub fn request_open(&mut self, preview_id: &str) {
        let state = self.previews.entry(preview_id.to_owned()).or_default();
        state.pending.push_back(PendingOperation::Open);
        state.publication_id = None;
        state.rejected = false;
    }

    pub fn request_close(&mut self, preview_id: &str) {
        let state = self.previews.entry(preview_id.to_owned()).or_default();
        state.pending.push_back(PendingOperation::Close);
        state.publication_id = None;
        state.rejected = false;
    }

    pub fn apply(&mut self, update: InteractivePreviewServerUpdate) {
        let preview_id = update.preview_id().to_owned();
        let Some(state) = self.previews.get_mut(&preview_id) else {
            return;
        };
        let expected = state.pending.pop_front();
        state.publication_id = None;

        match (update, expected) {
            (
                InteractivePreviewServerUpdate::Opened { publication_id, .. },
                Some(PendingOperation::Open),
            ) => {
                if state.pending.is_empty() {
                    state.publication_id = Some(publication_id);
                }
                state.rejected = false;
            }
            (InteractivePreviewServerUpdate::Closed { .. }, Some(PendingOperation::Close)) => {
                state.rejected = false;
            }
            (InteractivePreviewServerUpdate::Rejected { .. }, Some(_)) => {
                state.rejected = state.pending.is_empty();
            }
            _ => {
                state.pending.clear();
                state.rejected = true;
            }
        }

        if state.pending.is_empty() && state.publication_id.is_none() && !state.rejected {
            self.previews.remove(&preview_id);
        }
    }

    pub fn clear(&mut self) {
        self.previews.clear();
    }

    /// Every preview id the tracker currently knows about.
    #[must_use]
    pub fn known_preview_ids(&self) -> Vec<String> {
        self.previews.keys().cloned().collect()
    }

    #[must_use]
    pub fn lifecycles(&self) -> HashMap<String, InteractivePreviewLifecycle> {
        self.previews
            .iter()
            .filter_map(|(preview_id, state)| {
                let lifecycle = match state.pending.back() {
                    Some(PendingOperation::Open) => InteractivePreviewLifecycle::Requested,
                    Some(PendingOperation::Close) => InteractivePreviewLifecycle::Closing,
                    None => state
                        .publication_id
                        .map(|publication_id| InteractivePreviewLifecycle::Opened {
                            publication_id,
                        })
                        .or_else(|| {
                            state
                                .rejected
                                .then_some(InteractivePreviewLifecycle::Rejected)
                        })?,
                };
                Some((preview_id.clone(), lifecycle))
            })
            .collect()
    }
}

/// Read every interactive preview state out of one acknowledgment.
///
/// A subscribe or unsubscribe acknowledgment reports the connection's
/// whole live subscription set, so an interactive preview that is absent
/// from it has been closed, and one that is present with a publication
/// id is open. An addressed error is a rejection of whatever the client
/// last asked for.
#[must_use]
pub fn server_updates(message: &Value) -> Vec<InteractivePreviewServerUpdate> {
    let Some(message_type) = message.get("type").and_then(Value::as_str) else {
        return Vec::new();
    };

    if message_type == "error" {
        return message
            .get("details")
            .and_then(|details| details.get("preview_id"))
            .and_then(Value::as_str)
            .filter(|preview_id| valid_preview_id(preview_id))
            .map(|preview_id| {
                vec![InteractivePreviewServerUpdate::Rejected {
                    preview_id: preview_id.to_owned(),
                }]
            })
            .unwrap_or_default();
    }

    if !matches!(message_type, "subscribed" | "unsubscribed" | "hello") {
        return Vec::new();
    }
    let entries = if message_type == "hello" {
        message.get("subscriptions")
    } else {
        message.get("topics")
    };
    let Some(entries) = entries.and_then(Value::as_array) else {
        return Vec::new();
    };

    entries
        .iter()
        .filter(|entry| entry.get("topic").and_then(Value::as_str) == Some("interactive_preview"))
        .filter_map(|entry| {
            let preview_id = entry.get("key").and_then(Value::as_str)?;
            if !valid_preview_id(preview_id) {
                return None;
            }
            let publication_id = entry
                .get("publication_id")
                .and_then(Value::as_u64)
                .filter(|id| *id > 0)?;
            Some(InteractivePreviewServerUpdate::Opened {
                preview_id: preview_id.to_owned(),
                publication_id,
            })
        })
        .collect()
}

/// The previews an acknowledgment says are no longer live, given the ones
/// the client believes it opened.
#[must_use]
pub fn closed_previews(message: &Value, known: &[String]) -> Vec<InteractivePreviewServerUpdate> {
    let Some(message_type) = message.get("type").and_then(Value::as_str) else {
        return Vec::new();
    };
    if !matches!(message_type, "subscribed" | "unsubscribed") {
        return Vec::new();
    }
    let live: Vec<&str> = message
        .get("topics")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| {
                    entry.get("topic").and_then(Value::as_str) == Some("interactive_preview")
                })
                .filter_map(|entry| entry.get("key").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();

    known
        .iter()
        .filter(|preview_id| !live.iter().any(|key| key == preview_id))
        .map(|preview_id| InteractivePreviewServerUpdate::Closed {
            preview_id: preview_id.clone(),
        })
        .collect()
}

fn valid_preview_id(preview_id: &str) -> bool {
    !preview_id.is_empty()
        && preview_id.len() <= INTERACTIVE_PREVIEW_ID_MAX_BYTES
        && !preview_id.chars().any(char::is_control)
}

#[must_use]
pub fn open_message(request: &InteractivePreviewRequest) -> Value {
    serde_json::json!({
        "type": "subscribe",
        "topics": [{
            "topic": "interactive_preview",
            "key": request.preview_id,
            "config": {
                "target": "active_scene",
                "fps": request.fps,
                "width": request.width,
                "height": request.height,
                "format": "jpeg",
            }
        }]
    })
}

#[must_use]
pub fn close_message(preview_id: &str) -> Value {
    serde_json::json!({
        "type": "unsubscribe",
        "topics": [{ "topic": "interactive_preview", "key": preview_id }]
    })
}

#[must_use]
pub fn input_inject_message(preview_id: &str, events: &[InputInjectEdge]) -> Value {
    serde_json::json!({
        "type": "input_inject",
        "preview_id": preview_id,
        "events": events,
    })
}

pub(super) fn send_open(ws: &dyn WebSocketConnection, request: &InteractivePreviewRequest) {
    let _ = send_json(ws, &open_message(request));
}

pub(super) fn send_close(ws: &dyn WebSocketConnection, preview_id: &str) {
    let _ = send_json(ws, &close_message(preview_id));
}

pub(super) fn send_input(
    ws: &dyn WebSocketConnection,
    preview_id: &str,
    events: &[InputInjectEdge],
) {
    if events.is_empty() {
        return;
    }
    let _ = send_json(ws, &input_inject_message(preview_id, events));
}
