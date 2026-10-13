//! Canvas capture: freshness, bursts, demand lifetime, encoding, and the
//! `GET /api/v1/canvas/frame` route built on it.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::{AppState, AppStateBuilder};
use hypercolor_daemon::canvas_capture::{
    CanvasCaptureRequest, CanvasImageFormat, MAX_CAPTURE_FRAMES, capture_canvas,
};
use hypercolor_daemon::domain::DomainError;
use hypercolor_daemon::preview_runtime::PreviewRuntime;
use hypercolor_types::canvas::{Canvas, Rgba};
use tower::ServiceExt;

const MAGENTA: Rgba = Rgba::new(255, 0, 200, 255);
/// Frame number for a frame published before a capture starts; the
/// simulated renderer counts up from 1 and never reaches it.
const STALE_FRAME_NUMBER: u32 = 900_000;

fn solid_frame(width: u32, height: u32, color: Rgba, frame_number: u32) -> CanvasFrame {
    let mut canvas = Canvas::new(width, height);
    canvas.fill(color);
    CanvasFrame::from_canvas(&canvas, frame_number, frame_number * 16)
}

fn runtime() -> (Arc<HypercolorBus>, PreviewRuntime) {
    let bus = Arc::new(HypercolorBus::new());
    let runtime = PreviewRuntime::new(Arc::clone(&bus));
    (bus, runtime)
}

/// Publish frames on the canvas lane until the capture finishes, the way the
/// render thread does while a canvas receiver exists.
fn spawn_renderer(bus: Arc<HypercolorBus>, width: u32, height: u32) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut frame_number = 1;
        loop {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let _ = bus
                .canvas_lane()
                .send(solid_frame(width, height, MAGENTA, frame_number));
            frame_number += 1;
        }
    })
}

#[tokio::test]
async fn capture_waits_for_a_frame_rendered_after_the_request() {
    let (bus, runtime) = runtime();
    bus.canvas_lane()
        .send_replace(solid_frame(8, 4, MAGENTA, STALE_FRAME_NUMBER));
    let renderer = spawn_renderer(Arc::clone(&bus), 8, 4);

    let frames = capture_canvas(
        &runtime,
        CanvasCaptureRequest::single(CanvasImageFormat::Png),
    )
    .await
    .expect("capture should succeed while frames publish");
    renderer.abort();

    assert_eq!(frames.len(), 1);
    let frame = &frames[0];
    assert!(frame.fresh, "a frame published after the request is fresh");
    assert_ne!(
        frame.frame_number, STALE_FRAME_NUMBER,
        "the frame already on the lane before the request is not the capture"
    );
    assert_eq!((frame.width, frame.height), (8, 4));
    assert_eq!((frame.source_width, frame.source_height), (8, 4));
}

#[tokio::test(start_paused = true)]
async fn capture_falls_back_to_the_last_frame_when_rendering_is_quiet() {
    let (bus, runtime) = runtime();
    // The last frame the render thread published before it went quiet.
    bus.canvas_lane()
        .send_replace(solid_frame(4, 4, MAGENTA, 42));

    let frames = capture_canvas(
        &runtime,
        CanvasCaptureRequest {
            frames: 4,
            ..CanvasCaptureRequest::single(CanvasImageFormat::Jpeg)
        },
    )
    .await
    .expect("a stale frame still captures");

    assert_eq!(
        frames.len(),
        1,
        "a burst stops at the first stale frame instead of repeating it"
    );
    assert!(!frames[0].fresh);
    assert_eq!(frames[0].frame_number, 42);
}

#[tokio::test(start_paused = true)]
async fn capture_reports_unavailable_before_any_frame_renders() {
    let (_bus, runtime) = runtime();

    let error = capture_canvas(
        &runtime,
        CanvasCaptureRequest::single(CanvasImageFormat::Jpeg),
    )
    .await
    .expect_err("an empty canvas lane has nothing to capture");

    assert!(
        matches!(error, DomainError::ServiceUnavailable { .. }),
        "unexpected error: {error:?}"
    );
}

