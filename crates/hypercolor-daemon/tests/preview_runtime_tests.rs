use hypercolor_core::bus::CanvasFrame;
use hypercolor_daemon::preview_runtime::PreviewRuntime;
use hypercolor_types::canvas::Canvas;

#[test]
fn preview_runtime_snapshot_tracks_canvas_publication_and_receivers() {
    let runtime = PreviewRuntime::new();
    let _canvas_rx = runtime.canvas_receiver();

    runtime.publish_canvas(CanvasFrame::from_canvas(&Canvas::new(2, 1), 9, 33));

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
    let runtime = PreviewRuntime::new();
    let _screen_rx = runtime.screen_canvas_receiver();

    runtime.publish_screen_canvas(CanvasFrame::from_canvas(&Canvas::new(1, 1), 17, 44));

    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.canvas_receivers, 0);
    assert_eq!(snapshot.screen_canvas_receivers, 1);
    assert_eq!(snapshot.canvas_frames_published, 0);
    assert_eq!(snapshot.screen_canvas_frames_published, 1);
    assert_eq!(snapshot.latest_screen_canvas_frame_number, 17);
    assert_eq!(snapshot.latest_screen_canvas_timestamp_ms, 44);
}
