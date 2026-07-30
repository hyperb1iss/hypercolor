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
#[cfg(feature = "spatial-workspace-test-hooks")]
pub use sampler::SpatialWorkspaceAllocationTestHook;
pub use sampler::{sample_canvas_position, sample_led, sample_zone};
pub use topology::generate_positions;
pub use viewport::sample_viewport;

use std::sync::Arc;

use hypercolor_types::canvas::{Canvas, SurfaceDescriptor};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::{Output, SpatialLayout};
use thiserror::Error;

/// Layout zone name reserved for display-only viewports.
pub const DISPLAY_ZONE_NAME: &str = "Display";

/// Failure to construct an addressable spatial sampling plan.
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum SpatialPlanError {
    /// The canvas has no addressable pixels.
    #[error("spatial canvas dimensions must be nonzero, got {width}x{height}")]
    EmptyCanvas { width: u32, height: u32 },
    /// The canvas byte length exceeds the host address space.
    #[error("spatial canvas {width}x{height} is not addressable on this host")]
    CanvasByteLengthOverflow { width: u32, height: u32 },
    /// The configured Gaussian kernel has no representable sample count.
    #[error("gaussian radius {radius} has an unaddressable kernel")]
    GaussianKernelUnaddressable { radius: u32 },
    /// The configured Gaussian kernel could not reserve its weight storage.
    #[error("gaussian kernel allocation failed for {sample_count} samples")]
    GaussianKernelAllocation { sample_count: usize },
    /// The prepared frame-sampling workspace could not be admitted.
    #[error(transparent)]
    SamplingResources(#[from] SpatialSamplingError),
}

/// Failure to prepare frame-scoped spatial sampling resources.
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum SpatialSamplingError {
    /// Summed-area geometry exceeds the host address space or accumulator range.
    #[error("summed-area workspace for {width}x{height} is not addressable on this host")]
    AreaWorkspaceUnaddressable { width: u32, height: u32 },
    /// The summed-area table could not reserve its checked entry count.
    #[error("summed-area workspace allocation failed for {width}x{height} ({entry_count} entries)")]
    AreaWorkspaceAllocation {
        width: u32,
        height: u32,
        entry_count: usize,
    },
    /// The aggregate workspace pool exceeds the caller's resource budget.
    #[error(
        "summed-area workspace for {width}x{height} requires {required_bytes} bytes, capacity is {capacity_bytes} bytes"
    )]
    AreaWorkspaceCapacityExceeded {
        width: u32,
        height: u32,
        required_bytes: usize,
        capacity_bytes: usize,
    },
}

/// Aggregate resource budget for reusable frame-sampling workspaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpatialSamplingCapacity {
    max_area_workspace_bytes: usize,
}

impl SpatialSamplingCapacity {
    /// Admit every workspace representable by the host allocator.
    pub const UNBOUNDED: Self = Self {
        max_area_workspace_bytes: usize::MAX,
    };

    #[must_use]
    pub const fn new(max_area_workspace_bytes: usize) -> Self {
        Self {
            max_area_workspace_bytes,
        }
    }

    #[must_use]
    pub const fn max_area_workspace_bytes(self) -> usize {
        self.max_area_workspace_bytes
    }
}

/// Current retained memory usage of the summed-area workspace pool.
///
/// Retained workspaces include both idle workspaces and workspaces leased by
/// concurrent frame samplers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpatialSamplingWorkspaceUsage {
    /// Number of retained summed-area workspaces.
    pub retained_workspaces: usize,
    /// Bytes occupied by all retained summed-area tables.
    pub retained_bytes: usize,
    /// Number of workspace allocations admitted but not yet completed.
    pub reserved_workspaces: usize,
    /// Bytes reserved for admitted workspace allocations.
    pub reserved_bytes: usize,
}

/// Return whether a layout zone represents a display viewport instead of LEDs.
#[must_use]
pub fn is_display_zone(zone: &Output) -> bool {
    zone.zone_name.as_deref() == Some(DISPLAY_ZONE_NAME)
}

/// Return whether a layout zone contributes sampled LED colors.
#[must_use]
pub fn is_led_sampled_zone(zone: &Output) -> bool {
    !is_display_zone(zone)
}

/// The spatial sampling engine.
///
/// Holds a [`SpatialLayout`] with precomputed LED positions for every zone.
/// On each frame, [`sample`](Self::sample) reads the canvas and produces
/// a `Vec<ZoneColors>` ready for dispatch to device backends.
///
/// LED positions are generated once from each zone's topology and cached
/// inside the layout's `Output::led_positions` field. Call
/// [`update_layout`](Self::update_layout) when the layout changes to
/// recompute positions.
#[derive(Debug, Clone)]
pub struct SpatialEngine {
    /// The active spatial layout with precomputed LED positions.
    layout: Arc<SpatialLayout>,
    /// Immutable per-zone sampling plans cached from the layout.
    prepared_zones: Arc<[PreparedZonePlan]>,
    area_workspaces: Option<Arc<sampler::AreaWorkspacePool>>,
    sampling_capacity: SpatialSamplingCapacity,
    plan_generation: u64,
}