#[tokio::test]
async fn capture_bursts_return_distinct_frames_in_order() {
    let (bus, runtime) = runtime();
    let renderer = spawn_renderer(Arc::clone(&bus), 8, 8);

    let frames = capture_canvas(
        &runtime,
        CanvasCaptureRequest {
            frames: 3,
            interval: Duration::from_millis(20),
            ..CanvasCaptureRequest::single(CanvasImageFormat::Jpeg)
        },
    )
    .await
    .expect("burst capture should succeed");
    renderer.abort();

    assert_eq!(frames.len(), 3);
    assert!(frames.iter().all(|frame| frame.fresh));
    assert!(
        frames
            .windows(2)
            .all(|pair| pair[0].frame_number < pair[1].frame_number),
        "burst frames must advance: {:?}",
        frames
            .iter()
            .map(|frame| frame.frame_number)
            .collect::<Vec<_>>()
    );
}

#[tokio::test(start_paused = true)]
async fn burst_counts_a_frame_published_during_the_interval_as_fresh() {
    let (bus, runtime) = runtime();
    let runtime = Arc::new(runtime);
    let publisher = {
        let runtime = Arc::clone(&runtime);
        let bus = Arc::clone(&bus);
        tokio::spawn(async move {
            while runtime.tracked_canvas_receiver_count() == 0 {
                tokio::task::yield_now().await;
            }
            bus.canvas_lane()
                .send_replace(solid_frame(4, 4, MAGENTA, 1));
            // One more frame lands mid-interval, then the renderer goes quiet.
            tokio::time::sleep(Duration::from_millis(50)).await;
            bus.canvas_lane()
                .send_replace(solid_frame(4, 4, MAGENTA, 2));
        })
    };

    let frames = capture_canvas(
        &runtime,
        CanvasCaptureRequest {
            frames: 3,
            interval: Duration::from_millis(100),
            ..CanvasCaptureRequest::single(CanvasImageFormat::Jpeg)
        },
    )
    .await
    .expect("burst capture should succeed");
    publisher.await.expect("publisher should finish");

    let summary: Vec<(u32, bool)> = frames
        .iter()
        .map(|frame| (frame.frame_number, frame.fresh))
        .collect();
    assert_eq!(
        summary,
        [(1, true), (2, true), (2, false)],
        "the frame published during the sleep is fresh; the quiet one after is not"
    );
}

#[tokio::test]
async fn capture_holds_canvas_demand_only_while_it_runs() {
    let (bus, runtime) = runtime();
    let runtime = Arc::new(runtime);
    assert_eq!(runtime.tracked_canvas_receiver_count(), 0);

    let observed = {
        let runtime = Arc::clone(&runtime);
        tokio::spawn(async move {
            while runtime.tracked_canvas_receiver_count() == 0 {
                tokio::task::yield_now().await;
            }
            runtime.tracked_canvas_demand()
        })
    };
    let renderer = spawn_renderer(Arc::clone(&bus), 4, 4);

    capture_canvas(
        &runtime,
        CanvasCaptureRequest::single(CanvasImageFormat::Jpeg),
    )
    .await
    .expect("capture should succeed");
    renderer.abort();
    let demand = observed.await.expect("observer task should finish");

    assert_eq!(demand.subscribers, 1);
    assert!(
        demand.any_full_resolution,
        "captures ask for the full canvas"
    );
    assert_eq!(
        runtime.tracked_canvas_receiver_count(),
        0,
        "finished captures release their demand"
    );
}

#[tokio::test]
async fn png_capture_keeps_exact_pixels_and_scales_without_upscaling() {
    let (bus, runtime) = runtime();
    let renderer = spawn_renderer(Arc::clone(&bus), 16, 8);

    let scaled = capture_canvas(
        &runtime,
        CanvasCaptureRequest {
            width: 8,
            ..CanvasCaptureRequest::single(CanvasImageFormat::Png)
        },
    )
    .await
    .expect("scaled capture should succeed");
    let oversized = capture_canvas(
        &runtime,
        CanvasCaptureRequest {
            width: 64,
            ..CanvasCaptureRequest::single(CanvasImageFormat::Png)
        },
    )
    .await
    .expect("oversized capture should succeed");
    renderer.abort();

    let image = image::load_from_memory_with_format(&scaled[0].bytes, image::ImageFormat::Png)
        .expect("capture should be a valid PNG")
        .to_rgb8();
    assert_eq!(
        image.dimensions(),
        (8, 4),
        "one side keeps the aspect ratio"
    );
    assert_eq!(image.get_pixel(3, 2).0, [255, 0, 200]);
    assert_eq!(
        (oversized[0].width, oversized[0].height),
        (16, 8),
        "requests past the canvas size fall back to the canvas size"
    );
}

