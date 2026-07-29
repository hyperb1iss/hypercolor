use std::mem::size_of;
use std::sync::Arc;
use std::time::Instant;

use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

use super::gpu_reduction::{GpuReducer, ReducedFrame, SubmitOutcome};
use super::gpu_surface::PointerResource;
use super::{CaptureMetadata, RetainedDesktop};
use crate::{
    CaptureError, CaptureExtent, CaptureResult, DisplayRotation, GpuAdapterLuid,
    GpuReductionAdmission, GpuReductionProvenance, GpuSurfaceCursorPolicy, GpuSurfaceDescriptor,
    GpuSurfacePlanGeneration, GpuSurfaceSourceColorSpace, GpuSurfaceUnsupportedReason,
};

/// Downstream handling for one completed descriptor-keyed readback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuReductionPublicationDisposition {
    /// The exact bytes were accepted and may be overwritten by later work.
    Accepted,
    /// Retain the completed bytes and retry without resubmitting the route.
    Retry,
}

/// Borrowed exact GPU reduction bytes and their immutable provenance.
pub struct GpuReductionPublishOutcome<'a> {
    provenance: &'a GpuReductionProvenance,
    pixels: &'a [u8],
}

impl<'a> GpuReductionPublishOutcome<'a> {
    /// Complete descriptor and source identity represented by `pixels`.
    #[must_use]
    pub const fn provenance(&self) -> &GpuReductionProvenance {
        self.provenance
    }

    /// Tightly packed row-major RGBA8 output bytes.
    #[must_use]
    pub const fn pixels(&self) -> &'a [u8] {
        self.pixels
    }
}

/// Work completed by one non-blocking GPU reduction pump.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuReductionBatchInfo {
    submitted: usize,
    completed: usize,
    busy: usize,
    readback_bytes: u64,
}

impl GpuReductionBatchInfo {
    /// Descriptor reductions submitted to D3D11 in this pump.
    #[must_use]
    pub const fn submitted(self) -> usize {
        self.submitted
    }

    /// Completed readbacks offered to the downstream callback.
    #[must_use]
    pub const fn completed(self) -> usize {
        self.completed
    }

    /// Selected routes already occupied by retained or in-flight work.
    #[must_use]
    pub const fn busy(self) -> usize {
        self.busy
    }

    /// Tightly packed bytes mapped from completed staging slots.
    #[must_use]
    pub const fn readback_bytes(self) -> u64 {
        self.readback_bytes
    }

    pub(super) fn merge(&mut self, other: Self) {
        self.submitted += other.submitted;
        self.completed += other.completed;
        self.busy += other.busy;
        self.readback_bytes = self.readback_bytes.saturating_add(other.readback_bytes);
    }
}

struct ReductionRoute {
    descriptor: Arc<GpuSurfaceDescriptor>,
    reducer: GpuReducer,
    rgba: Vec<u8>,
    in_flight: bool,
    completed: Option<GpuReductionProvenance>,
    selected_for_next_acquisition: bool,
}

/// Allocation-complete immutable descriptor-keyed D3D11 reduction plan.
pub struct PreparedGpuReductionPlan {
    plan_generation: GpuSurfacePlanGeneration,
    source_id: Arc<str>,
    topology_generation: u64,
    duplication_generation: u64,
    adapter_luid: GpuAdapterLuid,
    native_source_extent: CaptureExtent,
    logical_source_extent: CaptureExtent,
    source_rotation: DisplayRotation,
    source_color_space: GpuSurfaceSourceColorSpace,
    routes: Vec<ReductionRoute>,
    selection_controlled: bool,
    allocation_byte_len: u64,
    readback_byte_len: u64,
    publication_buffer_byte_len: usize,
}

impl std::fmt::Debug for PreparedGpuReductionPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedGpuReductionPlan")
            .field("plan_generation", &self.plan_generation)
            .field("source_id", &self.source_id)
            .field("descriptor_count", &self.routes.len())
            .field("allocation_byte_len", &self.allocation_byte_len)
            .field("readback_byte_len", &self.readback_byte_len)
            .finish_non_exhaustive()
    }
}

