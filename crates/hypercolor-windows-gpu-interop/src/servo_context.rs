use std::cell::{Cell, RefCell};
use std::ffi::OsStr;
use std::ffi::c_void;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Instant;

use dpi::PhysicalSize;
use euclid::default::Size2D;
use gleam::gl::{self, Gl};
use image::RgbaImage;
use paint_api::rendering_context::RenderingContext;
use surfman::{
    Adapter, Connection, Context, ContextAttributeFlags, ContextAttributes, Device, Error, GLApi,
    Surface, SurfaceAccess, SurfaceInfo, SurfaceTexture, SurfaceType,
};
use webrender_api::units::DeviceIntRect;
use winapi::Interface;
use winapi::shared::dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1,
};
use winapi::shared::winerror::{DXGI_ERROR_NOT_FOUND, SUCCEEDED};
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::core::PCWSTR;
use wio::com::ComPtr;

use crate::{
    ImportedEffectFrame, ImportedFrameFormat, Result, WindowsD3d11Device,
    WindowsD3d11SharedTexture, WindowsD3d11SharedTextureImportDescriptor,
    WindowsD3d11SharedTextureImporter, WindowsGpuInteropError,
};

/// Number of D3D11 shared textures in the publish ring: one being written,
/// one held by the Vulkan consumer, and one spare so the producer never
/// stalls.
const WINDOWS_SERVO_RING_SLOTS: usize = 3;
/// Bounded fence wait before reporting a transient fence timeout instead of
/// stalling the render thread. Steady-state waits return as soon as the
/// fence signals; the bound only caps cold-start submissions, where ANGLE's
/// first D3D11 work can take tens of milliseconds.
const PUBLISH_FENCE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(50);
/// Poll interval while waiting on a publish fence.
const PUBLISH_FENCE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_micros(500);
const GL_READ_FRAMEBUFFER: gl::GLenum = 0x8CA8;
const GL_DRAW_FRAMEBUFFER: gl::GLenum = 0x8CA9;

/// DXGI adapter identity used to pin ANGLE to wgpu's Vulkan adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsDxgiAdapterIdentity {
    /// PCI vendor identifier.
    pub vendor_id: u32,
    /// PCI device identifier.
    pub device_id: u32,
}

/// Origin convention for a native Windows Servo frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WindowsServoFrameOrigin {
    /// The first row in native framebuffer coordinates is the bottom row.
    BottomLeft,
}

/// Native D3D11 shared-texture frame exposed by the Windows ANGLE context.
#[derive(Debug, Clone, Copy)]
pub struct WindowsServoNativeFrame {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Native texture format.
    pub format: ImportedFrameFormat,
    /// Native framebuffer origin.
    pub origin: WindowsServoFrameOrigin,
    /// Ring slot that produced this frame.
    pub slot_index: usize,
    /// NT shared handle for this frame's D3D11 texture.
    pub shared_handle: windows::Win32::Foundation::HANDLE,
    /// Identity of the texture ring that minted `shared_handle`.
    ///
    /// Bumps whenever the ring is rebuilt (resize). Closed NT handle values
    /// can be recycled by the OS, so importer caches must reset when this
    /// changes.
    pub ring_epoch: u64,
    /// Monotonically increasing content version for this frame.
    ///
    /// Contents changed iff this changed; repeated fetches without a new
    /// publish return the same generation.
    pub content_generation: u64,
    /// Time spent waiting for the producer blit fence, in microseconds.
    pub sync_us: u64,
}

/// Windows Servo rendering context backed by an FBO render target published
/// into a ring of D3D11 shared textures.
///
/// Servo renders into one stable GL framebuffer. `present` blits that
/// framebuffer into the next available ring slot and fences the copy;
/// `native_frame` hands out the newest slot whose fence has signaled so the
/// Vulkan consumer never races in-flight GL writes.
pub struct WindowsAngleRenderingContext {
    size: Cell<PhysicalSize<u32>>,
    gleam_gl: Rc<dyn Gl>,
    glow_gl: Arc<glow::Context>,
    device: RefCell<Device>,
    context: RefCell<Context>,
    framebuffer: RefCell<Option<WindowsServoFramebuffer>>,
}

