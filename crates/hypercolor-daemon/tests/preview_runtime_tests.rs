use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_daemon::preview_runtime::{PreviewPixelFormat, PreviewRuntime, PreviewStreamDemand};
use hypercolor_types::canvas::Canvas;

#[test]
fn preview_runtime_snapshot_tracks_canvas_publication_and_receivers() {
    let bus = Arc::new(HypercolorBus::new());
    let runtime = PreviewRuntime::new(Arc::clone(&bus));
    let _canvas_rx = runtime.canvas_receiver();
    let frame = CanvasFrame::from_canvas(&Canvas::new(2, 1), 9, 33);
    let _ = bus.canvas_sender().send(frame.clone());

    runtime.record_canvas_publication(frame.frame_number, frame.timestamp_ms);

    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.canvas_receivers, 1);
    assert_eq!(snapshot.screen_canvas_receivers, 0);
    assert_eq!(snapshot.canvas_frames_published, 1);
    assert_eq!(snapshot.screen_canvas_frames_published, 0);
    assert_eq!(snapshot.latest_canvas_frame_number, 9);
    assert_eq!(snapshot.latest_canvas_timestamp_ms, 33);
}

#[test]
fn preview_runtime_snapshot_tracks_screen_publication_and_receivers() {
    let bus = Arc::new(HypercolorBus::new());
    let runtime = PreviewRuntime::new(Arc::clone(&bus));
    let _screen_rx = runtime.screen_canvas_receiver();
    let frame = CanvasFrame::from_canvas(&Canvas::new(1, 1), 17, 44);
    let _ = bus.screen_canvas_sender().send(frame.clone());

    runtime.record_screen_canvas_publication(frame.frame_number, frame.timestamp_ms);

    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.canvas_receivers, 0);
    assert_eq!(snapshot.screen_canvas_receivers, 1);
    assert_eq!(snapshot.canvas_frames_published, 0);
    assert_eq!(snapshot.screen_canvas_frames_published, 1);
    assert_eq!(snapshot.latest_screen_canvas_frame_number, 17);
    assert_eq!(snapshot.latest_screen_canvas_timestamp_ms, 44);
}

#[test]
fn preview_runtime_snapshot_tracks_latest_canvas_frame_without_publication() {
    let bus = Arc::new(HypercolorBus::new());
    let runtime = PreviewRuntime::new(bus);

    runtime.note_canvas_frame(23, 77);

    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.canvas_frames_published, 0);
    assert_eq!(snapshot.latest_canvas_frame_number, 23);
    assert_eq!(snapshot.latest_canvas_timestamp_ms, 77);
}

#[test]
fn preview_runtime_tracks_canvas_demand_across_receivers() {
    let bus = Arc::new(HypercolorBus::new());
    let runtime = PreviewRuntime::new(bus);
    let mut full_res = runtime.canvas_receiver();
    let mut remote_jpeg = runtime.canvas_receiver();

    full_res.update_demand(PreviewStreamDemand {
        fps: 30,
        format: PreviewPixelFormat::Rgba,
        width: 0,
        height: 0,
    });
    remote_jpeg.update_demand(PreviewStreamDemand {
        fps: 15,
        format: PreviewPixelFormat::Jpeg,
        width: 640,
        height: 0,
    });

    let demand = runtime.canvas_demand();
    assert_eq!(demand.subscribers, 2);
    assert_eq!(demand.max_fps, 30);
    assert_eq!(demand.max_width, 640);
    assert_eq!(demand.max_height, 0);
    assert!(demand.any_full_resolution);
    assert!(!demand.any_rgb);
    assert!(demand.any_rgba);
    assert!(demand.any_jpeg);
}

#[test]
fn preview_runtime_removes_demand_when_receiver_drops() {
    let bus = Arc::new(HypercolorBus::new());
    let runtime = PreviewRuntime::new(bus);

    {
        let mut screen = runtime.screen_canvas_receiver();
        screen.update_demand(PreviewStreamDemand {
            fps: 12,
            format: PreviewPixelFormat::Jpeg,
            width: 480,
            height: 270,
        });

        let demand = runtime.screen_canvas_demand();
        assert_eq!(demand.subscribers, 1);
        assert_eq!(demand.max_width, 480);
        assert_eq!(demand.max_height, 270);
        assert!(demand.any_jpeg);
    }

    assert_eq!(runtime.screen_canvas_demand().subscribers, 0);
}

