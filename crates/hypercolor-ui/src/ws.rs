//! WebSocket connection manager — connects to the daemon's streaming endpoint.
//!
//! Handles both JSON events and binary canvas frames (0x03 header).

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::MessageEvent;

// ── Connection State ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Disconnected,
    Error,
}

impl std::fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connecting => write!(f, "Connecting"),
            Self::Connected => write!(f, "Connected"),
            Self::Disconnected => write!(f, "Disconnected"),
            Self::Error => write!(f, "Error"),
        }
    }
}

// ── Canvas Data ─────────────────────────────────────────────────────────────

/// Decoded canvas frame from a binary WebSocket message.
#[derive(Debug, Clone)]
pub struct CanvasFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

// ── WebSocket Manager ───────────────────────────────────────────────────────

/// Reactive WebSocket connection to the daemon.
///
/// Returns signals for canvas data, connection state, and FPS.
/// Automatically subscribes to canvas + events channels.
pub struct WsManager {
    pub canvas_frame: ReadSignal<Option<CanvasFrame>>,
    pub connection_state: ReadSignal<ConnectionState>,
    pub fps: ReadSignal<f32>,
    pub active_effect: ReadSignal<Option<String>>,
}

impl WsManager {
    pub fn new() -> Self {
        let (canvas_frame, set_canvas_frame) = signal(None::<CanvasFrame>);
        let (connection_state, set_connection_state) = signal(ConnectionState::Disconnected);
        let (fps, set_fps) = signal(0.0_f32);
        let (active_effect, set_active_effect) = signal(None::<String>);

        // Build WebSocket URL relative to page origin
        let ws_url = build_ws_url();

        set_connection_state.set(ConnectionState::Connecting);

        // Track frame timing for FPS calculation
        let last_frame_time = StoredValue::new(0.0_f64);
        let frame_count = StoredValue::new(0_u32);
        let fps_update_time = StoredValue::new(0.0_f64);

        // Create WebSocket
        let ws = web_sys::WebSocket::new_with_str(&ws_url, "hypercolor-v1");
        let ws = match ws {
            Ok(ws) => ws,
            Err(_) => {
                set_connection_state.set(ConnectionState::Error);
                return Self {
                    canvas_frame,
                    connection_state,
                    fps,
                    active_effect,
                };
            }
        };

        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

        // onopen — subscribe to canvas + events
        let ws_clone = ws.clone();
        let on_open = Closure::<dyn FnMut()>::new(move || {
            set_connection_state.set(ConnectionState::Connected);

            let subscribe_msg = serde_json::json!({
                "type": "subscribe",
                "channels": ["canvas", "events"],
                "config": {
                    "canvas": { "fps": 30, "format": "rgba" }
                }
            });
            let _ = ws_clone.send_with_str(&subscribe_msg.to_string());
        });
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        on_open.forget();

        // onclose
        let on_close = Closure::<dyn FnMut()>::new(move || {
            set_connection_state.set(ConnectionState::Disconnected);
        });
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        on_close.forget();

        // onerror
        let on_error = Closure::<dyn FnMut()>::new(move || {
            set_connection_state.set(ConnectionState::Error);
        });
        ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();

        // onmessage — handle both JSON and binary frames
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            // Binary frame (ArrayBuffer)
            if let Ok(buffer) = event.data().dyn_into::<js_sys::ArrayBuffer>() {
                let array = js_sys::Uint8Array::new(&buffer);
                let data = array.to_vec();

                if let Some(frame) = decode_canvas_frame(&data) {
                    set_canvas_frame.set(Some(frame));

                    // FPS calculation
                    let now = js_sys::Date::now();
                    frame_count.update_value(|c| *c += 1);

                    let elapsed = now - fps_update_time.get_value();
                    if elapsed >= 1000.0 {
                        let count = frame_count.get_value();
                        #[allow(clippy::cast_precision_loss)]
                        let current_fps = (count as f64 / elapsed) * 1000.0;
                        #[allow(clippy::cast_possible_truncation)]
                        set_fps.set(current_fps as f32);
                        frame_count.set_value(0);
                        fps_update_time.set_value(now);
                    }
                    last_frame_time.set_value(now);
                }
                return;
            }

            // JSON message (String)
            if let Some(text) = event.data().as_string() {
                if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) {
                    handle_json_message(&msg, &set_active_effect);
                }
            }
        });
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        on_message.forget();

        Self {
            canvas_frame,
            connection_state,
            fps,
            active_effect,
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Build WS URL from current page origin.
fn build_ws_url() -> String {
    let window = web_sys::window().expect("no window");
    let location = window.location();
    let protocol = location.protocol().unwrap_or_else(|_| "http:".to_string());
    let host = location
        .host()
        .unwrap_or_else(|_| "127.0.0.1:9420".to_string());

    let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
    format!("{ws_protocol}//{host}/api/v1/ws")
}

/// Decode a binary canvas frame.
///
/// Format: `[0x03][frame_number:u32LE][timestamp:u32LE][width:u16LE][height:u16LE][format:u8][pixels...]`
fn decode_canvas_frame(data: &[u8]) -> Option<CanvasFrame> {
    if data.len() < 14 {
        return None;
    }

    // Magic byte check
    if data[0] != 0x03 {
        return None;
    }

    // Skip frame_number (4 bytes) and timestamp (4 bytes)
    let width = u16::from_le_bytes([data[9], data[10]]) as u32;
    let height = u16::from_le_bytes([data[11], data[12]]) as u32;
    let format = data[13]; // 0 = RGB, 1 = RGBA

    let pixel_data = &data[14..];
    let expected_size = (width * height) as usize;

    let rgba_pixels = if format == 1 {
        // Already RGBA
        if pixel_data.len() < expected_size * 4 {
            return None;
        }
        pixel_data[..expected_size * 4].to_vec()
    } else {
        // RGB → convert to RGBA
        if pixel_data.len() < expected_size * 3 {
            return None;
        }
        let mut rgba = Vec::with_capacity(expected_size * 4);
        for chunk in pixel_data[..expected_size * 3].chunks_exact(3) {
            rgba.push(chunk[0]);
            rgba.push(chunk[1]);
            rgba.push(chunk[2]);
            rgba.push(255);
        }
        rgba
    };

    Some(CanvasFrame {
        width,
        height,
        pixels: rgba_pixels,
    })
}

/// Handle incoming JSON events from the daemon.
fn handle_json_message(msg: &serde_json::Value, set_active: &WriteSignal<Option<String>>) {
    let msg_type = msg.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match msg_type {
        "hello" => {
            // Extract active effect from hello state
            if let Some(state) = msg.get("state") {
                if let Some(active) = state.get("active_effect") {
                    let name = active
                        .get("name")
                        .and_then(|n| n.as_str())
                        .map(String::from);
                    set_active.set(name);
                }
            }
        }
        "event" => {
            // Handle effect change events
            if let Some(event_type) = msg.get("event").and_then(|e| e.as_str()) {
                if event_type == "effect_activated" || event_type == "effect_changed" {
                    let name = msg
                        .get("data")
                        .and_then(|d| d.get("name"))
                        .and_then(|n| n.as_str())
                        .map(String::from);
                    set_active.set(name);
                } else if event_type == "effect_deactivated" || event_type == "effect_stopped" {
                    set_active.set(None);
                }
            }
        }
        _ => {}
    }
}
