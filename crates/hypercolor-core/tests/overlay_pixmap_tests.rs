use tiny_skia::{Pixmap, PremultipliedColorU8};

use hypercolor_core::overlay::{OverlayBuffer, OverlaySize, overlay_buffer_from_pixmap};

fn set_pixel(pixmap: &mut Pixmap, x: u32, y: u32, rgba: [u8; 4]) {
    let color = PremultipliedColorU8::from_rgba(rgba[0], rgba[1], rgba[2], rgba[3])
        .expect("premultiplied pixel should be valid");
    let width = pixmap.width();
    let index = (y * width + x) as usize;
    pixmap
        .pixels_mut()
        .get_mut(index)
        .expect("pixel should exist")
        .clone_from(&color);
}

#[test]
fn overlay_buffer_from_pixmap_preserves_premultiplied_bytes() {
    let mut pixmap = Pixmap::new(2, 2).expect("pixmap should allocate");
    set_pixel(&mut pixmap, 0, 0, [0, 0, 0, 0]);
    set_pixel(&mut pixmap, 1, 0, [255, 128, 64, 255]);
    set_pixel(&mut pixmap, 0, 1, [100, 50, 25, 128]);
    set_pixel(&mut pixmap, 1, 1, [1, 2, 3, 4]);

    let buffer = overlay_buffer_from_pixmap(&pixmap).expect("bridge should succeed");

    assert_eq!(buffer.width, 2);
    assert_eq!(buffer.height, 2);
    assert_eq!(
        buffer.pixels,
        vec![0, 0, 0, 0, 255, 128, 64, 255, 100, 50, 25, 128, 1, 2, 3, 4,]
    );
}

#[test]
fn overlay_buffer_copy_from_pixmap_rejects_size_mismatch() {
    let pixmap = Pixmap::new(3, 1).expect("pixmap should allocate");
    let mut buffer = OverlayBuffer::new(OverlaySize::new(2, 1));

    let error = buffer
        .copy_from_pixmap(&pixmap)
        .expect_err("size mismatch should fail");
    assert!(
        error
            .to_string()
            .contains("pixmap size 3x1 did not match overlay buffer 2x1")
    );
}