impl WindowsAngleRenderingContext {
    /// Creates an ANGLE context with an FBO render target and a D3D11
    /// shared-texture publish ring.
    pub fn new(
        width: u32,
        height: u32,
        adapter_identity: Option<WindowsDxgiAdapterIdentity>,
    ) -> Result<Self> {
        let size = validate_size(width, height)?;
        ensure_angle_dlls_loaded().map_err(|message| WindowsGpuInteropError::ServoContext {
            operation: "load ANGLE DLLs",
            message,
        })?;
        let connection = Connection::new().map_err(context_error("create connection"))?;
        let adapter = match adapter_identity {
            Some(identity) => adapter_from_identity(identity)?,
            None => connection
                .create_hardware_adapter()
                .map_err(context_error("create hardware adapter"))?,
        };
        let device = connection
            .create_device(&adapter)
            .map_err(context_error("create ANGLE device"))?;
        let mut context = create_context(&device, &connection)?;
        // Surfman contexts and surfaces panic when dropped live, so any
        // failure past this point must tear them down explicitly before
        // returning Err; Auto mode relies on Err for CPU fallback.
        let built = (|| -> Result<(Rc<dyn Gl>, glow::Context, WindowsServoFramebuffer)> {
            let surface = device
                .create_surface(
                    &context,
                    SurfaceAccess::GPUOnly,
                    SurfaceType::Generic {
                        size: Size2D::new(width as i32, height as i32),
                    },
                )
                .map_err(context_error("create context surface"))?;
            bind_surface(&device, &mut context, surface)?;
            device
                .make_context_current(&context)
                .map_err(context_error("make context current"))?;

            let gleam_gl = load_gleam_gl(&device, &context, connection.gl_api());
            let glow_gl = load_glow_gl(&device, &context);

            let native_device = device.native_device();
            // SAFETY: native_device.d3d11_device is an owned AddRef returned by Surfman.
            let d3d11 = unsafe {
                WindowsD3d11Device::from_owned_raw_d3d11_device(
                    native_device.d3d11_device.cast::<c_void>(),
                )?
            };
            let framebuffer = WindowsServoFramebuffer::new(
                Rc::clone(&gleam_gl),
                &device,
                &mut context,
                &d3d11,
                size,
                1,
                1,
            )?;
            Ok((gleam_gl, glow_gl, framebuffer))
        })();

        let (gleam_gl, glow_gl, framebuffer) = match built {
            Ok(parts) => parts,
            Err(error) => {
                if let Ok(Some(surface)) = device.unbind_surface_from_context(&mut context) {
                    destroy_surface_or_forget(&device, &mut context, surface);
                }
                let _ = device.destroy_context(&mut context);
                return Err(error);
            }
        };

        Ok(Self {
            size: Cell::new(size),
            gleam_gl,
            glow_gl: Arc::new(glow_gl),
            device: RefCell::new(device),
            context: RefCell::new(context),
            framebuffer: RefCell::new(Some(framebuffer)),
        })
    }

    /// Returns the newest published ring slot for import.
    ///
    /// The returned slot's blit fence has signaled, so Vulkan can sample the
    /// shared texture without racing Servo's in-flight GL writes. Repeated
    /// calls without a new [`RenderingContext::present`] publish return the
    /// same slot and content generation.
    pub fn native_frame(&self) -> Result<WindowsServoNativeFrame> {
        self.make_current()
            .map_err(context_error("make context current for native frame"))?;
        let mut framebuffer = self.framebuffer.borrow_mut();
        let framebuffer = framebuffer
            .as_mut()
            .ok_or(WindowsGpuInteropError::MissingWindowsAngleContext)?;
        framebuffer.acquire_native_frame()
    }

    fn resize_framebuffer(&self, size: PhysicalSize<u32>) -> Result<()> {
        self.make_current()
            .map_err(context_error("make context current for resize"))?;
        let device = self.device.borrow();
        let mut context = self.context.borrow_mut();
        let native_device = device.native_device();
        // SAFETY: native_device.d3d11_device is an owned AddRef returned by Surfman.
        let d3d11 = unsafe {
            WindowsD3d11Device::from_owned_raw_d3d11_device(
                native_device.d3d11_device.cast::<c_void>(),
            )?
        };
        let (next_generation, next_ring_epoch) =
            self.framebuffer
                .borrow()
                .as_ref()
                .map_or((1, 1), |current| {
                    (
                        current.next_generation(),
                        current.ring_epoch().saturating_add(1),
                    )
                });
        let framebuffer = WindowsServoFramebuffer::new(
            Rc::clone(&self.gleam_gl),
            &device,
            &mut context,
            &d3d11,
            size,
            next_generation,
            next_ring_epoch,
        )?;
        if let Some(mut previous) = self.framebuffer.borrow_mut().replace(framebuffer) {
            previous.destroy(&device, &mut context);
        }
        Ok(())
    }
}

impl WindowsD3d11SharedTextureImporter {
    /// Imports a native frame produced by `WindowsAngleRenderingContext`.
    pub fn import_servo_native_frame(
        &mut self,
        device: &wgpu::Device,
        frame: WindowsServoNativeFrame,
    ) -> Result<ImportedEffectFrame> {
        self.reset_cache_for_ring_epoch(frame.ring_epoch);
        // SAFETY: WindowsServoNativeFrame is only produced by the ANGLE
        // context after native_frame observes a completed GL blit fence.
        unsafe {
            self.import_shared_handle(
                device,
                frame.shared_handle,
                frame.content_generation,
                frame.sync_us,
            )
        }
    }
}

impl Drop for WindowsAngleRenderingContext {
    fn drop(&mut self) {
        let device = self.device.get_mut();
        let context = self.context.get_mut();
        let _ = device.make_context_current(context);
        if let Some(mut framebuffer) = self.framebuffer.get_mut().take() {
            framebuffer.destroy(device, context);
        }
        if let Ok(Some(surface)) = device.unbind_surface_from_context(context) {
            destroy_surface_or_forget(device, context, surface);
        }
        let _ = device.destroy_context(context);
    }
}

impl RenderingContext for WindowsAngleRenderingContext {
    fn prepare_for_rendering(&self) {
        if let Some(framebuffer) = self.framebuffer.borrow().as_ref() {
            framebuffer.bind();
        } else {
            self.gleam_gl.bind_framebuffer(gl::FRAMEBUFFER, 0);
        }
    }

