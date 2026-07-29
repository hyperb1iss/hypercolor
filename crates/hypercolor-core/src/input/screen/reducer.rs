//! Deterministic arbitrary-resolution CPU screen reduction.

use std::fmt;
use std::mem::size_of;
use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::Arc;

use rayon::prelude::*;
use rayon::{ThreadPool, ThreadPoolBuilder};
use thiserror::Error;

use hypercolor_types::canvas::{linear_to_srgb_u8, srgb_u8_to_linear};

use super::sampling::{
    CpuAxisInterpolation, CpuSamplingError, CpuSamplingRow, CpuSamplingTransform, CpuSamplingView,
    CpuStorageAxis, CpuStorageSpan, PreparedCpuSamplingPlan,
};

use super::{
    CaptureCursor, CaptureCursorContent, CaptureDynamicRange, CaptureFrame, CaptureFrameError,
    CapturePixelFormat, CaptureRotation, CaptureStorage, CaptureTransferFunction,
    CpuCaptureStorage, PixelExtent, PlatformGpuApi, PreparedScreenPublication, RawCaptureSurface,
    ResolvedScreenColorPipeline, ResolvedScreenColorTransform, ResolvedScreenPublicationDescriptor,
    ResolvedScreenSource, ScreenCapturePlan, ScreenColorTransformCapabilities, ScreenColorTuning,
    ScreenContentBarsPolicy, ScreenCursorPolicy, ScreenLetterboxFill,
    ScreenPhysicalReductionDescriptor, ScreenPlanGeneration, ScreenPublicationKind,
    ScreenReductionFilter, ScreenResourceApi, ScreenSmoothingPolicy, ScreenSourceReflection,
    ScreenSubpixelRect,
};

const CHANNELS_PER_PIXEL: u64 = 4;
const CPU_REDUCTION_ALGORITHM_REVISION: NonZeroU32 = NonZeroU32::MIN;

/// Platform work required before an exact request can enter the CPU reducer.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CpuFallbackNeed {
    /// The platform must expose CPU-addressable bytes for its opaque GPU source.
    #[error("platform GPU source {api:?} requires exact CPU readback")]
    PlatformReadback { api: PlatformGpuApi },
}

impl CpuFallbackNeed {
    /// GPU API whose exact source pixels require platform readback.
    #[must_use]
    pub const fn api(&self) -> &PlatformGpuApi {
        match self {
            Self::PlatformReadback { api } => api,
        }
    }
}

/// Exact resource geometry for one CPU reduction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuReductionLayout {
    source_extent: PixelExtent,
    target_extent: PixelExtent,
    source_packed_byte_len: u64,
    target_byte_len: u64,
    target_row_bytes: usize,
}

impl CpuReductionLayout {
    /// Validate source and target geometry without allocating pixel storage.
    ///
    /// # Errors
    ///
    /// Rejects packed byte counts that overflow `u64` or cannot be represented
    /// by the current process's `usize` address space.
    pub fn new(
        source_extent: PixelExtent,
        target_extent: PixelExtent,
    ) -> Result<Self, CpuReductionError> {
        let source_packed_byte_len = packed_byte_len(source_extent, "source")?;
        let target_byte_len = packed_byte_len(target_extent, "target")?;
        usize::try_from(source_packed_byte_len).map_err(|_| {
            CpuReductionError::ByteLengthNotAddressable {
                resource: "source",
                byte_len: source_packed_byte_len,
            }
        })?;
        let target_byte_len_usize = usize::try_from(target_byte_len).map_err(|_| {
            CpuReductionError::ByteLengthNotAddressable {
                resource: "target",
                byte_len: target_byte_len,
            }
        })?;
        let target_row_bytes_u64 = u64::from(target_extent.width())
            .checked_mul(CHANNELS_PER_PIXEL)
            .ok_or(CpuReductionError::GeometryOverflow { resource: "target" })?;
        let target_row_bytes = usize::try_from(target_row_bytes_u64).map_err(|_| {
            CpuReductionError::ByteLengthNotAddressable {
                resource: "target row",
                byte_len: target_row_bytes_u64,
            }
        })?;
        if target_row_bytes.checked_mul(usize::try_from(target_extent.height()).map_err(|_| {
            CpuReductionError::ByteLengthNotAddressable {
                resource: "target height",
                byte_len: u64::from(target_extent.height()),
            }
        })?) != Some(target_byte_len_usize)
        {
            return Err(CpuReductionError::GeometryOverflow { resource: "target" });
        }
        Ok(Self {
            source_extent,
            target_extent,
            source_packed_byte_len,
            target_byte_len,
            target_row_bytes,
        })
    }

    /// Source pixel extent.
    #[must_use]
    pub const fn source_extent(self) -> PixelExtent {
        self.source_extent
    }

    /// Target pixel extent.
    #[must_use]
    pub const fn target_extent(self) -> PixelExtent {
        self.target_extent
    }

    /// Packed source bytes used for resource admission.
    #[must_use]
    pub const fn source_packed_byte_len(self) -> u64 {
        self.source_packed_byte_len
    }

    /// Exact target bytes used for resource admission.
    #[must_use]
    pub const fn target_byte_len(self) -> u64 {
        self.target_byte_len
    }

    /// Exact target bytes as an addressable slice length.
    #[must_use]
    pub fn target_byte_len_usize(self) -> usize {
        usize::try_from(self.target_byte_len)
            .expect("validated target byte length remains addressable")
    }

    const fn target_row_bytes(self) -> usize {
        self.target_row_bytes
    }
}

#[derive(Clone, Copy, Debug)]
struct CpuReductionOutputLayout {
    extent: PixelExtent,
    byte_len: u64,
    row_bytes: usize,
}

impl CpuReductionOutputLayout {
    fn new(extent: PixelExtent) -> Result<Self, CpuReductionError> {
        let byte_len = packed_byte_len(extent, "target")?;
        let byte_len_usize =
            usize::try_from(byte_len).map_err(|_| CpuReductionError::ByteLengthNotAddressable {
                resource: "target",
                byte_len,
            })?;
        let row_bytes_u64 = u64::from(extent.width())
            .checked_mul(CHANNELS_PER_PIXEL)
            .ok_or(CpuReductionError::GeometryOverflow { resource: "target" })?;
        let row_bytes = usize::try_from(row_bytes_u64).map_err(|_| {
            CpuReductionError::ByteLengthNotAddressable {
                resource: "target row",
                byte_len: row_bytes_u64,
            }
        })?;
        if row_bytes.checked_mul(usize::try_from(extent.height()).map_err(|_| {
            CpuReductionError::ByteLengthNotAddressable {
                resource: "target height",
                byte_len: u64::from(extent.height()),
            }
        })?) != Some(byte_len_usize)
        {
            return Err(CpuReductionError::GeometryOverflow { resource: "target" });
        }
        Ok(Self {
            extent,
            byte_len,
            row_bytes,
        })
    }

    const fn extent(self) -> PixelExtent {
        self.extent
    }

    const fn byte_len(self) -> u64 {
        self.byte_len
    }

    fn byte_len_usize(self) -> usize {
        usize::try_from(self.byte_len).expect("validated target byte length remains addressable")
    }

    const fn row_bytes(self) -> usize {
        self.row_bytes
    }
}

/// Complete immutable request for one CPU reduction.
#[derive(Clone, Copy, Debug)]
pub struct CpuReductionRequest<'a> {
    source: &'a CpuCaptureStorage,
    layout: CpuReductionLayout,
    target_format: CapturePixelFormat,
    filter: ScreenReductionFilter,
    color_pipeline: ResolvedScreenColorPipeline,
}

#[derive(Clone, Debug)]
struct PreparedCpuReduction {
    descriptor: ScreenPhysicalReductionDescriptor,
    output: CpuReductionOutputLayout,
    sampling: PreparedCpuSamplingPlan,
    color: ReductionColor,
}

/// Descriptor-complete CPU work prepared once for one source and plan generation.
///
/// Each reduction retains constant-size exact sampling geometry. Execution
/// applies source regions and pending transforms directly over the shared
/// capture plane without materializing an intermediate raster.
#[derive(Clone, Debug)]
pub struct PreparedCpuReductionBatch {
    plan_generation: ScreenPlanGeneration,
    source: ResolvedScreenSource,
    sampling_transform: CpuSamplingTransform,
    reductions: Arc<[PreparedCpuReduction]>,
}

impl PreparedCpuReductionBatch {
    /// Plan generation whose canonical physical reductions produced this batch.
    #[must_use]
    pub const fn plan_generation(&self) -> ScreenPlanGeneration {
        self.plan_generation
    }

    /// Exact resolved source fenced by every prepared reduction.
    #[must_use]
    pub const fn source(&self) -> &ResolvedScreenSource {
        &self.source
    }

    /// Number of unique physical reductions prepared for this source.
    #[must_use]
    pub fn len(&self) -> usize {
        self.reductions.len()
    }

