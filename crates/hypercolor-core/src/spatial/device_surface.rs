//! Device touch surfaces placed on a render canvas.
//!
//! Drivers report touches in the touched device's own surface frame,
//! normalized to `[0, 1]` with the origin at the surface's top left. A
//! spatial layout places that device on a canvas, so the affine chain that
//! positions the device's LEDs also positions a contact: a touch maps to the
//! canvas point that the LED under the finger samples.
//!
//! A device with several LED outputs, such as a controller with separate pad
//! and strip segments, is placed per segment: a touch that names the segment
//! it landed on resolves through the output bound to that segment.

use std::sync::Arc;

use hypercolor_types::device::DeviceId;
use hypercolor_types::spatial::{
    Corner, EdgeBehavior, LedTopology, NormalizedPosition, SpatialLayout, StripDirection,
};

use super::is_led_sampled_zone;
use super::sampler::{OutputPlacement, resolve_edge_behavior};

/// Canvas placement of the device surfaces one spatial layout resolves.
///
/// A touch that names no segment resolves through the device's only LED
/// output in the layout. A touch that names a segment resolves through the
/// one LED output bound to that segment, or, when the device has a single
/// output that names no segment, through that output, which stands for the
/// whole device. Anything else gets no placement, and the contact carries
/// only its surface position: the layout says nothing about where that
/// surface sits on this canvas, or says several things, so there is no
/// single canvas point to report.
///
/// Cloning is cheap; the placements are shared.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceSurfaceMap {
    surfaces: Arc<[DeviceSurface]>,
}

#[derive(Debug, Clone, PartialEq)]
struct DeviceSurface {
    device_id: DeviceId,
    /// The output's segment, kept only when no other LED output of the same
    /// device names it, so a touch naming it has exactly one place to land.
    segment: Option<Arc<str>>,
    /// Whether this is the device's only LED output in the layout.
    sole: bool,
    /// Whether the output names no segment at all.
    whole_device: bool,
    placement: OutputPlacement,
    frame: SurfaceFrame,
    edge: EdgeBehavior,
}

impl DeviceSurfaceMap {
    /// Place every device surface `layout` resolves without ambiguity.
    ///
    /// `resolve` maps an output's layout `device_id` to the physical device
    /// it currently routes to, or `None` when nothing is bound to it.
    #[must_use]
    pub fn from_layout(
        layout: &SpatialLayout,
        mut resolve: impl FnMut(&str) -> Option<DeviceId>,
    ) -> Self {
        let outputs: Vec<(DeviceId, usize)> = layout
            .zones
            .iter()
            .enumerate()
            .filter(|(_, output)| is_led_sampled_zone(output))
            .filter_map(|(index, output)| Some((resolve(&output.device_id)?, index)))
            .collect();

        let surfaces = outputs
            .iter()
            .filter_map(|&(device_id, index)| {
                let output = &layout.zones[index];
                let siblings = || outputs.iter().filter(|(id, _)| *id == device_id);
                let sole = siblings().count() == 1;
                let segment = output.zone_name.as_deref().filter(|segment| {
                    siblings()
                        .filter(|(_, other)| {
                            layout.zones[*other].zone_name.as_deref() == Some(*segment)
                        })
                        .count()
                        == 1
                });
                if !sole && segment.is_none() {
                    return None;
                }
                Some(DeviceSurface {
                    device_id,
                    segment: segment.map(Arc::from),
                    sole,
                    whole_device: output.zone_name.is_none(),
                    placement: OutputPlacement::of(output),
                    frame: SurfaceFrame::of(&output.topology),
                    edge: resolve_edge_behavior(output, layout),
                })
            })
            .collect();
        Self { surfaces }
    }

    /// Normalized canvas position of a point on a device's surface, for a
    /// touch that names no segment.
    ///
    /// Equivalent to [`Self::segment_canvas_position`] with no segment.
    #[must_use]
    pub fn canvas_position(
        &self,
        device_id: DeviceId,
        x: f32,
        y: f32,
    ) -> Option<NormalizedPosition> {
        self.segment_canvas_position(device_id, None, x, y)
    }