    fn read_to_image(&self, source_rectangle: DeviceIntRect) -> Option<RgbaImage> {
        let width = source_rectangle.width();
        let height = source_rectangle.height();
        if width <= 0 || height <= 0 {
            return None;
        }

        self.prepare_for_rendering();
        self.gleam_gl.bind_vertex_array(0);
        let mut pixels = self.gleam_gl.read_pixels(
            source_rectangle.min.x,
            source_rectangle.min.y,
            width,
            height,
            gl::RGBA,
            gl::UNSIGNED_BYTE,
        );
        let width = usize::try_from(width).ok()?;
        let height = usize::try_from(height).ok()?;
        let stride = width.checked_mul(4)?;
        if pixels.len() != stride.checked_mul(height)? {
            return None;
        }
        flip_rows_in_place(&mut pixels, stride);
        RgbaImage::from_raw(width.try_into().ok()?, height.try_into().ok()?, pixels)
    }

    fn size(&self) -> PhysicalSize<u32> {
        self.size.get()
    }

    fn resize(&self, size: PhysicalSize<u32>) {
        if self.size.get() == size {
            return;
        }
        match validate_size(size.width, size.height).and_then(|_| self.resize_framebuffer(size)) {
            Ok(()) => self.size.set(size),
            Err(error) => tracing::warn!(%error, "failed to resize Windows ANGLE Servo context"),
        }
    }

    fn present(&self) {
        // Publish the render target into the next ring slot. The blit is
        // fenced (not glFinish-ed); `native_frame` only hands out slots whose
        // fences have signaled.
        if let Err(error) = self.make_current() {
            tracing::warn!(
                ?error,
                "Windows Servo present skipped: context could not be made current"
            );
            return;
        }
        if let Some(framebuffer) = self.framebuffer.borrow_mut().as_mut()
            && let Err(error) = framebuffer.copy_to_shared_texture()
        {
            tracing::warn!(%error, "Windows Servo shared-texture publish failed");
        }
    }

    fn make_current(&self) -> std::result::Result<(), Error> {
        let device = self.device.borrow();
        let context = self.context.borrow();
        device.make_context_current(&context)
    }

    fn gleam_gl_api(&self) -> Rc<dyn Gl> {
        Rc::clone(&self.gleam_gl)
    }

    fn glow_gl_api(&self) -> Arc<glow::Context> {
        Arc::clone(&self.glow_gl)
    }

    fn create_texture(&self, surface: Surface) -> Option<(SurfaceTexture, u32, Size2D<i32>)> {
        let device = self.device.borrow();
        let mut context = self.context.borrow_mut();
        let SurfaceInfo { size, .. } = device.surface_info(&surface);
        let surface_texture = match device.create_surface_texture(&mut context, surface) {
            Ok(surface_texture) => surface_texture,
            Err((_, surface)) => {
                destroy_surface_or_forget(&device, &mut context, surface);
                return None;
            }
        };
        let gl_texture = device
            .surface_texture_object(&surface_texture)
            .map_or(0, |texture| texture.0.get());
        Some((surface_texture, gl_texture, size))
    }

    fn destroy_texture(&self, surface_texture: SurfaceTexture) -> Option<Surface> {
        let device = self.device.borrow();
        let mut context = self.context.borrow_mut();
        match device.destroy_surface_texture(&mut context, surface_texture) {
            Ok(surface) => Some(surface),
            Err((error, surface_texture)) => {
                std::mem::forget(surface_texture);
                tracing::warn!(?error, "failed to destroy Windows ANGLE surface texture");
                None
            }
        }
    }

    fn connection(&self) -> Option<Connection> {
        Some(self.device.borrow().connection())
    }
}

/// One publish target in the D3D11 shared-texture ring.
struct WindowsSharedRingSlot {
    texture: WindowsD3d11SharedTexture,
    /// ANGLE binding of `texture` as a GL texture; kept alive so the slot
    /// framebuffer stays valid. Must be destroyed through the Surfman device.
    surface_texture: Option<SurfaceTexture>,
    framebuffer_id: u32,
    /// Fence inserted after the most recent blit into this slot; `None` once
    /// the blit is known complete (or the slot was never written).
    fence: Option<gl::GLsync>,
    /// Content version of the most recent blit; `0` means never written.
    content_generation: u64,
}

struct WindowsServoFramebuffer {
    gl: Rc<dyn Gl>,
    size: PhysicalSize<u32>,
    render_framebuffer_id: u32,
    render_texture_id: u32,
    depth_stencil_renderbuffer_id: u32,
    slots: Vec<WindowsSharedRingSlot>,
    /// Ring cursor: index after the most recently written slot.
    next_slot: usize,
    /// Slot most recently handed to the Vulkan consumer; never reused for a
    /// blit while it remains the newest completed frame.
    last_ready_slot: Option<usize>,
    /// Next content generation to assign; survives ring rebuilds.
    next_generation: u64,
    /// Ring identity; bumps on every rebuild so handle-keyed caches reset.
    ring_epoch: u64,
}