#[tokio::test]
async fn jpeg_capture_decodes() {
    let (bus, runtime) = runtime();
    let renderer = spawn_renderer(Arc::clone(&bus), 32, 16);

    let frames = capture_canvas(
        &runtime,
        CanvasCaptureRequest::single(CanvasImageFormat::Jpeg),
    )
    .await
    .expect("capture should succeed");
    renderer.abort();

    let image = image::load_from_memory_with_format(&frames[0].bytes, image::ImageFormat::Jpeg)
        .expect("capture should be a valid JPEG")
        .to_rgb8();
    assert_eq!(image.dimensions(), (32, 16));
    let [r, g, b] = image.get_pixel(16, 8).0;
    assert!(
        r > 230 && g < 30 && b > 170,
        "JPEG keeps the color: {r},{g},{b}"
    );
}

#[tokio::test]
async fn capture_rejects_out_of_range_requests() {
    let (_bus, runtime) = runtime();
    let single = CanvasCaptureRequest::single(CanvasImageFormat::Jpeg);
    let cases = [
        (
            "frames",
            CanvasCaptureRequest {
                frames: 0,
                ..single
            },
        ),
        (
            "frames",
            CanvasCaptureRequest {
                frames: MAX_CAPTURE_FRAMES + 1,
                ..single
            },
        ),
        (
            "interval_ms",
            CanvasCaptureRequest {
                interval: Duration::from_secs(5),
                ..single
            },
        ),
        (
            "width",
            CanvasCaptureRequest {
                width: 100_000,
                ..single
            },
        ),
    ];

    for (field, request) in cases {
        let error = capture_canvas(&runtime, request)
            .await
            .expect_err("out-of-range request should fail");
        match error {
            DomainError::Validation {
                field: Some(reported),
                ..
            } => assert_eq!(reported, field),
            other => panic!("expected a validation error on {field}, got {other:?}"),
        }
    }
}

// ── REST ─────────────────────────────────────────────────────────────────

fn isolated_state() -> (Arc<AppState>, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    (Arc::new(AppStateBuilder::new(data_dir).build()), tempdir)
}

#[tokio::test]
async fn canvas_frame_route_returns_a_png_with_frame_headers() {
    let (state, _tempdir) = isolated_state();
    let renderer = spawn_renderer(Arc::clone(&state.event_bus), 16, 8);
    let app = api::build_router(Arc::clone(&state), None);

    let response = app
        .oneshot(
            Request::get("/api/v1/canvas/frame?format=png&width=8")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("route should respond");
    renderer.abort();

    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(headers["content-type"], "image/png");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["x-canvas-width"], "8");
    assert_eq!(headers["x-canvas-height"], "4");
    assert_eq!(headers["x-canvas-source-width"], "16");
    assert_eq!(headers["x-canvas-fresh"], "1");
    assert!(headers.contains_key("x-canvas-frame-number"));

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let image = image::load_from_memory_with_format(&body, image::ImageFormat::Png)
        .expect("body should be a PNG");
    assert_eq!((image.width(), image.height()), (8, 4));
}

#[tokio::test(start_paused = true)]
async fn canvas_frame_route_reports_unavailable_without_a_rendered_frame() {
    let (state, _tempdir) = isolated_state();
    let app = api::build_router(state, None);

    let response = app
        .oneshot(
            Request::get("/api/v1/canvas/frame")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("route should respond");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn canvas_frame_route_rejects_unknown_formats() {
    let (state, _tempdir) = isolated_state();
    let app = api::build_router(state, None);

    let response = app
        .oneshot(
            Request::get("/api/v1/canvas/frame?format=gif")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("route should respond");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
