use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use cgl::{CGLGetCurrentContext, CGLTexImageIOSurface2D, kCGLNoError};
use dpi::PhysicalSize;
use euclid::default::Size2D;
use gleam::gl::{self, Gl};
use image::RgbaImage;
use objc2_core_foundation::CFRetained;
use objc2_io_surface::IOSurfaceRef;
use paint_api::rendering_context::RenderingContext;
use surfman::{
    Connection, Context, ContextAttributeFlags, ContextAttributes, Device, Error, GLApi, Surface,
    SurfaceAccess, SurfaceInfo, SurfaceTexture, SurfaceType,
};
use webrender_api::units::DeviceIntRect;

use crate::{ImportedFrameFormat, MacosGpuInteropError, MacosIosurfaceImportDescriptor, Result};

const GL_TEXTURE_RECTANGLE_ARB: gl::GLenum = 0x84F5;
const GL_READ_FRAMEBUFFER: gl::GLenum = 0x8CA8;
const GL_DRAW_FRAMEBUFFER: gl::GLenum = 0x8CA9;
const GL_UNSIGNED_INT_8_8_8_8_REV: gl::GLenum = 0x8367;

/// Number of IOSurfaces in the publish ring: one being written, one held by
/// the Metal consumer, and one spare so the producer never stalls.
const IOSURFACE_RING_SLOTS: usize = 3;
/// Bounded fence wait (~8ms, half a 60Hz frame) before reporting a transient
/// fence timeout instead of stalling the render thread.
const IOSURFACE_FENCE_TIMEOUT_NS: gl::GLuint64 = 8_000_000;

/// Origin convention for a native macOS Servo frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MacosServoFrameOrigin {
    /// The first row in native framebuffer coordinates is the bottom row.
    BottomLeft,
}

/// Native FBO-backed frame exposed by the macOS Servo hardware context.
#[derive(Debug, Clone)]
pub struct MacosServoNativeFrame {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Native IOSurface pixel format.
    pub format: ImportedFrameFormat,
    /// Native framebuffer origin.
    pub origin: MacosServoFrameOrigin,
    /// IOSurface identity for diagnostics and cache comparisons.
    pub surface_id: usize,
    /// Monotonically increasing content version for this frame.
    ///
    /// Contents changed iff this changed; repeated fetches without a new
    /// publish return the same generation.
    pub content_generation: u64,
    /// Retained IOSurface attached to the Servo render target FBO.
    pub iosurface: CFRetained<IOSurfaceRef>,
}

/// macOS hardware Servo rendering context backed by an IOSurface FBO.
pub struct MacosHardwareRenderingContext {
    size: Cell<PhysicalSize<u32>>,
    gleam_gl: Rc<dyn Gl>,
    glow_gl: Arc<glow::Context>,
    framebuffer: RefCell<Option<MacosServoFramebuffer>>,
    device: RefCell<Device>,
    context: RefCell<Context>,
}

impl MacosHardwareRenderingContext {
    /// Creates a hardware OpenGL context with an IOSurface-backed FBO render target.
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let size = validate_size(width, height)?;
        let connection = Connection::new().map_err(context_error("create connection"))?;
        let adapter = connection
            .create_adapter()
            .map_err(context_error("create adapter"))?;
        let device = connection
            .create_device(&adapter)
            .map_err(context_error("create device"))?;
        let mut context = create_context(&device, &connection)?;
        let gleam_gl = load_gleam_gl(&device, &context, connection.gl_api());
        let glow_gl = load_glow_gl(&device, &context);
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
        let framebuffer = MacosServoFramebuffer::new(Rc::clone(&gleam_gl), size, 1)?;

        Ok(Self {
            size: Cell::new(size),
            gleam_gl,
            glow_gl: Arc::new(glow_gl),
            framebuffer: RefCell::new(Some(framebuffer)),
            device: RefCell::new(device),
            context: RefCell::new(context),
        })
    }

    /// Returns the newest published IOSurface ring slot for import.
    ///
    /// The returned slot's blit fence has signaled, so Metal can sample the
    /// IOSurface without racing Servo's in-flight GL writes. Repeated calls
    /// without a new [`RenderingContext::present`] publish return the same
    /// slot and content generation.
    pub fn native_frame(&self) -> Result<MacosServoNativeFrame> {
        self.make_current()
            .map_err(context_error("make context current for native frame"))?;
        let mut framebuffer = self.framebuffer.borrow_mut();
        let framebuffer = framebuffer
            .as_mut()
            .ok_or(MacosGpuInteropError::MissingServoSurface)?;
        framebuffer.acquire_native_frame()
    }

    fn resize_framebuffer(&self, size: PhysicalSize<u32>) -> Result<()> {
        self.make_current()
            .map_err(context_error("make context current for resize"))?;
        let next_generation = self
            .framebuffer
            .borrow()
            .as_ref()
            .map_or(1, MacosServoFramebuffer::next_generation);
        let framebuffer =
            MacosServoFramebuffer::new(Rc::clone(&self.gleam_gl), size, next_generation)?;
        *self.framebuffer.borrow_mut() = Some(framebuffer);
        Ok(())
    }
}