    /// Whether this source has no physical work in the selected plan.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.reductions.is_empty()
    }

    /// Canonical physical descriptor at one output index.
    #[must_use]
    pub fn descriptor(&self, index: usize) -> Option<&ScreenPhysicalReductionDescriptor> {
        self.reductions
            .get(index)
            .map(|reduction| &reduction.descriptor)
    }

    /// Exact caller-owned output bytes required at one output index.
    #[must_use]
    pub fn output_byte_len(&self, index: usize) -> Option<usize> {
        self.reductions
            .get(index)
            .map(|reduction| reduction.output.byte_len_usize())
    }

    /// Allocate exact retained planes only for keys with branch-local work.
    ///
    /// Identity-only Surface keys can reduce directly into publication slots,
    /// while Zones and policy-bearing Surface keys retain one shared physical
    /// plane for independent logical cadence.
    ///
    /// # Errors
    ///
    /// Rejects another plan generation or physical-key set and reports
    /// fallible plan-lifetime allocation failures before returning a workspace.
    pub fn prepare_materialization_workspace(
        &self,
        plan: &ScreenCapturePlan,
    ) -> Result<PreparedCpuMaterializationWorkspace, CpuReductionError> {
        if plan.generation() != self.plan_generation {
            return Err(CpuReductionError::WorkspacePlanGenerationMismatch);
        }
        let selected_count = self.reductions.iter().enumerate().try_fold(
            0_usize,
            |count, (batch_index, reduction)| {
                if physical_requires_materialization(plan, reduction, batch_index)? {
                    count
                        .checked_add(1)
                        .ok_or(CpuReductionError::GeometryOverflow {
                            resource: "materialization workspace",
                        })
                } else {
                    Ok(count)
                }
            },
        )?;
        let mut planes = Vec::new();
        planes
            .try_reserve_exact(selected_count)
            .map_err(|_| CpuReductionError::WorkspaceMetadataAllocationFailed)?;
        for (batch_index, reduction) in self.reductions.iter().enumerate() {
            if !physical_requires_materialization(plan, reduction, batch_index)? {
                continue;
            }
            let byte_len = reduction.output.byte_len_usize();
            let mut pixels = Vec::new();
            pixels.try_reserve_exact(byte_len).map_err(|_| {
                CpuReductionError::WorkspacePlaneAllocationFailed {
                    batch_index,
                    byte_len,
                }
            })?;
            pixels.resize(byte_len, 0);
            let mut scratch = Vec::new();
            scratch.try_reserve_exact(byte_len).map_err(|_| {
                CpuReductionError::WorkspacePlaneAllocationFailed {
                    batch_index,
                    byte_len,
                }
            })?;
            scratch.resize(byte_len, 0);
            planes.push(CpuMaterializationPlane {
                batch_index,
                pixels,
                scratch,
                completed_source_sequence: None,
            });
        }
        let allocation_byte_len = workspace_allocation_byte_len(&planes, planes.capacity())?;
        Ok(PreparedCpuMaterializationWorkspace {
            plan_generation: self.plan_generation,
            reductions: Arc::clone(&self.reductions),
            planes,
            allocation_byte_len,
        })
    }
}

/// Retained physical planes required by branch-local CPU materialization.
#[derive(Debug)]
pub struct PreparedCpuMaterializationWorkspace {
    plan_generation: ScreenPlanGeneration,
    reductions: Arc<[PreparedCpuReduction]>,
    planes: Vec<CpuMaterializationPlane>,
    allocation_byte_len: u64,
}

impl PreparedCpuMaterializationWorkspace {
    pub(super) fn belongs_to(&self, batch: &PreparedCpuReductionBatch) -> bool {
        Arc::ptr_eq(&self.reductions, &batch.reductions)
    }

    /// Plan generation whose physical keys own this workspace.
    #[must_use]
    pub const fn plan_generation(&self) -> ScreenPlanGeneration {
        self.plan_generation
    }

    /// Number of physical planes retained for branch-local work.
    #[must_use]
    pub fn len(&self) -> usize {
        self.planes.len()
    }

    /// Whether every physical key can publish directly without logical work.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.planes.is_empty()
    }

    /// Full plan-lifetime heap allocation owned by workspace metadata and pixels.
    #[must_use]
    pub const fn allocation_byte_len(&self) -> u64 {
        self.allocation_byte_len
    }

    /// Prepared batch index backing one retained plane.
    #[must_use]
    pub fn batch_index(&self, workspace_index: usize) -> Option<usize> {
        self.planes
            .get(workspace_index)
            .map(|plane| plane.batch_index)
    }

    /// Exact physical key backing one retained plane.
    #[must_use]
    pub fn physical_descriptor(
        &self,
        workspace_index: usize,
    ) -> Option<&ScreenPhysicalReductionDescriptor> {
        self.batch_index(workspace_index)
            .and_then(|batch_index| self.reductions.get(batch_index))
            .map(|reduction| &reduction.descriptor)
    }

    /// Last completed physical bytes for logical branch materialization.
    #[must_use]
    pub fn pixels(&self, workspace_index: usize) -> Option<&[u8]> {
        self.planes
            .get(workspace_index)
            .filter(|plane| plane.completed_source_sequence.is_some())
            .map(|plane| plane.pixels.as_slice())
    }

    /// Native source sequence that last completed one retained plane.
    #[must_use]
    pub fn completed_source_sequence(&self, workspace_index: usize) -> Option<u64> {
        self.planes
            .get(workspace_index)
            .and_then(|plane| plane.completed_source_sequence)
    }
}

#[derive(Debug)]
struct CpuMaterializationPlane {
    batch_index: usize,
    pixels: Vec<u8>,
    scratch: Vec<u8>,
    completed_source_sequence: Option<u64>,
}

/// One caller-owned destination paired by index with a prepared physical key.
#[derive(Debug)]
pub struct CpuReductionBatchJob<'descriptor, 'output> {
    descriptor: &'descriptor ScreenPhysicalReductionDescriptor,
    output: &'output mut [u8],
}

/// One due identity Surface destination bound to its prepared batch index.
#[derive(Debug)]
pub struct CpuSurfaceReductionJob<'publication> {
    batch_index: usize,
    publication: &'publication mut PreparedScreenPublication,
}

impl<'publication> CpuSurfaceReductionJob<'publication> {
    /// Bind a due writable Surface slot to one precomputed physical index.
    #[must_use]
    pub const fn new(
        batch_index: usize,
        publication: &'publication mut PreparedScreenPublication,
    ) -> Self {
        Self {
            batch_index,
            publication,
        }
    }

    /// Prepared physical index written by this due job.
    #[must_use]
    pub const fn batch_index(&self) -> usize {
        self.batch_index
    }

    /// Exact logical Surface reservation receiving physical bytes.
    pub const fn publication(&self) -> &PreparedScreenPublication {
        self.publication
    }
}

impl<'descriptor, 'output> CpuReductionBatchJob<'descriptor, 'output> {
    /// Bind one exact physical key to one admitted mutable output plane.
    #[must_use]
    pub const fn new(
        descriptor: &'descriptor ScreenPhysicalReductionDescriptor,
        output: &'output mut [u8],
    ) -> Self {
        Self { descriptor, output }
    }

    /// Exact physical key whose bytes this output plane must receive.
    #[must_use]
    pub const fn descriptor(&self) -> &ScreenPhysicalReductionDescriptor {
        self.descriptor
    }

    /// Exact output storage supplied by the caller.
    #[must_use]
    pub fn output(&self) -> &[u8] {
        self.output
    }
}

trait CpuReductionDestination {
    fn physical_descriptor(&self) -> &ScreenPhysicalReductionDescriptor;

    fn output_mut(&mut self, index: usize) -> Result<&mut [u8], CpuReductionError>;
}

impl CpuReductionDestination for CpuReductionBatchJob<'_, '_> {
    fn physical_descriptor(&self) -> &ScreenPhysicalReductionDescriptor {
        self.descriptor
    }

    fn output_mut(&mut self, _index: usize) -> Result<&mut [u8], CpuReductionError> {
        Ok(self.output)
    }
}

impl CpuReductionDestination for CpuSurfaceReductionJob<'_> {
    fn physical_descriptor(&self) -> &ScreenPhysicalReductionDescriptor {
        self.publication.descriptor().physical()
    }

    fn output_mut(&mut self, _index: usize) -> Result<&mut [u8], CpuReductionError> {
        self.publication.output_mut(self.batch_index)
    }
}

impl CpuReductionDestination for PreparedScreenPublication {
    fn physical_descriptor(&self) -> &ScreenPhysicalReductionDescriptor {
        self.descriptor().physical()
    }

    fn output_mut(&mut self, index: usize) -> Result<&mut [u8], CpuReductionError> {
        if !matches!(self.descriptor().kind(), ScreenPublicationKind::Surface) {
            return Err(CpuReductionError::BatchPublicationNotSurface { index });
        }
        if branch_requires_materialization(self.descriptor()) {
            return Err(CpuReductionError::BatchPublicationRequiresMaterialization { index });
        }
        self.surface_pixels_mut()
            .map_err(|_| CpuReductionError::BatchPublicationReservationUnavailable { index })
    }
}

