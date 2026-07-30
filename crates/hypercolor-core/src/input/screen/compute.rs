use std::num::{NonZeroU64, NonZeroUsize};

use thiserror::Error;

use super::sampling::CpuSamplingTransform;
use super::{
    CaptureCadence, CaptureCadenceError, CaptureConfig, CaptureSourceId, CpuSamplingError,
    PixelExtent, ResolvedScreenSource, ScreenCapturePlan, ScreenPhysicalReductionDescriptor,
    ScreenPublicationExecutor, ScreenReductionFilter, ScreenSourceSelector, fit_within,
};
use crate::types::canvas::SurfaceResourceError;

const SOURCE_REDUCTION_WEIGHT: u64 = 5;
const SECTOR_FINALIZATION_WEIGHT: u64 = 32;
const LETTERBOX_SCAN_WEIGHT: u64 = 2;
const OUTPUT_RESAMPLE_WEIGHT: u64 = 3;
const OUTPUT_RGB_STAGING_WEIGHT: u64 = 2;
const TEMPORAL_SMOOTHING_WEIGHT: u64 = 6;
const COLOR_TUNING_WEIGHT: u64 = 4;
const SURFACE_WRITEBACK_WEIGHT: u64 = 2;
const POLICY_REDUCTION_WEIGHT: u64 = 5;
const ZONE_MATERIALIZATION_WEIGHT: u64 = 12;

const EXACT_NEAREST_TARGET_WEIGHT: u64 = 6;
const EXACT_BILINEAR_TARGET_WEIGHT: u64 = 18;
const EXACT_AREA_SOURCE_WEIGHT: u64 = 5;
const EXACT_AREA_TARGET_WEIGHT: u64 = 8;

/// Optional calibrated compute fences for live screen capture.
///
/// The default leaves work uncapped because no portable throughput constant
/// can describe every CPU. Callers with measured capacity can install exact
/// analysis and reduction fences without changing requested quality.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenComputeCapacityPolicy {
    analysis: Option<ScreenAnalysisComputeCapacity>,
    exact_weighted_work_units_per_worker_second: Option<NonZeroU64>,
}

impl ScreenComputeCapacityPolicy {
    /// Measure work for telemetry without rejecting it against an invented ceiling.
    pub const UNBOUNDED: Self = Self {
        analysis: None,
        exact_weighted_work_units_per_worker_second: None,
    };

    /// Install caller-calibrated compatibility and exact-reduction throughput.
    #[must_use]
    pub const fn calibrated(
        analysis: ScreenAnalysisComputeCapacity,
        exact_weighted_work_units_per_worker_second: NonZeroU64,
    ) -> Self {
        Self {
            analysis: Some(analysis),
            exact_weighted_work_units_per_worker_second: Some(
                exact_weighted_work_units_per_worker_second,
            ),
        }
    }

    /// Compatibility-analysis capacity, when the caller calibrated one.
    #[must_use]
    pub const fn analysis(self) -> Option<ScreenAnalysisComputeCapacity> {
        self.analysis
    }

    /// Exact-reduction capacity for the concrete worker pool, when calibrated.
    #[must_use]
    pub const fn exact(
        self,
        worker_count: NonZeroUsize,
    ) -> Option<CpuExactReductionComputeCapacity> {
        match self.exact_weighted_work_units_per_worker_second {
            Some(throughput) => Some(CpuExactReductionComputeCapacity::new(
                worker_count,
                throughput,
            )),
            None => None,
        }
    }
}

/// Caller-calibrated CPU capacity available to compatibility screen analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenAnalysisComputeCapacity {
    worker_count: NonZeroUsize,
    parallel_weighted_work_units_per_worker_second: NonZeroU64,
    serial_weighted_work_units_per_second: NonZeroU64,
}

impl ScreenAnalysisComputeCapacity {
    /// Describe an explicit analysis capacity.
    #[must_use]
    pub const fn new(
        worker_count: NonZeroUsize,
        weighted_work_units_per_worker_second: NonZeroU64,
    ) -> Self {
        Self::new_split(
            worker_count,
            weighted_work_units_per_worker_second,
            weighted_work_units_per_worker_second,
        )
    }

