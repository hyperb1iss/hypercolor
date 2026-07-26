//! DXGI Desktop Duplication capture loop.

use std::time::Duration;

use tracing::{debug, warn};
use windows::Win32::Foundation::{E_ACCESSDENIED, HMODULE};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_UNSUPPORTED,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput,
    IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

use crate::shared::{CaptureError, CaptureResult, Frame, subsample_stride, subsampled_extent};

/// Bytes per pixel in both the duplicated surface and our RGBA output.
const BYTES_PER_PIXEL: usize = 4;

/// Enumerate every output across every adapter, in adapter-then-output order.
fn enumerate_outputs() -> CaptureResult<Vec<(IDXGIAdapter1, IDXGIOutput)>> {
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
                Ok(output) => outputs.push((adapter.clone(), output)),
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(source) => {
                    return Err(CaptureError::windows("enumerate DXGI outputs", source));
                }
            }
        }
    }

    Ok(outputs)
}

/// How many display outputs are attached.
pub(crate) fn output_count() -> CaptureResult<usize> {
    enumerate_outputs().map(|outputs| outputs.len())
}

/// Describe every attached output for monitor pickers.
pub(crate) fn describe_outputs() -> CaptureResult<Vec<crate::shared::MonitorInfo>> {
    let outputs = enumerate_outputs()?;
    let mut monitors = Vec::with_capacity(outputs.len());

    for (index, (_, output)) in outputs.into_iter().enumerate() {
        // SAFETY: GetDesc fills a caller-owned struct from the live output.
        let desc = match unsafe { output.GetDesc() } {
            Ok(desc) => desc,
            Err(source) => {
                return Err(CaptureError::windows("describe DXGI output", source));
            }
        };

        let name_len = desc
            .DeviceName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(desc.DeviceName.len());
        let name = String::from_utf16_lossy(&desc.DeviceName[..name_len]);

        let bounds = desc.DesktopCoordinates;
        let width = u32::try_from(i64::from(bounds.right) - i64::from(bounds.left)).unwrap_or(0);
        let height = u32::try_from(i64::from(bounds.bottom) - i64::from(bounds.top)).unwrap_or(0);

        monitors.push(crate::shared::MonitorInfo {
            index,
            name,
            width,
            height,
            // The primary output anchors the virtual desktop at the origin.
            primary: bounds.left == 0 && bounds.top == 0,
        });
    }

    Ok(monitors)
}

/// A live Desktop Duplication session for one display output.
pub struct DesktopDuplicator {
    monitor: usize,
    max_width: u32,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    duplication: IDXGIOutputDuplication,
    /// CPU-readable copy target, rebuilt when the desktop dimensions change.
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    /// Reused RGBA output buffer.
    rgba: Vec<u8>,
    /// Set while a duplicated frame is held and must be released before the
    /// next acquire. DXGI rejects back-to-back acquires without a release.
    frame_held: bool,
    output_width: u32,
    output_height: u32,
}

impl DesktopDuplicator {
    /// Open Desktop Duplication for `monitor`, subsampling to `max_width`.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::MonitorNotFound`] when the index is out of
    /// range, [`CaptureError::AlreadyDuplicating`] when another process holds
    /// the duplication interface, or [`CaptureError::Windows`] for any other
    /// D3D11/DXGI failure.
    pub fn new(monitor: usize, max_width: u32) -> CaptureResult<Self> {
        let outputs = enumerate_outputs()?;
        let available = outputs.len();
        let (adapter, output) =
            outputs
                .into_iter()
                .nth(monitor)
                .ok_or(CaptureError::MonitorNotFound {
                    requested: monitor,
                    available,
                })?;

        let (device, context) = create_device(&adapter)?;
        let output = output
            .cast::<IDXGIOutput1>()
            .map_err(|source| CaptureError::windows("query IDXGIOutput1", source))?;
        let duplication = duplicate_output(&output, &device)?;
        let (output_width, output_height) = duplication_extent(&duplication);

        Ok(Self {
            monitor,
            max_width,
            device,
            context,
            output,
            duplication,
            staging: None,
            rgba: Vec::new(),
            frame_held: false,
            output_width,
            output_height,
        })
    }

    /// Which monitor index this duplicator is bound to.
    #[must_use]
    pub const fn monitor(&self) -> usize {
        self.monitor
    }

    /// Native (pre-subsample) desktop dimensions.
    #[must_use]
    pub const fn native_extent(&self) -> (u32, u32) {
        (self.output_width, self.output_height)
    }

    /// Change the subsample target for subsequent frames.
    pub const fn set_max_width(&mut self, max_width: u32) {
        self.max_width = max_width;
    }