pub(super) fn branch_requires_materialization(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> bool {
    if matches!(descriptor.kind(), ScreenPublicationKind::Zones { .. }) {
        return true;
    }
    let profile = descriptor.processing_profile();
    profile.content_bars() != ScreenContentBarsPolicy::Disabled
        || profile.letterbox_fill() != ScreenLetterboxFill::default()
        || profile.smoothing() != ScreenSmoothingPolicy::Disabled
        || profile.tuning() != ScreenColorTuning::default()
}

fn physical_requires_materialization(
    plan: &ScreenCapturePlan,
    reduction: &PreparedCpuReduction,
    batch_index: usize,
) -> Result<bool, CpuReductionError> {
    let plan_index = plan
        .physical_reductions()
        .binary_search_by(|demand| demand.descriptor().cmp(&reduction.descriptor))
        .map_err(|_| CpuReductionError::WorkspacePhysicalPlanMismatch { batch_index })?;
    Ok(plan.physical_reductions()[plan_index]
        .branch_indices()
        .iter()
        .any(|branch_index| {
            plan.branches()
                .get(*branch_index)
                .is_some_and(|branch| branch_requires_materialization(branch.descriptor()))
        }))
}

fn workspace_allocation_byte_len(
    planes: &[CpuMaterializationPlane],
    metadata_capacity: usize,
) -> Result<u64, CpuReductionError> {
    let metadata = u64::try_from(metadata_capacity)
        .ok()
        .and_then(|count| {
            u64::try_from(size_of::<CpuMaterializationPlane>())
                .ok()
                .and_then(|item_size| count.checked_mul(item_size))
        })
        .ok_or(CpuReductionError::GeometryOverflow {
            resource: "materialization workspace",
        })?;
    planes.iter().try_fold(metadata, |total, plane| {
        total
            .checked_add(u64::try_from(plane.pixels.capacity()).map_err(|_| {
                CpuReductionError::GeometryOverflow {
                    resource: "materialization workspace",
                }
            })?)
            .and_then(|total| {
                u64::try_from(plane.scratch.capacity())
                    .ok()
                    .and_then(|scratch| total.checked_add(scratch))
            })
            .ok_or(CpuReductionError::GeometryOverflow {
                resource: "materialization workspace",
            })
    })
}

fn validate_workspace_schedule(
    workspace: &PreparedCpuMaterializationWorkspace,
    workspace_indices: &[usize],
) -> Result<(), CpuReductionError> {
    let mut previous = None;
    for &workspace_index in workspace_indices {
        if previous.is_some_and(|previous| workspace_index <= previous) {
            return Err(CpuReductionError::WorkspaceScheduleNotCanonical);
        }
        if workspace_index >= workspace.planes.len() {
            return Err(CpuReductionError::WorkspaceScheduleIndexOutOfBounds {
                workspace_index,
                workspace_len: workspace.planes.len(),
            });
        }
        previous = Some(workspace_index);
    }
    Ok(())
}

fn validate_surface_schedule(
    batch: &PreparedCpuReductionBatch,
    jobs: &[CpuSurfaceReductionJob<'_>],
) -> Result<(), CpuReductionError> {
    let mut previous = None;
    for job in jobs {
        if previous.is_some_and(|previous| job.batch_index <= previous) {
            return Err(CpuReductionError::SurfaceScheduleNotCanonical);
        }
        let reduction = batch.reductions.get(job.batch_index).ok_or(
            CpuReductionError::SurfaceScheduleIndexOutOfBounds {
                batch_index: job.batch_index,
                batch_len: batch.reductions.len(),
            },
        )?;
        if job.physical_descriptor() != &reduction.descriptor {
            return Err(CpuReductionError::BatchDescriptorMismatch {
                index: job.batch_index,
            });
        }
        previous = Some(job.batch_index);
    }
    Ok(())
}

fn validate_schedule_disjoint(
    workspace: &PreparedCpuMaterializationWorkspace,
    workspace_indices: &[usize],
    surface_jobs: &[CpuSurfaceReductionJob<'_>],
) -> Result<(), CpuReductionError> {
    let mut workspace_cursor = 0;
    let mut surface_cursor = 0;
    while workspace_cursor < workspace_indices.len() && surface_cursor < surface_jobs.len() {
        let workspace_batch_index =
            workspace.planes[workspace_indices[workspace_cursor]].batch_index;
        let surface_batch_index = surface_jobs[surface_cursor].batch_index;
        match workspace_batch_index.cmp(&surface_batch_index) {
            std::cmp::Ordering::Less => workspace_cursor += 1,
            std::cmp::Ordering::Greater => surface_cursor += 1,
            std::cmp::Ordering::Equal => {
                return Err(CpuReductionError::ScheduledPhysicalKeyDuplicated {
                    batch_index: workspace_batch_index,
                });
            }
        }
    }
    Ok(())
}

fn prepared_cpu_sampling_view<'frame>(
    batch: &'frame PreparedCpuReductionBatch,
    frame: &'frame CaptureFrame<RawCaptureSurface>,
) -> Result<CpuSamplingView<'frame>, CpuReductionError> {
    frame.validate_epoch(batch.source.epoch())?;
    if let CaptureStorage::Gpu(surface) = frame.storage() {
        return Err(CpuFallbackNeed::PlatformReadback {
            api: surface.api().clone(),
        }
        .into());
    }
    CpuSamplingView::try_new_prepared(frame, &batch.source, batch.sampling_transform)
        .map_err(Into::into)
}

#[expect(
    clippy::too_many_arguments,
    reason = "preflight updates the exact shared receipt without allocating a temporary record"
)]
fn preflight_reduction(
    reduction: &PreparedCpuReduction,
    output: &[u8],
    frame: &CaptureFrame<RawCaptureSurface>,
    output_bytes: &mut u64,
    scheduled_tiles: &mut u64,
    worker_count: NonZeroUsize,
    tile_rows: NonZeroU32,
    batch_index: usize,
) -> Result<(), CpuReductionError> {
    let expected = reduction.output.byte_len_usize();
    if output.len() != expected {
        return Err(CpuReductionError::BatchOutputLengthMismatch {
            index: batch_index,
            expected,
            actual: output.len(),
        });
    }
    validate_cursor(reduction.descriptor.cursor(), &frame.metadata().cursor)?;
    *output_bytes = output_bytes
        .checked_add(reduction.output.byte_len())
        .ok_or(CpuReductionError::GeometryOverflow {
            resource: "scheduled output",
        })?;
    *scheduled_tiles = scheduled_tiles
        .checked_add(
            prepare_reduction_tiles(reduction.output, worker_count, tile_rows)?.scheduled_tiles,
        )
        .ok_or(CpuReductionError::GeometryOverflow {
            resource: "scheduled tile count",
        })?;
    Ok(())
}

/// Completion receipt for one allocation-free CPU batch execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuReductionBatchReport {
    source_sequence: u64,
    completed_jobs: usize,
    scheduled_tiles: u64,
    output_bytes: u64,
}

impl CpuReductionBatchReport {
    /// Native acquisition sequence consumed by every completed physical key.
    #[must_use]
    pub const fn source_sequence(self) -> u64 {
        self.source_sequence
    }

    /// Number of unique physical reductions completed.
    #[must_use]
    pub const fn completed_jobs(self) -> usize {
        self.completed_jobs
    }

    /// Deterministic output tiles exposed to the local Rayon pool.
    #[must_use]
    pub const fn scheduled_tiles(self) -> u64 {
        self.scheduled_tiles
    }

    /// Sum of caller-owned output bytes written by this batch.
    #[must_use]
    pub const fn output_bytes(self) -> u64 {
        self.output_bytes
    }
}

impl<'a> CpuReductionRequest<'a> {
    /// Bind validated geometry and color identity to one retained CPU plane.
    #[must_use]
    pub const fn new(
        source: &'a CpuCaptureStorage,
        layout: CpuReductionLayout,
        target_format: CapturePixelFormat,
        filter: ScreenReductionFilter,
        color_pipeline: ResolvedScreenColorPipeline,
    ) -> Self {
        Self {
            source,
            layout,
            target_format,
            filter,
            color_pipeline,
        }
    }

    /// Exact request geometry.
    #[must_use]
    pub const fn layout(self) -> CpuReductionLayout {
        self.layout
    }

    /// Requested output format.
    #[must_use]
    pub const fn target_format(self) -> CapturePixelFormat {
        self.target_format
    }

    /// Requested deterministic sampling filter.
    #[must_use]
    pub const fn filter(self) -> ScreenReductionFilter {
        self.filter
    }

    /// Exact resolved color pipeline.
    #[must_use]
    pub const fn color_pipeline(self) -> ResolvedScreenColorPipeline {
        self.color_pipeline
    }
}

#[derive(Debug)]
struct CpuReductionExecutorInner {
    pool: ThreadPool,
    worker_count: NonZeroUsize,
    tile_rows: NonZeroU32,
}

/// Cloneable handle to one core-owned local CPU reduction pool.
#[derive(Clone)]
pub struct CpuReductionExecutor {
    inner: Arc<CpuReductionExecutorInner>,
}

impl CpuReductionExecutor {
    /// Build one isolated pool with an explicit worker count and fixed tile size.
    ///
    /// # Errors
    ///
    /// Returns a typed pool construction failure when Rayon cannot create the
    /// requested workers.
    pub fn new(
        worker_count: NonZeroUsize,
        tile_rows: NonZeroU32,
    ) -> Result<Self, CpuReductionError> {
        let pool = ThreadPoolBuilder::new()
            .num_threads(worker_count.get())
            .thread_name(|index| format!("hypercolor-capture-{index}"))
            .build()
            .map_err(|error| CpuReductionError::ThreadPoolBuild(error.to_string()))?;
        Ok(Self {
            inner: Arc::new(CpuReductionExecutorInner {
                pool,
                worker_count,
                tile_rows,
            }),
        })
    }

    /// Number of workers owned by the local pool.
    #[must_use]
    pub fn worker_count(&self) -> NonZeroUsize {
        self.inner.worker_count
    }

    /// Maximum whole-row work used to size deterministic batch tiles.
    ///
    /// Short outputs split across their width to keep every worker useful.
    #[must_use]
    pub fn tile_rows(&self) -> NonZeroU32 {
        self.inner.tile_rows
    }

    /// Exact color operations and algorithm revision implemented by this executor.
    #[must_use]
    pub const fn capabilities(&self) -> ScreenColorTransformCapabilities {
        ScreenColorTransformCapabilities::new(true, false, false, CPU_REDUCTION_ALGORITHM_REVISION)
    }

    /// Prepare every canonical physical reduction owned by one resolved source.
    ///
    /// Preparation allocates only plan-lifetime descriptor metadata. Execution
    /// consumes caller-owned planes and performs no grouping, sorting, or output
    /// allocation.
    ///
    /// # Errors
    ///
    /// Rejects any descriptor the current CPU implementation cannot execute
    /// byte-exactly or whose geometry cannot be represented with checked exact
    /// arithmetic.
    pub fn prepare_batch(
        &self,
        source: &ResolvedScreenSource,
        plan: &ScreenCapturePlan,
    ) -> Result<PreparedCpuReductionBatch, CpuReductionError> {
        if let ScreenResourceApi::PlatformGpu(api) = source.config().resources().api() {
            return Err(CpuFallbackNeed::PlatformReadback { api: api.clone() }.into());
        }
        let sampling_transform = CpuSamplingTransform::try_from_source(source)?;
        let source_id = source.epoch().source_id.as_str();
        let physical_reductions = plan.physical_reductions();
        let first = physical_reductions.partition_point(|reduction| {
            reduction.descriptor().source_epoch().source_id.as_str() < source_id
        });
        let last = physical_reductions.partition_point(|reduction| {
            reduction.descriptor().source_epoch().source_id.as_str() <= source_id
        });
        let mut reductions = Vec::new();
        reductions
            .try_reserve(last - first)
            .map_err(|_| CpuReductionError::BatchPreparationAllocationFailed)?;
        for reduction in &physical_reductions[first..last] {
            let descriptor = reduction.descriptor();
            if descriptor.source_epoch() != source.epoch() || descriptor.source() != source.config()
            {
                return Err(CpuReductionError::SourceConfigMismatch);
            }
            reductions.push(prepare_physical_reduction(descriptor, sampling_transform)?);
        }
        Ok(PreparedCpuReductionBatch {
            plan_generation: plan.generation(),
            source: source.clone(),
            sampling_transform,
            reductions: Arc::from(reductions),
        })
    }

