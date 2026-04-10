use std::sync::Arc;

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_daemon::preview_runtime::PreviewRuntime;
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
