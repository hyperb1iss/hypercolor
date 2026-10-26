use hypercolor_core::bus::CanvasFrame;
use hypercolor_leptos_ext::ws::{PreviewFrame, PreviewFrameChannel, PreviewPixelFormat};
use hypercolor_types::canvas::{Canvas, PublishedSurface, Rgba};

use super::{CanvasFormat, shared_canvas_rgba, try_encode_cached_canvas_preview_binary};

#[test]
fn raw_preview_body_shares_surface_storage_and_outlives_the_frame() {
    let mut canvas = Canvas::new(3, 2);
    canvas.fill(Rgba::new(17, 32, 64, 127));
    let frame = CanvasFrame::from_surface(PublishedSurface::from_owned_canvas(canvas, 9, 11));
    let source_pointer = frame.rgba_bytes().as_ptr();
    let bytes = shared_canvas_rgba(&frame);
    assert_eq!(
        bytes.as_ptr(),
        source_pointer,
        "the body must not copy pixels"
    );
    drop(frame);
    assert_eq!(&bytes[..], &[17, 32, 64, 127].repeat(6));
    let clone = bytes.clone();
    drop(bytes);
    assert_eq!(clone.as_ptr(), source_pointer);
    assert_eq!(clone.len(), 24);
}

#[test]
fn raw_preview_keeps_pixels_and_wire_metadata_after_source_release() {
    let mut canvas = Canvas::new(3, 2);
    canvas.fill(Rgba::new(17, 32, 64, 127));
    let frame = CanvasFrame::from_surface(PublishedSurface::from_owned_canvas(canvas, 9, 11));
    let payload = try_encode_cached_canvas_preview_binary(&frame, CanvasFormat::Rgba, 1.0, 0, 0)
        .expect("full-size raw preview encodes");
    drop(frame);
    let decoded = PreviewFrame::decode(&payload).expect("wire decoder accepts raw preview");
    assert_eq!(decoded.channel, PreviewFrameChannel::Canvas);
    assert_eq!(decoded.frame_number, 9);
    assert_eq!(decoded.timestamp_ms, 11);
    assert_eq!((decoded.width, decoded.height), (3, 2));
    assert_eq!(decoded.format, PreviewPixelFormat::Rgba);
    assert_eq!(&decoded.payload[..], &[17, 32, 64, 127].repeat(6));
}
