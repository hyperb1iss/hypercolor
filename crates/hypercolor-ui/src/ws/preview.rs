//! Preview FPS cap logic, subscription management, and backpressure handling.

use leptos::prelude::*;
use wasm_bindgen::{JsCast, JsValue};

use super::messages::CanvasFrame;

pub const DEFAULT_PREVIEW_FPS_CAP: u32 = 30;
pub(super) const HIDDEN_TAB_PREVIEW_FPS_CAP: u32 = 6;
pub(super) const SCREEN_PREVIEW_FPS_CAP: u32 = 15;
const REMOTE_PREVIEW_WIDTH_HIGH: u32 = 320;
const REMOTE_PREVIEW_WIDTH_MEDIUM: u32 = 240;
const REMOTE_PREVIEW_WIDTH_LOW: u32 = 160;

pub(super) fn desired_preview_fps(
    engine_target_fps: u32,
    client_cap: u32,
    page_visible: bool,
) -> u32 {
    let capped_target = engine_target_fps.clamp(1, 60).min(client_cap.clamp(1, 60));
    if page_visible {
        capped_target
    } else {
        capped_target.min(HIDDEN_TAB_PREVIEW_FPS_CAP)
    }
}

pub(super) fn preview_canvas_format() -> &'static str {
    match preview_hostname().as_str() {
        host if is_loopback_host(host) => "rgba",
        _ if supports_remote_jpeg_preview() => "jpeg",
        _ => "rgb",
    }
}

fn supports_remote_jpeg_preview() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let Some(document) = window.document() else {
        return false;
    };
    let Ok(canvas) = document.create_element("canvas") else {
        return false;
    };
    let Ok(canvas) = canvas.dyn_into::<web_sys::HtmlCanvasElement>() else {
        return false;
    };

    let has_bitmap_renderer = canvas
        .get_context("bitmaprenderer")
        .ok()
        .flatten()
        .is_some();
    let global = js_sys::global();
    let has_create_image_bitmap =
        js_sys::Reflect::has(&global, &JsValue::from_str("createImageBitmap")).unwrap_or(false);
    let has_worker = js_sys::Reflect::has(&global, &JsValue::from_str("Worker")).unwrap_or(false);

    has_bitmap_renderer && has_create_image_bitmap && has_worker
}

fn preview_canvas_request_dimensions(requested_fps: u32) -> (u32, u32) {
    preview_canvas_request_dimensions_for_host(preview_hostname().as_str(), requested_fps)
}

pub(super) fn request_preview_subscription(
    ws: &web_sys::WebSocket,
    requested_preview_fps: StoredValue<u32>,
    set_preview_target_fps: WriteSignal<u32>,
    engine_target_fps: u32,
    client_cap: u32,
    page_visible: bool,
) {
    let desired_fps = desired_preview_fps(engine_target_fps, client_cap, page_visible);
    if desired_fps == requested_preview_fps.get_value() {
        return;
    }

    requested_preview_fps.set_value(desired_fps);
    set_preview_target_fps.set(desired_fps);
    let (preview_width, preview_height) = preview_canvas_request_dimensions(desired_fps);

    let subscribe_msg = serde_json::json!({
        "type": "subscribe",
        "channels": ["canvas"],
        "config": {
            "canvas": {
                "fps": desired_fps,
                "format": preview_canvas_format(),
                "width": preview_width,
                "height": preview_height
            }
        }
    });
    let _ = ws.send_with_str(&subscribe_msg.to_string());
}

pub(super) fn request_screen_preview_subscription(
    ws: &web_sys::WebSocket,
    requested_preview_fps: StoredValue<u32>,
    engine_target_fps: u32,
    page_visible: bool,
) {
    let desired_fps = desired_preview_fps(engine_target_fps, SCREEN_PREVIEW_FPS_CAP, page_visible);
    if desired_fps == requested_preview_fps.get_value() {
        return;
    }

    requested_preview_fps.set_value(desired_fps);
    let (preview_width, preview_height) = preview_canvas_request_dimensions(desired_fps);

    let subscribe_msg = serde_json::json!({
        "type": "subscribe",
        "channels": ["screen_canvas"],
        "config": {
            "screen_canvas": {
                "fps": desired_fps,
                "format": preview_canvas_format(),
                "width": preview_width,
                "height": preview_height
            }
        }
    });
    let _ = ws.send_with_str(&subscribe_msg.to_string());
}

