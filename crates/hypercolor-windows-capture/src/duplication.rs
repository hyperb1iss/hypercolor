//! DXGI Desktop Duplication capture loop.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tracing::{debug, warn};
use windows::Win32::Foundation::{E_ACCESSDENIED, E_OUTOFMEMORY, HMODULE};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
    DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020, DXGI_MODE_ROTATION, DXGI_MODE_ROTATION_ROTATE90,
    DXGI_MODE_ROTATION_ROTATE180, DXGI_MODE_ROTATION_ROTATE270,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET,
    DXGI_ERROR_NOT_CURRENTLY_AVAILABLE, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_SESSION_DISCONNECTED,
    DXGI_ERROR_UNSUPPORTED, DXGI_ERROR_WAIT_TIMEOUT, DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
    DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_POINTER_SHAPE_INFO,
    DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR,
    DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIAdapter1,
    IDXGIAdapter3, IDXGIDevice, IDXGIFactory1, IDXGIOutput, IDXGIOutput1, IDXGIOutput6,
    IDXGIOutputDuplication, IDXGIResource,
};
use windows::Win32::Graphics::Gdi::{DISPLAY_DEVICEW, EnumDisplayDevicesW};
use windows::core::{HRESULT, Interface, PCWSTR};

use crate::shared::{
    CaptureError, CaptureExtent, CaptureLane, CaptureRegion, CaptureResourceAdmission,
    CaptureResourceKind, CaptureResourceLease, CaptureResult, CpuDesktopFrame, CursorInfo,
    DisplayRotation, Frame, GpuAdapterLuid, GpuReductionAdmission, GpuSurfaceAdmission,
    GpuSurfaceDescriptor, GpuSurfacePlanGeneration, GpuSurfaceSourceColorSpace, MonitorInfo,
    MonitorSelector, ReductionPath, ReductionTelemetry, RgbaFramePlane, RgbaFramePool,
    commit_capture_resource, default_capture_resource_admission, recycle_rgba_frame_plane,
    reserve_capture_resource, subsample_stride_within, subsampled_extent,
};

mod cpu_readback;
mod gpu_readback;
pub(crate) mod gpu_reduction;
mod gpu_surface;

pub(crate) use cpu_readback::cpu_readback_metadata_byte_len;
pub(crate) use gpu_readback::{
    gpu_reduction_constant_buffer_byte_len, gpu_reduction_metadata_byte_len,
};
pub(crate) use gpu_surface::{
    gpu_surface_constant_buffer_byte_len, gpu_surface_metadata_byte_len,
    gpu_surface_target_preparation_metadata_byte_len,
};

pub use cpu_readback::PreparedCpuDesktopReadback;
pub use gpu_readback::{
    GpuReductionBatchInfo, GpuReductionPublicationDisposition, GpuReductionPublishOutcome,
    PreparedGpuReductionPlan,
};
pub use gpu_surface::{
    GpuSurfaceBatchInfo, GpuSurfaceLease, GpuSurfacePublication, GpuSurfacePublicationDisposition,
    GpuSurfacePublishOutcome, GpuSurfaceTargetPreparation, GpuSurfaceTargetPreparationSlot,
    PreparedGpuSurfacePlan,
};

/// Requested consumers for one Desktop Duplication acquisition cycle.
pub struct CapturePumpRequest<'a> {
    gpu: Option<&'a mut PreparedGpuSurfacePlan>,
    reduction: Option<&'a mut PreparedGpuReductionPlan>,
    cpu: Option<&'a mut PreparedCpuDesktopReadback>,
}

impl<'a> CapturePumpRequest<'a> {
    /// Request any combination of exact GPU and native CPU outputs.
    #[must_use]
    pub const fn new(
        gpu: Option<&'a mut PreparedGpuSurfacePlan>,
        cpu: Option<&'a mut PreparedCpuDesktopReadback>,
    ) -> Self {
        Self {
            gpu,
            reduction: None,
            cpu,
        }
    }

    /// Request any combination of native GPU, reduced GPU readback, and
    /// native CPU outputs from one retained desktop acquisition.
    #[must_use]
    pub const fn with_reduction(
        gpu: Option<&'a mut PreparedGpuSurfacePlan>,
        reduction: Option<&'a mut PreparedGpuReductionPlan>,
        cpu: Option<&'a mut PreparedCpuDesktopReadback>,
    ) -> Self {
        Self {
            gpu,
            reduction,
            cpu,
        }
    }

    /// Request only exact GPU publications.
    #[must_use]
    pub const fn gpu(plan: &'a mut PreparedGpuSurfacePlan) -> Self {
        Self::new(Some(plan), None)
    }

    /// Request only an exact native CPU frame.
    #[must_use]
    pub const fn cpu(readback: &'a mut PreparedCpuDesktopReadback) -> Self {
        Self::new(None, Some(readback))
    }

    /// Request only exact descriptor-keyed GPU reduction readback.
    #[must_use]
    pub const fn reduction(plan: &'a mut PreparedGpuReductionPlan) -> Self {
        Self::with_reduction(None, Some(plan), None)
    }

    /// Request exact GPU publications and native CPU readback together.
    #[must_use]
    pub const fn hybrid(
        plan: &'a mut PreparedGpuSurfacePlan,
        readback: &'a mut PreparedCpuDesktopReadback,
    ) -> Self {
        Self::new(Some(plan), Some(readback))
    }
}

/// Independent results produced by one capture pump cycle.
#[derive(Debug)]
pub struct CapturePumpReport {
    /// Whether Desktop Duplication delivered a desktop or cursor update.
    pub acquired: bool,
    /// Exact shareable GPU publication lane.
    pub gpu: CaptureLane<GpuSurfaceBatchInfo>,
    /// Descriptor-keyed GPU reduction/readback lane.
    pub reduction: CaptureLane<GpuReductionBatchInfo>,
    /// Exact tightly packed native BGRA readback lane.
    pub cpu: CaptureLane<CpuDesktopFrame>,
}

use gpu_reduction::{GpuReducer, ReducedFrame, SubmitOutcome};

enum AnalysisGpuReducer {
    Uninitialized,
    Ready(GpuReducer),
    Disabled,
}

impl AnalysisGpuReducer {
    const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    const fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled)
    }

    fn ready_mut(&mut self) -> Option<&mut GpuReducer> {
        match self {
            Self::Ready(reducer) => Some(reducer),
            Self::Uninitialized | Self::Disabled => None,
        }
    }
}

/// Bytes per pixel in both the duplicated surface and our RGBA output.
const BYTES_PER_PIXEL: usize = 4;
const TOPOLOGY_CHECK_INTERVAL: Duration = Duration::from_secs(1);
const MAX_POINTER_SHAPE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DesktopFrameSource {
    AcquiredResource,
    RetainedStaging,
}

#[derive(Clone, Copy)]
struct BgraRows<'a> {
    bytes: &'a [u8],
    row_pitch: usize,
    width: u32,
    height: u32,
}

struct MappedTexture<'a> {
    context: &'a ID3D11DeviceContext,
    texture: &'a ID3D11Texture2D,
    mapped: D3D11_MAPPED_SUBRESOURCE,
}

impl<'a> MappedTexture<'a> {
    fn map(context: &'a ID3D11DeviceContext, texture: &'a ID3D11Texture2D) -> CaptureResult<Self> {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: the caller supplies a staging texture created for CPU reads.
        unsafe { context.Map(texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }
            .map_err(|source| classify_windows_error("map staging texture", source))?;
        Ok(Self {
            context,
            texture,
            mapped,
        })
    }

    fn rows(&self, width: u32, height: u32) -> CaptureResult<BgraRows<'_>> {
        let row_pitch = self.mapped.RowPitch as usize;
        let minimum_row_bytes = checked_rgba_len(width, 1, "validate mapped row pitch")?;
        let source_len = row_pitch
            .checked_mul(height as usize)
            .filter(|len| *len <= isize::MAX as usize);
        if self.mapped.pData.is_null() || row_pitch < minimum_row_bytes || source_len.is_none() {
            return Err(CaptureError::InvalidBufferGeometry {
                operation: "map staging texture",
                width,
                height,
                row_pitch,
            });
        }
        // SAFETY: Map succeeded with a non-null pointer and the checked length
        // stays within both the reported row geometry and slice limits.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                self.mapped.pData.cast::<u8>(),
                source_len.expect("mapped length was validated above"),
            )
        };
        Ok(BgraRows {
            bytes,
            row_pitch,
            width,
            height,
        })
    }
}

impl Drop for MappedTexture<'_> {
    fn drop(&mut self) {
        // SAFETY: this guard exists only after a successful Map for the same
        // texture, context, and subresource.
        unsafe { self.context.Unmap(self.texture, 0) };
    }
}

fn checked_rgba_len(width: u32, height: u32, operation: &'static str) -> CaptureResult<usize> {
    usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL))
        .ok_or(CaptureError::GeometryOverflow {
            operation,
            width,
            height,
        })
}

fn require_plane_capacity(
    rgba: &mut Vec<u8>,
    requested_bytes: usize,
    operation: &'static str,
) -> CaptureResult<()> {
    if requested_bytes <= rgba.capacity() {
        return Ok(());
    }
    Err(CaptureError::ResourceExhausted {
        operation,
        requested_bytes,
    })
}

fn take_frame_plane(
    pool: &RgbaFramePool,
    admission: &dyn CaptureResourceAdmission,
    requested_bytes: usize,
) -> CaptureResult<RgbaFramePlane> {
    let mut pool = pool
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(index) = pool
        .iter()
        .enumerate()
        .filter(|(_, plane)| plane.rgba.capacity() >= requested_bytes)
        .min_by_key(|(_, plane)| plane.rgba.capacity())
        .map(|(index, _)| index)
    {
        return Ok(pool.swap_remove(index));
    }
    drop(pool);

    let retained_bytes =
        u64::try_from(requested_bytes).map_err(|_| CaptureError::ResourceExhausted {
            operation: "reserve RGBA capture plane",
            requested_bytes,
        })?;
    let reservation = reserve_capture_resource(
        admission,
        CaptureResourceKind::RgbaFramePlane,
        retained_bytes,
        "reserve RGBA capture plane",
    )?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(requested_bytes)
        .map_err(|_| CaptureError::ResourceExhausted {
            operation: "allocate RGBA capture plane",
            requested_bytes,
        })?;
    rgba.resize(requested_bytes, 0);
    let mut rgba = rgba.into_boxed_slice().into_vec();
    rgba.clear();
    let resource_lease =
        commit_capture_resource(reservation, retained_bytes, "commit RGBA capture plane")?;
    Ok(RgbaFramePlane {
        rgba,
        resource_lease,
    })
}

#[derive(Clone)]
struct CaptureMetadata {
    source_id: Arc<str>,
    topology_generation: u64,
    sequence: u64,
    captured_at: Instant,
    cursor: CursorInfo,
    pointer: PointerState,
    source_width: u32,
    source_height: u32,
    origin_x: i32,
    origin_y: i32,
    rotation: DisplayRotation,
    source_color_space: GpuSurfaceSourceColorSpace,
    region: CaptureRegion,
}

struct NativeCaptureUpdate {
    metadata: CaptureMetadata,
}

#[derive(Clone)]
struct RetainedDesktop {
    texture: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    metadata: CaptureMetadata,
    _resource_lease: Option<Arc<dyn CaptureResourceLease>>,
}

struct AdmittedStagingTexture {
    texture: ID3D11Texture2D,
    _resource_lease: Arc<dyn CaptureResourceLease>,
}

fn advance_cpu_clean(
    clean: &RetainedDesktop,
    readback: Option<&mut PreparedCpuDesktopReadback>,
    lane: &mut CaptureLane<CpuDesktopFrame>,
) {
    let Some(readback) = readback else {
        return;
    };
    if !matches!(lane, CaptureLane::Idle) || !readback.should_submit(clean) {
        return;
    }
    *lane = match readback.submit(clean) {
        Ok(true) => readback.poll(),
        Ok(false) => CaptureLane::Busy,
        Err(error) => CaptureLane::Failed(error),
    };
}

fn prepare_duplication<D, S, A, E>(
    duplication: &mut Option<D>,
    staging: &mut Option<S>,
    acquire: impl FnOnce() -> Result<D, E>,
    admit: impl FnOnce(&D) -> Result<A, E>,
) -> Result<(D, A), E> {
    *staging = None;
    *duplication = None;
    let duplication = acquire()?;
    let admission = admit(&duplication)?;
    Ok((duplication, admission))
}

