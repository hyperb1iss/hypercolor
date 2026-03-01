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

use hypercolor_types::canvas::Canvas;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::SpatialLayout;

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
    layout: SpatialLayout,
}

impl SpatialEngine {
    /// Create a new spatial engine from a layout definition.
    ///
    /// Generates LED positions for every zone's topology on construction.
    #[must_use]
    pub fn new(layout: SpatialLayout) -> Self {
        let mut engine = Self { layout };
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
        self.layout
            .zones
            .iter()
            .map(|zone| {
                let colors = sampler::sample_zone(canvas, zone, &self.layout);
                ZoneColors {
                    zone_id: zone.id.clone(),
                    colors,
                }
            })
            .collect()
    }

    /// Replace the active layout and recompute all LED positions.
    ///
    /// Call this when the user edits the layout (moves/adds/removes zones,
    /// changes topology, etc.). The next [`sample`](Self::sample) call will
    /// use the new positions.
    pub fn update_layout(&mut self, layout: SpatialLayout) {
        self.layout = layout;
        self.rebuild_positions();
    }

    /// Access the current layout.
    #[must_use]
    pub fn layout(&self) -> &SpatialLayout {
        &self.layout
    }

    /// Recompute `led_positions` for every zone from its topology.
    fn rebuild_positions(&mut self) {
        for zone in &mut self.layout.zones {
            zone.led_positions = topology::generate_positions(&zone.topology);
        }
    }
}
