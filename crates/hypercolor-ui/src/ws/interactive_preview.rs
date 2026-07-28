//! Connection-scoped interactive preview control messages.

use std::collections::{HashMap, VecDeque};

use hypercolor_leptos_ext::ws::INTERACTIVE_PREVIEW_ID_MAX_BYTES;
use serde_json::Value;

use hypercolor_leptos_ext::ws::transport::send_websocket_json;

use super::input::InputInjectEdge;

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

#[must_use]
pub fn server_update(message: &Value) -> Option<InteractivePreviewServerUpdate> {
    let message_type = message.get("type")?.as_str()?;
    let preview_id = match message_type {
        "interactive_preview_opened" | "interactive_preview_closed" => {
            message.get("preview_id")?.as_str()?
        }
        "error" => message.get("details")?.get("preview_id")?.as_str()?,
        _ => return None,
    };
    if preview_id.is_empty()
        || preview_id.len() > INTERACTIVE_PREVIEW_ID_MAX_BYTES
        || preview_id.chars().any(char::is_control)
    {
        return None;
    }

    match message_type {
        "interactive_preview_opened" => Some(InteractivePreviewServerUpdate::Opened {
            preview_id: preview_id.to_owned(),
            publication_id: message
                .get("publication_id")?
                .as_u64()
                .filter(|id| *id > 0)?,
        }),
        "interactive_preview_closed" => Some(InteractivePreviewServerUpdate::Closed {
            preview_id: preview_id.to_owned(),
        }),
        "error" => Some(InteractivePreviewServerUpdate::Rejected {
            preview_id: preview_id.to_owned(),
        }),
        _ => None,
    }
}

#[must_use]
pub fn open_message(request: &InteractivePreviewRequest) -> Value {
    serde_json::json!({
        "type": "interactive_preview_open",
        "preview_id": request.preview_id,
        "target": "active_scene",
        "fps": request.fps,
        "width": request.width,
        "height": request.height,
        "format": "jpeg",
    })
}

#[must_use]
pub fn close_message(preview_id: &str) -> Value {
    serde_json::json!({
        "type": "interactive_preview_close",
        "preview_id": preview_id,
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

pub(super) fn send_open(ws: &web_sys::WebSocket, request: &InteractivePreviewRequest) {
    let _ = send_websocket_json(ws, &open_message(request));
}

pub(super) fn send_close(ws: &web_sys::WebSocket, preview_id: &str) {
    let _ = send_websocket_json(ws, &close_message(preview_id));
}

pub(super) fn send_input(ws: &web_sys::WebSocket, preview_id: &str, events: &[InputInjectEdge]) {
    if events.is_empty() {
        return;
    }
    let _ = send_websocket_json(ws, &input_inject_message(preview_id, events));
}
