//! Polygon footprints shared by the Studio drawing and picking paths.

use hypercolor_types::spatial::{NormalizedPosition, ZoneShape};

/// The vertices of an explicit, finite polygon with at least three corners.
#[must_use]
pub fn vertices(shape: Option<&ZoneShape>) -> Option<&[NormalizedPosition]> {
    let Some(ZoneShape::Custom { vertices }) = shape else {
        return None;
    };
    (vertices.len() >= 3
        && vertices
            .iter()
            .all(|point| point.x.is_finite() && point.y.is_finite()))
    .then_some(vertices.as_slice())
}

/// SVG points in a unit view box, preserving the panel's own proportions.
#[must_use]
pub fn svg_points(shape: Option<&ZoneShape>) -> Option<String> {
    vertices(shape).map(|vertices| {
        vertices
            .iter()
            .map(|point| format!("{},{}", point.x, point.y))
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// Test a point in the polygon's normalized local coordinates, including edges.
#[must_use]
pub fn contains(vertices: &[NormalizedPosition], point: NormalizedPosition) -> bool {
    let Some(mut previous) = vertices.last().copied() else {
        return false;
    };
    let mut inside = false;
    for &current in vertices {
        let edge_x = current.x - previous.x;
        let edge_y = current.y - previous.y;
        let offset_x = point.x - previous.x;
        let offset_y = point.y - previous.y;
        let cross = offset_x * edge_y - offset_y * edge_x;
        let along = offset_x * edge_x + offset_y * edge_y;
        let length_squared = edge_x * edge_x + edge_y * edge_y;
        if length_squared > 0.0
            && cross.abs() <= 1e-6 * length_squared.sqrt()
            && (0.0..=length_squared).contains(&along)
        {
            return true;
        }
        if (previous.y > point.y) != (current.y > point.y)
            && point.x < edge_x * (point.y - previous.y) / edge_y + previous.x
        {
            inside = !inside;
        }
        previous = current;
    }
    inside
}