fn session_rebuild_error(error: CaptureError) -> CaptureError {
    match error {
        CaptureError::ResourceExhausted {
            operation,
            requested_bytes,
        } => CaptureError::SessionResourceExhausted {
            operation,
            requested_bytes,
        },
        other => other,
    }
}

fn gpu_surface_acquire_timeout(requested: Duration, has_pending_routes: bool) -> Duration {
    if has_pending_routes {
        Duration::ZERO
    } else {
        requested
    }
}

const fn desktop_frame_source(
    desktop_updated: bool,
    staging_available: bool,
) -> DesktopFrameSource {
    if desktop_updated || !staging_available {
        DesktopFrameSource::AcquiredResource
    } else {
        DesktopFrameSource::RetainedStaging
    }
}

fn classify_hresult(
    context: &'static str,
    code: HRESULT,
    message: impl std::fmt::Display,
) -> CaptureError {
    match code {
        DXGI_ERROR_WAIT_TIMEOUT => CaptureError::Timeout,
        DXGI_ERROR_ACCESS_LOST => CaptureError::AccessLost,
        E_ACCESSDENIED => CaptureError::AccessDenied,
        DXGI_ERROR_NOT_CURRENTLY_AVAILABLE => CaptureError::AlreadyDuplicating,
        DXGI_ERROR_SESSION_DISCONNECTED => CaptureError::SessionUnavailable,
        DXGI_ERROR_DEVICE_REMOVED | DXGI_ERROR_DEVICE_RESET => CaptureError::DeviceLost,
        _ => CaptureError::windows(context, message),
    }
}

fn classify_windows_error(context: &'static str, source: windows::core::Error) -> CaptureError {
    classify_hresult(context, source.code(), source)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PointerShapeKind {
    Color,
    Monochrome,
    MaskedColor,
}

#[derive(Debug)]
struct PointerShape {
    kind: PointerShapeKind,
    width: u32,
    height: u32,
    pitch: u32,
    hotspot_x: i32,
    hotspot_y: i32,
    bytes: Box<[u8]>,
    _resource_lease: Option<Arc<dyn CaptureResourceLease>>,
}

#[derive(Clone, Default)]
struct PointerState {
    visible: bool,
    position_x: i32,
    position_y: i32,
    shape: Option<Arc<PointerShape>>,
    shape_generation: u64,
}

impl PointerState {
    fn cursor_info(
        &self,
        scanout_width: u32,
        scanout_height: u32,
        rotation: DisplayRotation,
    ) -> CursorInfo {
        let shape = self.shape.as_ref();
        let (position_x, position_y, width, height, hotspot_x, hotspot_y) =
            shape.map_or((0, 0, 0, 0, 0, 0), |shape| {
                pointer_scanout_geometry(
                    self.position_x,
                    self.position_y,
                    shape.width,
                    shape.visible_height(),
                    shape.hotspot_x,
                    shape.hotspot_y,
                    scanout_width,
                    scanout_height,
                    rotation,
                )
            });
        CursorInfo {
            visible: self.visible,
            position_x,
            position_y,
            hotspot_x,
            hotspot_y,
            width,
            height,
            shape_generation: self.shape_generation,
            composed: !self.visible || shape.is_some(),
        }
    }

    fn composite_bgra(
        &self,
        desktop: [u8; 4],
        scanout_x: u32,
        scanout_y: u32,
        scanout_width: u32,
        scanout_height: u32,
        rotation: DisplayRotation,
    ) -> [u8; 4] {
        let Some(shape) = self.shape.as_ref().filter(|_| self.visible) else {
            return desktop;
        };
        let (logical_x, logical_y) = scanout_to_logical(
            scanout_x,
            scanout_y,
            scanout_width,
            scanout_height,
            rotation,
        );
        let shape_x = logical_x - i64::from(self.position_x);
        let shape_y = logical_y - i64::from(self.position_y);
        if shape_x < 0
            || shape_y < 0
            || shape_x >= i64::from(shape.width)
            || shape_y >= i64::from(shape.visible_height())
        {
            return desktop;
        }
        shape.composite(desktop, shape_x as usize, shape_y as usize)
    }
}

impl PointerShape {
    #[cfg(test)]
    fn validate(&self) -> Result<(), &'static str> {
        self.validate_written_bytes(self.bytes.len())
    }

    fn validate_written_bytes(&self, written_bytes: usize) -> Result<(), &'static str> {
        if written_bytes > self.bytes.len() {
            return Err("pointer shape write exceeds its backing buffer");
        }
        let pitch = self.pitch as usize;
        let rows = self.height as usize;
        let required = pitch
            .checked_mul(rows)
            .ok_or("pointer shape size overflow")?;
        if required > written_bytes {
            return Err("pointer shape buffer is shorter than its pitch and height");
        }
        match self.kind {
            PointerShapeKind::Color | PointerShapeKind::MaskedColor => {
                let row_bytes = (self.width as usize)
                    .checked_mul(4)
                    .ok_or("pointer shape row size overflow")?;
                if row_bytes > pitch {
                    return Err("pointer shape pitch is shorter than its color row");
                }
            }
            PointerShapeKind::Monochrome => {
                if !self.height.is_multiple_of(2) {
                    return Err("monochrome pointer shape height is not even");
                }
                if self.width.div_ceil(8) as usize > pitch {
                    return Err("pointer shape pitch is shorter than its mask row");
                }
            }
        }
        Ok(())
    }

    const fn visible_height(&self) -> u32 {
        match self.kind {
            PointerShapeKind::Monochrome => self.height / 2,
            PointerShapeKind::Color | PointerShapeKind::MaskedColor => self.height,
        }
    }

    fn composite(&self, desktop: [u8; 4], x: usize, y: usize) -> [u8; 4] {
        match self.kind {
            PointerShapeKind::Color => self.color_pixel(desktop, x, y),
            PointerShapeKind::Monochrome => self.monochrome_pixel(desktop, x, y),
            PointerShapeKind::MaskedColor => self.masked_color_pixel(desktop, x, y),
        }
    }

    fn color_pixel(&self, desktop: [u8; 4], x: usize, y: usize) -> [u8; 4] {
        let Some(pixel) = self.bgra_pixel(x, y) else {
            return desktop;
        };
        let alpha = u16::from(pixel[3]);
        let inverse = 255 - alpha;
        [
            blend_channel(pixel[0], desktop[0], alpha, inverse),
            blend_channel(pixel[1], desktop[1], alpha, inverse),
            blend_channel(pixel[2], desktop[2], alpha, inverse),
            0xFF,
        ]
    }

    fn masked_color_pixel(&self, desktop: [u8; 4], x: usize, y: usize) -> [u8; 4] {
        let Some(pixel) = self.bgra_pixel(x, y) else {
            return desktop;
        };
        if pixel[3] == 0 {
            [pixel[0], pixel[1], pixel[2], 0xFF]
        } else {
            [
                desktop[0] ^ pixel[0],
                desktop[1] ^ pixel[1],
                desktop[2] ^ pixel[2],
                0xFF,
            ]
        }
    }

    fn monochrome_pixel(&self, desktop: [u8; 4], x: usize, y: usize) -> [u8; 4] {
        let pitch = self.pitch as usize;
        let byte = x / 8;
        let bit = 0x80_u8 >> (x % 8);
        let visible_height = self.visible_height() as usize;
        let and = self
            .bytes
            .get(y.saturating_mul(pitch).saturating_add(byte))
            .is_some_and(|value| value & bit != 0);
        let xor = self
            .bytes
            .get(
                y.saturating_add(visible_height)
                    .saturating_mul(pitch)
                    .saturating_add(byte),
            )
            .is_some_and(|value| value & bit != 0);
        let and_mask = if and { 0xFF } else { 0 };
        let xor_mask = if xor { 0xFF } else { 0 };
        [
            (desktop[0] & and_mask) ^ xor_mask,
            (desktop[1] & and_mask) ^ xor_mask,
            (desktop[2] & and_mask) ^ xor_mask,
            0xFF,
        ]
    }

    fn bgra_pixel(&self, x: usize, y: usize) -> Option<[u8; 4]> {
        let offset = y
            .checked_mul(self.pitch as usize)?
            .checked_add(x.checked_mul(4)?)?;
        let pixel = self.bytes.get(offset..offset.checked_add(4)?)?;
        Some([pixel[0], pixel[1], pixel[2], pixel[3]])
    }
}

const fn blend_channel(source: u8, desktop: u8, alpha: u16, inverse: u16) -> u8 {
    ((source as u16 * alpha + desktop as u16 * inverse + 127) / 255) as u8
}

const fn scanout_to_logical(
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    rotation: DisplayRotation,
) -> (i64, i64) {
    match rotation {
        DisplayRotation::Identity => (x as i64, y as i64),
        DisplayRotation::Clockwise90 => (height as i64 - 1 - y as i64, x as i64),
        DisplayRotation::Clockwise180 => {
            (width as i64 - 1 - x as i64, height as i64 - 1 - y as i64)
        }
        DisplayRotation::Clockwise270 => (y as i64, width as i64 - 1 - x as i64),
    }
}

const fn logical_to_scanout(
    x: i64,
    y: i64,
    width: u32,
    height: u32,
    rotation: DisplayRotation,
) -> (i64, i64) {
    match rotation {
        DisplayRotation::Identity => (x, y),
        DisplayRotation::Clockwise90 => (y, height as i64 - 1 - x),
        DisplayRotation::Clockwise180 => (width as i64 - 1 - x, height as i64 - 1 - y),
        DisplayRotation::Clockwise270 => (width as i64 - 1 - y, x),
    }
}

#[allow(clippy::too_many_arguments)]
fn pointer_scanout_geometry(
    position_x: i32,
    position_y: i32,
    shape_width: u32,
    shape_height: u32,
    hotspot_x: i32,
    hotspot_y: i32,
    scanout_width: u32,
    scanout_height: u32,
    rotation: DisplayRotation,
) -> (i32, i32, u32, u32, i32, i32) {
    if shape_width == 0 || shape_height == 0 {
        return (position_x, position_y, 0, 0, hotspot_x, hotspot_y);
    }
    let right = i64::from(position_x) + i64::from(shape_width) - 1;
    let bottom = i64::from(position_y) + i64::from(shape_height) - 1;
    let corners = [
        logical_to_scanout(
            i64::from(position_x),
            i64::from(position_y),
            scanout_width,
            scanout_height,
            rotation,
        ),
        logical_to_scanout(
            right,
            i64::from(position_y),
            scanout_width,
            scanout_height,
            rotation,
        ),
        logical_to_scanout(
            i64::from(position_x),
            bottom,
            scanout_width,
            scanout_height,
            rotation,
        ),
        logical_to_scanout(right, bottom, scanout_width, scanout_height, rotation),
    ];
    let min_x = corners.iter().map(|(x, _)| *x).min().unwrap_or(0);
    let max_x = corners.iter().map(|(x, _)| *x).max().unwrap_or(min_x);
    let min_y = corners.iter().map(|(_, y)| *y).min().unwrap_or(0);
    let max_y = corners.iter().map(|(_, y)| *y).max().unwrap_or(min_y);
    let hotspot = logical_to_scanout(
        i64::from(position_x) + i64::from(hotspot_x),
        i64::from(position_y) + i64::from(hotspot_y),
        scanout_width,
        scanout_height,
        rotation,
    );
    (
        min_x.try_into().unwrap_or(if min_x.is_negative() {
            i32::MIN
        } else {
            i32::MAX
        }),
        min_y.try_into().unwrap_or(if min_y.is_negative() {
            i32::MIN
        } else {
            i32::MAX
        }),
        (max_x - min_x + 1).try_into().unwrap_or(u32::MAX),
        (max_y - min_y + 1).try_into().unwrap_or(u32::MAX),
        (hotspot.0 - min_x).try_into().unwrap_or(0),
        (hotspot.1 - min_y).try_into().unwrap_or(0),
    )
}