    /// Describe independently calibrated parallel and serial throughput.
    #[must_use]
    pub const fn new_split(
        worker_count: NonZeroUsize,
        parallel_weighted_work_units_per_worker_second: NonZeroU64,
        serial_weighted_work_units_per_second: NonZeroU64,
    ) -> Self {
        Self {
            worker_count,
            parallel_weighted_work_units_per_worker_second,
            serial_weighted_work_units_per_second,
        }
    }

    /// Number of workers available to screen analysis.
    #[must_use]
    pub const fn worker_count(self) -> NonZeroUsize {
        self.worker_count
    }

    /// Calibrated weighted work throughput of each worker.
    #[must_use]
    pub const fn weighted_work_units_per_worker_second(self) -> NonZeroU64 {
        self.parallel_weighted_work_units_per_worker_second
    }

    /// Calibrated throughput for passes that execute on the calling thread.
    #[must_use]
    pub const fn serial_weighted_work_units_per_second(self) -> NonZeroU64 {
        self.serial_weighted_work_units_per_second
    }

    /// Total calibrated throughput across the parallel worker pool.
    #[must_use]
    pub fn total_parallel_weighted_work_units_per_second(self) -> Option<u64> {
        u64::try_from(self.worker_count.get())
            .ok()?
            .checked_mul(self.parallel_weighted_work_units_per_worker_second.get())
    }

    fn parallel_weighted_work_units_per_second(self) -> Result<u64, ScreenAnalysisAdmissionError> {
        u64::try_from(self.worker_count.get())
            .ok()
            .and_then(|workers| {
                workers.checked_mul(self.parallel_weighted_work_units_per_worker_second.get())
            })
            .ok_or(ScreenAnalysisAdmissionError::WorkArithmeticOverflow {
                term: "parallel_compute_capacity",
            })
    }
}

/// CPU throughput available to exact physical reductions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuExactReductionComputeCapacity {
    worker_count: NonZeroUsize,
    weighted_work_units_per_worker_second: NonZeroU64,
}

impl CpuExactReductionComputeCapacity {
    /// Describe an explicit exact-reduction capacity.
    #[must_use]
    pub const fn new(
        worker_count: NonZeroUsize,
        weighted_work_units_per_worker_second: NonZeroU64,
    ) -> Self {
        Self {
            worker_count,
            weighted_work_units_per_worker_second,
        }
    }

    /// Number of workers available to exact CPU reductions.
    #[must_use]
    pub const fn worker_count(self) -> NonZeroUsize {
        self.worker_count
    }

    /// Calibrated weighted work throughput of each worker.
    #[must_use]
    pub const fn weighted_work_units_per_worker_second(self) -> NonZeroU64 {
        self.weighted_work_units_per_worker_second
    }

    fn total_weighted_work_units_per_second(self) -> Result<u64, CpuExactReductionAdmissionError> {
        u64::try_from(self.worker_count.get())
            .ok()
            .and_then(|workers| {
                workers.checked_mul(self.weighted_work_units_per_worker_second.get())
            })
            .ok_or(CpuExactReductionAdmissionError::WorkArithmeticOverflow {
                term: "configured_exact_compute_capacity",
            })
    }
}

/// Unweighted exact-reduction work at each descriptor's grouped cadence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuExactReductionWorkTerms {
    /// Nearest-filter destination pixels per second.
    pub nearest_target_pixel_hz: u64,
    /// Bilinear-filter destination pixels per second.
    pub bilinear_target_pixel_hz: u64,
    /// Area-filter covered source samples per second.
    pub area_source_sample_hz: u64,
    /// Area-filter destination pixels per second.
    pub area_target_pixel_hz: u64,
}

/// Checked exact CPU work summed across grouped physical reductions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpuExactReductionWorkPlan {
    source_id: CaptureSourceId,
    cpu_reduction_count: usize,
    terms: CpuExactReductionWorkTerms,
    weighted_work_units_per_second: u64,
}