impl PreparedGpuReductionPlan {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        plan_generation: GpuSurfacePlanGeneration,
        source_id: Arc<str>,
        topology_generation: u64,
        duplication_generation: u64,
        adapter_luid: GpuAdapterLuid,
        native_source_extent: CaptureExtent,
        logical_source_extent: CaptureExtent,
        source_rotation: DisplayRotation,
        source_color_space: GpuSurfaceSourceColorSpace,
        descriptors: &[GpuSurfaceDescriptor],
        admission: GpuReductionAdmission,
    ) -> CaptureResult<Self> {
        let allocation_byte_len = admission.admit(logical_source_extent, descriptors)?;
        let mut routes = Vec::new();
        routes.try_reserve_exact(descriptors.len()).map_err(|_| {
            CaptureError::ResourceExhausted {
                operation: "allocate GPU reduction routes",
                requested_bytes: descriptors
                    .len()
                    .saturating_mul(size_of::<ReductionRoute>()),
            }
        })?;
        let mut readback_byte_len = 0_u64;
        let mut publication_buffer_byte_len = 0_usize;
        for descriptor in descriptors {
            if descriptor.source_rotation() != source_rotation {
                return Err(CaptureError::GpuSurfaceRotationMismatch {
                    descriptor_id: descriptor.id(),
                    descriptor_rotation: descriptor.source_rotation(),
                    source_rotation,
                });
            }
            if descriptor.source_color_space() != source_color_space {
                return Err(CaptureError::UnsupportedGpuSurface {
                    descriptor_id: descriptor.id(),
                    reason: GpuSurfaceUnsupportedReason::SourceColorSpace(source_color_space),
                });
            }
            let output_bytes = checked_output_bytes(descriptor.output_extent())?;
            let route_readback = u64::try_from(output_bytes)
                .ok()
                .and_then(|bytes| {
                    bytes.checked_mul(u64::from(admission.slots_per_descriptor().get()))
                })
                .ok_or(CaptureError::GeometryOverflow {
                    operation: "account GPU reduction staging slots",
                    width: descriptor.output_extent().width(),
                    height: descriptor.output_extent().height(),
                })?;
            readback_byte_len = readback_byte_len.checked_add(route_readback).ok_or(
                CaptureError::GeometryOverflow {
                    operation: "account GPU reduction staging plan",
                    width: descriptor.output_extent().width(),
                    height: descriptor.output_extent().height(),
                },
            )?;
            publication_buffer_byte_len = publication_buffer_byte_len
                .checked_add(output_bytes)
                .ok_or(CaptureError::GeometryOverflow {
                    operation: "account GPU reduction publication buffers",
                    width: descriptor.output_extent().width(),
                    height: descriptor.output_extent().height(),
                })?;
            let mut rgba = Vec::new();
            rgba.try_reserve_exact(output_bytes)
                .map_err(|_| CaptureError::ResourceExhausted {
                    operation: "allocate GPU reduction publication buffer",
                    requested_bytes: output_bytes,
                })?;
            rgba.resize(output_bytes, 0);
            let reducer = GpuReducer::new_exact(
                device,
                context,
                native_source_extent,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                descriptor,
                admission.slots_per_descriptor().get(),
            )
            .map_err(capture_gpu_error)?;
            routes.push(ReductionRoute {
                descriptor: Arc::new(descriptor.clone()),
                reducer,
                rgba,
                in_flight: false,
                completed: None,
                selected_for_next_acquisition: false,
            });
        }
        Ok(Self {
            plan_generation,
            source_id,
            topology_generation,
            duplication_generation,
            adapter_luid,
            native_source_extent,
            logical_source_extent,
            source_rotation,
            source_color_space,
            routes,
            selection_controlled: false,
            allocation_byte_len,
            readback_byte_len,
            publication_buffer_byte_len,
        })
    }

    /// Exact descriptors in stable physical publication order.
    pub fn descriptors(&self) -> impl ExactSizeIterator<Item = &GpuSurfaceDescriptor> {
        self.routes.iter().map(|route| route.descriptor.as_ref())
    }

    /// Select due physical routes for the next retained acquisition.
    pub fn select_routes_for_next_acquisition<F>(&mut self, mut select: F)
    where
        F: FnMut(&GpuSurfaceDescriptor) -> bool,
    {
        self.selection_controlled = true;
        for route in &mut self.routes {
            route.selected_for_next_acquisition = select(&route.descriptor);
        }
    }

    /// Whether at least one selected descriptor can accept a new source.
    #[must_use]
    pub fn has_selected_routes(&self) -> bool {
        self.routes.iter().any(|route| {
            (!self.selection_controlled || route.selected_for_next_acquisition)
                && !route.in_flight
                && route.completed.is_none()
        })
    }

    /// Whether asynchronous or backpressured route work remains retained.
    #[must_use]
    pub fn has_pending_routes(&self) -> bool {
        self.routes
            .iter()
            .any(|route| route.in_flight || route.completed.is_some())
    }

    /// Checked D3D11 texture bytes retained by this plan.
    #[must_use]
    pub const fn allocation_byte_len(&self) -> u64 {
        self.allocation_byte_len
    }

    /// Checked staging texture bytes retained by descriptor-keyed rings.
    #[must_use]
    pub const fn readback_byte_len(&self) -> u64 {
        self.readback_byte_len
    }

    /// Preallocated tightly packed CPU buffers retained for callback delivery.
    #[must_use]
    pub const fn publication_buffer_byte_len(&self) -> usize {
        self.publication_buffer_byte_len
    }

    pub(super) fn requires_pointer_for_next_publication(&self) -> bool {
        self.routes.iter().any(|route| {
            (!self.selection_controlled || route.selected_for_next_acquisition)
                && !route.in_flight
                && route.completed.is_none()
                && route.descriptor.cursor() == GpuSurfaceCursorPolicy::Include
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn validate_source(
        &self,
        source_id: &str,
        topology_generation: u64,
        duplication_generation: u64,
        adapter_luid: GpuAdapterLuid,
        native_source_extent: CaptureExtent,
        logical_source_extent: CaptureExtent,
        source_rotation: DisplayRotation,
        source_color_space: GpuSurfaceSourceColorSpace,
    ) -> CaptureResult<()> {
        if self.source_id.as_ref() == source_id
            && self.topology_generation == topology_generation
            && self.duplication_generation == duplication_generation
            && self.adapter_luid == adapter_luid
            && self.native_source_extent == native_source_extent
            && self.logical_source_extent == logical_source_extent
            && self.source_rotation == source_rotation
            && self.source_color_space == source_color_space
        {
            Ok(())
        } else {
            Err(CaptureError::GpuSurfacePlanInvalidated)
        }
    }

    pub(super) fn poll_with_feedback<F>(
        &mut self,
        mut emit: F,
    ) -> CaptureResult<GpuReductionBatchInfo>
    where
        F: FnMut(GpuReductionPublishOutcome<'_>) -> GpuReductionPublicationDisposition,
    {
        let mut report = GpuReductionBatchInfo::default();
        for route in &mut self.routes {
            if route.completed.is_none()
                && route.in_flight
                && let Some(frame) = route
                    .reducer
                    .poll(&mut route.rgba)
                    .map_err(capture_gpu_error)?
            {
                validate_reduced_frame(route, &frame)?;
                route.in_flight = false;
                route.completed = Some(provenance(
                    self.plan_generation,
                    self.adapter_luid,
                    self.duplication_generation,
                    self.native_source_extent,
                    self.logical_source_extent,
                    &route.descriptor,
                    &frame,
                )?);
                report.readback_bytes = report
                    .readback_bytes
                    .saturating_add(u64::try_from(frame.bytes).unwrap_or(u64::MAX));
            }
            let Some(provenance) = route.completed.as_ref() else {
                continue;
            };
            report.completed += 1;
            let disposition = emit(GpuReductionPublishOutcome {
                provenance,
                pixels: &route.rgba,
            });
            if disposition == GpuReductionPublicationDisposition::Accepted {
                route.completed = None;
            }
        }
        Ok(report)
    }

    pub(super) fn submit_selected(
        &mut self,
        clean: &RetainedDesktop,
        pointer_resource: Option<&PointerResource>,
        duplication_generation: u64,
    ) -> CaptureResult<GpuReductionBatchInfo> {
        self.validate_source(
            &clean.metadata.source_id,
            clean.metadata.topology_generation,
            duplication_generation,
            self.adapter_luid,
            CaptureExtent::try_new(clean.metadata.source_width, clean.metadata.source_height)?,
            logical_extent(
                clean.metadata.source_width,
                clean.metadata.source_height,
                clean.metadata.rotation,
            )?,
            clean.metadata.rotation,
            clean.metadata.source_color_space,
        )?;
        let mut report = GpuReductionBatchInfo::default();
        for route in &mut self.routes {
            if self.selection_controlled && !route.selected_for_next_acquisition {
                continue;
            }
            if route.in_flight || route.completed.is_some() {
                report.busy += 1;
                continue;
            }
            validate_pointer(route, &clean.metadata, pointer_resource)?;
            match route
                .reducer
                .submit_exact(
                    clean,
                    pointer_resource,
                    &route.descriptor,
                    clean.metadata.clone(),
                )
                .map_err(capture_gpu_error)?
            {
                SubmitOutcome::Submitted => {
                    route.in_flight = true;
                    report.submitted += 1;
                }
                SubmitOutcome::Busy => report.busy += 1,
            }
        }
        self.clear_selection();
        Ok(report)
    }

    fn clear_selection(&mut self) {
        for route in &mut self.routes {
            route.selected_for_next_acquisition = false;
        }
    }
}

fn validate_pointer(
    route: &ReductionRoute,
    metadata: &CaptureMetadata,
    pointer_resource: Option<&PointerResource>,
) -> CaptureResult<()> {
    if route.descriptor.cursor() != GpuSurfaceCursorPolicy::Include || !metadata.pointer.visible {
        return Ok(());
    }
    let shape =
        metadata
            .pointer
            .shape
            .as_ref()
            .ok_or(CaptureError::GpuSurfaceCursorShapeUnavailable {
                descriptor_id: route.descriptor.id(),
                source_sequence: metadata.sequence,
            })?;
    if pointer_resource.is_some_and(|resource| {
        resource.generation == metadata.pointer.shape_generation
            && resource.width == shape.width
            && resource.height == shape.visible_height()
    }) {
        Ok(())
    } else {
        Err(CaptureError::GpuSurfaceCursorShapeUnavailable {
            descriptor_id: route.descriptor.id(),
            source_sequence: metadata.sequence,
        })
    }
}

fn validate_reduced_frame(route: &ReductionRoute, frame: &ReducedFrame) -> CaptureResult<()> {
    let output = route.descriptor.output_extent();
    let expected = checked_output_bytes(output)?;
    if frame.width == output.width() && frame.height == output.height() && frame.bytes == expected {
        Ok(())
    } else {
        Err(CaptureError::InvalidBufferGeometry {
            operation: "map exact GPU reduction",
            width: frame.width,
            height: frame.height,
            row_pitch: frame.bytes.checked_div(frame.height as usize).unwrap_or(0),
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn provenance(
    plan_generation: GpuSurfacePlanGeneration,
    adapter_luid: GpuAdapterLuid,
    duplication_generation: u64,
    native_source_extent: CaptureExtent,
    logical_source_extent: CaptureExtent,
    descriptor: &Arc<GpuSurfaceDescriptor>,
    frame: &ReducedFrame,
) -> CaptureResult<GpuReductionProvenance> {
    let freshness_deadline = frame
        .metadata
        .captured_at
        .checked_add(descriptor.freshness())
        .ok_or(CaptureError::GpuSurfaceFreshnessOverflow)?;
    Ok(GpuReductionProvenance {
        descriptor: Arc::clone(descriptor),
        plan_generation,
        adapter_luid,
        source_id: Arc::clone(&frame.metadata.source_id),
        topology_generation: frame.metadata.topology_generation,
        duplication_generation,
        source_sequence: frame.metadata.sequence,
        captured_at: frame.metadata.captured_at,
        completed_at: Instant::now(),
        freshness_deadline,
        native_source_extent,
        logical_source_extent,
        source_color_space: frame.metadata.source_color_space,
        source_rotation: frame.metadata.rotation,
        cursor_composed: descriptor.cursor() == GpuSurfaceCursorPolicy::Include
            && frame.metadata.pointer.visible,
    })
}

fn checked_output_bytes(extent: CaptureExtent) -> CaptureResult<usize> {
    usize::try_from(extent.width())
        .ok()
        .and_then(|width| {
            usize::try_from(extent.height())
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(CaptureError::GeometryOverflow {
            operation: "allocate GPU reduction output",
            width: extent.width(),
            height: extent.height(),
        })
}

fn logical_extent(
    width: u32,
    height: u32,
    rotation: DisplayRotation,
) -> CaptureResult<CaptureExtent> {
    match rotation {
        DisplayRotation::Identity | DisplayRotation::Clockwise180 => {
            CaptureExtent::try_new(width, height)
        }
        DisplayRotation::Clockwise90 | DisplayRotation::Clockwise270 => {
            CaptureExtent::try_new(height, width)
        }
    }
}

fn capture_gpu_error(error: super::gpu_reduction::GpuReductionError) -> CaptureError {
    error
        .as_capture_error()
        .unwrap_or_else(|| CaptureError::windows("run exact D3D11 reduction", error))
}