    /// Execute all prepared physical keys once from one native CPU frame.
    ///
    /// The complete batch is preflighted before any destination is touched.
    /// Rayon composes job-level and row-tile parallelism on this executor's one
    /// fixed worker pool; no branch creates an operating-system thread.
    ///
    /// # Errors
    ///
    /// Rejects stale or mismatched frames, incompatible cursor storage, and any
    /// output count or length mismatch before writing a destination.
    pub fn execute_batch(
        &self,
        batch: &PreparedCpuReductionBatch,
        frame: &CaptureFrame<RawCaptureSurface>,
        jobs: &mut [CpuReductionBatchJob<'_, '_>],
    ) -> Result<CpuReductionBatchReport, CpuReductionError> {
        self.execute_destinations(batch, frame, jobs)
    }

    /// Execute every physical key directly into one exact writable surface slot.
    ///
    /// Reservations must follow the prepared batch's canonical physical-key
    /// order. A successful report leaves every reservation ready for
    /// `ScreenPublicationHub::finalize_writable_publication`. Every validation
    /// error returns before reducer writes begin. A reduction error leaves every
    /// slot unfinalized, so dropping the reservations preserves each branch's
    /// last-good publication.
    ///
    /// # Errors
    ///
    /// Rejects non-surface or branch-materialized reservations, reordered or
    /// substituted descriptors, stale frames, incompatible cursor storage, and
    /// count or length mismatches before writing any slot.
    pub fn execute_surface_publications(
        &self,
        batch: &PreparedCpuReductionBatch,
        frame: &CaptureFrame<RawCaptureSurface>,
        publications: &mut [PreparedScreenPublication],
    ) -> Result<CpuReductionBatchReport, CpuReductionError> {
        self.execute_destinations(batch, frame, publications)
    }

    /// Execute one sparse due set across retained planes and direct Surfaces.
    ///
    /// Both schedules are canonical, strictly increasing index lists prepared
    /// by the caller's plan-lifetime fanout. A physical key may appear in one
    /// schedule only: shared Surface/Zones work chooses the Surface slot when
    /// it is due, otherwise the retained plane. The union runs exactly once in
    /// the fixed Rayon pool without allocating, grouping, or sorting per frame.
    ///
    /// # Errors
    ///
    /// Rejects another workspace identity, malformed or overlapping schedules,
    /// stale workspace sequences, incompatible publications or cursor storage,
    /// and mismatched frames before any destination write begins.
    pub fn execute_scheduled_publications(
        &self,
        batch: &PreparedCpuReductionBatch,
        frame: &CaptureFrame<RawCaptureSurface>,
        workspace: &mut PreparedCpuMaterializationWorkspace,
        workspace_indices: &[usize],
        surface_jobs: &mut [CpuSurfaceReductionJob<'_>],
    ) -> Result<CpuReductionBatchReport, CpuReductionError> {
        if !Arc::ptr_eq(&batch.reductions, &workspace.reductions) {
            return Err(CpuReductionError::WorkspaceBatchMismatch);
        }
        validate_workspace_schedule(workspace, workspace_indices)?;
        validate_surface_schedule(batch, surface_jobs)?;
        validate_schedule_disjoint(workspace, workspace_indices, surface_jobs)?;
        let view = prepared_cpu_sampling_view(batch, frame)?;
        let source_sequence = frame.metadata().sequence;
        let mut output_bytes = 0_u64;
        let mut scheduled_tiles = 0_u64;
        for &workspace_index in workspace_indices {
            let plane = &workspace.planes[workspace_index];
            if plane
                .completed_source_sequence
                .is_some_and(|previous| source_sequence <= previous)
            {
                return Err(CpuReductionError::WorkspaceFrameSequenceNotIncreasing {
                    workspace_index,
                    previous: plane
                        .completed_source_sequence
                        .expect("non-increasing workspace sequence exists"),
                    actual: source_sequence,
                });
            }
            let reduction = &batch.reductions[plane.batch_index];
            preflight_reduction(
                reduction,
                &plane.scratch,
                frame,
                &mut output_bytes,
                &mut scheduled_tiles,
                self.inner.worker_count,
                self.inner.tile_rows,
                plane.batch_index,
            )?;
        }
        for job in surface_jobs.iter_mut() {
            let reduction = &batch.reductions[job.batch_index];
            let batch_index = job.batch_index;
            let output = job.output_mut(batch_index)?;
            preflight_reduction(
                reduction,
                output,
                frame,
                &mut output_bytes,
                &mut scheduled_tiles,
                self.inner.worker_count,
                self.inner.tile_rows,
                batch_index,
            )?;
        }
        let (workspace_result, surface_result) = self.inner.pool.install(|| {
            rayon::join(
                || {
                    workspace
                        .planes
                        .par_iter_mut()
                        .enumerate()
                        .filter(|(workspace_index, _)| {
                            workspace_indices.binary_search(workspace_index).is_ok()
                        })
                        .try_for_each(|(_, plane)| {
                            let reduction = &batch.reductions[plane.batch_index];
                            reduce_prepared_in_pool(
                                &view,
                                reduction,
                                self.inner.worker_count,
                                self.inner.tile_rows,
                                &mut plane.scratch,
                            )
                        })
                },
                || {
                    surface_jobs.par_iter_mut().try_for_each(|job| {
                        let batch_index = job.batch_index;
                        let reduction = &batch.reductions[batch_index];
                        reduce_prepared_in_pool(
                            &view,
                            reduction,
                            self.inner.worker_count,
                            self.inner.tile_rows,
                            job.output_mut(batch_index)?,
                        )
                    })
                },
            )
        });
        workspace_result?;
        surface_result?;
        for &workspace_index in workspace_indices {
            let plane = &mut workspace.planes[workspace_index];
            std::mem::swap(&mut plane.pixels, &mut plane.scratch);
            plane.completed_source_sequence = Some(source_sequence);
        }
        Ok(CpuReductionBatchReport {
            source_sequence,
            completed_jobs: workspace_indices.len() + surface_jobs.len(),
            scheduled_tiles,
            output_bytes,
        })
    }

    /// Execute only physical keys retained for branch-local materialization.
    ///
    /// The workspace is prepared once from this exact batch and owns no plane
    /// for identity-only Surface keys. Every selected key is preflighted before
    /// writes begin, then reduced in parallel through the executor's fixed pool.
    ///
    /// # Errors
    ///
    /// Rejects a workspace prepared from another batch, stale or mismatched
    /// frames, incompatible cursor storage, and corrupted plane lengths before
    /// reducer writes begin.
    pub fn execute_materialization_workspace(
        &self,
        batch: &PreparedCpuReductionBatch,
        frame: &CaptureFrame<RawCaptureSurface>,
        workspace: &mut PreparedCpuMaterializationWorkspace,
    ) -> Result<CpuReductionBatchReport, CpuReductionError> {
        if !Arc::ptr_eq(&batch.reductions, &workspace.reductions) {
            return Err(CpuReductionError::WorkspaceBatchMismatch);
        }
        let view = prepared_cpu_sampling_view(batch, frame)?;
        let source_sequence = frame.metadata().sequence;
        let mut output_bytes = 0_u64;
        let mut scheduled_tiles = 0_u64;
        for (workspace_index, plane) in workspace.planes.iter().enumerate() {
            if plane
                .completed_source_sequence
                .is_some_and(|previous| source_sequence <= previous)
            {
                return Err(CpuReductionError::WorkspaceFrameSequenceNotIncreasing {
                    workspace_index,
                    previous: plane
                        .completed_source_sequence
                        .expect("non-increasing workspace sequence exists"),
                    actual: source_sequence,
                });
            }
            let reduction = &batch.reductions[plane.batch_index];
            let expected = reduction.output.byte_len_usize();
            if plane.pixels.len() != expected {
                return Err(CpuReductionError::WorkspacePlaneLengthMismatch {
                    batch_index: plane.batch_index,
                    expected,
                    actual: plane.pixels.len(),
                });
            }
            if plane.scratch.len() != expected {
                return Err(CpuReductionError::WorkspacePlaneLengthMismatch {
                    batch_index: plane.batch_index,
                    expected,
                    actual: plane.scratch.len(),
                });
            }
            validate_cursor(reduction.descriptor.cursor(), &frame.metadata().cursor)?;
            output_bytes = output_bytes
                .checked_add(reduction.output.byte_len())
                .ok_or(CpuReductionError::GeometryOverflow {
                    resource: "materialization workspace output",
                })?;
            scheduled_tiles = scheduled_tiles
                .checked_add(
                    prepare_reduction_tiles(
                        reduction.output,
                        self.inner.worker_count,
                        self.inner.tile_rows,
                    )?
                    .scheduled_tiles,
                )
                .ok_or(CpuReductionError::GeometryOverflow {
                    resource: "materialization workspace tile count",
                })?;
        }
        let execution = self.inner.pool.install(|| {
            workspace.planes.par_iter_mut().try_for_each(|plane| {
                let reduction = &batch.reductions[plane.batch_index];
                reduce_prepared_in_pool(
                    &view,
                    reduction,
                    self.inner.worker_count,
                    self.inner.tile_rows,
                    &mut plane.scratch,
                )
            })
        });
        execution?;
        for plane in &mut workspace.planes {
            std::mem::swap(&mut plane.pixels, &mut plane.scratch);
            plane.completed_source_sequence = Some(source_sequence);
        }
        Ok(CpuReductionBatchReport {
            source_sequence,
            completed_jobs: workspace.planes.len(),
            scheduled_tiles,
            output_bytes,
        })
    }

    fn execute_destinations<D>(
        &self,
        batch: &PreparedCpuReductionBatch,
        frame: &CaptureFrame<RawCaptureSurface>,
        destinations: &mut [D],
    ) -> Result<CpuReductionBatchReport, CpuReductionError>
    where
        D: CpuReductionDestination + Send,
    {
        if destinations.len() != batch.reductions.len() {
            return Err(CpuReductionError::BatchOutputCountMismatch {
                expected: batch.reductions.len(),
                actual: destinations.len(),
            });
        }
        let view = prepared_cpu_sampling_view(batch, frame)?;
        let mut output_bytes = 0_u64;
        let mut scheduled_tiles = 0_u64;
        for (index, (reduction, destination)) in batch
            .reductions
            .iter()
            .zip(destinations.iter_mut())
            .enumerate()
        {
            if destination.physical_descriptor() != &reduction.descriptor {
                return Err(CpuReductionError::BatchDescriptorMismatch { index });
            }
            let expected = reduction.output.byte_len_usize();
            let actual = destination.output_mut(index)?.len();
            if actual != expected {
                return Err(CpuReductionError::BatchOutputLengthMismatch {
                    index,
                    expected,
                    actual,
                });
            }
            validate_cursor(reduction.descriptor.cursor(), &frame.metadata().cursor)?;
            output_bytes = output_bytes
                .checked_add(reduction.output.byte_len())
                .ok_or(CpuReductionError::GeometryOverflow {
                    resource: "batch output",
                })?;
            scheduled_tiles = scheduled_tiles
                .checked_add(
                    prepare_reduction_tiles(
                        reduction.output,
                        self.inner.worker_count,
                        self.inner.tile_rows,
                    )?
                    .scheduled_tiles,
                )
                .ok_or(CpuReductionError::GeometryOverflow {
                    resource: "batch tile count",
                })?;
        }
        self.inner.pool.install(|| {
            destinations
                .par_iter_mut()
                .zip(batch.reductions.par_iter())
                .enumerate()
                .try_for_each(|(index, (destination, reduction))| {
                    reduce_prepared_in_pool(
                        &view,
                        reduction,
                        self.inner.worker_count,
                        self.inner.tile_rows,
                        destination.output_mut(index)?,
                    )
                })
        })?;
        Ok(CpuReductionBatchReport {
            source_sequence: frame.metadata().sequence,
            completed_jobs: destinations.len(),
            scheduled_tiles,
            output_bytes,
        })
    }

    /// Reduce directly into caller-owned publication storage.
    ///
    /// # Errors
    ///
    /// Rejects malformed source storage, an incorrectly sized destination, or
    /// a color transform this CPU path does not implement.
    pub fn reduce(
        &self,
        request: CpuReductionRequest<'_>,
        output: &mut [u8],
    ) -> Result<(), CpuReductionError> {
        let expected = request.layout.target_byte_len_usize();
        if output.len() != expected {
            return Err(CpuReductionError::OutputLengthMismatch {
                expected,
                actual: output.len(),
            });
        }
        validate_source(request.source, request.layout)?;
        let color = ReductionColor::resolve(request)?;
        self.inner
            .pool
            .install(|| reduce_request_in_pool(request, color, self.inner.tile_rows, output))
    }
}

impl fmt::Debug for CpuReductionExecutor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CpuReductionExecutor")
            .field("worker_count", &self.worker_count())
            .field("tile_rows", &self.tile_rows())
            .finish_non_exhaustive()
    }
}