impl CpuExactReductionWorkPlan {
    /// Build the CPU workload for one source from canonical physical demands.
    ///
    /// `executes_on_cpu` is evaluated only for descriptors whose canonical
    /// executor is CPU. Platform routes that pre-reduce on the GPU return
    /// `false`, keeping their work outside the CPU fence.
    ///
    /// # Errors
    ///
    /// Returns a typed error when source-region, target, cadence, or weighted
    /// summation arithmetic is not representable.
    pub fn try_for_source(
        plan: &ScreenCapturePlan,
        source_id: &CaptureSourceId,
        mut executes_on_cpu: impl FnMut(&ScreenPhysicalReductionDescriptor) -> bool,
    ) -> Result<Self, CpuExactReductionAdmissionError> {
        let mut terms = CpuExactReductionWorkTerms::default();
        let mut cpu_reduction_count = 0_usize;
        for reduction in plan.physical_reductions() {
            let descriptor = reduction.descriptor();
            if descriptor.source_epoch().source_id != *source_id
                || !matches!(descriptor.executor(), ScreenPublicationExecutor::Cpu)
                || !executes_on_cpu(descriptor)
            {
                continue;
            }
            cpu_reduction_count = cpu_reduction_count.checked_add(1).ok_or(
                CpuExactReductionAdmissionError::WorkArithmeticOverflow {
                    term: "cpu_reduction_count",
                },
            )?;
            let target_pixels =
                exact_pixel_count(descriptor.reduction_extent(), "exact_target_pixels")?;
            let cadence = u64::from(reduction.requested_hz().get());
            match descriptor.reduction_filter() {
                ScreenReductionFilter::Nearest => {
                    terms.nearest_target_pixel_hz = checked_add_product(
                        terms.nearest_target_pixel_hz,
                        target_pixels,
                        cadence,
                        "nearest_target_pixel_hz",
                    )?;
                }
                ScreenReductionFilter::Bilinear => {
                    terms.bilinear_target_pixel_hz = checked_add_product(
                        terms.bilinear_target_pixel_hz,
                        target_pixels,
                        cadence,
                        "bilinear_target_pixel_hz",
                    )?;
                }
                ScreenReductionFilter::Area => {
                    let source_visits = exact_area_sample_visits(descriptor)?;
                    terms.area_source_sample_hz = checked_add_product(
                        terms.area_source_sample_hz,
                        source_visits,
                        cadence,
                        "area_source_sample_hz",
                    )?;
                    terms.area_target_pixel_hz = checked_add_product(
                        terms.area_target_pixel_hz,
                        target_pixels,
                        cadence,
                        "area_target_pixel_hz",
                    )?;
                }
            }
        }
        let weighted_work_units_per_second = [
            weighted_exact(
                terms.nearest_target_pixel_hz,
                EXACT_NEAREST_TARGET_WEIGHT,
                "nearest_target_work",
            )?,
            weighted_exact(
                terms.bilinear_target_pixel_hz,
                EXACT_BILINEAR_TARGET_WEIGHT,
                "bilinear_target_work",
            )?,
            weighted_exact(
                terms.area_source_sample_hz,
                EXACT_AREA_SOURCE_WEIGHT,
                "area_source_work",
            )?,
            weighted_exact(
                terms.area_target_pixel_hz,
                EXACT_AREA_TARGET_WEIGHT,
                "area_target_work",
            )?,
        ]
        .into_iter()
        .try_fold(0_u64, u64::checked_add)
        .ok_or(CpuExactReductionAdmissionError::WorkArithmeticOverflow {
            term: "exact_weighted_work_total",
        })?;
        Ok(Self {
            source_id: source_id.clone(),
            cpu_reduction_count,
            terms,
            weighted_work_units_per_second,
        })
    }

    /// Reject unless all exact CPU reductions fit at their requested cadences.
    ///
    /// # Errors
    ///
    /// Returns a typed capacity rejection without changing any descriptor,
    /// output resolution, reduction filter, or cadence.
    pub fn admit(
        self,
        capacity: CpuExactReductionComputeCapacity,
    ) -> Result<Self, CpuExactReductionAdmissionError> {
        let available = capacity.total_weighted_work_units_per_second()?;
        if self.weighted_work_units_per_second > available {
            return Err(CpuExactReductionAdmissionError::ComputeCapacityExceeded {
                source_id: self.source_id,
                cpu_reduction_count: self.cpu_reduction_count,
                required_weighted_work_units_per_second: self.weighted_work_units_per_second,
                available_weighted_work_units_per_second: available,
            });
        }
        Ok(self)
    }

