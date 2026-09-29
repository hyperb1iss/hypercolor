//! Preview FPS cap logic, subscription management, and backpressure handling.

use std::collections::VecDeque;

use leptos::prelude::*;
use serde::Serialize;

use super::messages::CanvasFrame;
use hypercolor_leptos_ext::canvas::supports_bitmap_worker_canvas;
use hypercolor_leptos_ext::prelude::current_page_location;
use hypercolor_types::spatial::SpatialLayout;

use super::transport::{WebSocketConnection, send_json};

pub const DEFAULT_PREVIEW_FPS_CAP: u32 = 60;
pub(super) const HIDDEN_TAB_PREVIEW_FPS_CAP: u32 = 6;
pub(super) const SCREEN_PREVIEW_FPS_CAP: u32 = 15;
pub(super) const WEB_VIEWPORT_PREVIEW_FPS_CAP: u32 = 15;
const REMOTE_PREVIEW_WIDTH_MEDIUM: u32 = 640;
const REMOTE_PREVIEW_WIDTH_LOW: u32 = 480;

/// Whether the socket may carry the canvas frame topics (`canvas`,
/// `screen_canvas` and `web_viewport_canvas`). Under a Remote bridge it never
/// does: the main canvas arrives as the bridge's video track, and no other
/// canvas streams frame by frame over Remote.
pub(super) const fn canvas_frame_topics_allowed(bridged: bool) -> bool {
    !bridged
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PreviewSubscriptionRequest {
    fps: u32,
    width: u32,
    height: u32,
    format: &'static str,
}

impl PreviewSubscriptionRequest {
    fn canvas(requested_fps: u32, width_cap: u32) -> Self {
        Self::canvas_for_host(preview_hostname().as_str(), requested_fps, width_cap)
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

    fn web_viewport(requested_fps: u32) -> Self {
        Self::web_viewport_for_host(preview_hostname().as_str(), requested_fps)
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

pub(super) fn request_preview_subscription(
    ws: &dyn WebSocketConnection,
    requested_preview_request: StoredValue<Option<PreviewSubscriptionRequest>>,
    set_preview_target_fps: WriteSignal<u32>,
    engine_target_fps: u32,
    client_cap: u32,
    width_cap: u32,
    page_visible: bool,
) {
    let request = PreviewSubscriptionRequest::canvas(
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
    engine_target_fps: u32,
    page_visible: bool,
) {
    let request = PreviewSubscriptionRequest::canvas(
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
    engine_target_fps: u32,
    page_visible: bool,
) {
    let request = PreviewSubscriptionRequest::web_viewport(desired_preview_fps(
        engine_target_fps,
        WEB_VIEWPORT_PREVIEW_FPS_CAP,
        page_visible,
    ));
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

/// Window property holding a function that returns the current
/// [`PreviewCounterSnapshot`] as a plain object, for browser test harnesses.
pub const PREVIEW_COUNTERS_GLOBAL: &str = "__HYPERCOLOR_PREVIEW_COUNTERS__";

/// Inclusive upper bounds, in milliseconds, of the displayed inter-frame
/// gap buckets; one more bucket counts every longer gap. The bounds bracket
/// the 60, 30, 20 and 15 fps intervals and twice the 15 fps interval.
pub const PREVIEW_GAP_BUCKET_BOUNDS_MS: [u32; 11] =
    [17, 34, 50, 67, 84, 100, 134, 200, 334, 500, 1_000];

/// Recent arrivals kept to tag a frame as it reaches a preview surface,
/// which happens right after it arrives.
const RECENT_FRAME_LIMIT: usize = 64;

/// Identifies one arrival on the main canvas stream: the generation of the
/// stream it arrived in, and its sequence number, which counts arrivals
/// across every stream. A surface takes the tag as the frame reaches it and
/// hands it back once the frame is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewTag {
    stream: u64,
    arrival: u64,
}

/// Point-in-time counters for the main canvas preview stream.
///
/// `received` counts every arrival. The measured surface is the first
/// main-stream preview to show a frame, until it unmounts. Each arrival is
/// classified once, when something settles it:
///
/// - `displayed` when the measured surface shows it;
/// - `dropped` when the measured surface shows a newer arrival first, or
///   the stream ends while that surface is still measuring;
/// - `unobserved` when no surface is measuring at that point: it arrived
///   before a surface started measuring, or its surface unmounted and the
///   stream ended or another surface took over.
///
/// Arrivals not yet settled appear in none of the three, so `received`
/// equals their sum only after the stream ends.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PreviewCounterSnapshot {
    /// Frames that arrived on the canvas stream.
    pub received: u64,
    /// Arrivals the measured surface put on screen.
    pub displayed: u64,
    /// Arrivals the measured surface never showed.
    pub dropped: u64,
    /// Arrivals settled while no surface was measuring.
    pub unobserved: u64,
    /// Inclusive upper bounds of the gap buckets, in milliseconds.
    pub gap_bounds_ms: Vec<u32>,
    /// Gaps between consecutive displayed frames, bucketed by
    /// `gap_bounds_ms`; the final entry counts every longer gap.
    pub gap_counts: Vec<u64>,
}

/// Received, displayed and dropped accounting for the main canvas stream,
/// with a histogram of the gaps between displayed frames.
///
/// Every arrival gets a sequence number, carried in its [`PreviewTag`]. A
/// display accounts for every arrival since the previous display, so frames
/// between them count as dropped however long the presenter took. One
/// surface measures at a time. A stream ends when the socket closes or the
/// preview unsubscribes, and the next one is a new generation, so a late
/// completion from an earlier stream is ignored.
#[derive(Debug, Clone, Default)]
pub struct PreviewCounters {
    received: u64,
    displayed: u64,
    dropped: u64,
    unobserved: u64,
    accounted_through: u64,
    recent: VecDeque<(u32, u64)>,
    stream: u64,
    next_surface: u64,
    measuring: Option<u64>,
    last_displayed_at_ms: Option<f64>,
    gap_counts: [u64; PREVIEW_GAP_BUCKET_BOUNDS_MS.len() + 1],
}

impl PreviewCounters {
    /// A fresh identity for one preview surface.
    pub fn register_surface(&mut self) -> u64 {
        self.next_surface += 1;
        self.next_surface
    }

    /// The tag of the newest arrival numbered `frame_number` in the current
    /// stream, to pass back to [`Self::record_displayed`] once it shows.
    #[must_use]
    pub fn tag(&self, frame_number: u32) -> Option<PreviewTag> {
        self.recent
            .iter()
            .rev()
            .find(|(number, _)| *number == frame_number)
            .map(|(_, arrival)| PreviewTag {
                stream: self.stream,
                arrival: *arrival,
            })
    }

    pub fn record_received(&mut self, frame_number: u32) {
        self.received += 1;
        if self.recent.len() == RECENT_FRAME_LIMIT {
            self.recent.pop_front();
        }
        self.recent.push_back((frame_number, self.received));
    }

    /// Count the arrival `tag` names as shown by `surface` at `now_ms`.
    ///
    /// The first surface to show a frame becomes the measured one; arrivals
    /// before that frame were unobserved. Returns `false`, changing nothing,
    /// for another surface, an earlier stream, or an arrival that is not
    /// newer than the last one accounted for.
    pub fn record_displayed(&mut self, surface: u64, tag: PreviewTag, now_ms: f64) -> bool {
        let sequence = tag.arrival;
        if tag.stream != self.stream
            || sequence <= self.accounted_through
            || self.measuring.is_some_and(|owner| owner != surface)
        {
            return false;
        }
        if self.measuring.is_none() {
            self.measuring = Some(surface);
            self.unobserved += sequence - 1 - self.accounted_through;
            self.accounted_through = sequence - 1;
            self.last_displayed_at_ms = None;
        }
        self.dropped += sequence - 1 - self.accounted_through;
        self.accounted_through = sequence;
        self.displayed += 1;
        if let Some(previous) = self.last_displayed_at_ms.replace(now_ms) {
            self.gap_counts[gap_bucket(now_ms - previous)] += 1;
        }
        true
    }

    /// `surface` stopped showing the stream. Arrivals it had not shown yet
    /// become unobserved unless another surface starts measuring.
    pub fn release_surface(&mut self, surface: u64) {
        if self.measuring == Some(surface) {
            self.measuring = None;
            self.last_displayed_at_ms = None;
        }
    }

    /// Close the stream (the socket closed or the preview unsubscribed).
    /// Unsettled arrivals are dropped if a surface is measuring and
    /// unobserved otherwise, and a new stream generation begins.
    pub fn end_stream(&mut self) {
        let outstanding = self.received - self.accounted_through;
        if self.measuring.is_some() {
            self.dropped += outstanding;
        } else {
            self.unobserved += outstanding;
        }
        self.accounted_through = self.received;
        self.recent.clear();
        self.stream += 1;
        self.last_displayed_at_ms = None;
    }

    #[must_use]
    pub fn snapshot(&self) -> PreviewCounterSnapshot {
        PreviewCounterSnapshot {
            received: self.received,
            displayed: self.displayed,
            dropped: self.dropped,
            unobserved: self.unobserved,
            gap_bounds_ms: PREVIEW_GAP_BUCKET_BOUNDS_MS.to_vec(),
            gap_counts: self.gap_counts.to_vec(),
        }
    }
}

fn gap_bucket(gap_ms: f64) -> usize {
    PREVIEW_GAP_BUCKET_BOUNDS_MS
        .iter()
        .position(|bound| gap_ms <= f64::from(*bound))
        .unwrap_or(PREVIEW_GAP_BUCKET_BOUNDS_MS.len())
}

/// Shared handle to the main canvas stream's [`PreviewCounters`].
#[derive(Debug, Clone, Copy)]
pub struct PreviewCounterHandle(StoredValue<PreviewCounters>);

impl PreviewCounterHandle {
    pub(super) fn new() -> Self {
        Self(StoredValue::new(PreviewCounters::default()))
    }

    #[must_use]
    pub fn snapshot(self) -> PreviewCounterSnapshot {
        self.0
            .try_with_value(PreviewCounters::snapshot)
            .unwrap_or_default()
    }

    pub(crate) fn register_surface(self) -> u64 {
        self.0
            .try_update_value(PreviewCounters::register_surface)
            .unwrap_or_default()
    }

    pub(crate) fn tag(self, frame_number: u32) -> Option<PreviewTag> {
        self.0
            .try_with_value(|counters| counters.tag(frame_number))
            .flatten()
    }

    pub(super) fn record_received(self, frame_number: u32) {
        let _ = self
            .0
            .try_update_value(|counters| counters.record_received(frame_number));
    }

    pub(crate) fn record_displayed(self, surface: u64, tag: PreviewTag, now_ms: f64) {
        let _ = self
            .0
            .try_update_value(|counters| counters.record_displayed(surface, tag, now_ms));
    }

    pub(crate) fn release_surface(self, surface: u64) {
        let _ = self
            .0
            .try_update_value(|counters| counters.release_surface(surface));
    }

    pub(super) fn end_stream(self) {
        let _ = self.0.try_update_value(PreviewCounters::end_stream);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PreviewSubscriptionRequest, REMOTE_PREVIEW_WIDTH_LOW, REMOTE_PREVIEW_WIDTH_MEDIUM,
        canvas_frame_topics_allowed, desired_preview_fps, preview_canvas_format_for_host,
        preview_canvas_request_dimensions_for_host, remote_preview_width_for_fps,
        should_stream_preview, web_viewport_preview_request_dimensions,
    };

    #[test]
    fn a_bridge_never_carries_canvas_frame_topics() {
        assert!(!canvas_frame_topics_allowed(true));
        assert!(canvas_frame_topics_allowed(false));
    }

    #[test]
    fn local_pages_keep_their_frame_formats() {
        assert_eq!(
            PreviewSubscriptionRequest::canvas_for_host("localhost", 30, 0),
            PreviewSubscriptionRequest {
                fps: 30,
                width: 0,
                height: 0,
                format: "rgba",
            }
        );
        assert_eq!(
            PreviewSubscriptionRequest::web_viewport_for_host("127.0.0.1", 15).format,
            "rgba"
        );
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
