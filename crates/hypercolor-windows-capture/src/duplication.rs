//! DXGI Desktop Duplication capture loop.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tracing::{debug, warn};
use windows::Win32::Foundation::{E_ACCESSDENIED, E_OUTOFMEMORY, HMODULE};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_MODE_ROTATION, DXGI_MODE_ROTATION_ROTATE90, DXGI_MODE_ROTATION_ROTATE180,
    DXGI_MODE_ROTATION_ROTATE270,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET,
    DXGI_ERROR_NOT_CURRENTLY_AVAILABLE, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_SESSION_DISCONNECTED,
    DXGI_ERROR_UNSUPPORTED, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    DXGI_OUTDUPL_POINTER_SHAPE_INFO, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR,
    DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME,
    IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::Win32::Graphics::Gdi::{DISPLAY_DEVICEW, EnumDisplayDevicesW};
use windows::core::{HRESULT, Interface, PCWSTR};

use crate::shared::{
    CaptureError, CaptureExtent, CaptureRegion, CaptureResult, CursorInfo, DisplayRotation, Frame,
    MonitorInfo, MonitorSelector, ReductionPath, ReductionTelemetry, subsample_stride_within,
    subsampled_extent,
};

pub(crate) mod gpu_reduction;

use gpu_reduction::{GpuReducer, ReducedFrame, SubmitOutcome};

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

fn admit_plane(
    rgba: &mut Vec<u8>,
    requested_bytes: usize,
    operation: &'static str,
) -> CaptureResult<()> {
    if requested_bytes <= rgba.capacity() {
        return Ok(());
    }
    rgba.try_reserve(requested_bytes.saturating_sub(rgba.len()))
        .map_err(|_| CaptureError::ResourceExhausted {
            operation,
            requested_bytes,
        })
}

#[derive(Clone)]
struct CaptureMetadata {
    source_id: Arc<str>,
    topology_generation: u64,
    sequence: u64,
    captured_at: Instant,
    cursor: CursorInfo,
    pointer: Arc<PointerState>,
    source_width: u32,
    source_height: u32,
    origin_x: i32,
    origin_y: i32,
    rotation: DisplayRotation,
    region: CaptureRegion,
}

