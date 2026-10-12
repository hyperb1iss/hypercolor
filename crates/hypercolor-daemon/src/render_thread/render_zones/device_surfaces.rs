//! Per-zone canvas placement of device touch surfaces.
//!
//! Each render zone samples its canvas through its own spatial layout, so a
//! touch on a device lands at a different canvas point in every zone that
//! places that device, and at none in a zone that does not. The cache keeps
//! one [`DeviceSurfaceMap`] per zone, rebuilt only when the zone's layout or
//! the layout-to-device bindings change.

use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_core::spatial::{DeviceSurfaceMap, SpatialEngine};
use hypercolor_types::device::DeviceId;
use hypercolor_types::scene::ZoneId;
use hypercolor_types::spatial::SpatialLayout;

#[derive(Default)]
pub(crate) struct DeviceSurfaceCache {
    bindings: HashMap<String, DeviceId>,
    bindings_generation: Option<u64>,
    zones: HashMap<ZoneId, CachedZoneSurfaces>,
}

struct CachedZoneSurfaces {
    layout: Arc<SpatialLayout>,
    surfaces: DeviceSurfaceMap,
}

impl DeviceSurfaceCache {
    /// Adopt the layout-device bindings the output stage routes with.
    ///
    /// `generation` is the backend manager's mapping generation; bindings
    /// are copied and every zone's placement dropped only when it moves.
    pub(crate) fn observe_bindings<'a>(
        &mut self,
        generation: u64,
        bindings: impl Iterator<Item = (&'a str, DeviceId)>,
    ) {
        if self.bindings_generation == Some(generation) {
            return;
        }
        self.bindings.clear();
        self.bindings.extend(
            bindings.map(|(layout_device_id, device_id)| (layout_device_id.to_owned(), device_id)),
        );
        self.bindings_generation = Some(generation);
        self.zones.clear();
    }

    /// Device surfaces placed on `zone_id`'s canvas, if any device is placed.
    pub(crate) fn surfaces_for(
        &mut self,
        zone_id: ZoneId,
        engine: Option<&SpatialEngine>,
    ) -> Option<DeviceSurfaceMap> {
        if self.bindings.is_empty() {
            return None;
        }
        let layout = engine?.layout();
        let current = self
            .zones
            .get(&zone_id)
            .filter(|cached| Arc::ptr_eq(&cached.layout, &layout));
        let surfaces = if let Some(cached) = current {
            cached.surfaces.clone()
        } else {
            let bindings = &self.bindings;
            let surfaces = DeviceSurfaceMap::from_layout(&layout, |layout_device_id| {
                bindings.get(layout_device_id).copied()
            });
            self.zones.insert(
                zone_id,
                CachedZoneSurfaces {
                    layout,
                    surfaces: surfaces.clone(),
                },
            );
            surfaces
        };
        (!surfaces.is_empty()).then_some(surfaces)
    }

    /// Forget placements for zones that no longer render.
    pub(crate) fn retain_zones(&mut self, mut keep: impl FnMut(&ZoneId) -> bool) {
        self.zones.retain(|zone_id, _| keep(zone_id));
    }

    pub(crate) fn clear_zones(&mut self) {
        self.zones.clear();
    }
}
