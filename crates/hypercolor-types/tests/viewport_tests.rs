use hypercolor_types::viewport::{MIN_VIEWPORT_EDGE, PixelRect, ViewportRect};

#[test]
fn viewport_rect_clamp_keeps_rect_in_bounds() {
    let rect = ViewportRect::new(0.9, 0.95, 0.5, 0.5).clamp();

    assert_eq!(rect, ViewportRect::new(0.5, 0.5, 0.5, 0.5));
}

#[test]
fn viewport_rect_clamp_normalizes_non_finite_values() {
    let rect = ViewportRect::new(f32::NAN, f32::INFINITY, 0.0, f32::NEG_INFINITY).clamp();

    assert_eq!(rect.x, 0.0);
    assert_eq!(rect.y, 0.0);
    assert_eq!(rect.width, MIN_VIEWPORT_EDGE);
    assert_eq!(rect.height, 1.0);
}

#[test]
fn viewport_rect_to_pixel_rect_rounds_outward() {
    let rect = ViewportRect::new(0.25, 0.25, 0.5, 0.5);

    assert_eq!(
        rect.to_pixel_rect(8, 4),
        PixelRect {
            x: 2,
            y: 1,
            width: 4,
            height: 2,
        }
    );
}

#[test]
fn viewport_rect_to_pixel_rect_guarantees_at_least_one_pixel() {
    let rect = ViewportRect::new(0.99, 0.99, MIN_VIEWPORT_EDGE, MIN_VIEWPORT_EDGE);

    assert_eq!(
        rect.to_pixel_rect(1, 1),
        PixelRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }
    );
}