struct EnumeratedOutput {
    adapter: IDXGIAdapter1,
    output: IDXGIOutput,
    monitor: MonitorInfo,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TopologyEntry {
    id: String,
    origin_x: i32,
    origin_y: i32,
    width: u32,
    height: u32,
    primary: bool,
    rotation: DisplayRotation,
}

#[derive(Default)]
struct TopologyState {
    entries: Vec<TopologyEntry>,
    generation: u64,
}

impl TopologyState {
    fn observe(&mut self, entries: Vec<TopologyEntry>) -> u64 {
        if self.generation == 0 || self.entries != entries {
            self.entries = entries;
            self.generation = self.generation.wrapping_add(1).max(1);
        }
        self.generation
    }
}

static TOPOLOGY_STATE: OnceLock<Mutex<TopologyState>> = OnceLock::new();
const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 1;

fn topology_entries(monitors: &[MonitorInfo]) -> Vec<TopologyEntry> {
    let mut entries = monitors
        .iter()
        .map(|monitor| TopologyEntry {
            id: monitor.id.clone(),
            origin_x: monitor.origin_x,
            origin_y: monitor.origin_y,
            width: monitor.width,
            height: monitor.height,
            primary: monitor.primary,
            rotation: monitor.rotation,
        })
        .collect::<Vec<_>>();
    entries.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    entries
}

fn utf16_string(value: &[u16]) -> String {
    let len = value
        .iter()
        .position(|&character| character == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..len])
}

fn persistent_display_id(device_name: &[u16; 32]) -> CaptureResult<String> {
    let mut display = DISPLAY_DEVICEW {
        cb: u32::try_from(size_of::<DISPLAY_DEVICEW>()).unwrap_or(u32::MAX),
        ..DISPLAY_DEVICEW::default()
    };
    // SAFETY: DeviceName is a NUL-terminated array owned by the live DXGI
    // descriptor. `display` has the required cb size and outlives the call.
    let found = unsafe {
        EnumDisplayDevicesW(
            PCWSTR(device_name.as_ptr()),
            0,
            &mut display,
            EDD_GET_DEVICE_INTERFACE_NAME,
        )
    };
    if !found.as_bool() {
        return Err(CaptureError::windows(
            "resolve persistent display identity",
            "EnumDisplayDevicesW returned no monitor device",
        ));
    }
    let id = utf16_string(&display.DeviceID).to_ascii_lowercase();
    if id.is_empty() {
        return Err(CaptureError::windows(
            "resolve persistent display identity",
            "monitor device interface path was empty",
        ));
    }
    Ok(format!("display:{id}"))
}