impl WindowsServoFramebuffer {
    fn new(
        gl: Rc<dyn Gl>,
        device: &Device,
        context: &mut Context,
        d3d11: &WindowsD3d11Device,
        size: PhysicalSize<u32>,
        next_generation: u64,
        ring_epoch: u64,
    ) -> Result<Self> {
        let mut framebuffer = Self {
            gl,
            size,
            render_framebuffer_id: 0,
            render_texture_id: 0,
            depth_stencil_renderbuffer_id: 0,
            slots: Vec::with_capacity(WINDOWS_SERVO_RING_SLOTS),
            next_slot: 0,
            last_ready_slot: None,
            next_generation,
            ring_epoch,
        };
        if let Err(error) = framebuffer.init(device, context, d3d11) {
            framebuffer.destroy(device, context);
            return Err(error);
        }
        Ok(framebuffer)
    }

    fn init(
        &mut self,
        device: &Device,
        context: &mut Context,
        d3d11: &WindowsD3d11Device,
    ) -> Result<()> {
        let gl = Rc::clone(&self.gl);
        let gl = gl.as_ref();
        let size = self.size;

        self.render_framebuffer_id = single_gl_id(gl.gen_framebuffers(1), "framebuffer")?;
        gl.bind_framebuffer(gl::FRAMEBUFFER, self.render_framebuffer_id);

        self.render_texture_id = single_gl_id(gl.gen_textures(1), "texture")?;
        gl.bind_texture(gl::TEXTURE_2D, self.render_texture_id);
        gl.tex_image_2d(
            gl::TEXTURE_2D,
            0,
            gl::RGBA as gl::GLint,
            size.width as gl::GLsizei,
            size.height as gl::GLsizei,
            0,
            gl::RGBA,
            gl::UNSIGNED_BYTE,
            None,
        );
        gl.tex_parameter_i(
            gl::TEXTURE_2D,
            gl::TEXTURE_MAG_FILTER,
            gl::NEAREST as gl::GLint,
        );
        gl.tex_parameter_i(
            gl::TEXTURE_2D,
            gl::TEXTURE_MIN_FILTER,
            gl::NEAREST as gl::GLint,
        );
        gl.tex_parameter_i(
            gl::TEXTURE_2D,
            gl::TEXTURE_WRAP_S,
            gl::CLAMP_TO_EDGE as gl::GLint,
        );
        gl.tex_parameter_i(
            gl::TEXTURE_2D,
            gl::TEXTURE_WRAP_T,
            gl::CLAMP_TO_EDGE as gl::GLint,
        );
        check_gl_error(gl, "glTexImage2D")?;
        gl.framebuffer_texture_2d(
            gl::FRAMEBUFFER,
            gl::COLOR_ATTACHMENT0,
            gl::TEXTURE_2D,
            self.render_texture_id,
            0,
        );

        self.depth_stencil_renderbuffer_id = single_gl_id(gl.gen_renderbuffers(1), "renderbuffer")?;
        gl.bind_renderbuffer(gl::RENDERBUFFER, self.depth_stencil_renderbuffer_id);
        gl.renderbuffer_storage(
            gl::RENDERBUFFER,
            gl::DEPTH24_STENCIL8,
            size.width as gl::GLsizei,
            size.height as gl::GLsizei,
        );
        gl.framebuffer_renderbuffer(
            gl::FRAMEBUFFER,
            gl::DEPTH_STENCIL_ATTACHMENT,
            gl::RENDERBUFFER,
            self.depth_stencil_renderbuffer_id,
        );
        gl.bind_renderbuffer(gl::RENDERBUFFER, 0);
        check_gl_error(gl, "glFramebufferRenderbuffer")?;

        let status = gl.check_frame_buffer_status(gl::FRAMEBUFFER);
        if status != gl::FRAMEBUFFER_COMPLETE {
            return Err(WindowsGpuInteropError::GlFramebufferIncomplete { status });
        }

        for _ in 0..WINDOWS_SERVO_RING_SLOTS {
            let slot = create_ring_slot(gl, device, context, d3d11, size)?;
            self.slots.push(slot);
        }

        gl.bind_texture(gl::TEXTURE_2D, 0);
        gl.bind_framebuffer(gl::FRAMEBUFFER, self.render_framebuffer_id);
        Ok(())
    }

    fn bind(&self) {
        self.gl
            .bind_framebuffer(gl::FRAMEBUFFER, self.render_framebuffer_id);
    }

    const fn next_generation(&self) -> u64 {
        self.next_generation
    }

    const fn ring_epoch(&self) -> u64 {
        self.ring_epoch
    }