fn prepare_physical_reduction(
    descriptor: &ScreenPhysicalReductionDescriptor,
    sampling_transform: CpuSamplingTransform,
) -> Result<PreparedCpuReduction, CpuReductionError> {
    if descriptor.algorithm_revision() != CPU_REDUCTION_ALGORITHM_REVISION {
        return Err(CpuReductionError::AlgorithmRevisionMismatch {
            expected: CPU_REDUCTION_ALGORITHM_REVISION,
            actual: descriptor.algorithm_revision(),
        });
    }
    let cursor_capabilities = descriptor.source().cursor_capabilities();
    match descriptor.cursor() {
        ScreenCursorPolicy::Exclude if !cursor_capabilities.has_clean_surface() => {
            return Err(CpuReductionError::CursorExclusionUnavailable);
        }
        ScreenCursorPolicy::Include if !cursor_capabilities.has_composed_surface() => {
            return Err(CpuReductionError::CursorCompositionRequired);
        }
        ScreenCursorPolicy::Exclude | ScreenCursorPolicy::Include => {}
    }
    let output = CpuReductionOutputLayout::new(descriptor.reduction_extent())?;
    let sampling = sampling_transform
        .prepare_region(descriptor.source_region(), descriptor.reduction_extent())?;
    let color = ReductionColor::resolve_prepared(descriptor, output)?;
    Ok(PreparedCpuReduction {
        descriptor: descriptor.clone(),
        output,
        sampling,
        color,
    })
}

fn is_full_source_region(region: ScreenSubpixelRect, extent: PixelExtent) -> bool {
    rational_is_integer(region.x(), 0)
        && rational_is_integer(region.y(), 0)
        && rational_is_integer(region.width(), extent.width())
        && rational_is_integer(region.height(), extent.height())
}

fn rational_is_integer(value: super::ScreenRational, expected: u32) -> bool {
    u128::from(value.numerator()) == u128::from(expected) * u128::from(value.denominator().get())
}

fn validate_cursor(
    policy: ScreenCursorPolicy,
    cursor: &CaptureCursor,
) -> Result<(), CpuReductionError> {
    if !cursor.visible {
        return Ok(());
    }
    match (policy, &cursor.content) {
        (ScreenCursorPolicy::Exclude, CaptureCursorContent::Composed) => {
            Err(CpuReductionError::CursorExclusionUnavailable)
        }
        (ScreenCursorPolicy::Include, CaptureCursorContent::Composed) => Ok(()),
        (ScreenCursorPolicy::Include, _) => Err(CpuReductionError::CursorCompositionRequired),
        (ScreenCursorPolicy::Exclude, _) => Ok(()),
    }
}

fn reduce_request_in_pool(
    request: CpuReductionRequest<'_>,
    color: ReductionColor,
    tile_rows: NonZeroU32,
    output: &mut [u8],
) -> Result<(), CpuReductionError> {
    let configured_rows_per_tile = usize::try_from(tile_rows.get()).map_err(|_| {
        CpuReductionError::ByteLengthNotAddressable {
            resource: "tile rows",
            byte_len: u64::from(tile_rows.get()),
        }
    })?;
    let target_rows = usize::try_from(request.layout.target_extent().height()).map_err(|_| {
        CpuReductionError::ByteLengthNotAddressable {
            resource: "target height",
            byte_len: u64::from(request.layout.target_extent().height()),
        }
    })?;
    let rows_per_tile = configured_rows_per_tile.min(target_rows);
    let tile_bytes = request
        .layout
        .target_row_bytes()
        .checked_mul(rows_per_tile)
        .ok_or(CpuReductionError::GeometryOverflow { resource: "tile" })?;
    output
        .par_chunks_mut(tile_bytes)
        .enumerate()
        .try_for_each(|(tile_index, tile)| {
            reduce_tile(request, color, tile_index, rows_per_tile, tile)
        })
}

fn reduce_prepared_in_pool(
    view: &CpuSamplingView<'_>,
    reduction: &PreparedCpuReduction,
    worker_count: NonZeroUsize,
    tile_rows: NonZeroU32,
    output: &mut [u8],
) -> Result<(), CpuReductionError> {
    let tile_plan = prepare_reduction_tiles(reduction.output, worker_count, tile_rows)?;
    output
        .par_chunks_mut(tile_plan.bytes_per_tile)
        .enumerate()
        .try_for_each(|(tile_index, tile)| {
            reduce_prepared_tile(view, reduction, tile_index, tile_plan.pixels_per_tile, tile)
        })
}

#[derive(Clone, Copy, Debug)]
struct CpuReductionTilePlan {
    pixels_per_tile: usize,
    bytes_per_tile: usize,
    scheduled_tiles: u64,
}

fn prepare_reduction_tiles(
    output: CpuReductionOutputLayout,
    worker_count: NonZeroUsize,
    tile_rows: NonZeroU32,
) -> Result<CpuReductionTilePlan, CpuReductionError> {
    let configured_rows_per_tile = usize::try_from(tile_rows.get()).map_err(|_| {
        CpuReductionError::ByteLengthNotAddressable {
            resource: "tile rows",
            byte_len: u64::from(tile_rows.get()),
        }
    })?;
    let target_rows = usize::try_from(output.extent().height()).map_err(|_| {
        CpuReductionError::ByteLengthNotAddressable {
            resource: "target height",
            byte_len: u64::from(output.extent().height()),
        }
    })?;
    let rows_per_tile = configured_rows_per_tile.min(target_rows);
    let configured_tile_pixels = output
        .row_bytes()
        .checked_div(4)
        .and_then(|width| width.checked_mul(rows_per_tile))
        .ok_or(CpuReductionError::GeometryOverflow { resource: "tile" })?;
    let target_pixels = output.byte_len_usize() / 4;
    let target_tile_count =
        worker_count
            .get()
            .checked_mul(4)
            .ok_or(CpuReductionError::GeometryOverflow {
                resource: "worker tile count",
            })?;
    let balanced_tile_pixels = target_pixels.div_ceil(target_tile_count);
    let pixels_per_tile = configured_tile_pixels.min(balanced_tile_pixels).max(1);
    let tile_bytes = pixels_per_tile
        .checked_mul(4)
        .ok_or(CpuReductionError::GeometryOverflow { resource: "tile" })?;
    let scheduled_tiles =
        u64::try_from(output.byte_len_usize().div_ceil(tile_bytes)).map_err(|_| {
            CpuReductionError::ByteLengthNotAddressable {
                resource: "tile count",
                byte_len: u64::MAX,
            }
        })?;
    Ok(CpuReductionTilePlan {
        pixels_per_tile,
        bytes_per_tile: tile_bytes,
        scheduled_tiles,
    })
}