/// Enumerate every output across every adapter, in adapter-then-output order.
fn enumerate_outputs() -> CaptureResult<Vec<EnumeratedOutput>> {
    // SAFETY: CreateDXGIFactory1 is a plain COM factory call with no
    // preconditions; the returned interface is checked by `?`.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }
        .map_err(|source| CaptureError::windows("create DXGI factory", source))?;

    let mut outputs = Vec::new();
    for adapter_index in 0.. {
        // SAFETY: EnumAdapters1 reports DXGI_ERROR_NOT_FOUND past the last
        // adapter, which is the documented loop terminator.
        let adapter: IDXGIAdapter1 = match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(source) => {
                return Err(CaptureError::windows("enumerate DXGI adapters", source));
            }
        };

        for output_index in 0.. {
            // SAFETY: same NOT_FOUND termination contract as adapters.
            match unsafe { adapter.EnumOutputs(output_index) } {
                Ok(output) => {
                    // SAFETY: GetDesc copies the live output descriptor into a
                    // return value and does not retain caller-owned storage.
                    let desc = unsafe { output.GetDesc() }
                        .map_err(|source| CaptureError::windows("describe DXGI output", source))?;
                    if !desc.AttachedToDesktop.as_bool() {
                        continue;
                    }
                    let name = utf16_string(&desc.DeviceName);
                    let bounds = desc.DesktopCoordinates;
                    let width = u32::try_from(i64::from(bounds.right) - i64::from(bounds.left))
                        .unwrap_or(0);
                    let height = u32::try_from(i64::from(bounds.bottom) - i64::from(bounds.top))
                        .unwrap_or(0);
                    if width == 0 || height == 0 {
                        continue;
                    }
                    let id = persistent_display_id(&desc.DeviceName)?;
                    outputs.push(EnumeratedOutput {
                        adapter: adapter.clone(),
                        output,
                        monitor: MonitorInfo {
                            index: outputs.len(),
                            id,
                            name,
                            width,
                            height,
                            origin_x: bounds.left,
                            origin_y: bounds.top,
                            primary: bounds.left == 0 && bounds.top == 0,
                            rotation: display_rotation(desc.Rotation),
                            topology_generation: 0,
                        },
                    });
                }
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(source) => {
                    return Err(CaptureError::windows("enumerate DXGI outputs", source));
                }
            }
        }
    }

    let monitors = outputs
        .iter()
        .map(|entry| entry.monitor.clone())
        .collect::<Vec<_>>();
    let generation = TOPOLOGY_STATE
        .get_or_init(|| Mutex::new(TopologyState::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .observe(topology_entries(&monitors));
    for output in &mut outputs {
        output.monitor.topology_generation = generation;
    }

    Ok(outputs)
}

/// How many display outputs are attached.
pub(crate) fn output_count() -> CaptureResult<usize> {
    enumerate_outputs().map(|outputs| outputs.len())
}

/// Describe every attached output for monitor pickers.
pub(crate) fn describe_outputs() -> CaptureResult<Vec<crate::shared::MonitorInfo>> {
    enumerate_outputs().map(|outputs| outputs.into_iter().map(|output| output.monitor).collect())
}

/// A live Desktop Duplication session for one display output.
pub struct DesktopDuplicator {
    selector: MonitorSelector,
    monitor: usize,
    source_id: Arc<str>,
    primary: bool,
    topology_generation: u64,
    duplication_generation: u64,
    adapter_luid: GpuAdapterLuid,
    last_topology_check: Instant,
    requested_extent: CaptureExtent,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    duplication: Option<IDXGIOutputDuplication>,
    /// Canonical GPU-resident desktop paired with the acquisition that produced it.
    clean_desktop: Option<RetainedDesktop>,
    /// CPU-readable scratch texture used only by the CPU fallback path.
    cpu_staging: Option<AdmittedStagingTexture>,
    /// RGBA allocations returned by frames after their last consumer drops.
    frame_pool: RgbaFramePool,
    /// Set while a duplicated frame is held and must be released before the
    /// next acquire. DXGI rejects back-to-back acquires without a release.
    frame_held: bool,
    logical_width: u32,
    logical_height: u32,
    native_width: u32,
    native_height: u32,
    origin_x: i32,
    origin_y: i32,
    rotation: DisplayRotation,
    source_color_space: GpuSurfaceSourceColorSpace,
    pointer: PointerState,
    gpu_pointer: Option<gpu_surface::PointerResource>,
    region: Option<CaptureRegion>,
    capture_sequence: u64,
    latest_capture: Option<CaptureMetadata>,
    gpu_reducer: AnalysisGpuReducer,
    reduction_telemetry: ReductionTelemetry,
    analysis_pending: bool,
    resource_admission: Arc<dyn CaptureResourceAdmission>,
}

impl DesktopDuplicator {
    /// Open Desktop Duplication for `monitor` at the requested extent.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::MonitorNotFound`] when the index is out of
    /// range, [`CaptureError::AlreadyDuplicating`] when another process holds
    /// the duplication interface, or [`CaptureError::Windows`] for any other
    /// D3D11/DXGI failure.
    pub fn new(monitor: usize, requested_extent: CaptureExtent) -> CaptureResult<Self> {
        Self::open(MonitorSelector::Index(monitor), requested_extent)
    }

    /// Open Desktop Duplication for a stable or primary-aware selector.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::MonitorNotFound`] when no outputs are attached
    /// or an index is out of range, [`CaptureError::SourceNotFound`] when a
    /// stable output disappeared, and the same platform errors as [`Self::new`].
    pub fn open(selector: MonitorSelector, requested_extent: CaptureExtent) -> CaptureResult<Self> {
        Self::open_with_resource_admission(
            selector,
            requested_extent,
            default_capture_resource_admission(),
        )
    }

    /// Open Desktop Duplication with a caller-owned source resource fence.
    ///
    /// The admission authority is installed before device-local resources are
    /// created and remains attached to every source-owned replacement.
    ///
    /// # Errors
    ///
    /// Returns the same platform errors as [`Self::open`] plus a typed
    /// resource rejection when the caller's fence cannot admit source backing.
    pub fn open_with_resource_admission(
        selector: MonitorSelector,
        requested_extent: CaptureExtent,
        resource_admission: Arc<dyn CaptureResourceAdmission>,
    ) -> CaptureResult<Self> {
        let outputs = enumerate_outputs()?;
        let monitors = outputs
            .iter()
            .map(|entry| entry.monitor.clone())
            .collect::<Vec<_>>();
        let selected = selector.resolve(&monitors)?.clone();
        let selected_index = outputs
            .iter()
            .position(|entry| entry.monitor.id == selected.id)
            .expect("resolved monitor belongs to enumerated outputs");
        let EnumeratedOutput {
            adapter,
            output,
            monitor,
        } = outputs
            .into_iter()
            .nth(selected_index)
            .expect("resolved output index remains in range");
        let selector = match selector {
            MonitorSelector::Auto => MonitorSelector::Auto,
            MonitorSelector::StableId(_) | MonitorSelector::Index(_) => {
                MonitorSelector::StableId(monitor.id.clone())
            }
        };

        let adapter_luid = adapter_luid(&adapter)?;
        let (device, context) = create_device(&adapter)?;
        let output = output
            .cast::<IDXGIOutput1>()
            .map_err(|source| CaptureError::windows("query IDXGIOutput1", source))?;
        let duplication = duplicate_output(&output, &device)?;
        let (logical_width, logical_height, rotation) = duplication_geometry(&duplication);
        let (native_width, native_height) =
            native_scanout_extent(logical_width, logical_height, rotation);
        let (origin_x, origin_y) = output_origin(&output)?;
        let source_color_space = output_color_space(&output);
        let gpu_reducer = AnalysisGpuReducer::Uninitialized;
        let reduction_telemetry = ReductionTelemetry {
            path: ReductionPath::Gpu,
            ..ReductionTelemetry::default()
        };

        Ok(Self {
            selector,
            monitor: monitor.index,
            source_id: Arc::from(monitor.id),
            primary: monitor.primary,
            topology_generation: monitor.topology_generation,
            duplication_generation: 1,
            adapter_luid,
            last_topology_check: Instant::now(),
            requested_extent,
            device,
            context,
            output,
            duplication: Some(duplication),
            clean_desktop: None,
            cpu_staging: None,
            frame_pool: Arc::new(Mutex::new(Vec::new())),
            frame_held: false,
            logical_width,
            logical_height,
            native_width,
            native_height,
            origin_x,
            origin_y,
            rotation,
            source_color_space,
            pointer: PointerState::default(),
            gpu_pointer: None,
            region: None,
            capture_sequence: 0,
            latest_capture: None,
            gpu_reducer,
            reduction_telemetry,
            analysis_pending: false,
            resource_admission,
        })
    }

    /// Which monitor index this duplicator is bound to.
    #[must_use]
    pub const fn monitor(&self) -> usize {
        self.monitor
    }

    /// Stable id of the currently selected output.
    #[must_use]
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Whether this session is bound to the current primary output.
    #[must_use]
    pub const fn is_primary(&self) -> bool {
        self.primary
    }

    /// Monotonic generation of the topology used to open this output.
    #[must_use]
    pub const fn topology_generation(&self) -> u64 {
        self.topology_generation
    }

    /// Native (pre-subsample) desktop dimensions.
    #[must_use]
    pub const fn native_extent(&self) -> (u32, u32) {
        (self.native_width, self.native_height)
    }

    /// Logical desktop dimensions after applying the pending display rotation.
    #[must_use]
    pub const fn logical_extent(&self) -> (u32, u32) {
        (self.logical_width, self.logical_height)
    }

    /// Monotonic generation of the live Desktop Duplication interface.
    #[must_use]
    pub const fn duplication_generation(&self) -> u64 {
        self.duplication_generation
    }

    /// Physical DXGI adapter that owns this capture session.
    #[must_use]
    pub const fn adapter_luid(&self) -> GpuAdapterLuid {
        self.adapter_luid
    }

    /// Current local-memory headroom reported by DXGI for this adapter.
    ///
    /// # Errors
    ///
    /// Returns a typed Windows capture error when the device cannot expose a
    /// DXGI 1.4 adapter budget or the live budget query fails.
    pub fn available_gpu_memory_bytes(&self) -> CaptureResult<u64> {
        let dxgi_device = self
            .device
            .cast::<IDXGIDevice>()
            .map_err(|error| CaptureError::windows("query capture DXGI device", error))?;
        // SAFETY: the live D3D11 device owns the returned adapter reference.
        let adapter = unsafe { dxgi_device.GetAdapter() }
            .map_err(|error| CaptureError::windows("query capture DXGI adapter", error))?;
        let adapter = adapter
            .cast::<IDXGIAdapter3>()
            .map_err(|error| CaptureError::windows("query capture DXGI 1.4 adapter", error))?;
        let mut memory = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        // SAFETY: the output pointer references initialized writable storage
        // for the duration of the synchronous adapter query.
        unsafe { adapter.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut memory) }
            .map_err(|error| CaptureError::windows("query capture GPU memory budget", error))?;
        Ok(memory.Budget.saturating_sub(memory.CurrentUsage))
    }

    fn prepare_gpu_pointer_resource(
        &mut self,
        plan: &PreparedGpuSurfacePlan,
        clean: &RetainedDesktop,
    ) -> CaptureResult<()> {
        if !plan.requires_pointer_for_next_publication() || !clean.metadata.pointer.visible {
            return Ok(());
        }
        let available_bytes = self.available_gpu_memory_bytes()?;
        gpu_surface::ensure_pointer_resource(
            &self.device,
            &mut self.gpu_pointer,
            &clean.metadata.pointer,
            available_bytes,
            self.resource_admission.as_ref(),
        )
    }

    fn publish_acquired_clean<F>(
        &mut self,
        clean: &RetainedDesktop,
        gpu: Option<&mut PreparedGpuSurfacePlan>,
        cpu: Option<&mut PreparedCpuDesktopReadback>,
        report: &mut CapturePumpReport,
        mut emit: F,
    ) where
        F: FnMut(GpuSurfacePublishOutcome) -> GpuSurfacePublicationDisposition,
    {
        if let Some(plan) = gpu {
            let pointer_result = self.prepare_gpu_pointer_resource(plan, clean);
            report.gpu = match pointer_result.and_then(|()| {
                plan.publish_with_feedback(
                    clean,
                    self.gpu_pointer.as_ref(),
                    self.duplication_generation,
                    &mut emit,
                )
            }) {
                Ok(info) => CaptureLane::Ready(info),
                Err(error) => CaptureLane::Failed(error),
            };
        }
        advance_cpu_clean(clean, cpu, &mut report.cpu);
    }

    /// Virtual-desktop origin of the captured output.
    #[must_use]
    pub const fn origin(&self) -> (i32, i32) {
        (self.origin_x, self.origin_y)
    }

    /// Display transform still pending on native scanout pixels.
    #[must_use]
    pub const fn rotation(&self) -> DisplayRotation {
        self.rotation
    }

    /// DXGI color space attached to native scanout pixels.
    #[must_use]
    pub const fn source_color_space(&self) -> GpuSurfaceSourceColorSpace {
        self.source_color_space
    }

    /// Requested reduction extent for subsequent frames.
    #[must_use]
    pub const fn requested_extent(&self) -> CaptureExtent {
        self.requested_extent
    }

    /// Change the requested reduction extent for subsequent frames.
    pub fn set_requested_extent(&mut self, requested_extent: CaptureExtent) {
        if self.requested_extent == requested_extent {
            return;
        }
        self.requested_extent = requested_extent;
        self.refresh_latest_capture();
    }

    /// Select a native scanout rectangle for subsequent reductions.
    ///
    /// Passing `None` restores full-output capture. A changed region is
    /// re-reduced from the retained clean desktop even while the display is
    /// static.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Windows`] when the region is outside the active
    /// native scanout extent.
    pub fn set_region(&mut self, region: Option<CaptureRegion>) -> CaptureResult<()> {
        if region.is_some_and(|region| !region.fits_within(self.native_width, self.native_height)) {
            return Err(CaptureError::windows(
                "configure desktop capture region",
                "capture region is outside the active scanout extent",
            ));
        }
        if self.region == region {
            return Ok(());
        }
        self.region = region;
        self.refresh_latest_capture();
        Ok(())
    }

    /// Current reduction implementation, health issue, and throughput totals.
    #[must_use]
    pub fn reduction_telemetry(&self) -> ReductionTelemetry {
        self.reduction_telemetry.clone()
    }

    fn effective_region(&self) -> CaptureRegion {
        self.region
            .unwrap_or_else(|| CaptureRegion::full(self.native_width, self.native_height))
    }

    fn new_capture_metadata(&mut self, captured_at: Instant) -> CaptureMetadata {
        self.capture_sequence = self.capture_sequence.wrapping_add(1).max(1);
        let cursor = self
            .pointer
            .cursor_info(self.native_width, self.native_height, self.rotation);
        CaptureMetadata {
            source_id: Arc::clone(&self.source_id),
            topology_generation: self.topology_generation,
            sequence: self.capture_sequence,
            captured_at,
            cursor,
            pointer: self.pointer.clone(),
            source_width: self.native_width,
            source_height: self.native_height,
            origin_x: self.origin_x,
            origin_y: self.origin_y,
            rotation: self.rotation,
            source_color_space: self.source_color_space,
            region: self.effective_region(),
        }
    }

    fn refresh_latest_capture(&mut self) {
        if self.clean_desktop.is_none() {
            return;
        }
        let metadata = self.new_capture_metadata(Instant::now());
        if let Some(clean) = self.clean_desktop.as_mut() {
            clean.metadata = metadata.clone();
        }
        self.latest_capture = Some(metadata);
        self.analysis_pending = true;
    }

    fn update_pointer(&mut self, frame_info: &DXGI_OUTDUPL_FRAME_INFO) -> CaptureResult<bool> {
        if frame_info.LastMouseUpdateTime == 0 {
            return Ok(false);
        }

        let mut next_pointer = self.pointer.clone();
        next_pointer.visible = frame_info.PointerPosition.Visible.as_bool();
        next_pointer.position_x = frame_info.PointerPosition.Position.x;
        next_pointer.position_y = frame_info.PointerPosition.Position.y;

        if frame_info.PointerShapeBufferSize == 0 {
            self.pointer = next_pointer;
            return Ok(true);
        }
        let buffer_size = frame_info.PointerShapeBufferSize as usize;
        if buffer_size > MAX_POINTER_SHAPE_BYTES {
            return Err(CaptureError::windows(
                "read desktop pointer shape",
                format_args!(
                    "shape buffer is {buffer_size} bytes; limit is {MAX_POINTER_SHAPE_BYTES}"
                ),
            ));
        }

        let retained_bytes =
            u64::try_from(buffer_size).map_err(|_| CaptureError::ResourceExhausted {
                operation: "reserve desktop pointer shape",
                requested_bytes: buffer_size,
            })?;
        let reservation = reserve_capture_resource(
            self.resource_admission.as_ref(),
            CaptureResourceKind::PointerShape,
            retained_bytes,
            "reserve desktop pointer shape",
        )?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(buffer_size)
            .map_err(|_| CaptureError::ResourceExhausted {
                operation: "read desktop pointer shape",
                requested_bytes: buffer_size,
            })?;
        bytes.resize(buffer_size, 0);
        let mut bytes = bytes.into_boxed_slice();
        let mut required = 0_u32;
        let mut info = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
        // SAFETY: bytes owns `buffer_size` writable bytes and both out-params
        // are live locals. The duplication interface is held by self.
        unsafe {
            self.duplication
                .as_ref()
                .expect("pointer updates require a live duplication interface")
                .GetFramePointerShape(
                    frame_info.PointerShapeBufferSize,
                    bytes.as_mut_ptr().cast(),
                    &mut required,
                    &mut info,
                )
        }
        .map_err(|source| classify_windows_error("read desktop pointer shape", source))?;
        let required = required as usize;
        if required > bytes.len() {
            return Err(CaptureError::windows(
                "read desktop pointer shape",
                "pointer shape grew beyond the advertised buffer",
            ));
        }
        let kind = if info.Type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0.cast_unsigned() {
            PointerShapeKind::Color
        } else if info.Type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0.cast_unsigned() {
            PointerShapeKind::Monochrome
        } else if info.Type
            == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR
                .0
                .cast_unsigned()
        {
            PointerShapeKind::MaskedColor
        } else {
            return Err(CaptureError::windows(
                "read desktop pointer shape",
                format_args!("unsupported pointer shape type {}", info.Type),
            ));
        };
        let resource_lease =
            commit_capture_resource(reservation, retained_bytes, "commit desktop pointer shape")?;
        let shape = Arc::new(PointerShape {
            kind,
            width: info.Width,
            height: info.Height,
            pitch: info.Pitch,
            hotspot_x: info.HotSpot.x,
            hotspot_y: info.HotSpot.y,
            bytes,
            _resource_lease: Some(resource_lease),
        });
        shape
            .validate_written_bytes(required)
            .map_err(|message| CaptureError::windows("validate desktop pointer shape", message))?;
        next_pointer.shape = Some(shape);
        next_pointer.shape_generation = next_pointer.shape_generation.wrapping_add(1).max(1);
        self.pointer = next_pointer;
        Ok(true)
    }

    /// Prepare descriptor-keyed shareable textures without changing the active
    /// capture or CPU readback path.
    ///
    /// # Errors
    ///
    /// Rejects unsupported exact semantics, duplicate identities, source-region
    /// mismatches, byte-budget overflow, allocation failure, and devices without
    /// explicit shared-fence synchronization.
    pub fn prepare_gpu_surface_plan(
        &self,
        plan_generation: GpuSurfacePlanGeneration,
        descriptors: &[GpuSurfaceDescriptor],
        admission: GpuSurfaceAdmission,
    ) -> CaptureResult<PreparedGpuSurfacePlan> {
        PreparedGpuSurfacePlan::prepare(
            &self.device,
            &self.context,
            plan_generation,
            Arc::clone(&self.source_id),
            self.topology_generation,
            self.duplication_generation,
            self.adapter_luid,
            CaptureExtent::try_new(self.native_width, self.native_height)?,
            CaptureExtent::try_new(self.logical_width, self.logical_height)?,
            self.rotation,
            self.source_color_space,
            descriptors,
            admission,
        )
    }

    /// Allocate a fixed-capacity asynchronous native BGRA readback lane.
    ///
    /// The slot count controls in-flight ownership, not source resolution.
    /// Each slot retains one GPU staging texture and one pooled CPU plane.
    ///
    /// # Errors
    ///
    /// Returns typed geometry or allocation failures without reducing the
    /// requested native extent.
    pub fn prepare_cpu_desktop_readback(
        &self,
        slot_count: NonZeroU32,
    ) -> CaptureResult<PreparedCpuDesktopReadback> {
        PreparedCpuDesktopReadback::prepare(
            &self.device,
            &self.context,
            Arc::clone(&self.source_id),
            self.topology_generation,
            self.duplication_generation,
            self.adapter_luid,
            CaptureExtent::try_new(self.native_width, self.native_height)?,
            self.rotation,
            self.source_color_space,
            slot_count,
        )
    }

    /// Allocate immutable descriptor-keyed GPU reduction and readback rings.
    ///
    /// Each supported physical descriptor owns a fixed output UAV and a fixed
    /// asynchronous staging ring. Native full-frame staging remains separate
    /// and is unnecessary when every physical route is admitted here.
    ///
    /// # Errors
    ///
    /// Returns typed descriptor, geometry, device, or allocation failures
    /// without reducing resolution, cadence, or requested semantics.
    pub fn prepare_gpu_reduction_plan(
        &self,
        plan_generation: GpuSurfacePlanGeneration,
        descriptors: &[GpuSurfaceDescriptor],
        admission: GpuReductionAdmission,
    ) -> CaptureResult<PreparedGpuReductionPlan> {
        PreparedGpuReductionPlan::prepare(
            &self.device,
            &self.context,
            plan_generation,
            Arc::clone(&self.source_id),
            self.topology_generation,
            self.duplication_generation,
            self.adapter_luid,
            CaptureExtent::try_new(self.native_width, self.native_height)?,
            CaptureExtent::try_new(self.logical_width, self.logical_height)?,
            self.rotation,
            self.source_color_space,
            descriptors,
            admission,
        )
    }

    /// Acquire once and independently advance exact GPU and native CPU lanes.
    ///
    /// CPU staging is never created, copied, queried, or mapped unless the
    /// request contains a CPU lane. A lane failure is returned in that lane so
    /// the healthy sibling can still publish from the same acquisition.
    ///
    /// # Errors
    ///
    /// Returns only acquisition or session failures. Consumer validation,
    /// resource pressure, and execution failures remain lane-local.
    pub fn pump<F>(
        &mut self,
        request: CapturePumpRequest<'_>,
        timeout: Duration,
        mut emit: F,
    ) -> CaptureResult<CapturePumpReport>
    where
        F: FnMut(GpuSurfacePublishOutcome),
    {
        self.pump_with_feedback(request, timeout, |outcome| {
            emit(outcome);
            GpuSurfacePublicationDisposition::Accepted
        })
    }

    /// Acquire once and retain native retry state until downstream acceptance.
    ///
    /// The feedback callback runs after GPU submission. Returning `Retry`
    /// preserves that route's exact source sequence for the next pump without
    /// affecting healthy siblings or allocating a side queue.
    pub fn pump_with_feedback<F>(
        &mut self,
        request: CapturePumpRequest<'_>,
        timeout: Duration,
        emit: F,
    ) -> CaptureResult<CapturePumpReport>
    where
        F: FnMut(GpuSurfacePublishOutcome) -> GpuSurfacePublicationDisposition,
    {
        self.pump_with_reduction_feedback(request, timeout, emit, |_| {
            GpuReductionPublicationDisposition::Accepted
        })
    }

    /// Advance native Surface, descriptor-keyed reduction, and optional full
    /// native readback lanes from one retained Desktop Duplication acquisition.
    ///
    /// Completed reduced bytes remain owned by their immutable descriptor ring
    /// until the reduction callback accepts them. Failures stay lane-local so
    /// supported and fallback physical routes remain independent.
    pub fn pump_with_reduction_feedback<F, R>(
        &mut self,
        mut request: CapturePumpRequest<'_>,
        timeout: Duration,
        mut emit: F,
        mut emit_reduction: R,
    ) -> CaptureResult<CapturePumpReport>
    where
        F: FnMut(GpuSurfacePublishOutcome) -> GpuSurfacePublicationDisposition,
        R: FnMut(GpuReductionPublishOutcome<'_>) -> GpuReductionPublicationDisposition,
    {
        self.release_frame();

        let mut gpu = request.gpu.take();
        let mut gpu_lane = if gpu.is_some() {
            CaptureLane::Idle
        } else {
            CaptureLane::NotRequested
        };
        if let Some(plan) = gpu.as_deref_mut()
            && let Err(error) = self.validate_gpu_surface_plan(plan)
        {
            gpu_lane = CaptureLane::Failed(error);
            gpu = None;
        }

        let mut reduction = request.reduction.take();
        let mut reduction_lane = if let Some(plan) = reduction.as_deref_mut() {
            if let Err(error) = self.validate_gpu_reduction_plan(plan) {
                reduction = None;
                CaptureLane::Failed(error)
            } else {
                match plan.poll_with_feedback(&mut emit_reduction) {
                    Ok(info) => CaptureLane::Ready(info),
                    Err(error) => {
                        reduction = None;
                        CaptureLane::Failed(error)
                    }
                }
            }
        } else {
            CaptureLane::NotRequested
        };

        let mut cpu = request.cpu.take();
        let mut cpu_lane = if let Some(readback) = cpu.as_deref_mut() {
            if self.validate_cpu_readback(readback) {
                readback.poll()
            } else {
                cpu = None;
                CaptureLane::Failed(CaptureError::GpuSurfacePlanInvalidated)
            }
        } else {
            CaptureLane::NotRequested
        };
        if matches!(cpu_lane, CaptureLane::Failed(_)) {
            cpu = None;
        }

        if gpu.is_none() && reduction.is_none() && cpu.is_none() {
            return Ok(CapturePumpReport {
                acquired: false,
                gpu: gpu_lane,
                reduction: reduction_lane,
                cpu: cpu_lane,
            });
        }

        let gpu_pending = gpu
            .as_deref()
            .is_some_and(PreparedGpuSurfacePlan::has_pending_routes);
        let cpu_pending = cpu
            .as_deref()
            .is_some_and(PreparedCpuDesktopReadback::has_pending);
        let reduction_pending = reduction
            .as_deref()
            .is_some_and(PreparedGpuReductionPlan::has_pending_routes);
        let return_ready = matches!(cpu_lane, CaptureLane::Ready(_) | CaptureLane::Busy)
            || matches!(reduction_lane, CaptureLane::Ready(_));
        let acquire_timeout = gpu_surface_acquire_timeout(
            timeout,
            gpu_pending || reduction_pending || cpu_pending || return_ready,
        );
        let update = self.acquire_native_update(acquire_timeout)?;
        self.release_frame();
        let acquired = update.is_some();

        if let Some(plan) = gpu.as_deref_mut()
            && let Err(error) = self.validate_gpu_surface_plan(plan)
        {
            gpu_lane = CaptureLane::Failed(error);
            gpu = None;
        }
        if let Some(readback) = cpu.as_deref_mut()
            && !self.validate_cpu_readback(readback)
        {
            cpu_lane = CaptureLane::Failed(CaptureError::GpuSurfacePlanInvalidated);
            cpu = None;
        }
        if let Some(plan) = reduction.as_deref_mut()
            && let Err(error) = self.validate_gpu_reduction_plan(plan)
        {
            reduction_lane = CaptureLane::Failed(error);
            reduction = None;
        }

        let clean = self.clean_desktop.clone();
        let mut report = CapturePumpReport {
            acquired,
            gpu: gpu_lane,
            reduction: reduction_lane,
            cpu: cpu_lane,
        };
        if acquired {
            let clean = clean.as_ref().ok_or_else(|| {
                CaptureError::windows(
                    "fan out desktop capture",
                    "acquisition produced no retained clean desktop",
                )
            })?;
            if reduction
                .as_deref()
                .is_some_and(PreparedGpuReductionPlan::requires_pointer_for_next_publication)
                && clean.metadata.pointer.visible
            {
                let available_bytes = self.available_gpu_memory_bytes()?;
                gpu_surface::ensure_pointer_resource(
                    &self.device,
                    &mut self.gpu_pointer,
                    &clean.metadata.pointer,
                    available_bytes,
                    self.resource_admission.as_ref(),
                )?;
            }
            self.publish_acquired_clean(
                clean,
                gpu.as_deref_mut(),
                cpu.as_deref_mut(),
                &mut report,
                &mut emit,
            );
            if let Some(plan) = reduction {
                match plan.submit_selected(
                    clean,
                    self.gpu_pointer.as_ref(),
                    self.duplication_generation,
                ) {
                    Ok(submitted) => match &mut report.reduction {
                        CaptureLane::Ready(info) => info.merge(submitted),
                        lane => *lane = CaptureLane::Ready(submitted),
                    },
                    Err(error) => report.reduction = CaptureLane::Failed(error),
                }
            }
        } else {
            if let (Some(plan), Some(clean)) = (gpu, clean.as_ref())
                && plan.has_pending_routes()
            {
                let pointer_result = self.prepare_gpu_pointer_resource(plan, clean);
                report.gpu = match pointer_result.and_then(|()| {
                    plan.retry_pending_with_feedback(clean, self.gpu_pointer.as_ref(), &mut emit)
                }) {
                    Ok(info) => CaptureLane::Ready(info),
                    Err(error) => CaptureLane::Failed(error),
                };
            }
            if let (Some(plan), Some(clean)) = (reduction, clean.as_ref()) {
                if plan.requires_pointer_for_next_publication() && clean.metadata.pointer.visible {
                    let available_bytes = self.available_gpu_memory_bytes()?;
                    gpu_surface::ensure_pointer_resource(
                        &self.device,
                        &mut self.gpu_pointer,
                        &clean.metadata.pointer,
                        available_bytes,
                        self.resource_admission.as_ref(),
                    )?;
                }
                match plan.submit_selected(
                    clean,
                    self.gpu_pointer.as_ref(),
                    self.duplication_generation,
                ) {
                    Ok(submitted) => match &mut report.reduction {
                        CaptureLane::Ready(info) => info.merge(submitted),
                        lane => *lane = CaptureLane::Ready(submitted),
                    },
                    Err(error) => report.reduction = CaptureLane::Failed(error),
                }
            }
        }

        if !acquired && let Some(clean) = clean.as_ref() {
            advance_cpu_clean(clean, cpu, &mut report.cpu);
        }

        Ok(report)
    }

    /// Acquire one native desktop update and fan it out into every exact GPU
    /// Surface descriptor without staging any result through CPU memory.
    ///
    /// `Ok(None)` means the desktop and pointer remained static through the
    /// timeout. Busy descriptors are reported independently in the returned
    /// callback; healthy descriptor slots continue publishing. The callback
    /// executes in canonical descriptor order without a per-frame batch
    /// allocation.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::GpuSurfacePlanInvalidated`] when topology or the
    /// duplication session no longer matches the prepared plan. Other errors
    /// preserve the same typed capture, resource, and device failures as
    /// [`Self::next_frame`].
    pub fn next_gpu_surfaces<F>(
        &mut self,
        plan: &mut PreparedGpuSurfacePlan,
        timeout: Duration,
        emit: F,
    ) -> CaptureResult<Option<GpuSurfaceBatchInfo>>
    where
        F: FnMut(GpuSurfacePublishOutcome),
    {
        let report = self.pump(CapturePumpRequest::gpu(plan), timeout, emit)?;
        match report.gpu {
            CaptureLane::Ready(info) => Ok(Some(info)),
            CaptureLane::Idle | CaptureLane::Busy | CaptureLane::NotRequested => Ok(None),
            CaptureLane::Failed(error) => Err(error),
        }
    }

    fn validate_gpu_surface_plan(&self, plan: &PreparedGpuSurfacePlan) -> CaptureResult<()> {
        plan.validate_source(
            &self.source_id,
            self.topology_generation,
            self.duplication_generation,
            self.adapter_luid,
            CaptureExtent::try_new(self.native_width, self.native_height)?,
            CaptureExtent::try_new(self.logical_width, self.logical_height)?,
            self.rotation,
            self.source_color_space,
        )
    }

    fn validate_gpu_reduction_plan(&self, plan: &PreparedGpuReductionPlan) -> CaptureResult<()> {
        plan.validate_source(
            &self.source_id,
            self.topology_generation,
            self.duplication_generation,
            self.adapter_luid,
            CaptureExtent::try_new(self.native_width, self.native_height)?,
            CaptureExtent::try_new(self.logical_width, self.logical_height)?,
            self.rotation,
            self.source_color_space,
        )
    }

    fn validate_cpu_readback(&self, readback: &PreparedCpuDesktopReadback) -> bool {
        CaptureExtent::try_new(self.native_width, self.native_height).is_ok_and(|source_extent| {
            readback.matches_source(
                &self.source_id,
                self.topology_generation,
                self.duplication_generation,
                self.adapter_luid,
                source_extent,
                self.rotation,
                self.source_color_space,
            )
        })
    }

    fn acquire_native_update(
        &mut self,
        timeout: Duration,
    ) -> CaptureResult<Option<NativeCaptureUpdate>> {
        if self.duplication.is_none() {
            self.rebuild()?;
            return Ok(None);
        }

        if self.last_topology_check.elapsed() >= TOPOLOGY_CHECK_INTERVAL {
            self.last_topology_check = Instant::now();
            let outputs = enumerate_outputs()?;
            let monitors = outputs
                .iter()
                .map(|entry| entry.monitor.clone())
                .collect::<Vec<_>>();
            let selected = self.selector.resolve(&monitors)?;
            if selected.id != self.source_id.as_ref()
                || selected.topology_generation != self.topology_generation
            {
                debug!(
                    source_id = %selected.id,
                    topology_generation = selected.topology_generation,
                    "desktop topology changed; rebuilding capture session"
                );
                self.rebuild()?;
                return Ok(None);
            }
        }

        let timeout_ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        // SAFETY: both out-params are owned locals living past the call, and
        // the duplication interface is kept alive by `self`.
        let acquire = unsafe {
            self.duplication
                .as_ref()
                .expect("frame acquisition requires a live duplication interface")
                .AcquireNextFrame(timeout_ms, &mut frame_info, &mut resource)
        };
        if let Err(error) = acquire {
            return match classify_windows_error("acquire duplicated frame", error) {
                CaptureError::Timeout => Ok(None),
                CaptureError::AccessLost => {
                    debug!("desktop duplication access lost; rebuilding session");
                    self.rebuild()?;
                    Ok(None)
                }
                error => Err(error),
            };
        }
        self.frame_held = true;
        let captured_at = Instant::now();

        let (current_width, current_height, current_rotation) = duplication_geometry(
            self.duplication
                .as_ref()
                .expect("geometry checks require a live duplication interface"),
        );
        let (current_origin_x, current_origin_y) = match output_origin(&self.output) {
            Ok(origin) => origin,
            Err(error) => {
                self.release_frame();
                return Err(error);
            }
        };
        let current_color_space = output_color_space(&self.output);
        if current_width != self.logical_width
            || current_height != self.logical_height
            || current_origin_x != self.origin_x
            || current_origin_y != self.origin_y
            || current_rotation != self.rotation
            || current_color_space != self.source_color_space
        {
            self.release_frame();
            self.rebuild()?;
            return Ok(None);
        }

        let pointer_updated = match self.update_pointer(&frame_info) {
            Ok(updated) => updated,
            Err(CaptureError::AccessLost) => {
                debug!(
                    operation = "pointer update",
                    "desktop duplication access lost; rebuilding session"
                );
                self.rebuild()?;
                return Ok(None);
            }
            Err(error) => {
                self.release_frame();
                return Err(error);
            }
        };
        let desktop_updated = frame_info.LastPresentTime != 0;
        if !desktop_updated && !pointer_updated {
            self.release_frame();
            return Ok(None);
        }
        let metadata = self.new_capture_metadata(captured_at);
        self.latest_capture = Some(metadata.clone());
        self.analysis_pending = true;
        let texture = match desktop_frame_source(desktop_updated, self.clean_desktop.is_some()) {
            DesktopFrameSource::AcquiredResource => {
                let Some(resource) = resource else {
                    self.release_frame();
                    return Ok(None);
                };
                match resource.cast::<ID3D11Texture2D>() {
                    Ok(texture) => Some(texture),
                    Err(source) => {
                        self.release_frame();
                        return Err(CaptureError::windows("query duplicated texture", source));
                    }
                }
            }
            DesktopFrameSource::RetainedStaging => None,
        };
        if let Some(texture) = texture.as_ref() {
            if let Err(error) = self.retain_desktop(texture, metadata.clone()) {
                self.release_frame();
                return Err(error);
            }
        } else if let Some(clean) = self.clean_desktop.as_mut() {
            clean.metadata = metadata.clone();
        }
        Ok(Some(NativeCaptureUpdate { metadata }))
    }

    /// Wait up to `timeout` for the next desktop frame.
    ///
    /// Returns `Ok(None)` when nothing new arrived, which is the common and
    /// cheap case: DXGI reports a timeout whenever both desktop and pointer
    /// state are static.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Windows`] when acquiring, copying, or mapping
    /// the frame fails for a reason that is not recoverable in place. Access
    /// loss is handled internally by rebuilding the duplication interface.
    pub fn next_frame(&mut self, timeout: Duration) -> CaptureResult<Option<Frame>> {
        self.release_frame();
        self.ensure_analysis_reducer()?;

        let mut ready = self.poll_gpu_frame()?;
        if self.analysis_pending && self.gpu_reducer.is_ready() {
            if let Some(metadata) = self.latest_capture.clone() {
                self.submit_gpu(metadata)?;
            }
            if ready.is_none() {
                ready = self.poll_gpu_frame()?;
            }
        }
        if self.analysis_pending && self.gpu_reducer.is_disabled() {
            ready = self.cpu_frame_from_retained()?;
            self.analysis_pending = false;
        }

        let acquire_timeout = if ready.is_some() {
            Duration::ZERO
        } else {
            timeout
        };
        let Some(NativeCaptureUpdate { metadata }) = self.acquire_native_update(acquire_timeout)?
        else {
            return Ok(ready);
        };

        if self.gpu_reducer.is_ready() {
            self.analysis_pending = true;
            self.submit_gpu(metadata)?;
        }
        self.release_frame();

        if self.gpu_reducer.is_disabled() {
            self.analysis_pending = false;
            return self.cpu_frame_from_retained();
        }
        if ready.is_none() {
            ready = self.poll_gpu_frame()?;
        }
        Ok(ready)
    }

    fn ensure_analysis_reducer(&mut self) -> CaptureResult<()> {
        if !matches!(self.gpu_reducer, AnalysisGpuReducer::Uninitialized) {
            return Ok(());
        }
        match GpuReducer::new(
            &self.device,
            &self.context,
            Arc::clone(&self.resource_admission),
        ) {
            Ok(reducer) => {
                self.gpu_reducer = AnalysisGpuReducer::Ready(reducer);
                Ok(())
            }
            Err(error) => {
                if let Some(capture_error) = error.as_capture_error() {
                    return Err(capture_error);
                }
                self.degrade_gpu(error.to_string());
                Ok(())
            }
        }
    }

    fn submit_gpu(&mut self, metadata: CaptureMetadata) -> CaptureResult<()> {
        let clean = self.clean_desktop.clone().ok_or_else(|| {
            CaptureError::windows(
                "submit desktop reduction",
                "no retained clean desktop is available",
            )
        })?;
        if metadata.pointer.visible {
            let available_bytes = self.available_gpu_memory_bytes()?;
            gpu_surface::ensure_pointer_resource(
                &self.device,
                &mut self.gpu_pointer,
                &metadata.pointer,
                available_bytes,
                self.resource_admission.as_ref(),
            )?;
        }
        let Some(reducer) = self.gpu_reducer.ready_mut() else {
            return Ok(());
        };
        match reducer.submit(
            &clean,
            self.gpu_pointer.as_ref(),
            self.requested_extent,
            metadata,
        ) {
            Ok(SubmitOutcome::Submitted) => {
                self.reduction_telemetry.gpu_submitted =
                    self.reduction_telemetry.gpu_submitted.saturating_add(1);
                self.analysis_pending = false;
            }
            Ok(SubmitOutcome::Busy) => {
                self.reduction_telemetry.ring_busy =
                    self.reduction_telemetry.ring_busy.saturating_add(1);
            }
            Err(error) => {
                if let Some(capture_error) = error.as_capture_error() {
                    return Err(capture_error);
                }
                self.degrade_gpu(error.to_string());
                self.analysis_pending = true;
            }
        }
        Ok(())
    }

    fn poll_gpu_frame(&mut self) -> CaptureResult<Option<Frame>> {
        let Some(reducer) = self.gpu_reducer.ready_mut() else {
            return Ok(None);
        };
        let Some(output_len) = reducer.output_byte_len().map_err(|error| {
            error
                .as_capture_error()
                .unwrap_or_else(|| CaptureError::windows("quote analysis reduction frame", error))
        })?
        else {
            return Ok(None);
        };
        let mut plane = take_frame_plane(
            &self.frame_pool,
            self.resource_admission.as_ref(),
            output_len,
        )?;
        plane.rgba.resize(output_len, 0);
        let result = reducer.poll_preallocated(&mut plane.rgba);
        match result {
            Ok(Some(reduced)) => {
                self.reduction_telemetry.gpu_completed =
                    self.reduction_telemetry.gpu_completed.saturating_add(1);
                self.reduction_telemetry.readback_bytes = self
                    .reduction_telemetry
                    .readback_bytes
                    .saturating_add(reduced.bytes as u64);
                Ok(Some(self.frame_from_reduction(reduced, plane)))
            }
            Ok(None) => {
                self.recycle_plane(plane);
                Ok(None)
            }
            Err(error) => {
                if let Some(capture_error) = error.as_capture_error() {
                    self.recycle_plane(plane);
                    return Err(capture_error);
                }
                self.recycle_plane(plane);
                self.degrade_gpu(error.to_string());
                self.analysis_pending = true;
                let frame = self.cpu_frame_from_retained()?;
                self.analysis_pending = false;
                Ok(frame)
            }
        }
    }

    fn cpu_frame_from_retained(&mut self) -> CaptureResult<Option<Frame>> {
        let Some(output_len) = self.analysis_output_byte_len()? else {
            return Ok(None);
        };
        let mut plane = take_frame_plane(
            &self.frame_pool,
            self.resource_admission.as_ref(),
            output_len,
        )?;
        match self.read_back(&mut plane.rgba) {
            Ok(Some((width, height, metadata))) => {
                self.reduction_telemetry.cpu_completed =
                    self.reduction_telemetry.cpu_completed.saturating_add(1);
                Ok(Some(
                    self.frame_from_metadata(metadata, width, height, plane),
                ))
            }
            Ok(None) => {
                self.recycle_plane(plane);
                Ok(None)
            }
            Err(error) => {
                self.recycle_plane(plane);
                Err(error)
            }
        }
    }

    fn analysis_output_byte_len(&self) -> CaptureResult<Option<usize>> {
        let Some(clean) = self.clean_desktop.as_ref() else {
            return Ok(None);
        };
        let region = clean.metadata.region;
        let stride =
            subsample_stride_within(region.width(), region.height(), self.requested_extent);
        let width = subsampled_extent(region.width(), stride);
        let height = subsampled_extent(region.height(), stride);
        checked_rgba_len(width, height, "reserve RGBA capture plane").map(Some)
    }

    fn frame_from_reduction(&self, reduced: ReducedFrame, plane: RgbaFramePlane) -> Frame {
        self.frame_from_metadata(reduced.metadata, reduced.width, reduced.height, plane)
    }

    fn frame_from_metadata(
        &self,
        metadata: CaptureMetadata,
        width: u32,
        height: u32,
        plane: RgbaFramePlane,
    ) -> Frame {
        let (origin_x, origin_y) = capture_region_origin(&metadata);
        let cursor = region_cursor(metadata.cursor, metadata.region);
        Frame::new(
            metadata.source_id,
            metadata.topology_generation,
            metadata.sequence,
            metadata.captured_at,
            cursor,
            width,
            height,
            metadata.region.width(),
            metadata.region.height(),
            origin_x,
            origin_y,
            metadata.rotation,
            plane.rgba,
            plane.resource_lease,
            Arc::clone(&self.frame_pool),
        )
    }

    fn recycle_plane(&self, mut plane: RgbaFramePlane) {
        plane.rgba.clear();
        recycle_rgba_frame_plane(&self.frame_pool, plane);
    }

    fn degrade_gpu(&mut self, issue: String) {
        self.gpu_reducer = AnalysisGpuReducer::Disabled;
        self.reduction_telemetry.path = ReductionPath::CpuFallback;
        self.reduction_telemetry.gpu_failures =
            self.reduction_telemetry.gpu_failures.saturating_add(1);
        self.reduction_telemetry.issue = Some(issue.into());
        warn!(
            reduction_path = ?self.reduction_telemetry.path,
            gpu_submitted = self.reduction_telemetry.gpu_submitted,
            gpu_completed = self.reduction_telemetry.gpu_completed,
            cpu_completed = self.reduction_telemetry.cpu_completed,
            ring_busy = self.reduction_telemetry.ring_busy,
            readback_bytes = self.reduction_telemetry.readback_bytes,
            gpu_failures = self.reduction_telemetry.gpu_failures,
            issue = %self.reduction_telemetry.issue.as_deref().unwrap_or("unknown"),
            "GPU capture reduction unavailable; using CPU fallback"
        );
    }

    fn retain_desktop(
        &mut self,
        texture: &ID3D11Texture2D,
        metadata: CaptureMetadata,
    ) -> CaptureResult<()> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: GetDesc fills a caller-owned struct and cannot fail.
        unsafe { texture.GetDesc(&mut desc) };
        if let Some(retained) = self.clean_desktop.as_mut() {
            let mut retained_desc = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: GetDesc fills a caller-owned struct and cannot fail.
            unsafe { retained.texture.GetDesc(&mut retained_desc) };
            if retained_desc.Width == desc.Width
                && retained_desc.Height == desc.Height
                && retained_desc.Format == desc.Format
            {
                // SAFETY: both textures are same-desc 2D textures on this device.
                unsafe { self.context.CopyResource(&retained.texture, texture) };
                retained.metadata = metadata;
                return Ok(());
            }
        }
        let texture_bytes = u64::try_from(checked_rgba_len(
            desc.Width,
            desc.Height,
            "reserve clean desktop texture",
        )?)
        .map_err(|_| CaptureError::ResourceExhausted {
            operation: "reserve clean desktop texture",
            requested_bytes: usize::MAX,
        })?;
        let reservation = reserve_capture_resource(
            self.resource_admission.as_ref(),
            CaptureResourceKind::CanonicalDesktop,
            texture_bytes,
            "reserve clean desktop texture",
        )?;
        let clean = create_clean_texture(&self.device, &desc)?;
        // SAFETY: both textures are same-desc 2D textures on this device.
        unsafe { self.context.CopyResource(&clean, texture) };
        let srv = gpu_reduction::create_srv(&self.device, &clean)
            .map_err(|error| CaptureError::windows("create clean desktop view", error))?;
        let resource_lease =
            commit_capture_resource(reservation, texture_bytes, "commit clean desktop texture")?;
        self.clean_desktop = Some(RetainedDesktop {
            texture: clean,
            srv,
            metadata,
            _resource_lease: Some(resource_lease),
        });
        Ok(())
    }

    /// Map the retained clean desktop, then subsample its configured region.
    fn read_back(
        &mut self,
        rgba: &mut Vec<u8>,
    ) -> CaptureResult<Option<(u32, u32, CaptureMetadata)>> {
        let Some(retained) = self.clean_desktop.as_ref() else {
            return Ok(None);
        };
        let clean = retained.texture.clone();
        let metadata = retained.metadata.clone();
        let native_width = metadata.source_width;
        let native_height = metadata.source_height;

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: GetDesc fills caller-owned storage and cannot fail.
        unsafe { clean.GetDesc(&mut desc) };
        let staging_matches = if let Some(staging) = self.cpu_staging.as_ref() {
            let mut staging_desc = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: GetDesc fills caller-owned storage and cannot fail.
            unsafe { staging.texture.GetDesc(&mut staging_desc) };
            staging_desc.Width == desc.Width
                && staging_desc.Height == desc.Height
                && staging_desc.Format == desc.Format
        } else {
            false
        };
        let staging = if staging_matches {
            self.cpu_staging
                .as_ref()
                .expect("matching staging texture is retained")
                .texture
                .clone()
        } else {
            let texture_bytes = u64::try_from(checked_rgba_len(
                desc.Width,
                desc.Height,
                "reserve analysis staging texture",
            )?)
            .map_err(|_| CaptureError::ResourceExhausted {
                operation: "reserve analysis staging texture",
                requested_bytes: usize::MAX,
            })?;
            let reservation = reserve_capture_resource(
                self.resource_admission.as_ref(),
                CaptureResourceKind::AnalysisCpuStagingTexture,
                texture_bytes,
                "reserve analysis staging texture",
            )?;
            let staging = create_staging_texture(&self.device, &desc)?;
            let resource_lease = commit_capture_resource(
                reservation,
                texture_bytes,
                "commit analysis staging texture",
            )?;
            self.cpu_staging = Some(AdmittedStagingTexture {
                texture: staging.clone(),
                _resource_lease: resource_lease,
            });
            staging
        };
        // SAFETY: both textures have matching geometry and format on this device.
        unsafe { self.context.CopyResource(&staging, &clean) };
        let mapped = MappedTexture::map(&self.context, &staging)?;
        let dimensions = Self::copy_bgra_rows(
            mapped.rows(native_width, native_height)?,
            rgba,
            self.requested_extent,
            &metadata.pointer,
            metadata.rotation,
            metadata.region,
        )?;

        let Some((width, height)) = dimensions else {
            return Ok(None);
        };

        Ok(Some((width, height, metadata)))
    }

    /// Box-filter BGRA staging rows into the packed RGBA output buffer.
    ///
    /// Every source pixel in each stride x stride block is averaged rather
    /// than one being picked. Point sampling is tempting here — the ambilight
    /// sector grid averages the result anyway — but the same buffer is
    /// published as `canvas_downscale` and consumed as an actual image by
    /// screen-reactive effects, then downscaled a second time. Two successive
    /// point samplings of a 4K desktop shred thin text into aliased noise. The
    /// Wayland path never had this problem because PipeWire hands over an
    /// already-filtered frame.
    fn copy_bgra_rows(
        rows: BgraRows<'_>,
        rgba: &mut Vec<u8>,
        requested_extent: CaptureExtent,
        pointer: &PointerState,
        rotation: DisplayRotation,
        region: CaptureRegion,
    ) -> CaptureResult<Option<(u32, u32)>> {
        let BgraRows {
            bytes: source,
            row_pitch,
            width,
            height,
        } = rows;
        let minimum_row_bytes = checked_rgba_len(width, 1, "validate BGRA source rows")?;
        let source_len = row_pitch
            .checked_mul(height as usize)
            .filter(|len| *len <= isize::MAX as usize);
        if row_pitch < minimum_row_bytes || source_len.is_none_or(|len| source.len() < len) {
            return Ok(None);
        }
        if !region.fits_within(width, height) {
            return Ok(None);
        }
        let stride = subsample_stride_within(region.width(), region.height(), requested_extent);
        let out_width = subsampled_extent(region.width(), stride);
        let out_height = subsampled_extent(region.height(), stride);
        let output_len = checked_rgba_len(out_width, out_height, "allocate CPU capture plane")?;
        require_plane_capacity(rgba, output_len, "use admitted CPU capture plane")?;
        rgba.resize(output_len, 0);

        let stride = stride as usize;
        let width = width as usize;
        let height = height as usize;

        for out_y in 0..out_height as usize {
            let dst_row_start = out_y * out_width as usize * BYTES_PER_PIXEL;
            // Blocks on the right and bottom edges are clipped when the
            // desktop does not divide evenly by the stride.
            let src_y0 = region.origin_y() as usize + out_y * stride;
            let src_y1 = (src_y0 + stride).min((region.origin_y() + region.height()) as usize);

            for out_x in 0..out_width as usize {
                let src_x0 = region.origin_x() as usize + out_x * stride;
                let src_x1 = (src_x0 + stride).min((region.origin_x() + region.width()) as usize);

                let mut blue = 0_u64;
                let mut green = 0_u64;
                let mut red = 0_u64;
                let mut samples = 0_u64;

                for src_y in src_y0..src_y1 {
                    let row = src_y * row_pitch;
                    for src_x in src_x0..src_x1 {
                        let src = row + src_x * BYTES_PER_PIXEL;
                        let pixel = pointer.composite_bgra(
                            [
                                source[src],
                                source[src + 1],
                                source[src + 2],
                                source[src + 3],
                            ],
                            src_x as u32,
                            src_y as u32,
                            width as u32,
                            height as u32,
                            rotation,
                        );
                        blue += u64::from(pixel[0]);
                        green += u64::from(pixel[1]);
                        red += u64::from(pixel[2]);
                        samples += 1;
                    }
                }

                let samples = samples.max(1);
                let dst = dst_row_start + out_x * BYTES_PER_PIXEL;
                rgba[dst] = average_channel(red, samples);
                rgba[dst + 1] = average_channel(green, samples);
                rgba[dst + 2] = average_channel(blue, samples);
                rgba[dst + 3] = 0xFF;
            }
        }

        Ok(Some((out_width, out_height)))
    }

    /// Drop the duplication interface and open a fresh one.
    fn rebuild(&mut self) -> CaptureResult<()> {
        self.release_frame();
        let outputs = enumerate_outputs()?;
        let monitors = outputs
            .iter()
            .map(|entry| entry.monitor.clone())
            .collect::<Vec<_>>();
        let selected = self.selector.resolve(&monitors)?.clone();
        let entry = outputs
            .into_iter()
            .find(|entry| entry.monitor.id == selected.id)
            .expect("resolved monitor belongs to enumerated outputs");
        let adapter_luid = adapter_luid(&entry.adapter)?;
        let (device, context) = if adapter_luid == self.adapter_luid {
            (self.device.clone(), self.context.clone())
        } else {
            create_device(&entry.adapter)?
        };
        let output = entry
            .output
            .cast::<IDXGIOutput1>()
            .map_err(|source| CaptureError::windows("query IDXGIOutput1", source))?;
        let (origin_x, origin_y) = output_origin(&output)?;
        let source_color_space = output_color_space(&output);

        let (duplication, (logical_width, logical_height, rotation)) = prepare_duplication(
            &mut self.duplication,
            &mut self.clean_desktop,
            || duplicate_output(&output, &device),
            |duplication| Ok(duplication_geometry(duplication)),
        )
        .map_err(session_rebuild_error)?;
        let (native_width, native_height) =
            native_scanout_extent(logical_width, logical_height, rotation);
        let region = self
            .region
            .filter(|region| region.fits_within(native_width, native_height));

        self.pointer = PointerState::default();
        self.gpu_pointer = None;
        self.device = device;
        self.context = context;
        self.output = output;
        self.duplication = Some(duplication);
        self.clean_desktop = None;
        self.cpu_staging = None;
        self.monitor = entry.monitor.index;
        self.source_id = Arc::from(entry.monitor.id);
        self.primary = entry.monitor.primary;
        self.topology_generation = entry.monitor.topology_generation;
        self.duplication_generation = self.duplication_generation.wrapping_add(1).max(1);
        self.adapter_luid = adapter_luid;
        self.last_topology_check = Instant::now();
        self.logical_width = logical_width;
        self.logical_height = logical_height;
        self.native_width = native_width;
        self.native_height = native_height;
        self.origin_x = origin_x;
        self.origin_y = origin_y;
        self.rotation = rotation;
        self.source_color_space = source_color_space;
        self.region = region;
        self.latest_capture = None;
        self.gpu_reducer = AnalysisGpuReducer::Uninitialized;
        self.analysis_pending = false;
        self.reduction_telemetry.path = ReductionPath::Gpu;
        self.reduction_telemetry.issue = None;
        Ok(())
    }

    /// Release a held frame if there is one. Safe to call unconditionally.
    fn release_frame(&mut self) {
        if !self.frame_held {
            return;
        }
        self.frame_held = false;
        let Some(duplication) = self.duplication.as_ref() else {
            return;
        };
        // SAFETY: paired with a successful AcquireNextFrame.
        if let Err(error) = unsafe { duplication.ReleaseFrame() } {
            // Access loss here is normal during mode changes and is repaired
            // on the next acquire, so this stays a debug line.
            debug!(%error, "releasing duplicated frame failed");
        }
    }
}