    /// Blits the render target into the next available ring slot and fences
    /// the copy, without stalling the CPU on the GPU.
    fn copy_to_shared_texture(&mut self) -> Result<()> {
        let slot_index = self.acquire_blit_slot()?;
        let gl = self.gl.as_ref();
        gl.bind_framebuffer(GL_READ_FRAMEBUFFER, self.render_framebuffer_id);
        gl.bind_framebuffer(GL_DRAW_FRAMEBUFFER, self.slots[slot_index].framebuffer_id);
        // gleam's GLES backend panics on glReadBuffer; framebuffer objects
        // already default their read buffer to COLOR_ATTACHMENT0.
        gl.draw_buffers(&[gl::COLOR_ATTACHMENT0]);
        gl.blit_framebuffer(
            0,
            0,
            self.size.width as gl::GLint,
            self.size.height as gl::GLint,
            0,
            0,
            self.size.width as gl::GLint,
            self.size.height as gl::GLint,
            gl::COLOR_BUFFER_BIT,
            gl::NEAREST,
        );
        let blit_result = check_gl_error(gl, "glBlitFramebuffer");
        self.bind();
        blit_result?;

        let fence = create_blit_fence(self.gl.as_ref())?;
        // Flush so the blit and fence reach the GPU promptly; consumers wait
        // on the fence instead of a full glFinish.
        self.gl.flush();
        let generation = self.next_generation;
        self.next_generation += 1;
        let slot = &mut self.slots[slot_index];
        slot.fence = Some(fence);
        slot.content_generation = generation;
        self.next_slot = (slot_index + 1) % self.slots.len();
        Ok(())
    }

    /// Picks the next reusable ring slot for a blit.
    ///
    /// Scans in ring order, skipping the consumer-visible slot, and accepts
    /// slots whose previous blit fence is absent or signaled. When every
    /// candidate is still in flight, bounded-waits the oldest one.
    fn acquire_blit_slot(&mut self) -> Result<usize> {
        for offset in 0..self.slots.len() {
            let index = (self.next_slot + offset) % self.slots.len();
            if self.last_ready_slot == Some(index) {
                continue;
            }
            match self.slots[index].fence {
                None => return Ok(index),
                Some(fence) => {
                    if fence_signaled(self.gl.as_ref(), fence)? {
                        self.gl.delete_sync(fence);
                        self.slots[index].fence = None;
                        return Ok(index);
                    }
                }
            }
        }

        let oldest = self
            .slots
            .iter()
            .enumerate()
            .filter(|(index, slot)| self.last_ready_slot != Some(*index) && slot.fence.is_some())
            .min_by_key(|(_, slot)| slot.content_generation)
            .map(|(index, _)| index)
            .ok_or(WindowsGpuInteropError::PublishFenceTimeout)?;
        let Some(fence) = self.slots[oldest].fence else {
            return Ok(oldest);
        };
        if wait_fence_bounded(self.gl.as_ref(), fence)? {
            self.gl.delete_sync(fence);
            self.slots[oldest].fence = None;
            Ok(oldest)
        } else {
            Err(WindowsGpuInteropError::PublishFenceTimeout)
        }
    }

    /// Returns the newest ring slot whose blit has completed.
    fn acquire_native_frame(&mut self) -> Result<WindowsServoNativeFrame> {
        // First-frame case: nothing has ever been published, so blit the
        // current render target before looking for a completed slot.
        if self.slots.iter().all(|slot| slot.content_generation == 0) {
            self.copy_to_shared_texture()?;
        }

        // Retire every signaled fence so completed slots become visible to
        // the consumer and reusable for later blits.
        for index in 0..self.slots.len() {
            if let Some(fence) = self.slots[index].fence
                && fence_signaled(self.gl.as_ref(), fence)?
            {
                self.gl.delete_sync(fence);
                self.slots[index].fence = None;
            }
        }

        if let Some(index) = self.newest_ready_slot() {
            self.last_ready_slot = Some(index);
            return Ok(self.frame_for_slot(index, 0));
        }

        // No blit has completed yet: bounded-wait the newest pending fence.
        let pending = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.fence.is_some())
            .max_by_key(|(_, slot)| slot.content_generation)
            .map(|(index, _)| index)
            .ok_or(WindowsGpuInteropError::PublishFenceTimeout)?;
        let Some(fence) = self.slots[pending].fence else {
            return Err(WindowsGpuInteropError::PublishFenceTimeout);
        };
        let sync_start = Instant::now();
        if wait_fence_bounded(self.gl.as_ref(), fence)? {
            let sync_us = elapsed_micros(sync_start);
            self.gl.delete_sync(fence);
            self.slots[pending].fence = None;
            self.last_ready_slot = Some(pending);
            Ok(self.frame_for_slot(pending, sync_us))
        } else {
            Err(WindowsGpuInteropError::PublishFenceTimeout)
        }
    }

    fn newest_ready_slot(&self) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.fence.is_none() && slot.content_generation != 0)
            .max_by_key(|(_, slot)| slot.content_generation)
            .map(|(index, _)| index)
    }

    fn frame_for_slot(&self, index: usize, sync_us: u64) -> WindowsServoNativeFrame {
        let slot = &self.slots[index];
        WindowsServoNativeFrame {
            width: self.size.width,
            height: self.size.height,
            format: ImportedFrameFormat::Bgra8Unorm,
            origin: WindowsServoFrameOrigin::BottomLeft,
            slot_index: index,
            shared_handle: slot.texture.shared_handle(),
            ring_epoch: self.ring_epoch,
            content_generation: slot.content_generation,
            sync_us,
        }
    }

    /// Releases every GL and Surfman resource owned by this framebuffer.
    ///
    /// Must be called with the owning ANGLE context current. After this, the
    /// plain `Drop` impl has nothing left to release.
    fn destroy(&mut self, device: &Device, context: &mut Context) {
        let gl = Rc::clone(&self.gl);
        let gl = gl.as_ref();
        gl.bind_framebuffer(gl::FRAMEBUFFER, 0);
        for mut slot in self.slots.drain(..) {
            if let Some(fence) = slot.fence.take() {
                gl.delete_sync(fence);
            }
            if slot.framebuffer_id != 0 {
                gl.delete_framebuffers(&[slot.framebuffer_id]);
            }
            if let Some(surface_texture) = slot.surface_texture.take() {
                destroy_slot_surface_texture(device, context, surface_texture);
            }
        }
        if self.render_texture_id != 0 {
            gl.delete_textures(&[self.render_texture_id]);
            self.render_texture_id = 0;
        }
        if self.depth_stencil_renderbuffer_id != 0 {
            gl.delete_renderbuffers(&[self.depth_stencil_renderbuffer_id]);
            self.depth_stencil_renderbuffer_id = 0;
        }
        if self.render_framebuffer_id != 0 {
            gl.delete_framebuffers(&[self.render_framebuffer_id]);
            self.render_framebuffer_id = 0;
        }
    }
}