fn reduce_prepared_tile(
    view: &CpuSamplingView<'_>,
    reduction: &PreparedCpuReduction,
    tile_index: usize,
    tile_pixels: usize,
    mut tile: &mut [u8],
) -> Result<(), CpuReductionError> {
    let target_width = usize::try_from(reduction.output.extent().width()).map_err(|_| {
        CpuReductionError::ByteLengthNotAddressable {
            resource: "target width",
            byte_len: u64::from(reduction.output.extent().width()),
        }
    })?;
    let mut first_pixel = tile_index
        .checked_mul(tile_pixels)
        .ok_or(CpuReductionError::GeometryOverflow { resource: "tile" })?;
    while !tile.is_empty() {
        let first_target_x = first_pixel % target_width;
        let target_y = u32::try_from(first_pixel / target_width)
            .map_err(|_| CpuReductionError::GeometryOverflow { resource: "target" })?;
        let run_pixels = (target_width - first_target_x).min(tile.len() / 4);
        let run_bytes = run_pixels
            .checked_mul(4)
            .ok_or(CpuReductionError::GeometryOverflow { resource: "tile" })?;
        let (row, remainder) = tile.split_at_mut(run_bytes);
        reduce_prepared_row(view, reduction, target_y, first_target_x, row)?;
        first_pixel = first_pixel
            .checked_add(run_pixels)
            .ok_or(CpuReductionError::GeometryOverflow { resource: "tile" })?;
        tile = remainder;
    }
    Ok(())
}

fn reduce_prepared_row(
    view: &CpuSamplingView<'_>,
    reduction: &PreparedCpuReduction,
    target_y: u32,
    first_target_x: usize,
    row: &mut [u8],
) -> Result<(), CpuReductionError> {
    match reduction.descriptor.reduction_filter() {
        ScreenReductionFilter::Nearest => {
            reduce_prepared_nearest_row(view, reduction, target_y, first_target_x, row)
        }
        ScreenReductionFilter::Bilinear => {
            reduce_prepared_bilinear_row(view, reduction, target_y, first_target_x, row)
        }
        ScreenReductionFilter::Area => {
            reduce_prepared_area_row(view, reduction, target_y, first_target_x, row)
        }
    }
}

fn reduce_prepared_nearest_row(
    view: &CpuSamplingView<'_>,
    reduction: &PreparedCpuReduction,
    target_y: u32,
    first_target_x: usize,
    row: &mut [u8],
) -> Result<(), CpuReductionError> {
    let fixed = reduction.sampling.logical_y_nearest(target_y);
    match reduction.sampling.logical_x_storage_axis() {
        CpuStorageAxis::X => {
            let source_row = view.storage_row(fixed)?;
            write_prepared_row(reduction, first_target_x, row, |target_x| {
                Ok(source_row.read_rgba(reduction.sampling.logical_x_nearest(target_x))?)
            })
        }
        CpuStorageAxis::Y => write_prepared_row(reduction, first_target_x, row, |target_x| {
            let source_row = view.storage_row(reduction.sampling.logical_x_nearest(target_x))?;
            Ok(source_row.read_rgba(fixed)?)
        }),
    }
}

fn reduce_prepared_bilinear_row(
    view: &CpuSamplingView<'_>,
    reduction: &PreparedCpuReduction,
    target_y: u32,
    first_target_x: usize,
    row: &mut [u8],
) -> Result<(), CpuReductionError> {
    let fixed = reduction.sampling.logical_y_bilinear(target_y);
    match reduction.sampling.logical_x_storage_axis() {
        CpuStorageAxis::X => {
            let top = view.storage_row(fixed.lower())?;
            let bottom = view.storage_row(fixed.upper())?;
            write_prepared_row(reduction, first_target_x, row, |target_x| {
                let x = reduction.sampling.logical_x_bilinear(target_x);
                sample_prepared_bilinear(top, bottom, x, fixed, reduction.color)
            })
        }
        CpuStorageAxis::Y => write_prepared_row(reduction, first_target_x, row, |target_x| {
            let y = reduction.sampling.logical_x_bilinear(target_x);
            let top = view.storage_row(y.lower())?;
            let bottom = view.storage_row(y.upper())?;
            sample_prepared_bilinear(top, bottom, fixed, y, reduction.color)
        }),
    }
}

fn sample_prepared_bilinear(
    top: CpuSamplingRow<'_>,
    bottom: CpuSamplingRow<'_>,
    x: CpuAxisInterpolation,
    y: CpuAxisInterpolation,
    color: ReductionColor,
) -> Result<[u8; 4], CpuReductionError> {
    let top_left = color.decode(top.read_rgba(x.lower())?);
    let top_right = color.decode(top.read_rgba(x.upper())?);
    let bottom_left = color.decode(bottom.read_rgba(x.lower())?);
    let bottom_right = color.decode(bottom.read_rgba(x.upper())?);
    let mut output = [0.0; 4];
    for channel in 0..4 {
        let top = lerp(top_left[channel], top_right[channel], x.upper_weight());
        let bottom = lerp(
            bottom_left[channel],
            bottom_right[channel],
            x.upper_weight(),
        );
        output[channel] = lerp(top, bottom, y.upper_weight());
    }
    Ok(color.encode(output))
}

fn reduce_prepared_area_row(
    view: &CpuSamplingView<'_>,
    reduction: &PreparedCpuReduction,
    target_y: u32,
    first_target_x: usize,
    row: &mut [u8],
) -> Result<(), CpuReductionError> {
    let fixed = reduction.sampling.logical_y_area(target_y);
    match reduction.sampling.logical_x_storage_axis() {
        CpuStorageAxis::X => write_prepared_row(reduction, first_target_x, row, |target_x| {
            sample_prepared_area(
                view,
                reduction.sampling.logical_x_area(target_x),
                fixed,
                reduction.color,
            )
        }),
        CpuStorageAxis::Y => write_prepared_row(reduction, first_target_x, row, |target_x| {
            sample_prepared_area(
                view,
                fixed,
                reduction.sampling.logical_x_area(target_x),
                reduction.color,
            )
        }),
    }
}

fn write_prepared_row(
    reduction: &PreparedCpuReduction,
    first_target_x: usize,
    row: &mut [u8],
    mut sample: impl FnMut(u32) -> Result<[u8; 4], CpuReductionError>,
) -> Result<(), CpuReductionError> {
    for (target_x, target) in row.chunks_exact_mut(4).enumerate() {
        let target_x = first_target_x
            .checked_add(target_x)
            .and_then(|target_x| u32::try_from(target_x).ok())
            .ok_or(CpuReductionError::GeometryOverflow { resource: "target" })?;
        write_pixel(
            target,
            reduction.descriptor.target_pixel_format(),
            sample(target_x)?,
        );
    }
    Ok(())
}

fn sample_prepared_area(
    view: &CpuSamplingView<'_>,
    x_span: CpuStorageSpan,
    y_span: CpuStorageSpan,
    color: ReductionColor,
) -> Result<[u8; 4], CpuReductionError> {
    let mut sums = [0.0; 4];
    for source_y in y_span.start()..y_span.end() {
        let y_weight = y_span.normalized_weight(source_y);
        let row = view.storage_row(source_y)?;
        for source_x in x_span.start()..x_span.end() {
            let weight = x_span.normalized_weight(source_x) * y_weight;
            let sample = color.decode(row.read_rgba(source_x)?);
            for channel in 0..4 {
                sums[channel] += sample[channel] * weight;
            }
        }
    }
    Ok(color.encode(sums))
}

