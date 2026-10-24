//! Connection-scoped interactive preview control messages.

use std::collections::{HashMap, VecDeque};

use hypercolor_leptos_ext::ws::registry::CanvasFormat;
pub use hypercolor_leptos_ext::ws::registry::InteractivePreviewTarget;
use hypercolor_leptos_ext::ws::{INTERACTIVE_PREVIEW_ID_MAX_BYTES, PreviewPixelFormat};
use serde_json::Value;

use super::input::InputInjectEdge;
use super::transport::{WebSocketConnection, send_json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractivePreviewRequest {
    pub preview_id: String,
    pub target: InteractivePreviewTarget,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub format: CanvasFormat,
}

/// The size and pixel format of the main preview's frames, which an
/// interactive lane standing in for it renders at.
///
/// Matching the format matters as much as the size: the canvas keeps the
/// presenter the main preview's frames set up, and a raw-pixel presenter
/// owns a WebGL context that a JPEG decoder cannot take over, so frames in
/// another format would never paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractivePreviewShape {
    pub width: u32,
    pub height: u32,
    pub format: CanvasFormat,
}

impl InteractivePreviewShape {
    #[must_use]
    pub const fn new(width: u32, height: u32, format: PreviewPixelFormat) -> Self {
        Self {
            width,
            height,
            format: match format {
                PreviewPixelFormat::Rgb => CanvasFormat::Rgb,
                PreviewPixelFormat::Rgba => CanvasFormat::Rgba,
                PreviewPixelFormat::Jpeg => CanvasFormat::Jpeg,
            },
        }
    }
}

/// What an interactive preview renders when it stands in for the main
/// preview: the zone that preview shows, or the scene while it shows the
/// scene canvas. Following the same zone keeps a click on the picture
/// landing in the effect the picture shows.
#[must_use]
pub fn interactive_preview_target(shown_zone: Option<&str>) -> InteractivePreviewTarget {
    shown_zone.map_or(InteractivePreviewTarget::ActiveScene, |zone| {
        InteractivePreviewTarget::Zone(zone.to_owned())
    })
}

/// The interactive preview a surface should hold open, or `None` to close
/// it.
///
/// The lane renders `shown_zone`, or the scene for `None`, in the shape
/// of the main preview's newest frame. That preview drops its last frame
/// when it moves to another zone, so until the new zone's frames land the
/// lane keeps the shape it last had (`held_shape`). Following another
/// zone therefore re-sends the open lane's config with the new target,
/// which the daemon applies to the live lane, rather than closing it and
/// opening a new one.
#[must_use]
pub fn interactive_preview_request(
    requested: bool,
    preview_id: &str,
    shown_zone: Option<&str>,
    fps: u32,
    latest_shape: Option<InteractivePreviewShape>,
    held_shape: Option<InteractivePreviewShape>,
) -> Option<InteractivePreviewRequest> {
    if !requested {
        return None;
    }
    let InteractivePreviewShape {
        width,
        height,
        format,
    } = latest_shape.or(held_shape)?;
    Some(InteractivePreviewRequest {
        preview_id: preview_id.to_owned(),
        target: interactive_preview_target(shown_zone),
        fps,
        width,
        height,
        format,
    })
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

        match (update, state.pending.front().copied()) {
            (
                InteractivePreviewServerUpdate::Opened { publication_id, .. },
                Some(PendingOperation::Open),
            ) => {
                state.pending.pop_front();
                state.publication_id = state.pending.is_empty().then_some(publication_id);
                state.rejected = false;
            }
            (InteractivePreviewServerUpdate::Closed { .. }, Some(PendingOperation::Close)) => {
                state.pending.pop_front();
                state.publication_id = None;
                state.rejected = false;
            }
            (InteractivePreviewServerUpdate::Rejected { .. }, Some(_)) => {
                state.pending.pop_front();
                state.publication_id = None;
                state.rejected = state.pending.is_empty();
            }
            // An acknowledgment restates the connection's whole live set,
            // so the ack of any other subscribe on this socket (the main
            // preview's, when it follows another zone) lists this preview
            // too. With nothing of ours pending it only restates the lane.
            (InteractivePreviewServerUpdate::Opened { publication_id, .. }, None) => {
                if !state.rejected {
                    state.publication_id = Some(publication_id);
                }
            }
            (InteractivePreviewServerUpdate::Closed { .. }, None) => {
                state.publication_id = None;
                state.rejected = false;
            }
            // Acknowledgments arrive in request order, so one that disagrees
            // with the request at the front answered a request sent before
            // it and says nothing about ours.
            (InteractivePreviewServerUpdate::Opened { .. }, Some(PendingOperation::Close))
            | (InteractivePreviewServerUpdate::Closed { .. }, Some(PendingOperation::Open)) => {}
            (InteractivePreviewServerUpdate::Rejected { .. }, None) => {
                state.publication_id = None;
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
                "target": request.target,
                "fps": request.fps,
                "width": request.width,
                "height": request.height,
                "format": request.format,
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