impl Drop for WindowsServoFramebuffer {
    fn drop(&mut self) {
        for slot in &mut self.slots {
            if let Some(surface_texture) = slot.surface_texture.take() {
                // destroy() did not run, so the Surfman device is gone;
                // forgetting avoids Surfman's drop panic at the cost of a
                // leak that context destruction reclaims.
                mem::forget(surface_texture);
            }
        }
    }
}

/// Creates one D3D11 shared-texture FBO slot for the publish ring.
fn create_ring_slot(
    gl: &dyn Gl,
    device: &Device,
    context: &mut Context,
    d3d11: &WindowsD3d11Device,
    size: PhysicalSize<u32>,
) -> Result<WindowsSharedRingSlot> {
    let descriptor = WindowsD3d11SharedTextureImportDescriptor::new(
        size.width,
        size.height,
        ImportedFrameFormat::Bgra8Unorm,
    )?;
    let texture = d3d11.create_shared_texture(descriptor)?;
    // SAFETY: texture stays alive in the slot while the Surfman binding exists.
    let surfman_texture = unsafe { texture.to_surfman_texture() };
    let surfman_size = Size2D::new(size.width as i32, size.height as i32);
    // SAFETY: the surface texture is used on this Servo worker thread, and the
    // D3D11 texture remains owned by the slot for at least as long.
    let surface_texture = unsafe {
        device
            .create_surface_texture_from_texture(context, &surfman_size, surfman_texture)
            .map_err(context_error(
                crate::WINDOWS_ANGLE_CLIENT_BUFFER_SURFACE_OPERATION,
            ))?
    };
    let gl_texture = device
        .surface_texture_object(&surface_texture)
        .map_or(0, |texture| texture.0.get());

    let mut framebuffer_id = 0;
    let result = (|| {
        if gl_texture == 0 {
            return Err(WindowsGpuInteropError::GlCreateResource {
                resource: "surface texture",
                message: "Surfman returned no GL texture for the shared slot".to_owned(),
            });
        }
        framebuffer_id = single_gl_id(gl.gen_framebuffers(1), "framebuffer")?;
        gl.bind_framebuffer(gl::FRAMEBUFFER, framebuffer_id);
        gl.framebuffer_texture_2d(
            gl::FRAMEBUFFER,
            gl::COLOR_ATTACHMENT0,
            gl::TEXTURE_2D,
            gl_texture,
            0,
        );
        check_gl_error(gl, "glFramebufferTexture2D")?;

        let status = gl.check_frame_buffer_status(gl::FRAMEBUFFER);
        if status != gl::FRAMEBUFFER_COMPLETE {
            return Err(WindowsGpuInteropError::GlFramebufferIncomplete { status });
        }
        Ok(())
    })();

    if let Err(error) = result {
        if framebuffer_id != 0 {
            gl.delete_framebuffers(&[framebuffer_id]);
        }
        destroy_slot_surface_texture(device, context, surface_texture);
        return Err(error);
    }

    Ok(WindowsSharedRingSlot {
        texture,
        surface_texture: Some(surface_texture),
        framebuffer_id,
        fence: None,
        content_generation: 0,
    })
}

/// Destroys a ring slot's Surfman binding, including the client-buffer
/// surface beneath it.
fn destroy_slot_surface_texture(
    device: &Device,
    context: &mut Context,
    surface_texture: SurfaceTexture,
) {
    match device.destroy_surface_texture(context, surface_texture) {
        Ok(surface) => destroy_surface_or_forget(device, context, surface),
        Err((error, surface_texture)) => {
            mem::forget(surface_texture);
            tracing::warn!(?error, "failed to destroy Windows shared slot binding");
        }
    }
}

/// Inserts a fence after the slot blit so consumers can wait on the copy
/// without a full pipeline sync.
fn create_blit_fence(gl: &dyn Gl) -> Result<gl::GLsync> {
    let fence = gl.fence_sync(gl::SYNC_GPU_COMMANDS_COMPLETE, 0);
    if fence.is_null() {
        return Err(WindowsGpuInteropError::GlCreateResource {
            resource: "sync object",
            message: "glFenceSync returned null".to_owned(),
        });
    }
    check_gl_error(gl, "glFenceSync")?;
    Ok(fence)
}

