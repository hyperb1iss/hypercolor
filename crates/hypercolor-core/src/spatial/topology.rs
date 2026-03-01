//! LED position generation from topology definitions.
//!
//! Each [`LedTopology`] variant describes a geometric arrangement of LEDs
//! within a zone's `[0.0, 1.0]` bounding box. This module computes the
//! concrete [`NormalizedPosition`] for every LED in the topology.

use hypercolor_types::spatial::{Corner, LedTopology, NormalizedPosition, StripDirection, Winding};

/// Margin inset for ring topologies — keeps LEDs away from the zone edge.
const RING_MARGIN: f32 = 0.45;

/// Generate zone-local LED positions from a topology definition.
///
/// All returned positions are in `[0.0, 1.0]` zone-local space where
/// `(0.0, 0.0)` is the top-left of the zone's bounding box and
/// `(1.0, 1.0)` is the bottom-right.
#[must_use]
pub fn generate_positions(topology: &LedTopology) -> Vec<NormalizedPosition> {
    match topology {
        LedTopology::Strip { count, direction } => compute_strip(*count, *direction),
        LedTopology::Matrix {
            width,
            height,
            start_corner,
            ..
        } => compute_matrix(*width, *height, *start_corner),
        LedTopology::Ring {
            count,
            start_angle,
            direction,
        } => compute_ring(*count, *start_angle, *direction),
        LedTopology::ConcentricRings { rings } => compute_concentric(rings),
        LedTopology::PerimeterLoop {
            top,
            right,
            bottom,
            left,
            start_corner,
            direction,
        } => compute_perimeter(*top, *right, *bottom, *left, *start_corner, *direction),
        LedTopology::Point => vec![NormalizedPosition::new(0.5, 0.5)],
        LedTopology::Custom { positions } => positions.clone(),
    }
}

/// Strip topology: LEDs in a straight line along one axis.
///
/// The perpendicular axis is fixed at 0.5 (zone midline).
/// A single LED is centered at `(0.5, 0.5)`.
#[allow(clippy::as_conversions, clippy::cast_precision_loss)]
fn compute_strip(count: u32, direction: StripDirection) -> Vec<NormalizedPosition> {
    (0..count)
        .map(|i| {
            let t = if count <= 1 {
                0.5
            } else {
                i as f32 / (count - 1) as f32
            };
            match direction {
                StripDirection::LeftToRight => NormalizedPosition::new(t, 0.5),
                StripDirection::RightToLeft => NormalizedPosition::new(1.0 - t, 0.5),
                StripDirection::TopToBottom => NormalizedPosition::new(0.5, t),
                StripDirection::BottomToTop => NormalizedPosition::new(0.5, 1.0 - t),
            }
        })
        .collect()
}

/// Matrix topology: LEDs in a regular 2D grid.
///
/// Row-major order. Single-dimension axes center at 0.5.
#[allow(clippy::as_conversions, clippy::cast_precision_loss)]
fn compute_matrix(width: u32, height: u32, corner: Corner) -> Vec<NormalizedPosition> {
    let mut positions = Vec::with_capacity((width * height) as usize);
    for row in 0..height {
        for col in 0..width {
            let u = if width <= 1 {
                0.5
            } else {
                col as f32 / (width - 1) as f32
            };
            let v = if height <= 1 {
                0.5
            } else {
                row as f32 / (height - 1) as f32
            };
            let pos = match corner {
                Corner::TopLeft => NormalizedPosition::new(u, v),
                Corner::TopRight => NormalizedPosition::new(1.0 - u, v),
                Corner::BottomLeft => NormalizedPosition::new(u, 1.0 - v),
                Corner::BottomRight => NormalizedPosition::new(1.0 - u, 1.0 - v),
            };
            positions.push(pos);
        }
    }
    positions
}

