//! Integration tests for the half-block canvas widget.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;

use hypercolor_tui::widgets::HalfBlockCanvas;

/// Build a solid-color RGB pixel buffer.
fn solid_pixels(width: u16, height: u16, r: u8, g: u8, b: u8) -> Vec<u8> {
    let count = usize::from(width) * usize::from(height);
    let mut pixels = Vec::with_capacity(count * 3);
    for _ in 0..count {
        pixels.extend_from_slice(&[r, g, b]);
    }
    pixels
}

#[test]
fn solid_color_fills_all_cells() {
    let pixels = solid_pixels(4, 4, 255, 0, 0);
    let canvas = HalfBlockCanvas::new(&pixels, 4, 4);
    let area = Rect::new(0, 0, 4, 2); // 4 cols × 2 rows = 8 half-block rows
    let mut buf = Buffer::empty(area);
    canvas.render(area, &mut buf);

    for y in 0..2 {
        for x in 0..4 {
            let cell = &buf[(x, y)];
            assert_eq!(
                cell.symbol(),
                "\u{2580}",
                "cell ({x},{y}) should be half-block"
            );
            assert_eq!(cell.fg, Color::Rgb(255, 0, 0));
            assert_eq!(cell.bg, Color::Rgb(255, 0, 0));
        }
    }
}

#[test]
fn vertical_stripe_pattern() {
    // 2x2 image: top row red, bottom row blue
    let pixels = [
        255, 0, 0, 255, 0, 0, // row 0: red, red
        0, 0, 255, 0, 0, 255, // row 1: blue, blue
    ];
    let canvas = HalfBlockCanvas::new(&pixels, 2, 2);
    let area = Rect::new(0, 0, 2, 1); // 2 cols × 1 row → 2 virtual rows
    let mut buf = Buffer::empty(area);
    canvas.render(area, &mut buf);

    for x in 0..2 {
        let cell = &buf[(x, 0)];
        assert_eq!(cell.fg, Color::Rgb(255, 0, 0), "top should be red");
        assert_eq!(cell.bg, Color::Rgb(0, 0, 255), "bottom should be blue");
    }
}

#[test]
fn downsampling_larger_image() {
    // 10x10 solid green, rendered into 3x2 terminal area
    let pixels = solid_pixels(10, 10, 0, 255, 0);
    let canvas = HalfBlockCanvas::new(&pixels, 10, 10);
    let area = Rect::new(0, 0, 3, 2);
    let mut buf = Buffer::empty(area);
    canvas.render(area, &mut buf);

    // All cells should be green (nearest-neighbor downsample of solid color)
    for y in 0..2 {
        for x in 0..3 {
            let cell = &buf[(x, y)];
            assert_eq!(cell.fg, Color::Rgb(0, 255, 0));
            assert_eq!(cell.bg, Color::Rgb(0, 255, 0));
        }
    }
}

#[test]
fn zero_source_dimensions_no_panic() {
    let canvas = HalfBlockCanvas::new(&[], 0, 0);
    let area = Rect::new(0, 0, 5, 5);
    let mut buf = Buffer::empty(area);
    canvas.render(area, &mut buf);
}

#[test]
fn zero_area_no_panic() {
    let pixels = solid_pixels(2, 2, 128, 128, 128);
    let canvas = HalfBlockCanvas::new(&pixels, 2, 2);
    let area = Rect::new(0, 0, 0, 0);
    let mut buf = Buffer::empty(area);
    canvas.render(area, &mut buf);
}

#[test]
fn offset_area_renders_correctly() {
    // Render at a non-zero origin
    let pixels = solid_pixels(1, 2, 100, 200, 50);
    let canvas = HalfBlockCanvas::new(&pixels, 1, 2);
    let area = Rect::new(5, 3, 1, 1);
    let mut buf = Buffer::empty(Rect::new(0, 0, 10, 10));
    canvas.render(area, &mut buf);

    let cell = &buf[(5, 3)];
    assert_eq!(cell.symbol(), "\u{2580}");
    assert_eq!(cell.fg, Color::Rgb(100, 200, 50));
}

#[test]
fn truncated_pixel_data_renders_black() {
    // 2x2 image needs 12 bytes, provide only 6 (1 row)
    let pixels = [255, 0, 0, 0, 255, 0];
    let canvas = HalfBlockCanvas::new(&pixels, 2, 2);
    let area = Rect::new(0, 0, 2, 1);
    let mut buf = Buffer::empty(area);
    canvas.render(area, &mut buf);

    // Top row has data, bottom row is out of bounds → black
    let cell = &buf[(0, 0)];
    assert_eq!(cell.fg, Color::Rgb(255, 0, 0)); // top pixel exists
    assert_eq!(cell.bg, Color::Rgb(0, 0, 0)); // bottom pixel → black fallback
}