    /// Wait up to `timeout` for the next desktop frame.
    ///
    /// Returns `Ok(None)` when nothing new arrived, which is the common and
    /// cheap case: DXGI reports a timeout whenever the desktop is static, and
    /// a mouse-only update carries no new desktop image worth re-analyzing.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Windows`] when acquiring, copying, or mapping
    /// the frame fails for a reason that is not recoverable in place. Access
    /// loss is handled internally by rebuilding the duplication interface.
    pub fn next_frame(&mut self, timeout: Duration) -> CaptureResult<Option<Frame<'_>>> {
        self.release_frame();

        let timeout_ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;

        // SAFETY: both out-params are owned locals living past the call, and
        // the duplication interface is kept alive by `self`.
        let acquire = unsafe {
            self.duplication
                .AcquireNextFrame(timeout_ms, &mut frame_info, &mut resource)
        };

        if let Err(error) = acquire {
            return match error.code() {
                DXGI_ERROR_WAIT_TIMEOUT => Ok(None),
                DXGI_ERROR_ACCESS_LOST => {
                    // Mode change, a full-screen app taking over, or the
                    // secure desktop during a UAC prompt. All are transient
                    // and all are fixed by re-duplicating the output.
                    debug!("desktop duplication access lost; rebuilding session");
                    self.rebuild()?;
                    Ok(None)
                }
                _ => Err(CaptureError::windows("acquire duplicated frame", error)),
            };
        }
        self.frame_held = true;

        // LastPresentTime stays zero when only the pointer moved. There is no
        // new desktop image behind that, so re-running the sector grid would
        // burn a readback to produce identical colors.
        if frame_info.LastPresentTime == 0 {
            self.release_frame();
            return Ok(None);
        }

        let Some(resource) = resource else {
            self.release_frame();
            return Ok(None);
        };

        let texture = resource
            .cast::<ID3D11Texture2D>()
            .map_err(|source| CaptureError::windows("query duplicated texture", source))?;

        let result = self.read_back(&texture);
        self.release_frame();
        let (width, height) = result?;

