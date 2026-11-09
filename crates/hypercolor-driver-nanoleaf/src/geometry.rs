//! Panel polygons in the Nanoleaf API's Cartesian coordinate system.

use hypercolor_types::device::{SegmentLayoutHint, SegmentPlacement};
use hypercolor_types::spatial::{LedTopology, NormalizedPosition, ZoneShape};

use crate::{NanoleafPanelLayout, NanoleafShapeType};

pub(crate) fn panel_layout_hint(
    panel: &NanoleafPanelLayout,
    global_orientation: f32,
) -> Option<SegmentLayoutHint> {
    let shape = panel.shape_kind()?;
    let side = shape.side_length() as f32;
    let vertices = panel_vertices(shape, side)?;
    let panel_angle = (f32::from(panel.o) + global_orientation).to_radians();
    let centroid = screen_point(
        panel.x as f32,
        panel.y as f32,
        global_orientation.to_radians(),
    );
    let vertices: Vec<_> = vertices
        .into_iter()
        .map(|point| screen_point(point.x, point.y, panel_angle))
        .collect();
    let min_x = vertices
        .iter()
        .map(|point| point.x)
        .fold(f32::INFINITY, f32::min);
    let min_y = vertices
        .iter()
        .map(|point| point.y)
        .fold(f32::INFINITY, f32::min);
    let max_x = vertices
        .iter()
        .map(|point| point.x)
        .fold(f32::NEG_INFINITY, f32::max);
    let max_y = vertices
        .iter()
        .map(|point| point.y)
        .fold(f32::NEG_INFINITY, f32::max);
    let width = max_x - min_x;
    let height = max_y - min_y;
    let normalize = |point: NormalizedPosition| {
        NormalizedPosition::new((point.x - min_x) / width, (point.y - min_y) / height)
    };
    Some(SegmentLayoutHint {
        // A triangle's centroid is not its bounding-box center. The same
        // transform must map both the polygon and its color sample.
        topology: Some(LedTopology::Custom {
            positions: vec![normalize(NormalizedPosition::new(0.0, 0.0))],
        }),
        shape: Some(ZoneShape::Custom {
            vertices: vertices.into_iter().map(normalize).collect(),
        }),
        placement: Some(SegmentPlacement {
            position: NormalizedPosition::new(
                centroid.x + min_x.midpoint(max_x),
                centroid.y + min_y.midpoint(max_y),
            ),
            size: NormalizedPosition::new(width, height),
            // Rotation is baked into the polygon before taking its bounds.
            rotation: 0.0,
        }),
        ..SegmentLayoutHint::default()
    })
}

fn screen_point(x: f32, y: f32, radians: f32) -> NormalizedPosition {
    let (sin, cos) = radians.sin_cos();
    NormalizedPosition::new(x * cos - y * sin, -(x * sin + y * cos))
}

fn panel_vertices(shape: NanoleafShapeType, side: f32) -> Option<Vec<NormalizedPosition>> {
    use NanoleafShapeType as Shape;
    match shape {
        Shape::TriangleLightPanels | Shape::TriangleShapes | Shape::MiniTriangle => {
            let height = side * 3.0_f32.sqrt() / 2.0;
            Some(vec![
                NormalizedPosition::new(0.0, height * 2.0 / 3.0),
                NormalizedPosition::new(-side / 2.0, -height / 3.0),
                NormalizedPosition::new(side / 2.0, -height / 3.0),
            ])
        }
        Shape::SquareCanvas
        | Shape::ControlSquareMaster
        | Shape::ControlSquarePassive
        | Shape::SkylightPanel
        | Shape::SkylightCtrlPrimary
        | Shape::SkylightCtrlPassive => Some(vec![
            NormalizedPosition::new(-side / 2.0, -side / 2.0),
            NormalizedPosition::new(side / 2.0, -side / 2.0),
            NormalizedPosition::new(side / 2.0, side / 2.0),
            NormalizedPosition::new(-side / 2.0, side / 2.0),
        ]),
        Shape::HexagonShapes | Shape::ElementsHexagon => Some(
            (0..6)
                .map(|index| {
                    let (sin, cos) = (index as f32 * std::f32::consts::PI / 3.0).sin_cos();
                    NormalizedPosition::new(side * cos, side * sin)
                })
                .collect(),
        ),
        _ => None,
    }
}