impl SpatialEngine {
    /// Create a new spatial engine from a layout definition.
    ///
    /// Generates LED positions for every zone's topology on construction.
    #[must_use]
    pub fn new(layout: SpatialLayout) -> Self {
        let mut layout = layout;
        let sampling_capacity = SpatialSamplingCapacity::UNBOUNDED;
        match prepare_layout(&mut layout, 1, sampling_capacity) {
            Ok(prepared) => Self {
                layout: Arc::new(layout),
                prepared_zones: prepared.zones,
                area_workspaces: prepared.area_workspaces,
                sampling_capacity,
                plan_generation: 1,
            },
            Err(error) => {
                tracing::warn!(%error, "Rejected unaddressable spatial sampling plan");
                Self {
                    layout: Arc::new(layout),
                    prepared_zones: Arc::default(),
                    area_workspaces: None,
                    sampling_capacity,
                    plan_generation: 0,
                }
            }
        }
    }

    /// Construct an engine after validating every prepared byte offset.
    ///
    /// # Errors
    ///
    /// Returns [`SpatialPlanError`] when the canvas or a Gaussian kernel is
    /// not representable on this host, or when kernel storage cannot be
    /// reserved.
    pub fn try_new(layout: SpatialLayout) -> Result<Self, SpatialPlanError> {
        Self::try_new_with_sampling_capacity(layout, SpatialSamplingCapacity::UNBOUNDED)
    }

    /// Construct an engine with an explicit reusable-workspace budget.
    ///
    /// # Errors
    ///
    /// Returns [`SpatialPlanError`] when the plan or its canonical workspace
    /// cannot be admitted within `sampling_capacity`.
    pub fn try_new_with_sampling_capacity(
        mut layout: SpatialLayout,
        sampling_capacity: SpatialSamplingCapacity,
    ) -> Result<Self, SpatialPlanError> {
        let plan_generation = 1;
        let prepared = prepare_layout(&mut layout, plan_generation, sampling_capacity)?;
        Ok(Self {
            layout: Arc::new(layout),
            prepared_zones: prepared.zones,
            area_workspaces: prepared.area_workspaces,
            sampling_capacity,
            plan_generation,
        })
    }

    /// Sample the canvas at every LED's position, producing per-zone color data.
    ///
    /// Iterates all zones in the layout, transforms each LED's zone-local
    /// position to canvas coordinates, samples the canvas using the zone's
    /// sampling mode, and returns the results grouped by zone.
    #[must_use]
    pub fn sample(&self, canvas: &Canvas) -> Vec<ZoneColors> {
        self.try_sample(canvas).unwrap_or_else(|error| {
            tracing::warn!(%error, "Spatial sampling resources were unavailable");
            Vec::new()
        })
    }

    /// Sample the canvas after fallibly preparing any frame-scoped resources.
    ///
    /// # Errors
    ///
    /// Returns [`SpatialSamplingError`] when an alternate canvas descriptor
    /// cannot acquire a reusable summed-area workspace.
    pub fn try_sample(&self, canvas: &Canvas) -> Result<Vec<ZoneColors>, SpatialSamplingError> {
        let mut zones = Vec::new();
        self.try_sample_into(canvas, &mut zones)?;
        Ok(zones)
    }

    /// Sample the canvas into an existing output buffer, reusing allocations.
    pub fn sample_into(&self, canvas: &Canvas, zones: &mut Vec<ZoneColors>) {
        if let Err(error) = self.try_sample_into(canvas, zones) {
            tracing::warn!(%error, "Spatial sampling resources were unavailable");
            zones.clear();
        }
    }