    /// Source covered by this exact-reduction plan.
    #[must_use]
    pub const fn source_id(&self) -> &CaptureSourceId {
        &self.source_id
    }

    /// Number of canonical physical reductions executing on CPU.
    #[must_use]
    pub const fn cpu_reduction_count(&self) -> usize {
        self.cpu_reduction_count
    }

    /// Unweighted filter-specific work at requested cadence.
    #[must_use]
    pub const fn terms(&self) -> CpuExactReductionWorkTerms {
        self.terms
    }

    /// Total calibrated work required per second.
    #[must_use]
    pub const fn weighted_work_units_per_second(&self) -> u64 {
        self.weighted_work_units_per_second
    }
}

/// Exact CPU physical reductions exceed representable or calibrated compute.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CpuExactReductionAdmissionError {
    /// An exact descriptor could not be mapped to its CPU storage grid.
    #[error(transparent)]
    Sampling(#[from] CpuSamplingError),
    /// Checked exact-work arithmetic exceeded the representable range.
    #[error("exact CPU reduction work arithmetic overflowed while calculating {term}")]
    WorkArithmeticOverflow {
        /// Work term that could not be represented.
        term: &'static str,
    },
    /// Requested exact CPU work exceeds the worker pool's throughput.
    #[error(
        "exact CPU reductions for {source_id} require {required_weighted_work_units_per_second} weighted work units/s across {cpu_reduction_count} reductions but only {available_weighted_work_units_per_second} are available"
    )]
    ComputeCapacityExceeded {
        /// Source whose exact reductions were rejected.
        source_id: CaptureSourceId,
        /// Canonical CPU physical reduction count.
        cpu_reduction_count: usize,
        /// Weighted throughput required by all grouped cadences.
        required_weighted_work_units_per_second: u64,
        /// Caller-calibrated throughput for the exact worker pool.
        available_weighted_work_units_per_second: u64,
    },
}

/// Weighted work performed by each compatibility-analysis frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenAnalysisFrameWork {
    /// Native source-pixel reduction.
    pub source_reduction: u64,
    /// Source-grid color finalization.
    pub source_sector_finalization: u64,
    /// Optional dynamic content-bar scan.
    pub letterbox_scan: u64,
    /// Native-to-publication resampling.
    pub output_resample: u64,
    /// RGBA-to-RGB staging for temporal analysis.
    pub output_rgb_staging: u64,
    /// Scene-cut comparison and temporal smoothing.
    pub temporal_smoothing: u64,
    /// Per-pixel color tuning.
    pub color_tuning: u64,
    /// Publication surface writeback.
    pub surface_writeback: u64,
    /// Publication-grid reduction.
    pub policy_reduction: u64,
    /// Publication-grid color finalization.
    pub policy_sector_finalization: u64,
    /// Zone identifiers and color materialization.
    pub zone_materialization: u64,
}

impl ScreenAnalysisFrameWork {
    fn parallel_total(self) -> Result<u64, ScreenAnalysisAdmissionError> {
        [
            self.source_reduction,
            self.source_sector_finalization,
            self.policy_reduction,
            self.policy_sector_finalization,
        ]
        .into_iter()
        .try_fold(0_u64, u64::checked_add)
        .ok_or(ScreenAnalysisAdmissionError::WorkArithmeticOverflow {
            term: "parallel_weighted_frame_total",
        })
    }

    fn serial_total(self) -> Result<u64, ScreenAnalysisAdmissionError> {
        [
            self.letterbox_scan,
            self.output_resample,
            self.output_rgb_staging,
            self.temporal_smoothing,
            self.color_tuning,
            self.surface_writeback,
            self.zone_materialization,
        ]
        .into_iter()
        .try_fold(0_u64, u64::checked_add)
        .ok_or(ScreenAnalysisAdmissionError::WorkArithmeticOverflow {
            term: "serial_weighted_frame_total",
        })
    }

    fn total(self) -> Result<u64, ScreenAnalysisAdmissionError> {
        self.parallel_total()?
            .checked_add(self.serial_total()?)
            .ok_or(ScreenAnalysisAdmissionError::WorkArithmeticOverflow {
                term: "weighted_frame_total",
            })
    }
}