impl Drop for MacosHardwareRenderingContext {
    fn drop(&mut self) {
        let device = self.device.get_mut();
        let context = self.context.get_mut();
        let _ = device.make_context_current(context);
        drop(self.framebuffer.get_mut().take());
        if let Ok(Some(surface)) = device.unbind_surface_from_context(context) {
            destroy_surface_or_forget(device, context, surface);
        }
        let _ = device.destroy_context(context);
    }
}

impl RenderingContext for MacosHardwareRenderingContext {
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
        match self.resize_framebuffer(size) {
            Ok(()) => self.size.set(size),
            Err(error) => tracing::warn!(%error, "failed to resize macOS Servo hardware context"),
        }
    }

    fn present(&self) {
        // Publish the render target into the next IOSurface ring slot. The
        // blit is fenced (not glFinish-ed); `native_frame` only hands out
        // slots whose fences have signaled.
        if let Err(error) = self.make_current() {
            tracing::warn!(
                ?error,
                "macOS Servo present skipped: context could not be made current"
            );
            return;
        }
        if let Some(framebuffer) = self.framebuffer.borrow_mut().as_mut()
            && let Err(error) = framebuffer.copy_to_iosurface()
        {
            tracing::warn!(%error, "macOS Servo IOSurface publish failed");
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
                tracing::warn!(?error, "failed to destroy macOS Servo surface texture");
                None
            }
        }
    }

    fn connection(&self) -> Option<Connection> {
        Some(self.device.borrow().connection())
    }
}

/// One publish target in the IOSurface ring.
struct IosurfaceRingSlot {
    iosurface: CFRetained<IOSurfaceRef>,
    framebuffer_id: u32,
    texture_id: u32,
    /// Fence inserted after the most recent blit into this slot; `None` once
    /// the blit is known complete (or the slot was never written).
    fence: Option<gl::GLsync>,
    /// Content version of the most recent blit; `0` means never written.
    content_generation: u64,
}

struct MacosServoFramebuffer {
    gl: Rc<dyn Gl>,
    size: PhysicalSize<u32>,
    render_framebuffer_id: u32,
    render_texture_id: u32,
    depth_stencil_renderbuffer_id: u32,
    slots: Vec<IosurfaceRingSlot>,
    /// Ring cursor: index after the most recently written slot.
    next_slot: usize,
    /// Slot most recently handed to the Metal consumer; never reused for a
    /// blit while it remains the newest completed frame.
    last_ready_slot: Option<usize>,
    /// Next content generation to assign; survives ring rebuilds.
    next_generation: u64,
}

impl MacosServoFramebuffer {
    fn new(gl: Rc<dyn Gl>, size: PhysicalSize<u32>, next_generation: u64) -> Result<Self> {
        let mut framebuffer = Self {
            gl,
            size,
            render_framebuffer_id: 0,
            render_texture_id: 0,
            depth_stencil_renderbuffer_id: 0,
            slots: Vec::with_capacity(IOSURFACE_RING_SLOTS),
            next_slot: 0,
            last_ready_slot: None,
            next_generation,
        };
        // On error, dropping the partially initialized framebuffer releases
        // every GL resource created so far.
        framebuffer.init()?;
        Ok(framebuffer)
    }

    fn init(&mut self) -> Result<()> {
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
            return Err(MacosGpuInteropError::GlFramebufferIncomplete { status });
        }

        for _ in 0..IOSURFACE_RING_SLOTS {
            let slot = create_ring_slot(gl, size)?;
            self.slots.push(slot);
        }

        gl.bind_texture(gl::TEXTURE_2D, 0);
        gl.bind_texture(GL_TEXTURE_RECTANGLE_ARB, 0);
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

    /// Blits the render target into the next available ring slot and fences
    /// the copy, without stalling the CPU on the GPU.
    fn copy_to_iosurface(&mut self) -> Result<()> {
        let slot_index = self.acquire_blit_slot()?;
        let gl = self.gl.as_ref();
        gl.bind_framebuffer(GL_READ_FRAMEBUFFER, self.render_framebuffer_id);
        gl.bind_framebuffer(GL_DRAW_FRAMEBUFFER, self.slots[slot_index].framebuffer_id);
        gl.read_buffer(gl::COLOR_ATTACHMENT0);
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
            .ok_or(MacosGpuInteropError::IosurfaceFenceTimeout)?;
        let Some(fence) = self.slots[oldest].fence else {
            return Ok(oldest);
        };
        if wait_fence_bounded(self.gl.as_ref(), fence)? {
            self.gl.delete_sync(fence);
            self.slots[oldest].fence = None;
            Ok(oldest)
        } else {
            Err(MacosGpuInteropError::IosurfaceFenceTimeout)
        }
    }