fn create_staging_texture(
    device: &ID3D11Device,
    desc: &D3D11_TEXTURE2D_DESC,
) -> CaptureResult<ID3D11Texture2D> {
    let requested_bytes =
        checked_rgba_len(desc.Width, desc.Height, "create retained staging texture")?;
    let staging_desc = D3D11_TEXTURE2D_DESC {
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: u32::try_from(D3D11_CPU_ACCESS_READ.0).unwrap_or_default(),
        MiscFlags: 0,
        ..*desc
    };
    let mut texture = None;
    // SAFETY: staging_desc is valid and the caller-owned out-param is live.
    unsafe { device.CreateTexture2D(&staging_desc, None, Some(&mut texture)) }.map_err(
        |source| {
            if source.code() == E_OUTOFMEMORY {
                CaptureError::ResourceExhausted {
                    operation: "create retained staging texture",
                    requested_bytes,
                }
            } else {
                classify_windows_error("create staging texture", source)
            }
        },
    )?;
    texture.ok_or_else(|| {
        CaptureError::windows(
            "create staging texture",
            "CreateTexture2D returned no texture",
        )
    })
}

fn create_clean_texture(
    device: &ID3D11Device,
    desc: &D3D11_TEXTURE2D_DESC,
) -> CaptureResult<ID3D11Texture2D> {
    let requested_bytes =
        checked_rgba_len(desc.Width, desc.Height, "create clean desktop texture")?;
    let clean_desc = D3D11_TEXTURE2D_DESC {
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0.cast_unsigned(),
        CPUAccessFlags: 0,
        MiscFlags: 0,
        ..*desc
    };
    let mut texture = None;
    // SAFETY: clean_desc is valid and the caller-owned out-param is live.
    unsafe { device.CreateTexture2D(&clean_desc, None, Some(&mut texture)) }.map_err(|source| {
        if source.code() == E_OUTOFMEMORY {
            CaptureError::ResourceExhausted {
                operation: "create clean desktop texture",
                requested_bytes,
            }
        } else {
            classify_windows_error("create clean desktop texture", source)
        }
    })?;
    texture.ok_or_else(|| {
        CaptureError::windows(
            "create clean desktop texture",
            "CreateTexture2D returned no texture",
        )
    })
}

