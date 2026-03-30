#![allow(dead_code)]

#[path = "../src/control_geometry.rs"]
mod control_geometry;

use control_geometry::{
    FrameHandle, FrameRect, clamp_frame_rect, drag_frame_rect, resize_frame_rect,
};

#[test]
fn clamp_frame_rect_keeps_rect_inside_canvas_bounds() {
    let clamped = clamp_frame_rect(FrameRect::new(-0.2, 0.85, 0.6, 0.4), 0.05, 0.05);

    assert_eq!(clamped, FrameRect::new(0.0, 0.6, 0.6, 0.4));
}

#[test]
fn drag_frame_rect_stops_at_edges() {
    let dragged = drag_frame_rect(FrameRect::new(0.2, 0.25, 0.5, 0.4), 0.7, -0.4, 0.05, 0.05);

    assert_eq!(dragged, FrameRect::new(0.5, 0.0, 0.5, 0.4));
}

#[test]
fn resize_frame_rect_enforces_minimum_size_for_corner_drag() {
    let resized = resize_frame_rect(
        FrameRect::new(0.2, 0.15, 0.4, 0.35),
        FrameHandle::NorthWest,
        0.34,
        0.3,
        0.1,
        0.12,
    );

    assert!((resized.x - 0.5).abs() < 0.0001);
    assert!((resized.y - 0.38).abs() < 0.0001);
    assert!((resized.width - 0.1).abs() < 0.0001);
    assert!((resized.height - 0.12).abs() < 0.0001);
}

#[test]
fn resize_frame_rect_clamps_outward_growth_to_unit_square() {
    let resized = resize_frame_rect(
        FrameRect::new(0.3, 0.2, 0.45, 0.5),
        FrameHandle::SouthEast,
        0.5,
        0.6,
        0.05,
        0.05,
    );

    assert_eq!(resized, FrameRect::new(0.3, 0.2, 0.7, 0.8));
}