    /// Returns the newest ring slot whose blit has completed.
    fn acquire_native_frame(&mut self) -> Result<MacosServoNativeFrame> {
        // First-frame case: nothing has ever been published, so blit the
        // current render target before looking for a completed slot.
        if self.slots.iter().all(|slot| slot.content_generation == 0) {
            self.copy_to_iosurface()?;
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
            return Ok(self.frame_for_slot(index));
        }

        // No blit has completed yet: bounded-wait the newest pending fence.
        let pending = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.fence.is_some())
            .max_by_key(|(_, slot)| slot.content_generation)
            .map(|(index, _)| index)
            .ok_or(MacosGpuInteropError::IosurfaceFenceTimeout)?;
        let Some(fence) = self.slots[pending].fence else {
            return Err(MacosGpuInteropError::IosurfaceFenceTimeout);
        };
        if wait_fence_bounded(self.gl.as_ref(), fence)? {
            self.gl.delete_sync(fence);
            self.slots[pending].fence = None;
            self.last_ready_slot = Some(pending);
            Ok(self.frame_for_slot(pending))
        } else {
            Err(MacosGpuInteropError::IosurfaceFenceTimeout)
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

    fn frame_for_slot(&self, index: usize) -> MacosServoNativeFrame {
        let slot = &self.slots[index];
        MacosServoNativeFrame {
            width: self.size.width,
            height: self.size.height,
            format: ImportedFrameFormat::Bgra8Unorm,
            origin: MacosServoFrameOrigin::BottomLeft,
            surface_id: usize::try_from(slot.iosurface.id()).unwrap_or(usize::MAX),
            content_generation: slot.content_generation,
            iosurface: slot.iosurface.clone(),
        }
    }
}

impl Drop for MacosServoFramebuffer {
    fn drop(&mut self) {
        let gl = self.gl.as_ref();
        gl.bind_framebuffer(gl::FRAMEBUFFER, 0);
        for slot in &mut self.slots {
            if let Some(fence) = slot.fence.take() {
                gl.delete_sync(fence);
            }
            if slot.texture_id != 0 {
                gl.delete_textures(&[slot.texture_id]);
            }
            if slot.framebuffer_id != 0 {
                gl.delete_framebuffers(&[slot.framebuffer_id]);
            }
        }
        if self.render_texture_id != 0 {
            gl.delete_textures(&[self.render_texture_id]);
        }
        if self.depth_stencil_renderbuffer_id != 0 {
            gl.delete_renderbuffers(&[self.depth_stencil_renderbuffer_id]);
        }
        if self.render_framebuffer_id != 0 {
            gl.delete_framebuffers(&[self.render_framebuffer_id]);
        }
    }
}

/// Creates one IOSurface-backed FBO slot for the publish ring.
fn create_ring_slot(gl: &dyn Gl, size: PhysicalSize<u32>) -> Result<IosurfaceRingSlot> {
    let descriptor = MacosIosurfaceImportDescriptor::new(
        size.width,
        size.height,
        ImportedFrameFormat::Bgra8Unorm,
    )?;
    let iosurface = crate::macos::create_iosurface(descriptor)?;

    let mut framebuffer_id = 0;
    let mut texture_id = 0;
    let result = (|| {
        framebuffer_id = single_gl_id(gl.gen_framebuffers(1), "framebuffer")?;
        gl.bind_framebuffer(gl::FRAMEBUFFER, framebuffer_id);

        texture_id = single_gl_id(gl.gen_textures(1), "texture")?;
        gl.bind_texture(GL_TEXTURE_RECTANGLE_ARB, texture_id);
        bind_iosurface_to_rectangle_texture(&iosurface, size)?;
        gl.tex_parameter_i(
            GL_TEXTURE_RECTANGLE_ARB,
            gl::TEXTURE_MAG_FILTER,
            gl::NEAREST as gl::GLint,
        );
        gl.tex_parameter_i(
            GL_TEXTURE_RECTANGLE_ARB,
            gl::TEXTURE_MIN_FILTER,
            gl::NEAREST as gl::GLint,
        );
        gl.tex_parameter_i(
            GL_TEXTURE_RECTANGLE_ARB,
            gl::TEXTURE_WRAP_S,
            gl::CLAMP_TO_EDGE as gl::GLint,
        );
        gl.tex_parameter_i(
            GL_TEXTURE_RECTANGLE_ARB,
            gl::TEXTURE_WRAP_T,
            gl::CLAMP_TO_EDGE as gl::GLint,
        );
        check_gl_error(gl, "CGLTexImageIOSurface2D")?;
        gl.framebuffer_texture_2d(
            gl::FRAMEBUFFER,
            gl::COLOR_ATTACHMENT0,
            GL_TEXTURE_RECTANGLE_ARB,
            texture_id,
            0,
        );

        let status = gl.check_frame_buffer_status(gl::FRAMEBUFFER);
        if status != gl::FRAMEBUFFER_COMPLETE {
            return Err(MacosGpuInteropError::GlFramebufferIncomplete { status });
        }
        Ok(())
    })();

    if let Err(error) = result {
        if texture_id != 0 {
            gl.delete_textures(&[texture_id]);
        }
        if framebuffer_id != 0 {
            gl.delete_framebuffers(&[framebuffer_id]);
        }
        return Err(error);
    }

    Ok(IosurfaceRingSlot {
        iosurface,
        framebuffer_id,
        texture_id,
        fence: None,
        content_generation: 0,
    })
}

/// Inserts a fence after the slot blit so consumers can wait on the copy
/// without a full pipeline sync.
fn create_blit_fence(gl: &dyn Gl) -> Result<gl::GLsync> {
    let fence = gl.fence_sync(gl::SYNC_GPU_COMMANDS_COMPLETE, 0);
    if fence.is_null() {
        return Err(MacosGpuInteropError::GlCreateResource {
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
        code => Err(MacosGpuInteropError::GlOperation {
            operation: "glClientWaitSync",
            code,
        }),
    }
}

/// Bounded fence wait; `Ok(true)` when the blit completed within the window.
fn wait_fence_bounded(gl: &dyn Gl, fence: gl::GLsync) -> Result<bool> {
    match gl.client_wait_sync(
        fence,
        gl::SYNC_FLUSH_COMMANDS_BIT,
        IOSURFACE_FENCE_TIMEOUT_NS,
    ) {
        gl::ALREADY_SIGNALED | gl::CONDITION_SATISFIED => Ok(true),
        gl::TIMEOUT_EXPIRED => Ok(false),
        code => Err(MacosGpuInteropError::GlOperation {
            operation: "glClientWaitSync",
            code,
        }),
    }
}

fn single_gl_id(ids: Vec<u32>, resource: &'static str) -> Result<u32> {
    ids.into_iter()
        .find(|id| *id != 0)
        .ok_or_else(|| MacosGpuInteropError::GlCreateResource {
            resource,
            message: "driver returned no object name".to_owned(),
        })
}

fn bind_iosurface_to_rectangle_texture(
    iosurface: &IOSurfaceRef,
    size: PhysicalSize<u32>,
) -> Result<()> {
    // SAFETY: Surfman made the CGL context current before FBO creation, and
    // CGLTexImageIOSurface2D only binds storage to the texture currently bound
    // on that context.
    let code = unsafe {
        let context = CGLGetCurrentContext();
        if context.is_null() {
            return Err(MacosGpuInteropError::ServoContext {
                operation: "get current CGL context",
                message: "CGLGetCurrentContext returned null".to_owned(),
            });
        }
        CGLTexImageIOSurface2D(
            context,
            GL_TEXTURE_RECTANGLE_ARB,
            gl::RGBA,
            size.width as gl::GLsizei,
            size.height as gl::GLsizei,
            gl::BGRA,
            GL_UNSIGNED_INT_8_8_8_8_REV,
            iosurface as *const IOSurfaceRef as cgl::IOSurfaceRef,
            0,
        )
    };
    if code == kCGLNoError {
        Ok(())
    } else {
        Err(MacosGpuInteropError::GlOperation {
            operation: "CGLTexImageIOSurface2D",
            code: code as u32,
        })
    }
}

fn check_gl_error(gl: &dyn Gl, operation: &'static str) -> Result<()> {
    let code = gl.get_error();
    if code == gl::NO_ERROR {
        Ok(())
    } else {
        Err(MacosGpuInteropError::GlOperation { operation, code })
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
        Err(MacosGpuInteropError::InvalidDimensions { width, height })
    } else {
        Ok(PhysicalSize::new(width, height))
    }
}

fn bind_surface(device: &Device, context: &mut Context, surface: Surface) -> Result<()> {
    device
        .bind_surface_to_context(context, surface)
        .map_err(|(error, surface)| {
            destroy_surface_or_forget(device, context, surface);
            context_error("bind IOSurface-backed surface")(error)
        })
}

fn destroy_surface_or_forget(device: &Device, context: &mut Context, mut surface: Surface) {
    if device.destroy_surface(context, &mut surface).is_err() {
        std::mem::forget(surface);
    }
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

fn context_error(operation: &'static str) -> impl FnOnce(Error) -> MacosGpuInteropError {
    move |error| MacosGpuInteropError::ServoContext {
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