/// Checked compatibility-analysis work for one source/output/grid/cadence tuple.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenAnalysisWorkPlan {
    input_extent: PixelExtent,
    output_extent: PixelExtent,
    grid_extent: PixelExtent,
    target_fps: u32,
    frame_work: ScreenAnalysisFrameWork,
    parallel_weighted_work_units_per_second: u64,
    serial_weighted_work_units_per_second: u64,
    weighted_work_units_per_frame: u64,
    weighted_work_units_per_second: u64,
}

impl ScreenAnalysisWorkPlan {
    /// Build a checked work model without admitting it against a machine.
    ///
    /// # Errors
    ///
    /// Returns a typed error when cadence or work arithmetic is not representable.
    pub fn try_new(
        input_extent: PixelExtent,
        requested_output_extent: PixelExtent,
        config: &CaptureConfig,
    ) -> Result<Self, ScreenAnalysisAdmissionError> {
        CaptureCadence::new(config.target_fps)?;
        let grid_extent = PixelExtent::new(config.grid_cols.max(1), config.grid_rows.max(1))
            .expect("normalized analysis grid is non-empty");
        let output_extent = if config.letterbox_enabled {
            requested_output_extent
        } else {
            let (output_width, output_height) = fit_within(
                input_extent.width(),
                input_extent.height(),
                requested_output_extent.width(),
                requested_output_extent.height(),
            );
            PixelExtent::new(output_width, output_height)
                .expect("fitted analysis output is non-empty")
        };
        let source_reduction_visits =
            proportional_reduction_visits(input_extent, grid_extent, "source_reduction_visits")?;
        let output_pixels = pixel_count(output_extent, "output_pixels")?;
        let policy_reduction_visits =
            proportional_reduction_visits(output_extent, grid_extent, "policy_reduction_visits")?;
        let grid_cells = pixel_count(grid_extent, "grid_cells")?;

        let frame_work = ScreenAnalysisFrameWork {
            source_reduction: weighted(
                source_reduction_visits,
                SOURCE_REDUCTION_WEIGHT,
                "source_reduction",
            )?,
            source_sector_finalization: weighted(
                grid_cells,
                SECTOR_FINALIZATION_WEIGHT,
                "source_sector_finalization",
            )?,
            letterbox_scan: if config.letterbox_enabled {
                weighted(
                    grid_cells.checked_mul(4).ok_or(
                        ScreenAnalysisAdmissionError::WorkArithmeticOverflow {
                            term: "letterbox_scan_cells",
                        },
                    )?,
                    LETTERBOX_SCAN_WEIGHT,
                    "letterbox_scan",
                )?
            } else {
                0
            },
            output_resample: weighted(output_pixels, OUTPUT_RESAMPLE_WEIGHT, "output_resample")?,
            output_rgb_staging: weighted(
                output_pixels,
                OUTPUT_RGB_STAGING_WEIGHT,
                "output_rgb_staging",
            )?,
            temporal_smoothing: weighted(
                output_pixels,
                TEMPORAL_SMOOTHING_WEIGHT,
                "temporal_smoothing",
            )?,
            color_tuning: weighted(output_pixels, COLOR_TUNING_WEIGHT, "color_tuning")?,
            surface_writeback: weighted(
                output_pixels,
                SURFACE_WRITEBACK_WEIGHT,
                "surface_writeback",
            )?,
            policy_reduction: weighted(
                policy_reduction_visits,
                POLICY_REDUCTION_WEIGHT,
                "policy_reduction",
            )?,
            policy_sector_finalization: weighted(
                grid_cells,
                SECTOR_FINALIZATION_WEIGHT,
                "policy_sector_finalization",
            )?,
            zone_materialization: weighted(
                grid_cells,
                ZONE_MATERIALIZATION_WEIGHT,
                "zone_materialization",
            )?,
        };
        let parallel_weighted_work_units_per_second = frame_work
            .parallel_total()?
            .checked_mul(u64::from(config.target_fps))
            .ok_or(ScreenAnalysisAdmissionError::WorkArithmeticOverflow {
                term: "parallel_weighted_work_units_per_second",
            })?;
        let serial_weighted_work_units_per_second = frame_work
            .serial_total()?
            .checked_mul(u64::from(config.target_fps))
            .ok_or(ScreenAnalysisAdmissionError::WorkArithmeticOverflow {
                term: "serial_weighted_work_units_per_second",
            })?;
        let weighted_work_units_per_frame = frame_work.total()?;
        let weighted_work_units_per_second = weighted_work_units_per_frame
            .checked_mul(u64::from(config.target_fps))
            .ok_or(ScreenAnalysisAdmissionError::WorkArithmeticOverflow {
                term: "weighted_work_units_per_second",
            })?;

        Ok(Self {
            input_extent,
            output_extent,
            grid_extent,
            target_fps: config.target_fps,
            frame_work,
            parallel_weighted_work_units_per_second,
            serial_weighted_work_units_per_second,
            weighted_work_units_per_frame,
            weighted_work_units_per_second,
        })
    }

