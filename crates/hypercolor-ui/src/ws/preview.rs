//! Preview FPS cap logic, subscription management, and backpressure handling.

use leptos::prelude::*;

use super::messages::CanvasFrame;
use hypercolor_leptos_ext::canvas::supports_bitmap_worker_canvas;
use hypercolor_leptos_ext::prelude::current_page_location;
use hypercolor_types::spatial::SpatialLayout;

use super::transport::{WebSocketConnection, send_json};
use crate::remote_bridge::TransportPathReport;

pub const DEFAULT_PREVIEW_FPS_CAP: u32 = 60;
pub(super) const HIDDEN_TAB_PREVIEW_FPS_CAP: u32 = 6;
pub(super) const SCREEN_PREVIEW_FPS_CAP: u32 = 15;
pub(super) const WEB_VIEWPORT_PREVIEW_FPS_CAP: u32 = 15;
const REMOTE_PREVIEW_WIDTH_MEDIUM: u32 = 640;
const REMOTE_PREVIEW_WIDTH_LOW: u32 = 480;

/// A Remote session carries every preview frame as one sealed message with
/// a hard size limit, so it asks for JPEG on every path. Raw RGB or RGBA
/// exceeds that limit on every frame.
const REMOTE_PREVIEW_FORMAT: &str = "jpeg";

/// Transport path a Remote bridge reports for the live session, as far as
/// the preview cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemotePreviewPath {
    /// A peer-to-peer channel inside one local network.
    Local,
    /// A peer-to-peer channel across the internet.
    Direct,
    /// A relayed carrier, or a path the bridge has not classified.
    Relayed,
}

impl RemotePreviewPath {
    /// Classify the `kind` of a bridge transport path report.
    ///
    /// Only the peer-to-peer kinds earn a larger preview. Relay kinds,
    /// `unknown`, and kinds this build does not recognize all get the
    /// relayed profile, so a new path kind can never widen the stream.
    #[must_use]
    pub fn from_bridge_kind(kind: &str) -> Self {
        match kind {
            "local" => Self::Local,
            "direct" => Self::Direct,
            _ => Self::Relayed,
        }
    }
}

/// Upper bounds a Remote session puts on its preview stream.
///
/// Page caps still apply below these. JPEG size depends on content, so the
/// width keeps a frame inside the sealed-message limit in practice; the
/// computer's per-frame byte budget is what guarantees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemotePreviewProfile {
    pub max_fps: u32,
    pub max_width: u32,
}

impl RemotePreviewProfile {
    #[must_use]
    pub const fn for_path(path: RemotePreviewPath) -> Self {
        match path {
            // A LAN path keeps the local preview cadence; only the width is
            // bounded, by the per-frame limit rather than the link.
            RemotePreviewPath::Local => Self {
                max_fps: DEFAULT_PREVIEW_FPS_CAP,
                max_width: 480,
            },
            RemotePreviewPath::Direct => Self {
                max_fps: 30,
                max_width: 480,
            },
            // The relay shares one ordered channel with every API call and
            // encodes each frame twice, so it gets the lightest stream.
            RemotePreviewPath::Relayed => Self {
                max_fps: 15,
                max_width: 320,
            },
        }
    }
}

/// Where preview frames travel, which decides their format and bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreviewRoute {
    /// The page talks to the daemon over its own origin.
    Host,
    /// A Remote bridge carries the session over the reported path.
    Remote(RemotePreviewPath),
}

impl PreviewRoute {
    pub(super) fn from_remote_path(path: Option<RemotePreviewPath>) -> Self {
        path.map_or(Self::Host, Self::Remote)
    }
}