/// Non-blocking fence poll; `Ok(true)` when the blit has completed.
fn fence_signaled(gl: &dyn Gl, fence: gl::GLsync) -> Result<bool> {
    match gl.client_wait_sync(fence, 0, 0) {
        gl::ALREADY_SIGNALED | gl::CONDITION_SATISFIED => Ok(true),
        gl::TIMEOUT_EXPIRED => Ok(false),
        code => Err(WindowsGpuInteropError::GlOperation {
            operation: "glClientWaitSync",
            code,
        }),
    }
}

/// Bounded fence wait; `Ok(true)` when the blit completed within the window.
///
/// mozangle's `glClientWaitSync` does not reliably block on a nonzero
/// timeout, so this polls with a flush on the first check and an explicit
/// deadline.
fn wait_fence_bounded(gl: &dyn Gl, fence: gl::GLsync) -> Result<bool> {
    let deadline = Instant::now() + PUBLISH_FENCE_TIMEOUT;
    match gl.client_wait_sync(fence, gl::SYNC_FLUSH_COMMANDS_BIT, 0) {
        gl::ALREADY_SIGNALED | gl::CONDITION_SATISFIED => return Ok(true),
        gl::TIMEOUT_EXPIRED => {}
        code => {
            return Err(WindowsGpuInteropError::GlOperation {
                operation: "glClientWaitSync",
                code,
            });
        }
    }
    while Instant::now() < deadline {
        std::thread::sleep(PUBLISH_FENCE_POLL_INTERVAL);
        if fence_signaled(gl, fence)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn single_gl_id(ids: Vec<u32>, resource: &'static str) -> Result<u32> {
    ids.into_iter()
        .find(|id| *id != 0)
        .ok_or_else(|| WindowsGpuInteropError::GlCreateResource {
            resource,
            message: "driver returned no object name".to_owned(),
        })
}

fn check_gl_error(gl: &dyn Gl, operation: &'static str) -> Result<()> {
    let code = gl.get_error();
    if code == gl::NO_ERROR {
        Ok(())
    } else {
        Err(WindowsGpuInteropError::GlOperation { operation, code })
    }
}

fn create_context(device: &Device, connection: &Connection) -> Result<Context> {
    let flags = ContextAttributeFlags::ALPHA
        | ContextAttributeFlags::DEPTH
        | ContextAttributeFlags::STENCIL;
    let version = match connection.gl_api() {
        GLApi::GLES => surfman::GLVersion { major: 3, minor: 0 },
        GLApi::GL => surfman::GLVersion { major: 3, minor: 2 },
    };
    let descriptor = device
        .create_context_descriptor(&ContextAttributes { flags, version })
        .map_err(context_error("create context descriptor"))?;
    device
        .create_context(&descriptor, None)
        .map_err(context_error("create context"))
}

fn validate_size(width: u32, height: u32) -> Result<PhysicalSize<u32>> {
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        Err(WindowsGpuInteropError::InvalidDimensions { width, height })
    } else {
        Ok(PhysicalSize::new(width, height))
    }
}

fn adapter_from_identity(identity: WindowsDxgiAdapterIdentity) -> Result<Adapter> {
    let adapter = find_dxgi_adapter(identity)?;
    Ok(Adapter::from_dxgi_adapter(adapter))
}

fn find_dxgi_adapter(identity: WindowsDxgiAdapterIdentity) -> Result<ComPtr<IDXGIAdapter>> {
    // SAFETY: CreateDXGIFactory1 initializes a DXGI COM factory.
    let factory = unsafe {
        let mut factory: *mut IDXGIFactory1 = ptr::null_mut();
        let result = CreateDXGIFactory1(
            &IDXGIFactory1::uuidof(),
            &mut factory as *mut *mut IDXGIFactory1 as *mut *mut c_void,
        );
        if !SUCCEEDED(result) {
            return Err(WindowsGpuInteropError::DxgiFactoryCreateFailed { hresult: result });
        }
        ComPtr::from_raw(factory)
    };

    let mut index = 0;
    loop {
        // SAFETY: factory is live and index advances until DXGI reports not found.
        let adapter_1 = unsafe {
            let mut adapter: *mut IDXGIAdapter1 = ptr::null_mut();
            let result = factory.EnumAdapters1(index, &mut adapter);
            if result == DXGI_ERROR_NOT_FOUND {
                break;
            }
            if !SUCCEEDED(result) {
                return Err(WindowsGpuInteropError::DxgiAdapterQueryFailed { hresult: result });
            }
            ComPtr::from_raw(adapter)
        };
        // SAFETY: adapter_1 is a live DXGI adapter.
        let desc = unsafe {
            let mut desc = mem::zeroed();
            let result = adapter_1.GetDesc1(&mut desc);
            if !SUCCEEDED(result) {
                return Err(WindowsGpuInteropError::DxgiAdapterQueryFailed { hresult: result });
            }
            desc
        };
        let is_software = desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE != 0;
        if !is_software
            && desc.VendorId == identity.vendor_id
            && desc.DeviceId == identity.device_id
        {
            // SAFETY: QueryInterface returns an owned IDXGIAdapter reference on success.
            return unsafe {
                let mut adapter: *mut IDXGIAdapter = ptr::null_mut();
                let result = adapter_1.QueryInterface(
                    &IDXGIAdapter::uuidof(),
                    &mut adapter as *mut *mut IDXGIAdapter as *mut *mut c_void,
                );
                if !SUCCEEDED(result) {
                    return Err(WindowsGpuInteropError::DxgiAdapterQueryFailed { hresult: result });
                }
                Ok(ComPtr::from_raw(adapter))
            };
        }
        index += 1;
    }

    Err(WindowsGpuInteropError::DxgiAdapterNotFound {
        vendor_id: Some(identity.vendor_id),
        device_id: Some(identity.device_id),
    })
}

fn bind_surface(device: &Device, context: &mut Context, surface: Surface) -> Result<()> {
    device
        .bind_surface_to_context(context, surface)
        .map_err(|(error, surface)| {
            destroy_surface_or_forget(device, context, surface);
            context_error("bind ANGLE context surface")(error)
        })
}

fn destroy_surface_or_forget(device: &Device, context: &mut Context, mut surface: Surface) {
    if device.destroy_surface(context, &mut surface).is_err() {
        // destroy_surface failed, so Surfman's Drop would panic; intentionally
        // leak the platform handle instead.
        mem::forget(surface);
    }
}

fn ensure_angle_dlls_loaded() -> std::result::Result<(), String> {
    static RESULT: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    RESULT
        .get_or_init(|| {
            for directory in angle_dll_directories() {
                if directory.join("libEGL.dll").is_file()
                    && directory.join("libGLESv2.dll").is_file()
                {
                    return load_angle_dll_pair(&directory);
                }
            }
            load_angle_library(OsStr::new("libGLESv2.dll"))
                .and_then(|()| load_angle_library(OsStr::new("libEGL.dll")))
        })
        .clone()
}

fn angle_dll_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(binary_dir) = exe_path.parent()
    {
        directories.push(binary_dir.to_path_buf());
        push_mozangle_output_dirs(binary_dir.join("build"), &mut directories);
        if let Some(profile_dir) = binary_dir.parent() {
            push_mozangle_output_dirs(profile_dir.join("build"), &mut directories);
        }
    }
    directories
}

