//! Connection-scoped interactive preview control messages.

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