/// Whether a bridge path report is at least as new as the last one applied.
pub(super) fn accept_transport_path_report(
    last_generation: Option<u64>,
    report: &TransportPathReport,
) -> bool {
    last_generation.is_none_or(|last| report.generation >= last)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PreviewSubscriptionRequest {
    fps: u32,
    width: u32,
    height: u32,
    format: &'static str,
}

impl PreviewSubscriptionRequest {
    fn canvas(route: PreviewRoute, requested_fps: u32, width_cap: u32) -> Self {
        match route {
            PreviewRoute::Host => {
                Self::canvas_for_host(preview_hostname().as_str(), requested_fps, width_cap)
            }
            PreviewRoute::Remote(path) => Self::remote(path, requested_fps, width_cap),
        }
    }

    fn remote(path: RemotePreviewPath, requested_fps: u32, width_cap: u32) -> Self {
        let profile = RemotePreviewProfile::for_path(path);
        Self {
            fps: requested_fps.min(profile.max_fps),
            width: match width_cap {
                0 => profile.max_width,
                cap => cap.min(profile.max_width),
            },
            height: 0,
            format: REMOTE_PREVIEW_FORMAT,
        }
    }

    fn canvas_for_host(hostname: &str, requested_fps: u32, width_cap: u32) -> Self {
        let (width, height) =
            preview_canvas_request_dimensions_for_host(hostname, requested_fps, width_cap);
        Self {
            fps: requested_fps,
            width,
            height,
            format: preview_canvas_format_for_host(hostname),
        }
    }

    fn web_viewport(route: PreviewRoute, requested_fps: u32) -> Self {
        match route {
            PreviewRoute::Host => {
                Self::web_viewport_for_host(preview_hostname().as_str(), requested_fps)
            }
            PreviewRoute::Remote(path) => Self::remote(path, requested_fps, 0),
        }
    }

    fn web_viewport_for_host(hostname: &str, requested_fps: u32) -> Self {
        let (width, height) = web_viewport_preview_request_dimensions();
        Self {
            fps: requested_fps,
            width,
            height,
            format: preview_canvas_format_for_host(hostname),
        }
    }
}

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

pub(super) const fn should_stream_preview(
    app_window_visible: bool,
    engine_target_fps: u32,
    consumer_count: u32,
) -> bool {
    app_window_visible && engine_target_fps > 0 && consumer_count > 0
}

fn preview_canvas_format_for_host(hostname: &str) -> &'static str {
    match hostname {
        host if is_loopback_host(host) => "rgba",
        _ if supports_remote_jpeg_preview() => "jpeg",
        _ => "rgb",
    }
}

fn supports_remote_jpeg_preview() -> bool {
    supports_bitmap_worker_canvas()
}

fn web_viewport_preview_request_dimensions() -> (u32, u32) {
    (0, 0)
}

#[allow(
    clippy::too_many_arguments,
    reason = "each input is one reactive preview demand"
)]
pub(super) fn request_preview_subscription(
    ws: &dyn WebSocketConnection,
    requested_preview_request: StoredValue<Option<PreviewSubscriptionRequest>>,
    set_preview_target_fps: WriteSignal<u32>,
    route: PreviewRoute,
    engine_target_fps: u32,
    client_cap: u32,
    width_cap: u32,
    page_visible: bool,
) {
    let request = PreviewSubscriptionRequest::canvas(
        route,
        desired_preview_fps(engine_target_fps, client_cap, page_visible),
        width_cap,
    );
    if requested_preview_request.get_value() == Some(request) {
        return;
    }

    requested_preview_request.set_value(Some(request));
    set_preview_target_fps.set(request.fps);

    let subscribe_msg = serde_json::json!({
        "type": "subscribe",
        "topics": [{
            "topic": "canvas",
            "config": {
                "fps": request.fps,
                "format": request.format,
                "width": request.width,
                "height": request.height
            }
        }]
    });
    let _ = send_json(ws, &subscribe_msg);
}

pub(super) fn request_screen_preview_subscription(
    ws: &dyn WebSocketConnection,
    requested_preview_request: StoredValue<Option<PreviewSubscriptionRequest>>,
    route: PreviewRoute,
    engine_target_fps: u32,
    page_visible: bool,
) {
    let request = PreviewSubscriptionRequest::canvas(
        route,
        desired_preview_fps(engine_target_fps, SCREEN_PREVIEW_FPS_CAP, page_visible),
        0,
    );
    if requested_preview_request.get_value() == Some(request) {
        return;
    }

    requested_preview_request.set_value(Some(request));

    let subscribe_msg = serde_json::json!({
        "type": "subscribe",
        "topics": [{
            "topic": "screen_canvas",
            "config": {
                "fps": request.fps,
                "format": request.format,
                "width": request.width,
                "height": request.height
            }
        }]
    });
    let _ = send_json(ws, &subscribe_msg);
}

pub(super) fn request_web_viewport_preview_subscription(
    ws: &dyn WebSocketConnection,
    requested_preview_request: StoredValue<Option<PreviewSubscriptionRequest>>,
    route: PreviewRoute,
    engine_target_fps: u32,
    page_visible: bool,
) {
    let request = PreviewSubscriptionRequest::web_viewport(
        route,
        desired_preview_fps(
            engine_target_fps,
            WEB_VIEWPORT_PREVIEW_FPS_CAP,
            page_visible,
        ),
    );
    if requested_preview_request.get_value() == Some(request) {
        return;
    }

    requested_preview_request.set_value(Some(request));

    let subscribe_msg = serde_json::json!({
        "type": "subscribe",
        "topics": [{
            "topic": "web_viewport_canvas",
            "config": {
                "fps": request.fps,
                "format": request.format,
                "width": request.width,
                "height": request.height
            }
        }]
    });
    let _ = send_json(ws, &subscribe_msg);
}