fn average_channel(sum: u64, samples: u64) -> u8 {
    (sum / samples.max(1)) as u8
}

fn capture_region_origin(metadata: &CaptureMetadata) -> (i32, i32) {
    let region = metadata.region;
    let x = i64::from(region.origin_x());
    let y = i64::from(region.origin_y());
    let right = x + i64::from(region.width());
    let bottom = y + i64::from(region.height());
    let source_width = i64::from(metadata.source_width);
    let source_height = i64::from(metadata.source_height);
    let (logical_x, logical_y) = match metadata.rotation {
        DisplayRotation::Identity => (x, y),
        DisplayRotation::Clockwise90 => (source_height - bottom, x),
        DisplayRotation::Clockwise180 => (source_width - right, source_height - bottom),
        DisplayRotation::Clockwise270 => (y, source_width - right),
    };
    (
        saturating_i32(i64::from(metadata.origin_x) + logical_x),
        saturating_i32(i64::from(metadata.origin_y) + logical_y),
    )
}

fn region_cursor(mut cursor: CursorInfo, region: CaptureRegion) -> CursorInfo {
    cursor.position_x = cursor
        .position_x
        .saturating_sub(i32::try_from(region.origin_x()).unwrap_or(i32::MAX));
    cursor.position_y = cursor
        .position_y
        .saturating_sub(i32::try_from(region.origin_y()).unwrap_or(i32::MAX));
    cursor
}