pub(super) fn clear_preview_subscription(
    requested_preview_fps: StoredValue<u32>,
    set_preview_target_fps: &WriteSignal<u32>,
    set_preview_fps: &WriteSignal<f32>,
    set_canvas_frame: &WriteSignal<Option<CanvasFrame>>,
) {
    requested_preview_fps.set_value(0);
    set_preview_target_fps.set(0);
    set_preview_fps.set(0.0);
    set_canvas_frame.set(None);
}

pub(super) fn clear_screen_preview_subscription(
    requested_preview_fps: StoredValue<u32>,
    set_screen_canvas_frame: &WriteSignal<Option<CanvasFrame>>,
) {
    requested_preview_fps.set_value(0);
    set_screen_canvas_frame.set(None);
}

pub(super) fn send_canvas_unsubscribe(ws: &web_sys::WebSocket) {
    let unsubscribe_msg = serde_json::json!({
        "type": "unsubscribe",
        "channels": ["canvas"]
    });
    let _ = ws.send_with_str(&unsubscribe_msg.to_string());
}

pub(super) fn send_screen_canvas_unsubscribe(ws: &web_sys::WebSocket) {
    let unsubscribe_msg = serde_json::json!({
        "type": "unsubscribe",
        "channels": ["screen_canvas"]
    });
    let _ = ws.send_with_str(&unsubscribe_msg.to_string());
}

fn preview_hostname() -> String {
    web_sys::window()
        .map(|window| window.location())
        .and_then(|location| location.hostname().ok())
        .unwrap_or_default()
}

fn is_loopback_host(hostname: &str) -> bool {
    matches!(hostname, "localhost" | "127.0.0.1" | "::1")
}

fn preview_canvas_request_dimensions_for_host(hostname: &str, requested_fps: u32) -> (u32, u32) {
    if is_loopback_host(hostname) {
        return (0, 0);
    }

    (remote_preview_width_for_fps(requested_fps), 0)
}

const fn remote_preview_width_for_fps(requested_fps: u32) -> u32 {
    match requested_fps {
        24.. => REMOTE_PREVIEW_WIDTH_HIGH,
        12..=23 => REMOTE_PREVIEW_WIDTH_MEDIUM,
        _ => REMOTE_PREVIEW_WIDTH_LOW,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        REMOTE_PREVIEW_WIDTH_HIGH, REMOTE_PREVIEW_WIDTH_LOW, REMOTE_PREVIEW_WIDTH_MEDIUM,
        preview_canvas_request_dimensions_for_host, remote_preview_width_for_fps,
    };

    #[test]
    fn remote_preview_width_tracks_requested_fps() {
        assert_eq!(remote_preview_width_for_fps(30), REMOTE_PREVIEW_WIDTH_HIGH);
        assert_eq!(
            remote_preview_width_for_fps(15),
            REMOTE_PREVIEW_WIDTH_MEDIUM
        );
        assert_eq!(remote_preview_width_for_fps(6), REMOTE_PREVIEW_WIDTH_LOW);
    }

    #[test]
    fn loopback_preview_keeps_full_resolution() {
        assert_eq!(
            preview_canvas_request_dimensions_for_host("localhost", 6),
            (0, 0)
        );
        assert_eq!(
            preview_canvas_request_dimensions_for_host("127.0.0.1", 30),
            (0, 0)
        );
    }

    #[test]
    fn remote_preview_dimensions_scale_with_fps() {
        assert_eq!(
            preview_canvas_request_dimensions_for_host("remote.example", 30),
            (REMOTE_PREVIEW_WIDTH_HIGH, 0)
        );
        assert_eq!(
            preview_canvas_request_dimensions_for_host("remote.example", 15),
            (REMOTE_PREVIEW_WIDTH_MEDIUM, 0)
        );
        assert_eq!(
            preview_canvas_request_dimensions_for_host("remote.example", 6),
            (REMOTE_PREVIEW_WIDTH_LOW, 0)
        );
    }
}