    /// Sample into an existing output buffer with typed workspace failures.
    ///
    /// # Errors
    ///
    /// Returns [`SpatialSamplingError`] before changing `zones` when a
    /// summed-area workspace cannot be acquired.
    pub fn try_sample_into(
        &self,
        canvas: &Canvas,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<(), SpatialSamplingError> {
        let next_index = self.try_sample_append_into_at(canvas, zones, 0)?;
        zones.truncate(next_index);
        Ok(())
    }

    /// Append sampled zones to an existing output buffer without allocating a temporary vector.
    pub fn append_sample_into(&self, canvas: &Canvas, zones: &mut Vec<ZoneColors>) {
        let start_index = zones.len();
        if let Err(error) = self.try_sample_append_into_at(canvas, zones, start_index) {
            tracing::warn!(%error, "Spatial sampling resources were unavailable");
        }
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
        self.try_sample_append_into_at(canvas, zones, start_index)
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "Spatial sampling resources were unavailable");
                start_index
            })
    }

    /// Sample a range with typed workspace failures.
    ///
    /// # Errors
    ///
    /// Returns [`SpatialSamplingError`] before changing `zones` when a
    /// summed-area workspace cannot be acquired.
    pub fn try_sample_append_into_at(
        &self,
        canvas: &Canvas,
        zones: &mut Vec<ZoneColors>,
        start_index: usize,
    ) -> Result<usize, SpatialSamplingError> {
        let area_workspace = self
            .area_workspaces
            .as_ref()
            .map(|pool| pool.try_checkout(canvas))
            .transpose()?;
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
            sampler::sample_prepared_zone_into(
                canvas,
                prepared_zone,
                &mut zone.colors,
                area_workspace.as_deref(),
            );
        }

        for prepared_zone in &self.prepared_zones[reusable_count..] {
            let mut colors = Vec::with_capacity(prepared_zone.prepared_samples.len());
            sampler::sample_prepared_zone_into(
                canvas,
                prepared_zone,
                &mut colors,
                area_workspace.as_deref(),
            );
            zones.push(ZoneColors {
                zone_id: prepared_zone.zone_id.clone(),
                colors,
            });
        }

        Ok(next_index)
    }

    /// Pre-admit a reusable workspace for a future canvas descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`SpatialSamplingError`] without discarding an existing
    /// workspace when the candidate descriptor cannot be allocated.
    pub fn try_prepare_sampling_canvas(
        &self,
        width: u32,
        height: u32,
    ) -> Result<(), SpatialSamplingError> {
        self.area_workspaces
            .as_ref()
            .map_or(Ok(()), |pool| pool.try_prepare(width, height))
    }

    /// Report the retained summed-area workspace count and byte usage.
    #[must_use]
    pub fn sampling_workspace_usage(&self) -> SpatialSamplingWorkspaceUsage {
        let (retained_workspaces, retained_bytes, reserved_workspaces, reserved_bytes) = self
            .area_workspaces
            .as_ref()
            .map_or((0, 0, 0, 0), |pool| pool.usage());
        SpatialSamplingWorkspaceUsage {
            retained_workspaces,
            retained_bytes,
            reserved_workspaces,
            reserved_bytes,
        }
    }

    /// Install a deterministic workspace-allocation hook for contract tests.
    #[cfg(feature = "spatial-workspace-test-hooks")]
    pub fn install_sampling_workspace_allocation_test_hook(
        &self,
        hook: Arc<SpatialWorkspaceAllocationTestHook>,
    ) -> bool {
        let Some(pool) = &self.area_workspaces else {
            return false;
        };
        pool.install_test_hook(hook);
        true
    }

    /// Replace the active layout and recompute all LED positions.
    ///
    /// Call this when the user edits the layout (moves/adds/removes zones,
    /// changes topology, etc.). The next [`sample`](Self::sample) call will
    /// use the new positions. Invalid layouts are rejected and the active
    /// layout remains unchanged.
    pub fn update_layout(&mut self, layout: SpatialLayout) {
        if let Err(error) = self.try_update_layout(layout) {
            tracing::warn!(%error, "Rejected spatial layout update");
        }
    }

    /// Validate and prepare a candidate layout before replacing active state.
    ///
    /// # Errors
    ///
    /// Returns [`SpatialPlanError`] when the candidate cannot be represented
    /// or its Gaussian kernel storage cannot be reserved. The previous layout
    /// and sampling plan remain active on failure.
    pub fn try_update_layout(&mut self, mut layout: SpatialLayout) -> Result<(), SpatialPlanError> {
        let plan_generation = self.plan_generation.saturating_add(1);
        let prepared = prepare_layout(&mut layout, plan_generation, self.sampling_capacity)?;
        self.layout = Arc::new(layout);
        self.prepared_zones = prepared.zones;
        self.area_workspaces = prepared.area_workspaces;
        self.plan_generation = plan_generation;
        Ok(())
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
}

struct PreparedLayout {
    zones: Arc<[PreparedZonePlan]>,
    area_workspaces: Option<Arc<sampler::AreaWorkspacePool>>,
}

fn prepare_layout(
    layout: &mut SpatialLayout,
    plan_generation: u64,
    sampling_capacity: SpatialSamplingCapacity,
) -> Result<PreparedLayout, SpatialPlanError> {
    validate_canvas_descriptor(layout.canvas_width, layout.canvas_height)?;

    for zone in &mut layout.zones {
        zone.led_positions = topology::generate_positions(&zone.topology);
    }
    let zones: Arc<[PreparedZonePlan]> = layout
        .zones
        .iter()
        .filter(|zone| is_led_sampled_zone(zone))
        .map(|zone| sampler::prepare_zone(zone, layout, plan_generation))
        .collect::<Result<Vec<_>, _>>()?
        .into();
    let area_workspaces = sampler::prepare_area_workspace_pool(
        &zones,
        layout.canvas_width,
        layout.canvas_height,
        sampling_capacity,
    )?;
    Ok(PreparedLayout {
        zones,
        area_workspaces,
    })
}

fn validate_canvas_descriptor(width: u32, height: u32) -> Result<usize, SpatialPlanError> {
    if width == 0 || height == 0 {
        return Err(SpatialPlanError::EmptyCanvas { width, height });
    }
    SurfaceDescriptor::rgba8888(width, height)
        .checked_byte_len()
        .ok_or(SpatialPlanError::CanvasByteLengthOverflow { width, height })
}