    /// Normalized canvas position of a point on one segment of a device's
    /// surface.
    ///
    /// `segment` is the device segment the touch landed on, matched against
    /// the layout outputs' segment names, or `None` for a touch on a device
    /// with one surface. `x` and `y` are device-surface coordinates in
    /// `[0, 1]`; values outside that range are clamped first. Returns `None`
    /// when the touched surface has no placement in this map or the point is
    /// not finite.
    ///
    /// The result passes through the output's edge behavior exactly as its
    /// LED positions do, so it is the canvas point those LEDs would sample.
    #[must_use]
    pub fn segment_canvas_position(
        &self,
        device_id: DeviceId,
        segment: Option<&str>,
        x: f32,
        y: f32,
    ) -> Option<NormalizedPosition> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let surface = self.surface(device_id, segment)?;
        let local = surface.frame.to_local(x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
        Some(surface.placement.local_to_canvas(local, surface.edge))
    }

    fn surface(&self, device_id: DeviceId, segment: Option<&str>) -> Option<&DeviceSurface> {
        let mut surfaces = self
            .surfaces
            .iter()
            .filter(|surface| surface.device_id == device_id);
        match segment {
            None => surfaces.find(|surface| surface.sole),
            Some(segment) => {
                let mut whole_device = None;
                for surface in surfaces {
                    if surface.segment.as_deref() == Some(segment) {
                        return Some(surface);
                    }
                    if surface.sole && surface.whole_device {
                        whole_device = Some(surface);
                    }
                }
                whole_device
            }
        }
    }

    /// Whether any surface of `device_id` has a placement in this map.
    #[must_use]
    pub fn contains(&self, device_id: DeviceId) -> bool {
        self.surfaces
            .iter()
            .any(|surface| surface.device_id == device_id)
    }

    /// Number of placed surfaces.
    #[must_use]
    pub fn len(&self) -> usize {
        self.surfaces.len()
    }

    /// Whether no surface has a placement.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.surfaces.is_empty()
    }
}

/// How a device's surface frame sits inside its output's local box.
///
/// A matrix's LED index order is the device's own row-major order, so its
/// start corner says which corner of the output holds the device's top-left
/// LED. A strip's LED order runs along its surface's x axis from the first
/// LED, so its direction says which way the output lays that axis out. A
/// touch follows the same reflection, or turn, to land on the LED under it.
/// Other topologies take the surface frame as the output's local frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SurfaceFrame {
    /// Swap the surface axes before mirroring.
    transpose: bool,
    mirror_x: bool,
    mirror_y: bool,
}

impl SurfaceFrame {
    const IDENTITY: Self = Self {
        transpose: false,
        mirror_x: false,
        mirror_y: false,
    };

    const fn of(topology: &LedTopology) -> Self {
        let (transpose, mirror_x, mirror_y) = match topology {
            LedTopology::Matrix { start_corner, .. } => match start_corner {
                Corner::TopLeft => (false, false, false),
                Corner::TopRight => (false, true, false),
                Corner::BottomLeft => (false, false, true),
                Corner::BottomRight => (false, true, true),
            },
            LedTopology::Strip { direction, .. } => match direction {
                StripDirection::LeftToRight => (false, false, false),
                StripDirection::RightToLeft => (false, true, false),
                StripDirection::TopToBottom => (true, false, false),
                StripDirection::BottomToTop => (true, false, true),
            },
            LedTopology::Ring { .. }
            | LedTopology::ConcentricRings { .. }
            | LedTopology::PerimeterLoop { .. }
            | LedTopology::Point
            | LedTopology::Custom { .. } => return Self::IDENTITY,
        };
        Self {
            transpose,
            mirror_x,
            mirror_y,
        }
    }

    fn to_local(self, x: f32, y: f32) -> NormalizedPosition {
        let (u, v) = if self.transpose { (y, x) } else { (x, y) };
        NormalizedPosition::new(
            if self.mirror_x { 1.0 - u } else { u },
            if self.mirror_y { 1.0 - v } else { v },
        )
    }
}