fn saturating_i32(value: i64) -> i32 {
    value.try_into().unwrap_or(if value.is_negative() {
        i32::MIN
    } else {
        i32::MAX
    })
}

#[cfg(test)]
fn fallback_reduction_telemetry(issue: String) -> ReductionTelemetry {
    ReductionTelemetry {
        path: ReductionPath::CpuFallback,
        gpu_failures: 1,
        issue: Some(issue.into()),
        ..ReductionTelemetry::default()
    }
}

impl Drop for DesktopDuplicator {
    fn drop(&mut self) {
        self.release_frame();
    }
}

#[cfg(feature = "capture-fixtures")]
#[doc(hidden)]
pub mod fixtures {
    use std::num::NonZeroU32;

    use windows::Win32::Graphics::Direct3D11::{
        D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

    use super::*;
    /// Inputs for one deterministic exact GPU publication fixture.
    pub struct GpuSurfaceFixtureConfig {
        /// Renderer adapter on which the capture device must be created.
        pub adapter_luid: GpuAdapterLuid,
        /// Source-local plan generation stamped into the publication.
        pub plan_generation: GpuSurfacePlanGeneration,
        /// Stable display source identity.
        pub source_id: Arc<str>,
        /// Attached-output topology generation.
        pub topology_generation: u64,
        /// Capture-session incarnation.
        pub duplication_generation: u64,
        /// Exact publication descriptor.
        pub descriptor: GpuSurfaceDescriptor,
        /// Tightly packed BGRA source pixels.
        pub bgra: Vec<u8>,
        /// Source width.
        pub width: u32,
        /// Source height.
        pub height: u32,
    }

    /// Live producer resources and the exact publication they emitted.
    pub struct GpuSurfaceFixture {
        publication: Arc<GpuSurfacePublication>,
        target_preparation: GpuSurfaceTargetPreparation,
        _plan: PreparedGpuSurfacePlan,
    }

    impl GpuSurfaceFixture {
        /// Borrow the live exact publication.
        #[must_use]
        pub fn publication(&self) -> &Arc<GpuSurfacePublication> {
            &self.publication
        }

        /// Borrow the owned native target-preparation manifest.
        #[must_use]
        pub fn target_preparation(&self) -> &GpuSurfaceTargetPreparation {
            &self.target_preparation
        }

        /// Request a target-preparation manifest from the fixture's live plan.
        pub fn target_preparation_for(
            &self,
            descriptor_id: crate::GpuSurfaceDescriptorId,
        ) -> CaptureResult<GpuSurfaceTargetPreparation> {
            self._plan.target_preparation(descriptor_id)
        }
    }

    /// Publish deterministic BGRA pixels through the real exact GPU producer.
    ///
    /// # Errors
    ///
    /// Returns typed capture failures for adapter lookup, resource creation,
    /// exact plan preparation, and publication.
    pub fn publish_gpu_surface(
        config: GpuSurfaceFixtureConfig,
    ) -> CaptureResult<GpuSurfaceFixture> {
        let adapter = find_adapter(config.adapter_luid)?;
        let (device, context) = create_device(&adapter)?;
        let source_extent = CaptureExtent::try_new(config.width, config.height)?;
        let expected_len = checked_rgba_bytes(config.width, config.height)?;
        if config.bgra.len() != expected_len {
            return Err(CaptureError::InvalidBufferGeometry {
                operation: "validate GPU Surface fixture pixels",
                width: config.width,
                height: config.height,
                row_pitch: config.bgra.len(),
            });
        }
        let row_pitch = config.width.checked_mul(BYTES_PER_PIXEL as u32).ok_or(
            CaptureError::GeometryOverflow {
                operation: "calculate GPU Surface fixture row pitch",
                width: config.width,
                height: config.height,
            },
        )?;
        let source_desc = D3D11_TEXTURE2D_DESC {
            Width: config.width,
            Height: config.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0.cast_unsigned(),
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let initial = D3D11_SUBRESOURCE_DATA {
            pSysMem: config.bgra.as_ptr().cast(),
            SysMemPitch: row_pitch,
            SysMemSlicePitch: 0,
        };
        let source = gpu_reduction::create_texture(&device, &source_desc, Some(&initial))
            .map_err(|error| CaptureError::windows("create GPU Surface fixture texture", error))?;
        let admission = GpuSurfaceAdmission::new(
            u64::MAX,
            NonZeroU32::new(2).expect("two is a non-zero fixture slot count"),
        );
        let mut plan = PreparedGpuSurfacePlan::prepare(
            &device,
            &context,
            config.plan_generation,
            Arc::clone(&config.source_id),
            config.topology_generation,
            config.duplication_generation,
            config.adapter_luid,
            source_extent,
            source_extent,
            DisplayRotation::Identity,
            GpuSurfaceSourceColorSpace::RgbFullG22P709,
            std::slice::from_ref(&config.descriptor),
            admission,
        )?;
        let target_preparation = plan.target_preparation(config.descriptor.id())?;
        let metadata = CaptureMetadata {
            source_id: config.source_id,
            topology_generation: config.topology_generation,
            sequence: 1,
            captured_at: Instant::now(),
            cursor: CursorInfo::default(),
            pointer: PointerState::default(),
            source_width: config.width,
            source_height: config.height,
            origin_x: 0,
            origin_y: 0,
            rotation: DisplayRotation::Identity,
            source_color_space: GpuSurfaceSourceColorSpace::RgbFullG22P709,
            region: CaptureRegion::full(config.width, config.height),
        };
        let clean = RetainedDesktop {
            srv: gpu_reduction::create_srv(&device, &source)
                .map_err(|error| CaptureError::windows("create GPU Surface fixture view", error))?,
            texture: source,
            metadata,
            _resource_lease: None,
        };
        let mut publication = None;
        plan.publish_with_feedback(&clean, None, config.duplication_generation, |outcome| {
            if let GpuSurfacePublishOutcome::Published(published) = outcome {
                publication = Some(published);
            }
            GpuSurfacePublicationDisposition::Accepted
        })?;
        let publication = publication.ok_or_else(|| {
            CaptureError::windows(
                "publish GPU Surface fixture",
                "the exact producer emitted no publication",
            )
        })?;
        Ok(GpuSurfaceFixture {
            publication,
            target_preparation,
            _plan: plan,
        })
    }

    fn find_adapter(requested: GpuAdapterLuid) -> CaptureResult<IDXGIAdapter1> {
        // SAFETY: CreateDXGIFactory1 has no borrowed inputs or out-pointers.
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }
            .map_err(|error| CaptureError::windows("create GPU fixture DXGI factory", error))?;
        for index in 0.. {
            // SAFETY: EnumAdapters1 returns owned COM interfaces and uses
            // DXGI_ERROR_NOT_FOUND as its documented loop terminator.
            match unsafe { factory.EnumAdapters1(index) } {
                Ok(adapter) if adapter_luid(&adapter)? == requested => return Ok(adapter),
                Ok(_) => {}
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(error) => {
                    return Err(CaptureError::windows(
                        "enumerate GPU fixture adapters",
                        error,
                    ));
                }
            }
        }
        Err(CaptureError::windows(
            "find GPU fixture adapter",
            "renderer adapter was not enumerated by DXGI",
        ))
    }

    fn checked_rgba_bytes(width: u32, height: u32) -> CaptureResult<usize> {
        let bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL as u64))
            .ok_or(CaptureError::GeometryOverflow {
                operation: "calculate GPU Surface fixture bytes",
                width,
                height,
            })?;
        usize::try_from(bytes).map_err(|_| CaptureError::GeometryOverflow {
            operation: "represent GPU Surface fixture bytes",
            width,
            height,
        })
    }
}

