use hypercolor_core::spatial::sample_viewport;
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::viewport::{FitMode, ViewportRect};

fn canvas_with_pixels(width: u32, height: u32, pixels: &[[u8; 4]]) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for (index, pixel) in pixels.iter().enumerate() {
        let x = (index as u32) % width;
        let y = (index as u32) / width;
        canvas.set_pixel(x, y, Rgba::new(pixel[0], pixel[1], pixel[2], pixel[3]));
    }
    canvas
}

#[test]
fn sample_viewport_stretch_crops_the_requested_region() {
    let source = canvas_with_pixels(
        4,
        1,
        &[
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 255, 255],
        ],
    );
    let mut target = Canvas::new(2, 1);

    sample_viewport(
        &mut target,
        &source,
        ViewportRect::new(0.5, 0.0, 0.5, 1.0),
        FitMode::Stretch,
        1.0,
    );

    assert_eq!(target.get_pixel(0, 0), Rgba::new(0, 0, 255, 255));
    assert_eq!(target.get_pixel(1, 0), Rgba::new(255, 255, 255, 255));
}

#[test]
fn sample_viewport_contain_letterboxes_when_aspect_ratios_differ() {
    let source = canvas_with_pixels(
        4,
        2,
        &[
            [255, 0, 0, 255],
            [255, 0, 0, 255],
            [0, 0, 255, 255],
            [0, 0, 255, 255],
            [255, 0, 0, 255],
            [255, 0, 0, 255],
            [0, 0, 255, 255],
            [0, 0, 255, 255],
        ],
    );
    let mut target = Canvas::new(4, 4);

    sample_viewport(
        &mut target,
        &source,
        ViewportRect::full(),
        FitMode::Contain,
        1.0,
    );

    for x in 0..4 {
        assert_eq!(target.get_pixel(x, 0), Rgba::BLACK);
        assert_eq!(target.get_pixel(x, 3), Rgba::BLACK);
    }
    assert_eq!(target.get_pixel(0, 1), Rgba::new(255, 0, 0, 255));
    assert_eq!(target.get_pixel(3, 1), Rgba::new(0, 0, 255, 255));
}

#[test]
fn sample_viewport_cover_uses_center_crop() {
    let source = canvas_with_pixels(
        4,
        2,
        &[
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 255, 255],
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 255, 255],
        ],
    );
    let mut target = Canvas::new(2, 2);

    sample_viewport(
        &mut target,
        &source,
        ViewportRect::full(),
        FitMode::Cover,
        1.0,
    );

    assert_eq!(target.get_pixel(0, 0), Rgba::new(0, 255, 0, 255));
    assert_eq!(target.get_pixel(1, 0), Rgba::new(0, 0, 255, 255));
    assert_eq!(target.get_pixel(0, 1), Rgba::new(0, 255, 0, 255));
    assert_eq!(target.get_pixel(1, 1), Rgba::new(0, 0, 255, 255));
}
