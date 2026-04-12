use std::sync::Arc;

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
