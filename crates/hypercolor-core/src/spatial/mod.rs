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

mod sampler;
mod topology;

pub use sampler::{sample_led, sample_zone};
pub use topology::generate_positions;

use std::sync::Arc;

use hypercolor_types::canvas::Canvas;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::{DeviceZone, SpatialLayout};

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
    prepared_zones: Vec<sampler::PreparedZone>,
}

impl SpatialEngine {
    /// Create a new spatial engine from a layout definition.
    ///
    /// Generates LED positions for every zone's topology on construction.
    #[must_use]
    pub fn new(layout: SpatialLayout) -> Self {
        let mut engine = Self {
            layout: Arc::new(layout),
            prepared_zones: Vec::new(),
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
        zones.truncate(self.prepared_zones.len());
        zones.reserve(self.prepared_zones.len().saturating_sub(zones.len()));

        for (index, prepared_zone) in self.prepared_zones.iter().enumerate() {
            if index == zones.len() {
                let mut colors = Vec::new();
                sampler::sample_prepared_zone_into(canvas, prepared_zone, &mut colors);
                zones.push(ZoneColors {
                    zone_id: prepared_zone.zone_id.clone(),
                    colors,
                });
                continue;
            }

            let zone = &mut zones[index];
            if zone.zone_id != prepared_zone.zone_id {
                zone.zone_id.clone_from(&prepared_zone.zone_id);
            }
            sampler::sample_prepared_zone_into(canvas, prepared_zone, &mut zone.colors);
        }
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

    /// Recompute `led_positions` for every zone from its topology.
    fn rebuild_positions(&mut self) {
        let layout = Arc::make_mut(&mut self.layout);

        for zone in &mut layout.zones {
            zone.led_positions = topology::generate_positions(&zone.topology);
        }
        self.prepared_zones = layout
            .zones
            .iter()
            .filter(|zone| should_sample_zone(zone))
            .map(|zone| sampler::prepare_zone(zone, layout))
            .collect();
    }
}

fn should_sample_zone(zone: &DeviceZone) -> bool {
    // Display devices render through the dedicated display-output pipeline, but
    // existing layouts still persist those viewport helpers as `zone_name =
    // "Display"` matrix zones. Skip them here so the LED sampler only prepares
    // real LED-bearing zones.
    zone.zone_name.as_deref() != Some("Display")
}