        Ok(Some(Frame {
            width,
            height,
            rgba: &self.rgba[..(width as usize * height as usize * BYTES_PER_PIXEL)],
        }))
    }

    /// Copy the duplicated texture into staging, then subsample into `rgba`.
    fn read_back(&mut self, texture: &ID3D11Texture2D) -> CaptureResult<(u32, u32)> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: GetDesc fills a caller-owned struct and cannot fail.
        unsafe { texture.GetDesc(&mut desc) };

        let staging = self.ensure_staging(&desc)?;

        // SAFETY: both textures are same-desc 2D textures on this device;
        // CopyResource is the documented duplication readback path.
        unsafe { self.context.CopyResource(&staging, texture) };

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: staging was created USAGE_STAGING | CPU_ACCESS_READ, so
        // subresource 0 is mappable for reads.
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|source| CaptureError::windows("map staging texture", source))?;

        let extent = self.copy_mapped_rows(&mapped, desc.Width, desc.Height);

        // SAFETY: pairs with the Map above on the same subresource.
        unsafe { self.context.Unmap(&staging, 0) };

        Ok(extent)
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
    fn copy_mapped_rows(
        &mut self,
        mapped: &D3D11_MAPPED_SUBRESOURCE,
        width: u32,
        height: u32,
    ) -> (u32, u32) {
        let stride = subsample_stride(width, self.max_width);
        let out_width = subsampled_extent(width, stride);
        let out_height = subsampled_extent(height, stride);
        let row_pitch = mapped.RowPitch as usize;
        let source_len = row_pitch * height as usize;

        self.rgba.resize(
            out_width as usize * out_height as usize * BYTES_PER_PIXEL,
            0,
        );

        // SAFETY: Map handed back a buffer of at least RowPitch * Height
        // bytes for this subresource, and it stays valid until Unmap.
        let source = unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), source_len) };

        let stride = stride as usize;
        let width = width as usize;
        let height = height as usize;

        for out_y in 0..out_height as usize {
            let dst_row_start = out_y * out_width as usize * BYTES_PER_PIXEL;
            // Blocks on the right and bottom edges are clipped when the
            // desktop does not divide evenly by the stride.
            let src_y0 = out_y * stride;
            let src_y1 = (src_y0 + stride).min(height);

            for out_x in 0..out_width as usize {
                let src_x0 = out_x * stride;
                let src_x1 = (src_x0 + stride).min(width);

                let mut blue = 0_u32;
                let mut green = 0_u32;
                let mut red = 0_u32;
                let mut samples = 0_u32;

                for src_y in src_y0..src_y1 {
                    let row = src_y * row_pitch;
                    for src_x in src_x0..src_x1 {
                        let src = row + src_x * BYTES_PER_PIXEL;
                        // Desktop Duplication hands back BGRA.
                        blue += u32::from(source[src]);
                        green += u32::from(source[src + 1]);
                        red += u32::from(source[src + 2]);
                        samples += 1;
                    }
                }

                let samples = samples.max(1);
                let dst = dst_row_start + out_x * BYTES_PER_PIXEL;
                self.rgba[dst] = (red / samples) as u8;
                self.rgba[dst + 1] = (green / samples) as u8;
                self.rgba[dst + 2] = (blue / samples) as u8;
                self.rgba[dst + 3] = 0xFF;
            }
        }

        (out_width, out_height)
    }

    /// Return a staging texture matching `desc`, rebuilding on size change.
    ///
    /// Hands back a clone rather than a borrow: COM interfaces are refcounted
    /// so the clone is an AddRef, and it frees `self` for the copy and map
    /// calls that immediately follow.
    fn ensure_staging(&mut self, desc: &D3D11_TEXTURE2D_DESC) -> CaptureResult<ID3D11Texture2D> {
        let matches = self
            .staging
            .as_ref()
            .is_some_and(|(_, width, height)| *width == desc.Width && *height == desc.Height);

        if !matches {
            let staging_desc = D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: u32::try_from(D3D11_CPU_ACCESS_READ.0).unwrap_or_default(),
                MiscFlags: 0,
                ..*desc
            };

            let mut texture: Option<ID3D11Texture2D> = None;
            // SAFETY: staging_desc is a valid staging description and the
            // out-param outlives the call.
            unsafe {
                self.device
                    .CreateTexture2D(&staging_desc, None, Some(&mut texture))
            }
            .map_err(|source| CaptureError::windows("create staging texture", source))?;

            let texture = texture.ok_or_else(|| {
                CaptureError::windows(
                    "create staging texture",
                    "CreateTexture2D returned no texture",
                )
            })?;
            self.staging = Some((texture, desc.Width, desc.Height));
            self.output_width = desc.Width;
            self.output_height = desc.Height;
        }

        self.staging
            .as_ref()
            .map(|(texture, _, _)| texture.clone())
            .ok_or_else(|| {
                CaptureError::windows(
                    "resolve staging texture",
                    "staging texture missing after creation",
                )
            })
    }

    /// Drop the duplication interface and open a fresh one.
    fn rebuild(&mut self) -> CaptureResult<()> {
        self.release_frame();
        self.staging = None;
        self.duplication = duplicate_output(&self.output, &self.device)?;
        let (width, height) = duplication_extent(&self.duplication);
        self.output_width = width;
        self.output_height = height;
        Ok(())
    }

    /// Release a held frame if there is one. Safe to call unconditionally.
    fn release_frame(&mut self) {
        if !self.frame_held {
            return;
        }
        self.frame_held = false;
        // SAFETY: paired with a successful AcquireNextFrame.
        if let Err(error) = unsafe { self.duplication.ReleaseFrame() } {
            // Access loss here is normal during mode changes and is repaired
            // on the next acquire, so this stays a debug line.
            debug!(%error, "releasing duplicated frame failed");
        }
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

/// Open the duplication interface, mapping the two well-known refusals.
fn duplicate_output(
    output: &IDXGIOutput1,
    device: &ID3D11Device,
) -> CaptureResult<IDXGIOutputDuplication> {
    // SAFETY: both interfaces outlive the call.
    unsafe { output.DuplicateOutput(device) }.map_err(|source| match source.code() {
        // Documented as "already duplicating"; Windows allows one per output.
        E_ACCESSDENIED => CaptureError::AlreadyDuplicating,
        // Raised on hybrid-graphics hosts when the desktop is not on this
        // adapter's output. Callers surface it as "capture unavailable".
        DXGI_ERROR_UNSUPPORTED => {
            CaptureError::windows("duplicate output (desktop is not on this adapter)", source)
        }
        _ => CaptureError::windows("duplicate output", source),
    })
}

/// Read the duplicated desktop dimensions, defaulting to zero on failure.
fn duplication_extent(duplication: &IDXGIOutputDuplication) -> (u32, u32) {
    // SAFETY: GetDesc reads cached descriptor state and cannot fail.
    let desc = unsafe { duplication.GetDesc() };
    let mode = desc.ModeDesc;
    if mode.Width == 0 || mode.Height == 0 {
        warn!("desktop duplication reported a zero-sized mode");
    }
    (mode.Width, mode.Height)
}