#[test]
fn preview_runtime_updates_summary_when_receiver_demand_changes() {
    let bus = Arc::new(HypercolorBus::new());
    let runtime = PreviewRuntime::new(bus);
    let mut canvas = runtime.canvas_receiver();
    let mut screen = runtime.screen_canvas_receiver();

    canvas.update_demand(PreviewStreamDemand {
        fps: 30,
        format: PreviewPixelFormat::Rgba,
        width: 1280,
        height: 720,
    });
    screen.update_demand(PreviewStreamDemand {
        fps: 20,
        format: PreviewPixelFormat::Jpeg,
        width: 640,
        height: 360,
    });

    canvas.update_demand(PreviewStreamDemand {
        fps: 12,
        format: PreviewPixelFormat::Rgb,
        width: 320,
        height: 180,
    });

    let canvas_demand = runtime.canvas_demand();
    assert_eq!(canvas_demand.subscribers, 1);
    assert_eq!(canvas_demand.max_fps, 12);
    assert_eq!(canvas_demand.max_width, 320);
    assert_eq!(canvas_demand.max_height, 180);
    assert!(!canvas_demand.any_full_resolution);
    assert!(canvas_demand.any_rgb);
    assert!(!canvas_demand.any_rgba);
    assert!(!canvas_demand.any_jpeg);

    let screen_demand = runtime.screen_canvas_demand();
    assert_eq!(screen_demand.subscribers, 1);
    assert_eq!(screen_demand.max_fps, 20);
    assert_eq!(screen_demand.max_width, 640);
    assert_eq!(screen_demand.max_height, 360);
    assert!(!screen_demand.any_rgb);
    assert!(!screen_demand.any_rgba);
    assert!(screen_demand.any_jpeg);
}

#[test]
fn preview_runtime_demand_summary_stays_coherent_during_concurrent_updates() {
    let bus = Arc::new(HypercolorBus::new());
    let runtime = PreviewRuntime::new(bus);
    let mut canvas = runtime.canvas_receiver();
    let stop = Arc::new(AtomicBool::new(false));

    let rgba_summary = PreviewStreamDemand {
        fps: 30,
        format: PreviewPixelFormat::Rgba,
        width: 1280,
        height: 720,
    };
    let jpeg_summary = PreviewStreamDemand {
        fps: 15,
        format: PreviewPixelFormat::Jpeg,
        width: 640,
        height: 360,
    };
    canvas.update_demand(rgba_summary);

    let reader_runtime = runtime.clone();
    let reader_stop = Arc::clone(&stop);
    let reader = thread::spawn(move || {
        while !reader_stop.load(Ordering::Relaxed) {
            let summary = reader_runtime.canvas_demand();
            let is_rgba_state = summary
                == hypercolor_daemon::preview_runtime::PreviewDemandSummary {
                    subscribers: 1,
                    max_fps: 30,
                    max_width: 1280,
                    max_height: 720,
                    any_full_resolution: false,
                    any_rgb: false,
                    any_rgba: true,
                    any_jpeg: false,
                };
            let is_jpeg_state = summary
                == hypercolor_daemon::preview_runtime::PreviewDemandSummary {
                    subscribers: 1,
                    max_fps: 15,
                    max_width: 640,
                    max_height: 360,
                    any_full_resolution: false,
                    any_rgb: false,
                    any_rgba: false,
                    any_jpeg: true,
                };
            assert!(
                is_rgba_state || is_jpeg_state,
                "saw torn preview summary: {summary:?}"
            );
        }
    });

    for _ in 0..10_000 {
        canvas.update_demand(jpeg_summary);
        canvas.update_demand(rgba_summary);
    }

    stop.store(true, Ordering::Relaxed);
    reader.join().expect("reader thread should not panic");
}

#[test]
fn preview_runtime_preserves_large_demand_values() {
    let bus = Arc::new(HypercolorBus::new());
    let runtime = PreviewRuntime::new(bus);
    let mut canvas = runtime.canvas_receiver();

    canvas.update_demand(PreviewStreamDemand {
        fps: 8_192,
        format: PreviewPixelFormat::Jpeg,
        width: 120_000,
        height: 90_000,
    });

    let demand = runtime.canvas_demand();
    assert_eq!(demand.subscribers, 1);
    assert_eq!(demand.max_fps, 8_192);
    assert_eq!(demand.max_width, 120_000);
    assert_eq!(demand.max_height, 90_000);
    assert!(!demand.any_full_resolution);
    assert!(!demand.any_rgb);
    assert!(!demand.any_rgba);
    assert!(demand.any_jpeg);
}