    /// Reject this plan unless the complete requested workload fits capacity.
    ///
    /// # Errors
    ///
    /// Returns [`ScreenAnalysisAdmissionError::ComputeCapacityExceeded`] without
    /// changing requested resolution, grid shape, or frame cadence.
    pub fn admit(
        self,
        capacity: ScreenAnalysisComputeCapacity,
    ) -> Result<Self, ScreenAnalysisAdmissionError> {
        let available_parallel = capacity.parallel_weighted_work_units_per_second()?;
        for (lane, required, available) in [
            (
                ScreenAnalysisComputeLane::Parallel,
                self.parallel_weighted_work_units_per_second,
                available_parallel,
            ),
            (
                ScreenAnalysisComputeLane::Serial,
                self.serial_weighted_work_units_per_second,
                capacity.serial_weighted_work_units_per_second.get(),
            ),
        ] {
            if required <= available {
                continue;
            }
            return Err(ScreenAnalysisAdmissionError::ComputeCapacityExceeded {
                lane,
                input_width: self.input_extent.width(),
                input_height: self.input_extent.height(),
                output_width: self.output_extent.width(),
                output_height: self.output_extent.height(),
                grid_columns: self.grid_extent.width(),
                grid_rows: self.grid_extent.height(),
                target_fps: self.target_fps,
                required_weighted_work_units_per_second: required,
                available_weighted_work_units_per_second: available,
            });
        }
        Ok(self)
    }

    /// Frame-storage extent consumed by compatibility analysis.
    #[must_use]
    pub const fn input_extent(self) -> PixelExtent {
        self.input_extent
    }

    /// Maximum compatibility output extent covered by this plan.
    ///
    /// Dynamic content-bar cropping can change the effective aspect ratio, so
    /// letterbox-enabled plans cover the complete requested extent.
    #[must_use]
    pub const fn output_extent(self) -> PixelExtent {
        self.output_extent
    }

    /// Normalized analysis grid covered by this plan.
    #[must_use]
    pub const fn grid_extent(self) -> PixelExtent {
        self.grid_extent
    }

    /// Requested analysis cadence covered by this plan.
    #[must_use]
    pub const fn target_fps(self) -> u32 {
        self.target_fps
    }

    /// Named weighted terms for one frame.
    #[must_use]
    pub const fn frame_work(self) -> ScreenAnalysisFrameWork {
        self.frame_work
    }

    /// Total weighted work performed per frame.
    #[must_use]
    pub const fn weighted_work_units_per_frame(self) -> u64 {
        self.weighted_work_units_per_frame
    }

    /// Weighted work per second in Rayon-parallel sector passes.
    #[must_use]
    pub const fn parallel_weighted_work_units_per_second(self) -> u64 {
        self.parallel_weighted_work_units_per_second
    }

    /// Weighted work per second in passes that remain serial.
    #[must_use]
    pub const fn serial_weighted_work_units_per_second(self) -> u64 {
        self.serial_weighted_work_units_per_second
    }

    /// Total weighted work required at the requested cadence.
    #[must_use]
    pub const fn weighted_work_units_per_second(self) -> u64 {
        self.weighted_work_units_per_second
    }
}

/// Execution lane whose independently calibrated capacity is enforced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenAnalysisComputeLane {
    /// Sector reductions distributed across the Rayon worker pool.
    Parallel,
    /// Resampling, smoothing, tuning, writeback, and materialization passes.
    Serial,
}