/// Create a D3D11 device on `adapter`.
fn create_device(adapter: &IDXGIAdapter1) -> CaptureResult<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    let feature_levels = [D3D_FEATURE_LEVEL_11_0];

    // SAFETY: the adapter outlives the call, DRIVER_TYPE_UNKNOWN is required
    // when passing an explicit adapter, and both out-params are owned locals.
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&feature_levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .map_err(|source| CaptureError::windows("create D3D11 device", source))?;

    match (device, context) {
        (Some(device), Some(context)) => Ok((device, context)),
        _ => Err(CaptureError::windows(
            "create D3D11 device",
            "D3D11CreateDevice returned no device",
        )),
    }
}

fn adapter_luid(adapter: &IDXGIAdapter1) -> CaptureResult<GpuAdapterLuid> {
    // SAFETY: GetDesc1 copies the live adapter descriptor into a return value
    // and does not retain caller-owned storage.
    let desc = unsafe { adapter.GetDesc1() }
        .map_err(|source| CaptureError::windows("describe DXGI adapter", source))?;
    Ok(GpuAdapterLuid::new(
        desc.AdapterLuid.LowPart,
        desc.AdapterLuid.HighPart,
    ))
}

/// Open the duplication interface, mapping the two well-known refusals.
fn duplicate_output(
    output: &IDXGIOutput1,
    device: &ID3D11Device,
) -> CaptureResult<IDXGIOutputDuplication> {
    // SAFETY: both interfaces outlive the call.
    unsafe { output.DuplicateOutput(device) }.map_err(|source| {
        let context = if source.code() == DXGI_ERROR_UNSUPPORTED {
            "duplicate output (desktop is not on this adapter)"
        } else {
            "duplicate output"
        };
        classify_windows_error(context, source)
    })
}

/// Read the duplicated desktop dimensions, defaulting to zero on failure.
fn duplication_geometry(duplication: &IDXGIOutputDuplication) -> (u32, u32, DisplayRotation) {
    // SAFETY: GetDesc reads cached descriptor state and cannot fail.
    let desc = unsafe { duplication.GetDesc() };
    let mode = desc.ModeDesc;
    if mode.Width == 0 || mode.Height == 0 {
        warn!("desktop duplication reported a zero-sized mode");
    }
    (mode.Width, mode.Height, display_rotation(desc.Rotation))
}

fn display_rotation(rotation: DXGI_MODE_ROTATION) -> DisplayRotation {
    match rotation {
        DXGI_MODE_ROTATION_ROTATE90 => DisplayRotation::Clockwise90,
        DXGI_MODE_ROTATION_ROTATE180 => DisplayRotation::Clockwise180,
        DXGI_MODE_ROTATION_ROTATE270 => DisplayRotation::Clockwise270,
        _ => DisplayRotation::Identity,
    }
}

const fn native_scanout_extent(
    logical_width: u32,
    logical_height: u32,
    rotation: DisplayRotation,
) -> (u32, u32) {
    match rotation {
        DisplayRotation::Identity | DisplayRotation::Clockwise180 => {
            (logical_width, logical_height)
        }
        DisplayRotation::Clockwise90 | DisplayRotation::Clockwise270 => {
            (logical_height, logical_width)
        }
    }
}

fn output_origin(output: &IDXGIOutput1) -> CaptureResult<(i32, i32)> {
    // SAFETY: GetDesc fills a caller-owned struct from the live output.
    let desc = unsafe { output.GetDesc() }
        .map_err(|source| CaptureError::windows("describe DXGI output", source))?;
    Ok((desc.DesktopCoordinates.left, desc.DesktopCoordinates.top))
}

fn output_color_space(output: &IDXGIOutput1) -> GpuSurfaceSourceColorSpace {
    let Ok(output6) = output.cast::<IDXGIOutput6>() else {
        return GpuSurfaceSourceColorSpace::Unknown;
    };
    // SAFETY: GetDesc1 returns a value snapshot from the live output.
    let Ok(desc) = (unsafe { output6.GetDesc1() }) else {
        return GpuSurfaceSourceColorSpace::Unknown;
    };
    match desc.ColorSpace {
        DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709 => GpuSurfaceSourceColorSpace::RgbFullG22P709,
        DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709 => GpuSurfaceSourceColorSpace::RgbFullLinearP709,
        DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020 => GpuSurfaceSourceColorSpace::RgbFullPqP2020,
        _ => GpuSurfaceSourceColorSpace::Unknown,
    }
}

#[cfg(test)]
mod tests;