struct RetainedDesktop {
    texture: ID3D11Texture2D,
    metadata: CaptureMetadata,
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

#[derive(Clone, Debug)]
struct PointerShape {
    kind: PointerShapeKind,
    width: u32,
    height: u32,
    pitch: u32,
    hotspot_x: i32,
    hotspot_y: i32,
    bytes: Vec<u8>,
}

#[derive(Clone, Default)]
struct PointerState {
    visible: bool,
    position_x: i32,
    position_y: i32,
    shape: Option<PointerShape>,
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
    fn validate(&self) -> Result<(), &'static str> {
        let pitch = self.pitch as usize;
        let rows = self.height as usize;
        let required = pitch
            .checked_mul(rows)
            .ok_or("pointer shape size overflow")?;
        if required > self.bytes.len() {
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
    topology_generation: u64,
    duplication_generation: u64,
    adapter_luid: (u32, i32),
    last_topology_check: Instant,
    requested_extent: CaptureExtent,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    duplication: Option<IDXGIOutputDuplication>,
    /// CPU-readable clean desktop paired with the acquisition that produced it.
    staging: Option<RetainedDesktop>,
    /// RGBA allocations returned by frames after their last consumer drops.
    frame_pool: Arc<Mutex<Vec<Vec<u8>>>>,
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
    pointer: Arc<PointerState>,
    region: Option<CaptureRegion>,
    capture_sequence: u64,
    latest_capture: Option<CaptureMetadata>,
    gpu_reducer: Option<GpuReducer>,
    reduction_telemetry: ReductionTelemetry,
    analysis_pending: bool,
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
    /// or a legacy index is out of range, [`CaptureError::SourceNotFound`] when
    /// a stable output disappeared, and the same platform errors as [`Self::new`].
    pub fn open(selector: MonitorSelector, requested_extent: CaptureExtent) -> CaptureResult<Self> {
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
        let (gpu_reducer, reduction_telemetry) = initialize_gpu_reduction(&device, &context)?;

        Ok(Self {
            selector,
            monitor: monitor.index,
            source_id: Arc::from(monitor.id),
            topology_generation: monitor.topology_generation,
            duplication_generation: 1,
            adapter_luid,
            last_topology_check: Instant::now(),
            requested_extent,
            device,
            context,
            output,
            duplication: Some(duplication),
            staging: None,
            frame_pool: Arc::new(Mutex::new(Vec::new())),
            frame_held: false,
            logical_width,
            logical_height,
            native_width,
            native_height,
            origin_x,
            origin_y,
            rotation,
            pointer: Arc::new(PointerState::default()),
            region: None,
            capture_sequence: 0,
            latest_capture: None,
            gpu_reducer,
            reduction_telemetry,
            analysis_pending: false,
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
            pointer: Arc::clone(&self.pointer),
            source_width: self.native_width,
            source_height: self.native_height,
            origin_x: self.origin_x,
            origin_y: self.origin_y,
            rotation: self.rotation,
            region: self.effective_region(),
        }
    }

    fn refresh_latest_capture(&mut self) {
        let clean_available = self.staging.is_some()
            || self
                .gpu_reducer
                .as_ref()
                .is_some_and(GpuReducer::has_clean_desktop);
        if !clean_available {
            return;
        }
        let metadata = self.new_capture_metadata(Instant::now());
        if let Some(staging) = self.staging.as_mut() {
            staging.metadata = metadata.clone();
        }
        self.latest_capture = Some(metadata);
        self.analysis_pending = true;
    }

    fn update_pointer(&mut self, frame_info: &DXGI_OUTDUPL_FRAME_INFO) -> CaptureResult<bool> {
        if frame_info.LastMouseUpdateTime == 0 {
            return Ok(false);
        }

        let pointer = Arc::make_mut(&mut self.pointer);
        pointer.visible = frame_info.PointerPosition.Visible.as_bool();
        pointer.position_x = frame_info.PointerPosition.Position.x;
        pointer.position_y = frame_info.PointerPosition.Position.y;

        if frame_info.PointerShapeBufferSize == 0 {
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

        let mut bytes = Vec::new();
        admit_plane(&mut bytes, buffer_size, "read desktop pointer shape")?;
        bytes.resize(buffer_size, 0);
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
        bytes.truncate(required);
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
        let shape = PointerShape {
            kind,
            width: info.Width,
            height: info.Height,
            pitch: info.Pitch,
            hotspot_x: info.HotSpot.x,
            hotspot_y: info.HotSpot.y,
            bytes,
        };
        shape
            .validate()
            .map_err(|message| CaptureError::windows("validate desktop pointer shape", message))?;
        let pointer = Arc::make_mut(&mut self.pointer);
        pointer.shape = Some(shape);
        pointer.shape_generation = pointer.shape_generation.wrapping_add(1).max(1);
        Ok(true)
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

        let mut ready = self.poll_gpu_frame()?;
        if self.analysis_pending && self.gpu_reducer.is_some() {
            if let Some(metadata) = self.latest_capture.clone() {
                self.submit_gpu(None, metadata)?;
            }
            if ready.is_none() {
                ready = self.poll_gpu_frame()?;
            }
        }
        if self.analysis_pending && self.gpu_reducer.is_none() {
            ready = self.cpu_frame_from_retained()?;
            self.analysis_pending = false;
        }

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

        let timeout_ms = if ready.is_some() {
            0
        } else {
            u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX)
        };
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
                CaptureError::Timeout => Ok(ready),
                CaptureError::AccessLost => {
                    // Mode change, a full-screen app taking over, or the
                    // secure desktop during a UAC prompt. All are transient
                    // and all are fixed by re-duplicating the output.
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
        if current_width != self.logical_width
            || current_height != self.logical_height
            || current_origin_x != self.origin_x
            || current_origin_y != self.origin_y
            || current_rotation != self.rotation
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
            return Ok(ready);
        }
        let metadata = self.new_capture_metadata(captured_at);
        self.latest_capture = Some(metadata.clone());

        let clean_available = self.staging.is_some()
            || self
                .gpu_reducer
                .as_ref()
                .is_some_and(GpuReducer::has_clean_desktop);
        let texture = match desktop_frame_source(desktop_updated, clean_available) {
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

        let mut retained = Ok(());
        if self.gpu_reducer.is_some() {
            self.analysis_pending = true;
            self.submit_gpu(texture.as_ref(), metadata.clone())?;
            if self.gpu_reducer.is_none()
                && let Some(texture) = texture.as_ref()
            {
                retained = self.retain_desktop(texture, metadata.clone());
            }
        } else if let Some(texture) = texture.as_ref() {
            retained = self.retain_desktop(texture, metadata.clone());
        } else if let Some(staging) = self.staging.as_mut() {
            staging.metadata = metadata;
        }
        self.release_frame();
        if matches!(retained, Err(CaptureError::AccessLost)) {
            debug!(
                operation = "desktop readback",
                "desktop duplication access lost; rebuilding session"
            );
            self.rebuild()?;
            return Ok(None);
        }
        retained?;

        if self.gpu_reducer.is_none() {
            self.analysis_pending = false;
            return self.cpu_frame_from_retained();
        }
        if ready.is_none() {
            ready = self.poll_gpu_frame()?;
        }
        Ok(ready)
    }

    fn submit_gpu(
        &mut self,
        texture: Option<&ID3D11Texture2D>,
        metadata: CaptureMetadata,
    ) -> CaptureResult<()> {
        let Some(reducer) = self.gpu_reducer.as_mut() else {
            return Ok(());
        };
        match reducer.submit(texture, self.requested_extent, metadata) {
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
                let clean = reducer.clean_desktop();
                if let Some(clean) = clean {
                    self.retain_desktop(&clean.texture, clean.metadata)?;
                }
                self.degrade_gpu(error.to_string());
                self.analysis_pending = true;
            }
        }
        Ok(())
    }

    fn poll_gpu_frame(&mut self) -> CaptureResult<Option<Frame>> {
        let Some(reducer) = self.gpu_reducer.as_mut() else {
            return Ok(None);
        };
        let mut rgba = self
            .frame_pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop()
            .unwrap_or_default();
        let result = reducer.poll(&mut rgba);
        match result {
            Ok(Some(reduced)) => {
                self.reduction_telemetry.gpu_completed =
                    self.reduction_telemetry.gpu_completed.saturating_add(1);
                self.reduction_telemetry.readback_bytes = self
                    .reduction_telemetry
                    .readback_bytes
                    .saturating_add(reduced.bytes as u64);
                Ok(Some(self.frame_from_reduction(reduced, rgba)))
            }
            Ok(None) => {
                self.recycle_plane(rgba);
                Ok(None)
            }
            Err(error) => {
                if let Some(capture_error) = error.as_capture_error() {
                    self.recycle_plane(rgba);
                    return Err(capture_error);
                }
                let clean = reducer.clean_desktop();
                self.recycle_plane(rgba);
                if let Some(clean) = clean {
                    self.retain_desktop(&clean.texture, clean.metadata)?;
                }
                self.degrade_gpu(error.to_string());
                self.analysis_pending = true;
                let frame = self.cpu_frame_from_retained()?;
                self.analysis_pending = false;
                Ok(frame)
            }
        }
    }

    fn cpu_frame_from_retained(&mut self) -> CaptureResult<Option<Frame>> {
        let mut rgba = self
            .frame_pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop()
            .unwrap_or_default();
        match self.read_back(&mut rgba) {
            Ok(Some((width, height, metadata))) => {
                self.reduction_telemetry.cpu_completed =
                    self.reduction_telemetry.cpu_completed.saturating_add(1);
                Ok(Some(
                    self.frame_from_metadata(metadata, width, height, rgba),
                ))
            }
            Ok(None) => {
                self.recycle_plane(rgba);
                Ok(None)
            }
            Err(error) => {
                self.recycle_plane(rgba);
                Err(error)
            }
        }
    }

    fn frame_from_reduction(&self, reduced: ReducedFrame, rgba: Vec<u8>) -> Frame {
        self.frame_from_metadata(reduced.metadata, reduced.width, reduced.height, rgba)
    }

    fn frame_from_metadata(
        &self,
        metadata: CaptureMetadata,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
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
            rgba,
            Arc::clone(&self.frame_pool),
        )
    }

    fn recycle_plane(&self, mut rgba: Vec<u8>) {
        rgba.clear();
        self.frame_pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(rgba);
    }

    fn degrade_gpu(&mut self, issue: String) {
        self.gpu_reducer = None;
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
        let staging = if let Some(retained) = self.staging.as_ref() {
            let mut retained_desc = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: GetDesc fills a caller-owned struct and cannot fail.
            unsafe { retained.texture.GetDesc(&mut retained_desc) };
            if retained_desc.Width == desc.Width
                && retained_desc.Height == desc.Height
                && retained_desc.Format == desc.Format
            {
                retained.texture.clone()
            } else {
                create_staging_texture(&self.device, &desc)?
            }
        } else {
            create_staging_texture(&self.device, &desc)?
        };
        // SAFETY: both textures are same-desc 2D textures on this device.
        unsafe { self.context.CopyResource(&staging, texture) };
        self.staging = Some(RetainedDesktop {
            texture: staging,
            metadata,
        });
        Ok(())
    }

    /// Map the retained clean desktop, then subsample its configured region.
    fn read_back(
        &mut self,
        rgba: &mut Vec<u8>,
    ) -> CaptureResult<Option<(u32, u32, CaptureMetadata)>> {
        let Some(retained) = self.staging.as_ref() else {
            return Ok(None);
        };
        let staging = retained.texture.clone();
        let metadata = retained.metadata.clone();
        let native_width = metadata.source_width;
        let native_height = metadata.source_height;

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
        admit_plane(rgba, output_len, "allocate CPU capture plane")?;
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

        let (duplication, (logical_width, logical_height, rotation, gpu_reducer, reduction_status)) =
            prepare_duplication(
                &mut self.duplication,
                &mut self.staging,
                || duplicate_output(&output, &device),
                |duplication| {
                    let (logical_width, logical_height, rotation) =
                        duplication_geometry(duplication);
                    let (gpu_reducer, reduction_status) =
                        initialize_gpu_reduction(&device, &context)?;
                    Ok((
                        logical_width,
                        logical_height,
                        rotation,
                        gpu_reducer,
                        reduction_status,
                    ))
                },
            )
            .map_err(session_rebuild_error)?;
        let (native_width, native_height) =
            native_scanout_extent(logical_width, logical_height, rotation);
        let region = self
            .region
            .filter(|region| region.fits_within(native_width, native_height));

        self.pointer = Arc::new(PointerState::default());
        self.device = device;
        self.context = context;
        self.output = output;
        self.duplication = Some(duplication);
        self.staging = None;
        self.monitor = entry.monitor.index;
        self.source_id = Arc::from(entry.monitor.id);
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
        self.region = region;
        self.latest_capture = None;
        self.gpu_reducer = gpu_reducer;
        self.analysis_pending = false;
        self.reduction_telemetry.path = reduction_status.path;
        self.reduction_telemetry.issue = reduction_status.issue;
        self.reduction_telemetry.gpu_failures = self
            .reduction_telemetry
            .gpu_failures
            .saturating_add(reduction_status.gpu_failures);
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

fn initialize_gpu_reduction(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
) -> CaptureResult<(Option<GpuReducer>, ReductionTelemetry)> {
    match GpuReducer::new(device, context) {
        Ok(reducer) => Ok((
            Some(reducer),
            ReductionTelemetry {
                path: ReductionPath::Gpu,
                ..ReductionTelemetry::default()
            },
        )),
        Err(error) => {
            if let Some(capture_error) = error.as_capture_error() {
                return Err(capture_error);
            }
            let issue = error.to_string();
            warn!(
                reduction_path = ?ReductionPath::CpuFallback,
                gpu_failures = 1,
                %issue,
                "GPU capture reduction unavailable; using CPU fallback"
            );
            Ok((None, fallback_reduction_telemetry(issue)))
        }
    }
}

fn fallback_reduction_telemetry(issue: String) -> ReductionTelemetry {
    ReductionTelemetry {
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

fn adapter_luid(adapter: &IDXGIAdapter1) -> CaptureResult<(u32, i32)> {
    // SAFETY: GetDesc1 copies the live adapter descriptor into a return value
    // and does not retain caller-owned storage.
    let desc = unsafe { adapter.GetDesc1() }
        .map_err(|source| CaptureError::windows("describe DXGI adapter", source))?;
    Ok((desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart))
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

#[cfg(test)]
mod tests;