/// Compatibility screen analysis could not be admitted without reducing quality.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ScreenAnalysisAdmissionError {
    /// Scheduler cadence cannot preserve the requested FPS.
    #[error(transparent)]
    CaptureCadence(#[from] CaptureCadenceError),
    /// Publication storage could not be prepared.
    #[error(transparent)]
    SurfaceResource(#[from] SurfaceResourceError),
    /// Checked work arithmetic exceeded the representable range.
    #[error("screen analysis work arithmetic overflowed while calculating {term}")]
    WorkArithmeticOverflow {
        /// Work term that could not be represented.
        term: &'static str,
    },
    /// Requested compatibility analysis exceeds caller-calibrated CPU throughput.
    #[error(
        "screen analysis input {input_width}x{input_height} -> {output_width}x{output_height}, grid {grid_columns}x{grid_rows} at {target_fps} FPS requires {required_weighted_work_units_per_second} {lane:?} weighted work units/s but only {available_weighted_work_units_per_second} are available"
    )]
    ComputeCapacityExceeded {
        /// Independently capacity-limited execution lane.
        lane: ScreenAnalysisComputeLane,
        /// Analyzer input width.
        input_width: u32,
        /// Analyzer input height.
        input_height: u32,
        /// Aspect-preserving output width.
        output_width: u32,
        /// Aspect-preserving output height.
        output_height: u32,
        /// Analysis grid columns.
        grid_columns: u32,
        /// Analysis grid rows.
        grid_rows: u32,
        /// Requested analysis cadence.
        target_fps: u32,
        /// Weighted throughput required by the complete pipeline.
        required_weighted_work_units_per_second: u64,
        /// Caller-calibrated throughput for this process.
        available_weighted_work_units_per_second: u64,
    },
}

fn pixel_count(
    extent: PixelExtent,
    term: &'static str,
) -> Result<u64, ScreenAnalysisAdmissionError> {
    u64::from(extent.width())
        .checked_mul(u64::from(extent.height()))
        .ok_or(ScreenAnalysisAdmissionError::WorkArithmeticOverflow { term })
}

fn proportional_reduction_visits(
    pixels: PixelExtent,
    grid: PixelExtent,
    term: &'static str,
) -> Result<u64, ScreenAnalysisAdmissionError> {
    u64::from(pixels.width().max(grid.width()))
        .checked_mul(u64::from(pixels.height().max(grid.height())))
        .ok_or(ScreenAnalysisAdmissionError::WorkArithmeticOverflow { term })
}

fn weighted(
    count: u64,
    weight: u64,
    term: &'static str,
) -> Result<u64, ScreenAnalysisAdmissionError> {
    count
        .checked_mul(weight)
        .ok_or(ScreenAnalysisAdmissionError::WorkArithmeticOverflow { term })
}

fn checked_add_product(
    current: u64,
    count: u64,
    cadence: u64,
    term: &'static str,
) -> Result<u64, CpuExactReductionAdmissionError> {
    count
        .checked_mul(cadence)
        .and_then(|work| current.checked_add(work))
        .ok_or(CpuExactReductionAdmissionError::WorkArithmeticOverflow { term })
}

fn exact_pixel_count(
    extent: PixelExtent,
    term: &'static str,
) -> Result<u64, CpuExactReductionAdmissionError> {
    u64::from(extent.width())
        .checked_mul(u64::from(extent.height()))
        .ok_or(CpuExactReductionAdmissionError::WorkArithmeticOverflow { term })
}

fn weighted_exact(
    count: u64,
    weight: u64,
    term: &'static str,
) -> Result<u64, CpuExactReductionAdmissionError> {
    count
        .checked_mul(weight)
        .ok_or(CpuExactReductionAdmissionError::WorkArithmeticOverflow { term })
}

fn exact_area_sample_visits(
    descriptor: &ScreenPhysicalReductionDescriptor,
) -> Result<u64, CpuExactReductionAdmissionError> {
    let source = ResolvedScreenSource::new(
        ScreenSourceSelector::Exact(descriptor.source_epoch().source_id.clone()),
        descriptor.source_epoch().clone(),
        descriptor.source().clone(),
    );
    CpuSamplingTransform::try_from_cpu_executor(&source)?
        .prepare_region(descriptor.source_region(), descriptor.reduction_extent())?
        .area_sample_visits()
        .map_err(Into::into)
}