/// Validation or execution failure in the pure CPU reducer.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CpuReductionError {
    /// A checked packed pixel-byte calculation exceeded `u64`.
    #[error("{resource} reduction geometry exceeds the u64 byte ledger")]
    GeometryOverflow { resource: &'static str },
    /// A valid `u64` byte count cannot be addressed by this process.
    #[error("{resource} byte length {byte_len} does not fit this process address space")]
    ByteLengthNotAddressable {
        resource: &'static str,
        byte_len: u64,
    },
    /// The supplied destination is not the exact admitted size.
    #[error("output length mismatch: expected {expected} bytes, got {actual}")]
    OutputLengthMismatch { expected: usize, actual: usize },
    /// Source stride cannot retain one complete logical row.
    #[error("source stride {stride} is smaller than the {minimum}-byte row")]
    InvalidSourceStride { stride: i64, minimum: u64 },
    /// Source row addressing escapes retained CPU bytes.
    #[error("source plane addressing escapes its {buffer_len}-byte allocation")]
    SourceBufferOutOfBounds { buffer_len: usize },
    /// The exact preserve path was paired with byte-changing work.
    #[error("encoded-sample preservation requires equal extents, format, and nearest filtering")]
    InexactEncodedSamplePreservation,
    /// The resolved color operation has no implementation in this reducer.
    #[error("unsupported CPU reduction color transform: {0:?}")]
    UnsupportedColorTransform(ResolvedScreenColorTransform),
    /// The resolved SDR transfer function has no 8-bit CPU codec.
    #[error("unsupported CPU reduction transfer function: {0:?}")]
    UnsupportedTransferFunction(CaptureTransferFunction),
    /// The resolved linear-light contract is internally inconsistent.
    #[error("resolved linear-light SDR pipeline has inconsistent source and output metadata")]
    InconsistentLinearLightPipeline,
    /// Rayon could not construct the isolated worker pool.
    #[error("failed to build CPU reduction worker pool: {0}")]
    ThreadPoolBuild(String),
    /// Plan-lifetime descriptor storage could not be reserved.
    #[error("failed to allocate CPU reduction batch metadata")]
    BatchPreparationAllocationFailed,
    /// Platform work is required before exact CPU reduction can proceed.
    #[error(transparent)]
    FallbackRequired(#[from] CpuFallbackNeed),
    /// Workspace preparation received another committed plan generation.
    #[error("CPU materialization workspace plan generation does not match its prepared batch")]
    WorkspacePlanGenerationMismatch,
    /// A prepared physical key is absent from the supplied committed plan.
    #[error("CPU materialization workspace key {batch_index} is absent from its plan")]
    WorkspacePhysicalPlanMismatch { batch_index: usize },
    /// Plan-lifetime workspace metadata could not be reserved.
    #[error("failed to allocate CPU materialization workspace metadata")]
    WorkspaceMetadataAllocationFailed,
    /// One selected physical plane could not reserve its exact storage.
    #[error("failed to allocate {byte_len} bytes for CPU workspace key {batch_index}")]
    WorkspacePlaneAllocationFailed { batch_index: usize, byte_len: usize },
    /// Execution received a workspace prepared from another batch identity.
    #[error("CPU materialization workspace belongs to another prepared batch")]
    WorkspaceBatchMismatch,
    /// Due retained-plane indices must be unique and strictly increasing.
    #[error("CPU materialization workspace schedule is not canonical")]
    WorkspaceScheduleNotCanonical,
    /// One due retained-plane index escapes the prepared workspace.
    #[error(
        "CPU workspace schedule index {workspace_index} escapes its {workspace_len}-plane workspace"
    )]
    WorkspaceScheduleIndexOutOfBounds {
        workspace_index: usize,
        workspace_len: usize,
    },
    /// Due direct-Surface indices must be unique and strictly increasing.
    #[error("CPU direct-Surface schedule is not canonical")]
    SurfaceScheduleNotCanonical,
    /// One due direct-Surface index escapes the prepared physical batch.
    #[error("CPU direct-Surface index {batch_index} escapes its {batch_len}-key batch")]
    SurfaceScheduleIndexOutOfBounds {
        batch_index: usize,
        batch_len: usize,
    },
    /// One physical key was scheduled into both workspace and Surface storage.
    #[error("CPU physical key {batch_index} has duplicate scheduled destinations")]
    ScheduledPhysicalKeyDuplicated { batch_index: usize },
    /// Retained planes may only advance through native source sequences.
    #[error(
        "CPU workspace plane {workspace_index} source sequence {actual} does not advance {previous}"
    )]
    WorkspaceFrameSequenceNotIncreasing {
        workspace_index: usize,
        previous: u64,
        actual: u64,
    },
    /// Retained physical storage no longer matches its prepared exact length.
    #[error("CPU workspace key {batch_index} has {actual} bytes; expected exactly {expected}")]
    WorkspacePlaneLengthMismatch {
        batch_index: usize,
        expected: usize,
        actual: usize,
    },
    /// A physical key for this stable source named another epoch or source config.
    #[error("screen physical reduction source config does not match its worker source")]
    SourceConfigMismatch,
    /// The physical key names another revision of the CPU reduction algorithm.
    #[error("CPU reduction algorithm revision mismatch: expected {expected}, got {actual}")]
    AlgorithmRevisionMismatch {
        expected: NonZeroU32,
        actual: NonZeroU32,
    },
    /// The requested cursor-free output cannot be recovered from composed-only pixels.
    #[error("CPU reduction cannot exclude a cursor from composed-only storage")]
    CursorExclusionUnavailable,
    /// The requested cursor pixels require separate shape composition.
    #[error("CPU reduction requires separate cursor composition")]
    CursorCompositionRequired,
    /// The capture frame failed its source epoch fence.
    #[error(transparent)]
    CaptureFrame(#[from] CaptureFrameError),
    /// Source geometry or frame storage failed the shared CPU sampling contract.
    #[error(transparent)]
    Sampling(CpuSamplingError),
    /// Caller output count differs from the prepared unique physical-key count.
    #[error("CPU batch requires {expected} outputs, got {actual}")]
    BatchOutputCountMismatch { expected: usize, actual: usize },
    /// One caller-owned plane differs from its exact admitted byte size.
    #[error("CPU batch output {index} has {actual} bytes; expected exactly {expected}")]
    BatchOutputLengthMismatch {
        index: usize,
        expected: usize,
        actual: usize,
    },
    /// A caller reordered or substituted an output binding after preparation.
    #[error("CPU batch output {index} is bound to another physical descriptor")]
    BatchDescriptorMismatch { index: usize },
    /// A logical zone slot cannot receive a physical surface reduction.
    #[error("CPU batch publication {index} is not a writable surface")]
    BatchPublicationNotSurface { index: usize },
    /// Branch-local policy must materialize from the shared physical surface.
    #[error("CPU batch publication {index} requires logical branch materialization")]
    BatchPublicationRequiresMaterialization { index: usize },
    /// A prepared surface slot no longer owns its admitted storage.
    #[error("CPU batch publication {index} lost its writable reservation")]
    BatchPublicationReservationUnavailable { index: usize },
}

