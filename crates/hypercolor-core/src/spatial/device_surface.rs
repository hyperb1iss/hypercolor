//! Device touch surfaces placed on a render canvas.
//!
//! Drivers report touches in the touched device's own surface frame,
//! normalized to `[0, 1]` with the origin at the surface's top left. A
//! spatial layout places that device on a canvas, so the affine chain that
//! positions the device's LEDs also positions a contact: a touch maps to the
//! canvas point that the LED under the finger samples.

use std::sync::Arc;

use hypercolor_types::device::DeviceId;
use hypercolor_types::spatial::{
    Corner, EdgeBehavior, LedTopology, NormalizedPosition, SpatialLayout,
};

use super::is_led_sampled_zone;
use super::sampler::{OutputPlacement, resolve_edge_behavior};

/// Canvas placement of the device surfaces one spatial layout resolves.
///
/// A device gets a placement when exactly one LED output in the layout
/// routes to it. A device with no output there, or with several, gets none,
/// and its contacts carry only their surface position: the layout says
/// nothing about where that surface sits on this canvas, or says several
/// things, so there is no single canvas point to report.
///
/// Cloning is cheap; the placements are shared.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceSurfaceMap {
    surfaces: Arc<[DeviceSurface]>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DeviceSurface {
    device_id: DeviceId,
    placement: OutputPlacement,
    mirror_x: bool,
    mirror_y: bool,
    edge: EdgeBehavior,
}

impl DeviceSurfaceMap {
    /// Place every device that owns exactly one LED output in `layout`.
    ///
    /// `resolve` maps an output's layout `device_id` to the physical device
    /// it currently routes to, or `None` when nothing is bound to it.
    #[must_use]
    pub fn from_layout(
        layout: &SpatialLayout,
        mut resolve: impl FnMut(&str) -> Option<DeviceId>,
    ) -> Self {
        let mut placed: Vec<(DeviceId, usize)> = Vec::new();
        let mut ambiguous: Vec<DeviceId> = Vec::new();
        for (index, output) in layout.zones.iter().enumerate() {
            if !is_led_sampled_zone(output) {
                continue;
            }
            let Some(device_id) = resolve(&output.device_id) else {
                continue;
            };
            if ambiguous.contains(&device_id) {
                continue;
            }
            if let Some(existing) = placed.iter().position(|(id, _)| *id == device_id) {
                placed.swap_remove(existing);
                ambiguous.push(device_id);
                continue;
            }
            placed.push((device_id, index));
        }

        let surfaces = placed
            .into_iter()
            .map(|(device_id, index)| {
                let output = &layout.zones[index];
                let (mirror_x, mirror_y) = surface_orientation(&output.topology);
                DeviceSurface {
                    device_id,
                    placement: OutputPlacement::of(output),
                    mirror_x,
                    mirror_y,
                    edge: resolve_edge_behavior(output, layout),
                }
            })
            .collect();
        Self { surfaces }
    }

    /// Normalized canvas position of a point on a device's surface.
    ///
    /// `x` and `y` are device-surface coordinates in `[0, 1]`; values outside
    /// that range are clamped first. Returns `None` when the device has no
    /// placement in this map or the point is not finite.
    ///
    /// The result passes through the output's edge behavior exactly as its
    /// LED positions do, so it is the canvas point those LEDs would sample.
    #[must_use]
    pub fn canvas_position(
        &self,
        device_id: DeviceId,
        x: f32,
        y: f32,
    ) -> Option<NormalizedPosition> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let surface = self
            .surfaces
            .iter()
            .find(|surface| surface.device_id == device_id)?;
        let x = x.clamp(0.0, 1.0);
        let y = y.clamp(0.0, 1.0);
        let local = NormalizedPosition::new(
            if surface.mirror_x { 1.0 - x } else { x },
            if surface.mirror_y { 1.0 - y } else { y },
        );
        Some(surface.placement.local_to_canvas(local, surface.edge))
    }

    /// Whether `device_id` has a placement in this map.
    #[must_use]
    pub fn contains(&self, device_id: DeviceId) -> bool {
        self.surfaces
            .iter()
            .any(|surface| surface.device_id == device_id)
    }

    /// Number of placed devices.
    #[must_use]
    pub fn len(&self) -> usize {
        self.surfaces.len()
    }

    /// Whether no device has a placement.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.surfaces.is_empty()
    }
}

/// How a device's surface frame sits inside its output's local box.
///
/// A matrix's LED index order is the device's own row-major order, so its
/// start corner says which corner of the output holds the device's top-left
/// LED; a touch follows the same reflection to land on the LED under it.
/// Other topologies take the surface frame as the output's local frame.
const fn surface_orientation(topology: &LedTopology) -> (bool, bool) {
    match topology {
        LedTopology::Matrix { start_corner, .. } => match start_corner {
            Corner::TopLeft => (false, false),
            Corner::TopRight => (true, false),
            Corner::BottomLeft => (false, true),
            Corner::BottomRight => (true, true),
        },
        LedTopology::Strip { .. }
        | LedTopology::Ring { .. }
        | LedTopology::ConcentricRings { .. }
        | LedTopology::PerimeterLoop { .. }
        | LedTopology::Point
        | LedTopology::Custom { .. } => (false, false),
    }
}