fn push_mozangle_output_dirs(build_dir: PathBuf, directories: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(build_dir) else {
        return;
    };
    for entry in entries.filter_map(std::result::Result::ok) {
        let file_name = entry.file_name();
        if !file_name.to_string_lossy().starts_with("mozangle-") {
            continue;
        }
        let out_dir = entry.path().join("out");
        if out_dir.join("libEGL.dll").is_file() {
            directories.push(out_dir);
        }
    }
}

fn load_angle_dll_pair(directory: &Path) -> std::result::Result<(), String> {
    load_angle_library(directory.join("libGLESv2.dll").as_os_str())
        .and_then(|()| load_angle_library(directory.join("libEGL.dll").as_os_str()))
}

fn load_angle_library(path: &OsStr) -> std::result::Result<(), String> {
    let wide_path = wide_null(path);
    // SAFETY: LoadLibraryW reads a NUL-terminated UTF-16 path. The loaded
    // ANGLE modules intentionally stay resident for the process lifetime.
    unsafe { LoadLibraryW(PCWSTR(wide_path.as_ptr())) }
        .map(|_| ())
        .map_err(|error| format!("{} ({error})", path.to_string_lossy()))
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn load_gleam_gl(device: &Device, context: &Context, gl_api: GLApi) -> Rc<dyn Gl> {
    match gl_api {
        GLApi::GL => {
            // SAFETY: Surfman owns the current platform GL context and returns
            // function pointers valid for that context.
            unsafe { gl::GlFns::load_with(|name| device.get_proc_address(context, name)) }
        }
        GLApi::GLES => {
            // SAFETY: Surfman owns the current platform GLES context and returns
            // function pointers valid for that context.
            unsafe { gl::GlesFns::load_with(|name| device.get_proc_address(context, name)) }
        }
    }
}

fn load_glow_gl(device: &Device, context: &Context) -> glow::Context {
    // SAFETY: Surfman owns the current GL context and returns function pointers
    // valid for that context.
    unsafe { glow::Context::from_loader_function(|name| device.get_proc_address(context, name)) }
}

fn context_error(operation: &'static str) -> impl FnOnce(Error) -> WindowsGpuInteropError {
    move |error| WindowsGpuInteropError::ServoContext {
        operation,
        message: format!("{error:?}"),
    }
}

fn flip_rows_in_place(pixels: &mut [u8], stride: usize) {
    if stride == 0 {
        return;
    }
    let row_count = pixels.len() / stride;
    if row_count < 2 {
        return;
    }
    let mut top = 0;
    let mut bottom = row_count - 1;
    while top < bottom {
        let top_start = top * stride;
        let bottom_start = bottom * stride;
        let (upper, lower) = pixels.split_at_mut(bottom_start);
        upper[top_start..top_start + stride].swap_with_slice(&mut lower[..stride]);
        top += 1;
        bottom -= 1;
    }
}

fn elapsed_micros(start: Instant) -> u64 {
    start.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}