impl From<CpuSamplingError> for CpuReductionError {
    fn from(error: CpuSamplingError) -> Self {
        match error {
            CpuSamplingError::CaptureFrame(error) => Self::CaptureFrame(error),
            error => Self::Sampling(error),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ReductionColor {
    Encoded,
    Srgb,
    Linear,
}

impl ReductionColor {
    fn resolve(request: CpuReductionRequest<'_>) -> Result<Self, CpuReductionError> {
        let preserves_encoded_samples = request.layout.source_extent()
            == request.layout.target_extent()
            && request.source.format() == request.target_format
            && request.filter == ScreenReductionFilter::Nearest;
        Self::resolve_pipeline(request.color_pipeline, preserves_encoded_samples)
    }

    fn resolve_pipeline(
        color_pipeline: ResolvedScreenColorPipeline,
        preserves_encoded_samples: bool,
    ) -> Result<Self, CpuReductionError> {
        match color_pipeline.transform() {
            ResolvedScreenColorTransform::PreserveEncodedSamples => {
                if !preserves_encoded_samples {
                    return Err(CpuReductionError::InexactEncodedSamplePreservation);
                }
                Ok(Self::Encoded)
            }
            ResolvedScreenColorTransform::LinearLightSdr => {
                let Some(source) = color_pipeline.effective_source() else {
                    return Err(CpuReductionError::InconsistentLinearLightPipeline);
                };
                let output = color_pipeline
                    .output()
                    .try_known()
                    .map_err(|_| CpuReductionError::InconsistentLinearLightPipeline)?;
                if source.dynamic_range() != CaptureDynamicRange::Standard
                    || output.dynamic_range() != CaptureDynamicRange::Standard
                    || source.color_space() != output.color_space()
                    || source.transfer_function() != output.transfer_function()
                {
                    return Err(CpuReductionError::InconsistentLinearLightPipeline);
                }
                match source.transfer_function() {
                    CaptureTransferFunction::Srgb => Ok(Self::Srgb),
                    CaptureTransferFunction::Linear => Ok(Self::Linear),
                    transfer => Err(CpuReductionError::UnsupportedTransferFunction(transfer)),
                }
            }
            transform @ (ResolvedScreenColorTransform::LinearRelativeColorimetric { .. }
            | ResolvedScreenColorTransform::ToneMap(_)) => {
                Err(CpuReductionError::UnsupportedColorTransform(transform))
            }
        }
    }

    fn resolve_prepared(
        descriptor: &ScreenPhysicalReductionDescriptor,
        output: CpuReductionOutputLayout,
    ) -> Result<Self, CpuReductionError> {
        Self::resolve_pipeline(
            descriptor.color_pipeline(),
            prepared_preserves_encoded_samples(descriptor, output),
        )
    }

    fn decode(self, sample: [u8; 4]) -> [f64; 4] {
        match self {
            Self::Encoded => [
                f64::from(sample[0]) / 255.0,
                f64::from(sample[1]) / 255.0,
                f64::from(sample[2]) / 255.0,
                f64::from(sample[3]) / 255.0,
            ],
            Self::Srgb => [
                f64::from(srgb_u8_to_linear(sample[0])),
                f64::from(srgb_u8_to_linear(sample[1])),
                f64::from(srgb_u8_to_linear(sample[2])),
                f64::from(sample[3]) / 255.0,
            ],
            Self::Linear => [
                f64::from(sample[0]) / 255.0,
                f64::from(sample[1]) / 255.0,
                f64::from(sample[2]) / 255.0,
                f64::from(sample[3]) / 255.0,
            ],
        }
    }

    fn encode(self, sample: [f64; 4]) -> [u8; 4] {
        match self {
            Self::Srgb => [
                linear_to_srgb_u8(sample[0] as f32),
                linear_to_srgb_u8(sample[1] as f32),
                linear_to_srgb_u8(sample[2] as f32),
                encode_linear_byte(sample[3]),
            ],
            Self::Encoded | Self::Linear => [
                encode_linear_byte(sample[0]),
                encode_linear_byte(sample[1]),
                encode_linear_byte(sample[2]),
                encode_linear_byte(sample[3]),
            ],
        }
    }
}

fn prepared_preserves_encoded_samples(
    descriptor: &ScreenPhysicalReductionDescriptor,
    output: CpuReductionOutputLayout,
) -> bool {
    let source = descriptor.source();
    let geometry = source.geometry();
    let scale = geometry.source_scale();
    geometry.rotation() == CaptureRotation::Identity
        && geometry.crop().is_none()
        && source.reflection() == ScreenSourceReflection::None
        && scale.numerator() == scale.denominator()
        && geometry.native_extent() == geometry.storage_extent()
        && geometry.storage_extent() == source.logical_extent()
        && is_full_source_region(descriptor.source_region(), source.logical_extent())
        && output.extent() == source.logical_extent()
        && descriptor.reduction_filter() == ScreenReductionFilter::Nearest
        && descriptor.target_pixel_format() == source.pixel_format()
        && descriptor.cursor() == ScreenCursorPolicy::Exclude
        && source.cursor_capabilities().has_clean_surface()
}

fn packed_byte_len(extent: PixelExtent, resource: &'static str) -> Result<u64, CpuReductionError> {
    u64::from(extent.width())
        .checked_mul(u64::from(extent.height()))
        .and_then(|pixels| pixels.checked_mul(CHANNELS_PER_PIXEL))
        .ok_or(CpuReductionError::GeometryOverflow { resource })
}

fn validate_source(
    source: &CpuCaptureStorage,
    layout: CpuReductionLayout,
) -> Result<(), CpuReductionError> {
    let row_bytes = u64::from(layout.source_extent().width())
        .checked_mul(CHANNELS_PER_PIXEL)
        .ok_or(CpuReductionError::GeometryOverflow { resource: "source" })?;
    let stride_magnitude = source
        .row_stride()
        .checked_abs()
        .and_then(|stride| u64::try_from(stride).ok())
        .ok_or(CpuReductionError::InvalidSourceStride {
            stride: source.row_stride(),
            minimum: row_bytes,
        })?;
    if stride_magnitude < row_bytes {
        return Err(CpuReductionError::InvalidSourceStride {
            stride: source.row_stride(),
            minimum: row_bytes,
        });
    }

    let first = i128::try_from(source.row0_offset()).map_err(|_| {
        CpuReductionError::SourceBufferOutOfBounds {
            buffer_len: source.bytes().len(),
        }
    })?;
    let row_count = i128::from(layout.source_extent().height() - 1);
    let last = first
        .checked_add(
            i128::from(source.row_stride())
                .checked_mul(row_count)
                .ok_or(CpuReductionError::SourceBufferOutOfBounds {
                    buffer_len: source.bytes().len(),
                })?,
        )
        .ok_or(CpuReductionError::SourceBufferOutOfBounds {
            buffer_len: source.bytes().len(),
        })?;
    let lowest = first.min(last);
    let highest = first.max(last);
    let end = highest.checked_add(i128::from(row_bytes)).ok_or(
        CpuReductionError::SourceBufferOutOfBounds {
            buffer_len: source.bytes().len(),
        },
    )?;
    let buffer_len = i128::try_from(source.bytes().len()).map_err(|_| {
        CpuReductionError::SourceBufferOutOfBounds {
            buffer_len: source.bytes().len(),
        }
    })?;
    if lowest < 0 || end > buffer_len {
        return Err(CpuReductionError::SourceBufferOutOfBounds {
            buffer_len: source.bytes().len(),
        });
    }
    Ok(())
}

fn reduce_tile(
    request: CpuReductionRequest<'_>,
    color: ReductionColor,
    tile_index: usize,
    rows_per_tile: usize,
    tile: &mut [u8],
) -> Result<(), CpuReductionError> {
    let row_bytes = request.layout.target_row_bytes();
    let first_row = tile_index
        .checked_mul(rows_per_tile)
        .ok_or(CpuReductionError::GeometryOverflow { resource: "tile" })?;
    for (local_row, row) in tile.chunks_exact_mut(row_bytes).enumerate() {
        let target_y = first_row
            .checked_add(local_row)
            .and_then(|row| u32::try_from(row).ok())
            .ok_or(CpuReductionError::GeometryOverflow { resource: "target" })?;
        reduce_row(request, color, target_y, row)?;
    }
    Ok(())
}

fn reduce_row(
    request: CpuReductionRequest<'_>,
    color: ReductionColor,
    target_y: u32,
    row: &mut [u8],
) -> Result<(), CpuReductionError> {
    for (target_x, target) in row.chunks_exact_mut(4).enumerate() {
        let target_x = u32::try_from(target_x)
            .map_err(|_| CpuReductionError::GeometryOverflow { resource: "target" })?;
        let sample = match request.filter {
            ScreenReductionFilter::Nearest => sample_nearest(request, target_x, target_y)?,
            ScreenReductionFilter::Bilinear => sample_bilinear(request, color, target_x, target_y)?,
            ScreenReductionFilter::Area => sample_area(request, color, target_x, target_y)?,
        };
        write_pixel(target, request.target_format, sample);
    }
    Ok(())
}

fn sample_nearest(
    request: CpuReductionRequest<'_>,
    target_x: u32,
    target_y: u32,
) -> Result<[u8; 4], CpuReductionError> {
    let source = request.layout.source_extent();
    let target = request.layout.target_extent();
    let source_x = nearest_coordinate(target_x, source.width(), target.width());
    let source_y = nearest_coordinate(target_y, source.height(), target.height());
    read_pixel(request.source, source_x, source_y)
}

fn nearest_coordinate(target: u32, source_len: u32, target_len: u32) -> u32 {
    let numerator = (u128::from(target) * 2 + 1) * u128::from(source_len);
    let denominator = u128::from(target_len) * 2;
    u32::try_from(numerator / denominator)
        .expect("center-mapped nearest coordinate remains inside the source")
}

#[derive(Clone, Copy, Debug)]
struct AxisInterpolation {
    lower: u32,
    upper: u32,
    upper_weight: f64,
}

fn bilinear_axis(target: u32, source_len: u32, target_len: u32) -> AxisInterpolation {
    let centered = (u128::from(target) * 2 + 1) * u128::from(source_len);
    let target_len_u128 = u128::from(target_len);
    let denominator = target_len_u128 * 2;
    if centered <= target_len_u128 {
        return AxisInterpolation {
            lower: 0,
            upper: 0,
            upper_weight: 0.0,
        };
    }
    let position = centered - target_len_u128;
    let max_position = u128::from(source_len - 1) * denominator;
    if position >= max_position {
        return AxisInterpolation {
            lower: source_len - 1,
            upper: source_len - 1,
            upper_weight: 0.0,
        };
    }
    let lower = u32::try_from(position / denominator)
        .expect("bilinear lower coordinate remains inside the source");
    let remainder = position % denominator;
    AxisInterpolation {
        lower,
        upper: lower + 1,
        upper_weight: remainder as f64 / denominator as f64,
    }
}

fn sample_bilinear(
    request: CpuReductionRequest<'_>,
    color: ReductionColor,
    target_x: u32,
    target_y: u32,
) -> Result<[u8; 4], CpuReductionError> {
    let source_extent = request.layout.source_extent();
    let target_extent = request.layout.target_extent();
    let x = bilinear_axis(target_x, source_extent.width(), target_extent.width());
    let y = bilinear_axis(target_y, source_extent.height(), target_extent.height());
    let top_left = color.decode(read_pixel(request.source, x.lower, y.lower)?);
    let top_right = color.decode(read_pixel(request.source, x.upper, y.lower)?);
    let bottom_left = color.decode(read_pixel(request.source, x.lower, y.upper)?);
    let bottom_right = color.decode(read_pixel(request.source, x.upper, y.upper)?);
    let mut output = [0.0; 4];
    for channel in 0..4 {
        let top = lerp(top_left[channel], top_right[channel], x.upper_weight);
        let bottom = lerp(bottom_left[channel], bottom_right[channel], x.upper_weight);
        output[channel] = lerp(top, bottom, y.upper_weight);
    }
    Ok(color.encode(output))
}

#[derive(Clone, Copy, Debug)]
struct AreaSpan {
    start: u32,
    end: u32,
    left: u128,
    right: u128,
    denominator: u128,
}

impl AreaSpan {
    fn new(target: u32, source_len: u32, target_len: u32) -> Self {
        let source_len = u128::from(source_len);
        let denominator = u128::from(target_len);
        let left = u128::from(target) * source_len;
        let right = (u128::from(target) + 1) * source_len;
        let start = u32::try_from(left / denominator)
            .expect("area start coordinate remains inside the source");
        let end = u32::try_from(right.div_ceil(denominator))
            .expect("area end coordinate remains inside the source or at its edge");
        Self {
            start,
            end,
            left,
            right,
            denominator,
        }
    }

    fn weight(self, source: u32) -> u128 {
        let source_left = u128::from(source) * self.denominator;
        let source_right = (u128::from(source) + 1) * self.denominator;
        self.right.min(source_right) - self.left.max(source_left)
    }
}

fn sample_area(
    request: CpuReductionRequest<'_>,
    color: ReductionColor,
    target_x: u32,
    target_y: u32,
) -> Result<[u8; 4], CpuReductionError> {
    let source_extent = request.layout.source_extent();
    let target_extent = request.layout.target_extent();
    let x_span = AreaSpan::new(target_x, source_extent.width(), target_extent.width());
    let y_span = AreaSpan::new(target_y, source_extent.height(), target_extent.height());
    let total_weight = u128::from(source_extent.width()) * u128::from(source_extent.height());
    let mut sums = [0.0; 4];
    let mut source_y = y_span.start;
    while source_y < y_span.end {
        let y_weight = y_span.weight(source_y);
        let mut source_x = x_span.start;
        while source_x < x_span.end {
            let weight = x_span.weight(source_x) * y_weight;
            let sample = color.decode(read_pixel(request.source, source_x, source_y)?);
            let weight = weight as f64;
            for channel in 0..4 {
                sums[channel] += sample[channel] * weight;
            }
            source_x += 1;
        }
        source_y += 1;
    }
    let total_weight = total_weight as f64;
    Ok(color.encode(sums.map(|sum| sum / total_weight)))
}

fn read_pixel(source: &CpuCaptureStorage, x: u32, y: u32) -> Result<[u8; 4], CpuReductionError> {
    let row = i128::try_from(source.row0_offset())
        .ok()
        .and_then(|row0| {
            i128::from(source.row_stride())
                .checked_mul(i128::from(y))
                .and_then(|offset| row0.checked_add(offset))
        })
        .and_then(|row| usize::try_from(row).ok())
        .ok_or(CpuReductionError::SourceBufferOutOfBounds {
            buffer_len: source.bytes().len(),
        })?;
    let pixel = usize::try_from(x)
        .ok()
        .and_then(|x| x.checked_mul(4))
        .and_then(|offset| row.checked_add(offset))
        .ok_or(CpuReductionError::SourceBufferOutOfBounds {
            buffer_len: source.bytes().len(),
        })?;
    let bytes =
        source
            .bytes()
            .get(pixel..pixel + 4)
            .ok_or(CpuReductionError::SourceBufferOutOfBounds {
                buffer_len: source.bytes().len(),
            })?;
    Ok(match source.format() {
        CapturePixelFormat::Rgba8 => [bytes[0], bytes[1], bytes[2], bytes[3]],
        CapturePixelFormat::Bgra8 => [bytes[2], bytes[1], bytes[0], bytes[3]],
    })
}

fn write_pixel(target: &mut [u8], format: CapturePixelFormat, sample: [u8; 4]) {
    match format {
        CapturePixelFormat::Rgba8 => target.copy_from_slice(&sample),
        CapturePixelFormat::Bgra8 => {
            target.copy_from_slice(&[sample[2], sample[1], sample[0], sample[3]]);
        }
    }
}

fn lerp(lower: f64, upper: f64, upper_weight: f64) -> f64 {
    lower * (1.0 - upper_weight) + upper * upper_weight
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn encode_linear_byte(value: f64) -> u8 {
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}
