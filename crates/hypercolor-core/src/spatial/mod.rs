//! Spatial layout engine — maps effect canvas pixels to physical LED positions.
//!
//! The spatial engine is the bridge between beautiful pixels and physical photons.
//! It takes a [`SpatialLayout`] describing where every device zone sits on the
//! canvas, generates LED positions from each zone's [`LedTopology`], and samples
//! the [`Canvas`] at those positions to produce per-zone color data.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────┐    ┌──────────────────┐    ┌──────────────────┐
//! │ SpatialLayout │───▶│  SpatialEngine   │───▶│  Vec<ZoneColors> │
//! │ (zone defs)  │    │  (precomputed    │    │  (LED RGB data)  │
//! │              │    │   LED positions) │    │                  │
//! └──────────────┘    └───────┬──────────┘    └──────────────────┘
//!                             │
//!                     ┌───────▼──────────┐
//!                     │     Canvas       │
//!                     │ (320×200 RGBA)   │
//!                     └──────────────────┘
//! ```

mod plan;
mod sampler;
mod topology;
mod viewport;

pub use plan::{
    PreparedAreaSample, PreparedBilinearSample, PreparedNearestSample, PreparedZonePlan,
    PreparedZoneSamples,
};
pub use sampler::{sample_led, sample_zone};
pub use topology::generate_positions;
pub use viewport::sample_viewport;

use std::sync::Arc;

use hypercolor_types::canvas::Canvas;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::{DeviceZone, SpatialLayout};

/// Layout zone name reserved for display-only viewports.
pub const DISPLAY_ZONE_NAME: &str = "Display";

/// Return whether a layout zone represents a display viewport instead of LEDs.
#[must_use]
pub fn is_display_zone(zone: &DeviceZone) -> bool {
    zone.zone_name.as_deref() == Some(DISPLAY_ZONE_NAME)
}

/// Return whether a layout zone contributes sampled LED colors.
#[must_use]
pub fn is_led_sampled_zone(zone: &DeviceZone) -> bool {
    !is_display_zone(zone)
}

/// The spatial sampling engine.
///
/// Holds a [`SpatialLayout`] with precomputed LED positions for every zone.
/// On each frame, [`sample`](Self::sample) reads the canvas and produces
/// a `Vec<ZoneColors>` ready for dispatch to device backends.
///
/// LED positions are generated once from each zone's topology and cached
/// inside the layout's `DeviceZone::led_positions` field. Call
/// [`update_layout`](Self::update_layout) when the layout changes to
/// recompute positions.
#[derive(Debug, Clone)]
pub struct SpatialEngine {
    /// The active spatial layout with precomputed LED positions.
    layout: Arc<SpatialLayout>,
    /// Immutable per-zone sampling plans cached from the layout.
    prepared_zones: Arc<[PreparedZonePlan]>,
    plan_generation: u64,
}

impl SpatialEngine {
    /// Create a new spatial engine from a layout definition.
    ///
    /// Generates LED positions for every zone's topology on construction.
    #[must_use]
    pub fn new(layout: SpatialLayout) -> Self {
        let mut engine = Self {
            layout: Arc::new(layout),
            prepared_zones: Arc::default(),
            plan_generation: 0,
        };
        engine.rebuild_positions();
        engine
    }

    /// Sample the canvas at every LED's position, producing per-zone color data.
    ///
    /// Iterates all zones in the layout, transforms each LED's zone-local
    /// position to canvas coordinates, samples the canvas using the zone's
    /// sampling mode, and returns the results grouped by zone.
    #[must_use]
    pub fn sample(&self, canvas: &Canvas) -> Vec<ZoneColors> {
        let mut zones = Vec::new();
        self.sample_into(canvas, &mut zones);
        zones
    }

    /// Sample the canvas into an existing output buffer, reusing allocations.
    pub fn sample_into(&self, canvas: &Canvas, zones: &mut Vec<ZoneColors>) {
        let next_index = self.sample_append_into_at(canvas, zones, 0);
        zones.truncate(next_index);
    }

    /// Append sampled zones to an existing output buffer without allocating a temporary vector.
    pub fn append_sample_into(&self, canvas: &Canvas, zones: &mut Vec<ZoneColors>) {
        let start_index = zones.len();
        let _ = self.sample_append_into_at(canvas, zones, start_index);
    }

    /// Sample the canvas into `zones` starting at `start_index`, reusing existing entries when possible.
    ///
    /// Returns the exclusive end index of the sampled range.
    pub fn sample_append_into_at(
        &self,
        canvas: &Canvas,
        zones: &mut Vec<ZoneColors>,
        start_index: usize,
    ) -> usize {
        let next_index = start_index.saturating_add(self.prepared_zones.len());
        zones.reserve(next_index.saturating_sub(zones.len()));

        let reusable_count = zones
            .len()
            .saturating_sub(start_index)
            .min(self.prepared_zones.len());
        let append_start = start_index + reusable_count;

        for (zone, prepared_zone) in zones[start_index..append_start]
            .iter_mut()
            .zip(&self.prepared_zones[..reusable_count])
        {
            if zone.zone_id != prepared_zone.zone_id {
                zone.zone_id.clone_from(&prepared_zone.zone_id);
            }
            sampler::sample_prepared_zone_into(canvas, prepared_zone, &mut zone.colors);
        }

        for prepared_zone in &self.prepared_zones[reusable_count..] {
            let mut colors = Vec::with_capacity(prepared_zone.prepared_samples.len());
            sampler::sample_prepared_zone_into(canvas, prepared_zone, &mut colors);
            zones.push(ZoneColors {
                zone_id: prepared_zone.zone_id.clone(),
                colors,
            });
        }

        next_index
    }

    /// Replace the active layout and recompute all LED positions.
    ///
    /// Call this when the user edits the layout (moves/adds/removes zones,
    /// changes topology, etc.). The next [`sample`](Self::sample) call will
    /// use the new positions.
    pub fn update_layout(&mut self, layout: SpatialLayout) {
        self.layout = Arc::new(layout);
        self.rebuild_positions();
    }

    /// Access the current layout.
    #[must_use]
    pub fn layout(&self) -> Arc<SpatialLayout> {
        Arc::clone(&self.layout)
    }

    #[must_use]
    pub fn sampling_plan(&self) -> Arc<[PreparedZonePlan]> {
        Arc::clone(&self.prepared_zones)
    }

    #[must_use]
    pub const fn plan_generation(&self) -> u64 {
        self.plan_generation
    }

    /// Recompute `led_positions` for every zone from its topology.
    fn rebuild_positions(&mut self) {
        let layout = Arc::make_mut(&mut self.layout);

        for zone in &mut layout.zones {
            zone.led_positions = topology::generate_positions(&zone.topology);
        }
        self.plan_generation = self.plan_generation.saturating_add(1);
        self.prepared_zones = layout
            .zones
            .iter()
            .filter(|zone| is_led_sampled_zone(zone))
            .map(|zone| sampler::prepare_zone(zone, layout, self.plan_generation))
            .collect::<Vec<_>>()
            .into();
    }
}