pub(super) fn clear_preview_subscription(
    requested_preview_request: StoredValue<Option<PreviewSubscriptionRequest>>,
    set_preview_target_fps: &WriteSignal<u32>,
    set_preview_fps: &WriteSignal<f32>,
    set_canvas_frame: &WriteSignal<Option<CanvasFrame>>,
) {
    requested_preview_request.set_value(None);
    set_preview_target_fps.set(0);
    set_preview_fps.set(0.0);
    set_canvas_frame.set(None);
}

pub(super) fn clear_screen_preview_subscription(
    requested_preview_request: StoredValue<Option<PreviewSubscriptionRequest>>,
    set_screen_canvas_frame: &WriteSignal<Option<CanvasFrame>>,
) {
    requested_preview_request.set_value(None);
    set_screen_canvas_frame.set(None);
}

pub(super) fn clear_web_viewport_preview_subscription(
    requested_preview_request: StoredValue<Option<PreviewSubscriptionRequest>>,
    set_web_viewport_canvas_frame: &WriteSignal<Option<CanvasFrame>>,
) {
    requested_preview_request.set_value(None);
    set_web_viewport_canvas_frame.set(None);
}

pub(super) fn send_canvas_unsubscribe(ws: &dyn WebSocketConnection) {
    let unsubscribe_msg = serde_json::json!({
        "type": "unsubscribe",
        "topics": [{ "topic": "canvas" }]
    });
    let _ = send_json(ws, &unsubscribe_msg);
}

pub(super) fn send_screen_zones_subscribe(ws: &dyn WebSocketConnection) {
    let subscribe_msg = serde_json::json!({
        "type": "subscribe",
        "topics": [{ "topic": "screen_zones" }]
    });
    let _ = send_json(ws, &subscribe_msg);
}

pub(super) fn send_screen_zones_unsubscribe(ws: &dyn WebSocketConnection) {
    let unsubscribe_msg = serde_json::json!({
        "type": "unsubscribe",
        "topics": [{ "topic": "screen_zones" }]
    });
    let _ = send_json(ws, &unsubscribe_msg);
}

pub(super) fn send_screen_canvas_unsubscribe(ws: &dyn WebSocketConnection) {
    let unsubscribe_msg = serde_json::json!({
        "type": "unsubscribe",
        "topics": [{ "topic": "screen_canvas" }]
    });
    let _ = send_json(ws, &unsubscribe_msg);
}

pub(super) fn send_web_viewport_canvas_unsubscribe(ws: &dyn WebSocketConnection) {
    let unsubscribe_msg = serde_json::json!({
        "type": "unsubscribe",
        "topics": [{ "topic": "web_viewport_canvas" }]
    });
    let _ = send_json(ws, &unsubscribe_msg);
}

/// Follow one device's display output. The device is the subscription
/// key, so following a second display is a second subscription rather
/// than a retarget, and every frame names the device it came from.
pub(super) fn send_display_preview_subscribe(
    ws: &dyn WebSocketConnection,
    device_id: &str,
    fps: u32,
) {
    let subscribe_msg = serde_json::json!({
        "type": "subscribe",
        "topics": [{
            "topic": "display_preview",
            "key": device_id,
            "config": { "fps": fps }
        }]
    });
    let _ = send_json(ws, &subscribe_msg);
}

/// Stop following one device's display output.
pub(super) fn send_display_preview_unsubscribe(ws: &dyn WebSocketConnection, device_id: &str) {
    let unsubscribe_msg = serde_json::json!({
        "type": "unsubscribe",
        "topics": [{ "topic": "display_preview", "key": device_id }]
    });
    let _ = send_json(ws, &unsubscribe_msg);
}

/// Stage a drag preview on the live tree.
///
/// Zone-keyed only: previews apply to what is rendering, so the daemon
/// owns which scene that is (Spec 78 §1.5).
pub(super) fn send_zone_layout_preview(
    ws: &dyn WebSocketConnection,
    zone_id: &str,
    layout: &SpatialLayout,
) {
    let msg = serde_json::json!({
        "type": "zone_layout_preview",
        "zone_id": zone_id,
        "layout": layout
    });
    let _ = send_json(ws, &msg);
}

pub(super) fn send_zone_layout_preview_clear(ws: &dyn WebSocketConnection, zone_id: &str) {
    let msg = serde_json::json!({
        "type": "zone_layout_preview_clear",
        "zone_id": zone_id
    });
    let _ = send_json(ws, &msg);
}

