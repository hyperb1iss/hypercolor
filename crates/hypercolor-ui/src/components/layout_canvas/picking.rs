//! Which box a press on the canvas means.
//!
//! Boxes overlap constantly on a real rig, so a press resolves by geometry
//! rather than by whichever element paints on top: the smallest box under
//! the pointer wins, and an Alt-press walks down through the rest of the
//! stack beneath it. Small devices under long strips stay reachable
//! without anyone managing z-order.

use hypercolor_types::spatial::Output;

use crate::layout_geometry::DrawnBox;

/// One drawn box, in canvas pixel proportions (x spans the canvas aspect,
/// y spans 1).
#[derive(Debug, Clone, PartialEq)]
pub struct HitBox {
    pub id: String,
    pub center: (f32, f32),
    /// Half the drawn width and height, scale included.
    pub half: (f32, f32),
    pub rotation: f32,
    /// Ring-shaped boxes answer only inside their circle.
    pub circular: bool,
    /// Paint order; higher paints on top. Breaks ties between boxes of
    /// equal area.
    pub order: usize,
}

impl HitBox {
    /// The box an output draws on a canvas of pixel aspect `aspect`, as
    /// [`DrawnBox`] measures it.
    #[must_use]
    pub fn from_output(output: &Output, aspect: f32, order: usize) -> Self {
        let drawn = DrawnBox::of(output, aspect);
        Self {
            id: output.id.clone(),
            center: drawn.center,
            half: drawn.half,
            rotation: drawn.rotation,
            circular: drawn.circular,
            order,
        }
    }

    /// Whether `point` falls inside the drawn box.
    #[must_use]
    pub fn contains(&self, point: (f32, f32)) -> bool {
        let (dx, dy) = (point.0 - self.center.0, point.1 - self.center.1);
        let (sin, cos) = self.rotation.sin_cos();
        // Undo the box's on-screen rotation to test in its own frame.
        let local_x = dx * cos + dy * sin;
        let local_y = -dx * sin + dy * cos;
        if self.circular {
            local_x.hypot(local_y) <= self.half.0
        } else {
            local_x.abs() <= self.half.0 && local_y.abs() <= self.half.1
        }
    }

    fn area(&self) -> f32 {
        self.half.0 * self.half.1
    }
}

/// Every box under `point`, smallest first. Equal areas list the box that
/// paints on top first.
#[must_use]
pub fn hit_stack(boxes: &[HitBox], point: (f32, f32)) -> Vec<String> {
    let mut hits: Vec<&HitBox> = boxes.iter().filter(|hit| hit.contains(point)).collect();
    hits.sort_by(|a, b| {
        a.area()
            .total_cmp(&b.area())
            .then_with(|| b.order.cmp(&a.order))
    });
    hits.into_iter().map(|hit| hit.id.clone()).collect()
}

/// Resolve a press to one box of `stack`. A plain press takes the
/// smallest. A cycling (Alt) press takes the next selection down from the
/// box pressed last, wrapping to the smallest; boxes that `group` maps to
/// the same selection (one device's outputs, at the canvas root) count
/// once, so every cycling press visibly moves the selection.
#[must_use]
pub fn pick_hit<K: PartialEq>(
    stack: &[String],
    current: Option<&str>,
    cycle: bool,
    group: impl Fn(&str) -> K,
) -> Option<String> {
    let smallest = stack.first()?;
    if !cycle {
        return Some(smallest.clone());
    }
    let mut groups: Vec<(K, &String)> = Vec::new();
    for id in stack {
        let key = group(id);
        if !groups.iter().any(|(seen, _)| *seen == key) {
            groups.push((key, id));
        }
    }
    let at = current
        .filter(|current| stack.iter().any(|id| id == current))
        .and_then(|current| {
            let key = group(current);
            groups.iter().position(|(seen, _)| *seen == key)
        });
    let next = at.map_or(0, |index| (index + 1) % groups.len());
    groups.get(next).map(|(_, id)| (*id).clone())
}