/// Ring topology: LEDs arranged in a circle within the zone.
///
/// Uses `RING_MARGIN` (0.45) to inset from the zone edge.
#[allow(clippy::as_conversions, clippy::cast_precision_loss)]
fn compute_ring(count: u32, start_angle: f32, direction: Winding) -> Vec<NormalizedPosition> {
    (0..count)
        .map(|i| {
            let t = i as f32 / count.max(1) as f32;
            let angle = match direction {
                Winding::Clockwise => start_angle + t * std::f32::consts::TAU,
                Winding::CounterClockwise => start_angle - t * std::f32::consts::TAU,
            };
            NormalizedPosition::new(
                0.5 + RING_MARGIN * angle.cos(),
                0.5 + RING_MARGIN * angle.sin(),
            )
        })
        .collect()
}

/// Concentric rings: multiple rings at different radii, centered in the zone.
///
/// Rings are emitted outermost-first (order of the `rings` Vec).
#[allow(clippy::as_conversions, clippy::cast_precision_loss)]
fn compute_concentric(rings: &[hypercolor_types::spatial::RingDef]) -> Vec<NormalizedPosition> {
    let mut positions = Vec::new();
    for ring in rings {
        let r = ring.radius * RING_MARGIN;
        for i in 0..ring.count {
            let t = i as f32 / ring.count.max(1) as f32;
            let angle = match ring.direction {
                Winding::Clockwise => ring.start_angle + t * std::f32::consts::TAU,
                Winding::CounterClockwise => ring.start_angle - t * std::f32::consts::TAU,
            };
            positions.push(NormalizedPosition::new(
                0.5 + r * angle.cos(),
                0.5 + r * angle.sin(),
            ));
        }
    }
    positions
}

/// Perimeter loop: LEDs trace the rectangular boundary of the zone.
///
/// Builds edges in clockwise order from `TopLeft`, then rotates and/or
/// reverses to match the requested `start_corner` and `direction`.
#[allow(clippy::as_conversions, clippy::cast_precision_loss)]
fn compute_perimeter(
    top: u32,
    right: u32,
    bottom: u32,
    left: u32,
    start_corner: Corner,
    direction: Winding,
) -> Vec<NormalizedPosition> {
    // Build edges in CW order from TopLeft.
    let mut edges: Vec<Vec<NormalizedPosition>> = Vec::with_capacity(4);

    // Top edge: left to right
    edges.push(
        (0..top)
            .map(|i| {
                let t = if top <= 1 { 0.0 } else { i as f32 / top as f32 };
                NormalizedPosition::new(t, 0.0)
            })
            .collect(),
    );

    // Right edge: top to bottom
    edges.push(
        (0..right)
            .map(|i| {
                let t = if right <= 1 {
                    0.0
                } else {
                    i as f32 / right as f32
                };
                NormalizedPosition::new(1.0, t)
            })
            .collect(),
    );

    // Bottom edge: right to left
    edges.push(
        (0..bottom)
            .map(|i| {
                let t = if bottom <= 1 {
                    0.0
                } else {
                    i as f32 / bottom as f32
                };
                NormalizedPosition::new(1.0 - t, 1.0)
            })
            .collect(),
    );

    // Left edge: bottom to top
    edges.push(
        (0..left)
            .map(|i| {
                let t = if left <= 1 {
                    0.0
                } else {
                    i as f32 / left as f32
                };
                NormalizedPosition::new(0.0, 1.0 - t)
            })
            .collect(),
    );

    // Rotate edges so the start_corner's edge comes first.
    let rotation_offset = match start_corner {
        Corner::TopLeft => 0,
        Corner::TopRight => 1,
        Corner::BottomRight => 2,
        Corner::BottomLeft => 3,
    };
    edges.rotate_left(rotation_offset);

    // Flatten edges into a single list.
    let mut positions: Vec<NormalizedPosition> = edges.into_iter().flatten().collect();

    // Reverse for counter-clockwise traversal.
    if direction == Winding::CounterClockwise {
        positions.reverse();
    }

    positions
}