fn preview_hostname() -> String {
    current_page_location().hostname
}

fn is_loopback_host(hostname: &str) -> bool {
    matches!(hostname, "localhost" | "127.0.0.1" | "::1")
}

fn preview_canvas_request_dimensions_for_host(
    hostname: &str,
    requested_fps: u32,
    width_cap: u32,
) -> (u32, u32) {
    let default_width = if is_loopback_host(hostname) {
        None
    } else {
        remote_preview_width_for_fps(requested_fps)
    };

    let preview_width = match (default_width, width_cap) {
        (None, 0) => 0,
        (None, cap) => cap,
        (Some(width), 0) => width,
        (Some(width), cap) => width.min(cap),
    };

    (preview_width, 0)
}

const fn remote_preview_width_for_fps(requested_fps: u32) -> Option<u32> {
    match requested_fps {
        24.. => None,
        12..=23 => Some(REMOTE_PREVIEW_WIDTH_MEDIUM),
        _ => Some(REMOTE_PREVIEW_WIDTH_LOW),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PreviewRoute, PreviewSubscriptionRequest, REMOTE_PREVIEW_WIDTH_LOW,
        REMOTE_PREVIEW_WIDTH_MEDIUM, RemotePreviewPath, RemotePreviewProfile,
        accept_transport_path_report, desired_preview_fps, preview_canvas_format_for_host,
        preview_canvas_request_dimensions_for_host, remote_preview_width_for_fps,
        should_stream_preview, web_viewport_preview_request_dimensions,
    };
    use crate::remote_bridge::TransportPathReport;

    const REMOTE_PATHS: [RemotePreviewPath; 3] = [
        RemotePreviewPath::Local,
        RemotePreviewPath::Direct,
        RemotePreviewPath::Relayed,
    ];

    fn remote_requests(path: RemotePreviewPath) -> Vec<PreviewSubscriptionRequest> {
        let route = PreviewRoute::Remote(path);
        let mut requests = Vec::new();
        for page_visible in [true, false] {
            for engine_fps in [1, 10, 20, 30, 45, 60] {
                let fps = desired_preview_fps(engine_fps, 60, page_visible);
                requests.push(PreviewSubscriptionRequest::web_viewport(route, fps));
                for width_cap in [0, 160, 320, 480, 704, 960, 2560] {
                    requests.push(PreviewSubscriptionRequest::canvas(route, fps, width_cap));
                }
            }
        }
        requests
    }

    #[test]
    fn bridge_path_kinds_select_the_preview_path() {
        assert_eq!(
            RemotePreviewPath::from_bridge_kind("local"),
            RemotePreviewPath::Local
        );
        assert_eq!(
            RemotePreviewPath::from_bridge_kind("direct"),
            RemotePreviewPath::Direct
        );
        for kind in ["relay", "turn", "unknown", "", "Direct", "satellite"] {
            assert_eq!(
                RemotePreviewPath::from_bridge_kind(kind),
                RemotePreviewPath::Relayed,
                "{kind}"
            );
        }
    }

    #[test]
    fn remote_profile_is_chosen_by_path() {
        let request = |path, fps, width_cap| {
            PreviewSubscriptionRequest::canvas(PreviewRoute::Remote(path), fps, width_cap)
        };
        let expected = |fps, width| PreviewSubscriptionRequest {
            fps,
            width,
            height: 0,
            format: "jpeg",
        };
        assert_eq!(request(RemotePreviewPath::Local, 60, 0), expected(60, 480));
        assert_eq!(request(RemotePreviewPath::Direct, 60, 0), expected(30, 480));
        assert_eq!(
            request(RemotePreviewPath::Relayed, 60, 0),
            expected(15, 320)
        );
        assert_eq!(request(RemotePreviewPath::Relayed, 6, 0), expected(6, 320));
        assert_eq!(
            request(RemotePreviewPath::Direct, 60, 704),
            expected(30, 480)
        );
        assert_eq!(
            request(RemotePreviewPath::Direct, 20, 256),
            expected(20, 256)
        );
    }

    #[test]
    fn remote_sessions_never_request_raw_pixels() {
        for path in REMOTE_PATHS {
            let profile = RemotePreviewProfile::for_path(path);
            for request in remote_requests(path) {
                assert_eq!(request.format, "jpeg", "{path:?} {request:?}");
                assert!(request.width > 0, "{path:?} {request:?}");
                assert!(request.width <= profile.max_width, "{path:?} {request:?}");
                assert!(request.fps <= profile.max_fps, "{path:?} {request:?}");
            }
        }
    }

    #[test]
    fn a_bridge_overrides_the_loopback_raw_format() {
        assert_eq!(
            PreviewSubscriptionRequest::canvas_for_host("localhost", 30, 0).format,
            "rgba"
        );
        let route = PreviewRoute::from_remote_path(Some(RemotePreviewPath::Local));
        assert_eq!(route, PreviewRoute::Remote(RemotePreviewPath::Local));
        assert_eq!(
            PreviewSubscriptionRequest::canvas(route, 30, 0).format,
            "jpeg"
        );
        assert_eq!(PreviewRoute::from_remote_path(None), PreviewRoute::Host);
    }

    #[test]
    fn path_reports_apply_in_generation_order() {
        let report = |generation| TransportPathReport {
            kind: "direct".to_owned(),
            generation,
        };
        assert!(accept_transport_path_report(None, &report(0)));
        assert!(accept_transport_path_report(Some(4), &report(4)));
        assert!(accept_transport_path_report(Some(4), &report(5)));
        assert!(!accept_transport_path_report(Some(4), &report(3)));
    }

    #[test]
    fn hidden_page_caps_preview_fps() {
        assert_eq!(desired_preview_fps(60, 60, false), 6);
        assert_eq!(desired_preview_fps(20, 15, false), 6);
        assert_eq!(desired_preview_fps(5, 60, false), 5);
    }

    #[test]
    fn hidden_tauri_window_disables_preview_streaming() {
        assert!(should_stream_preview(true, 60, 1));
        assert!(!should_stream_preview(false, 60, 1));
        assert!(!should_stream_preview(true, 0, 1));
        assert!(!should_stream_preview(true, 60, 0));
    }

    #[test]
    fn remote_preview_width_tracks_requested_fps() {
        assert_eq!(remote_preview_width_for_fps(30), None);
        assert_eq!(
            remote_preview_width_for_fps(15),
            Some(REMOTE_PREVIEW_WIDTH_MEDIUM)
        );
        assert_eq!(
            remote_preview_width_for_fps(6),
            Some(REMOTE_PREVIEW_WIDTH_LOW)
        );
    }

    #[test]
    fn loopback_preview_uses_rgba_format() {
        assert_eq!(preview_canvas_format_for_host("localhost"), "rgba");
    }

    #[test]
    fn loopback_preview_keeps_full_resolution() {
        assert_eq!(
            preview_canvas_request_dimensions_for_host("localhost", 6, 0),
            (0, 0)
        );
        assert_eq!(
            preview_canvas_request_dimensions_for_host("127.0.0.1", 30, 0),
            (0, 0)
        );
    }

    #[test]
    fn remote_preview_dimensions_scale_with_fps() {
        assert_eq!(
            preview_canvas_request_dimensions_for_host("remote.example", 30, 0),
            (0, 0)
        );
        assert_eq!(
            preview_canvas_request_dimensions_for_host("remote.example", 15, 0),
            (REMOTE_PREVIEW_WIDTH_MEDIUM, 0)
        );
        assert_eq!(
            preview_canvas_request_dimensions_for_host("remote.example", 6, 0),
            (REMOTE_PREVIEW_WIDTH_LOW, 0)
        );
    }

    #[test]
    fn loopback_preview_honors_explicit_width_cap() {
        assert_eq!(
            preview_canvas_request_dimensions_for_host("localhost", 60, 960),
            (960, 0)
        );
    }

    #[test]
    fn remote_preview_clamps_to_explicit_width_cap() {
        assert_eq!(
            preview_canvas_request_dimensions_for_host("remote.example", 15, 320),
            (320, 0)
        );
        assert_eq!(
            preview_canvas_request_dimensions_for_host("remote.example", 15, 960),
            (REMOTE_PREVIEW_WIDTH_MEDIUM, 0)
        );
    }

    #[test]
    fn web_viewport_preview_stays_full_resolution() {
        assert_eq!(web_viewport_preview_request_dimensions(), (0, 0));
    }

    #[test]
    fn preview_subscription_request_tracks_width_caps() {
        assert_ne!(
            PreviewSubscriptionRequest::canvas_for_host("localhost", 15, 0),
            PreviewSubscriptionRequest::canvas_for_host("localhost", 15, 320)
        );
    }

    #[test]
    fn web_viewport_request_uses_host_format() {
        assert_eq!(
            PreviewSubscriptionRequest::web_viewport_for_host("localhost", 15),
            PreviewSubscriptionRequest {
                fps: 15,
                width: 0,
                height: 0,
                format: "rgba",
            }
        );
    }
}
