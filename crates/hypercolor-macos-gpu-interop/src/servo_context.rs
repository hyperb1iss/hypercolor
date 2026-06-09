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
        let framebuffer = MacosServoFramebuffer::new(Rc::clone(&gleam_gl), size)?;

        Ok(Self {
            size: Cell::new(size),
            gleam_gl,
            glow_gl: Arc::new(glow_gl),
            framebuffer: RefCell::new(Some(framebuffer)),
            device: RefCell::new(device),
            context: RefCell::new(context),
        })
    }

    /// Returns the retained native IOSurface attached to the current render FBO.
    pub fn native_frame(&self) -> Result<MacosServoNativeFrame> {
        self.make_current()
            .map_err(context_error("make context current for native frame"))?;
        let framebuffer = self.framebuffer.borrow();
        let framebuffer = framebuffer
            .as_ref()
            .ok_or(MacosGpuInteropError::MissingServoSurface)?;
        framebuffer.copy_to_iosurface()?;
        // Metal samples this IOSurface immediately after handoff; wait for
        // Servo's GL writes so we don't import an in-flight render target.
        self.gleam_gl.finish();
        Ok(framebuffer.native_frame())
    }

    fn resize_framebuffer(&self, size: PhysicalSize<u32>) -> Result<()> {
        self.make_current()
            .map_err(context_error("make context current for resize"))?;
        let framebuffer = MacosServoFramebuffer::new(Rc::clone(&self.gleam_gl), size)?;
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

    fn present(&self) {}

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

struct MacosServoFramebuffer {
    gl: Rc<dyn Gl>,
    size: PhysicalSize<u32>,
    render_framebuffer_id: u32,
    render_texture_id: u32,
    depth_stencil_renderbuffer_id: u32,
    iosurface_framebuffer_id: u32,
    iosurface_texture_id: u32,
    iosurface: CFRetained<IOSurfaceRef>,
}

impl MacosServoFramebuffer {
    fn new(gl: Rc<dyn Gl>, size: PhysicalSize<u32>) -> Result<Self> {
        let descriptor = MacosIosurfaceImportDescriptor::new(
            size.width,
            size.height,
            ImportedFrameFormat::Bgra8Unorm,
        )?;
        let iosurface = crate::macos::create_iosurface(descriptor)?;

        let mut render_framebuffer_id = 0;
        let mut render_texture_id = 0;
        let mut depth_stencil_renderbuffer_id = 0;
        let mut iosurface_framebuffer_id = 0;
        let mut iosurface_texture_id = 0;
        let result = (|| {
            render_framebuffer_id = single_gl_id(gl.gen_framebuffers(1), "framebuffer")?;
            gl.bind_framebuffer(gl::FRAMEBUFFER, render_framebuffer_id);

            render_texture_id = single_gl_id(gl.gen_textures(1), "texture")?;
            gl.bind_texture(gl::TEXTURE_2D, render_texture_id);
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
            check_gl_error(gl.as_ref(), "glTexImage2D")?;
            gl.framebuffer_texture_2d(
                gl::FRAMEBUFFER,
                gl::COLOR_ATTACHMENT0,
                gl::TEXTURE_2D,
                render_texture_id,
                0,
            );

            depth_stencil_renderbuffer_id = single_gl_id(gl.gen_renderbuffers(1), "renderbuffer")?;
            gl.bind_renderbuffer(gl::RENDERBUFFER, depth_stencil_renderbuffer_id);
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
                depth_stencil_renderbuffer_id,
            );
            gl.bind_renderbuffer(gl::RENDERBUFFER, 0);
            check_gl_error(gl.as_ref(), "glFramebufferRenderbuffer")?;

            let status = gl.check_frame_buffer_status(gl::FRAMEBUFFER);
            if status != gl::FRAMEBUFFER_COMPLETE {
                return Err(MacosGpuInteropError::GlFramebufferIncomplete { status });
            }

            iosurface_framebuffer_id = single_gl_id(gl.gen_framebuffers(1), "framebuffer")?;
            gl.bind_framebuffer(gl::FRAMEBUFFER, iosurface_framebuffer_id);

            iosurface_texture_id = single_gl_id(gl.gen_textures(1), "texture")?;
            gl.bind_texture(GL_TEXTURE_RECTANGLE_ARB, iosurface_texture_id);
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
            check_gl_error(gl.as_ref(), "CGLTexImageIOSurface2D")?;
            gl.framebuffer_texture_2d(
                gl::FRAMEBUFFER,
                gl::COLOR_ATTACHMENT0,
                GL_TEXTURE_RECTANGLE_ARB,
                iosurface_texture_id,
                0,
            );

            let status = gl.check_frame_buffer_status(gl::FRAMEBUFFER);
            if status != gl::FRAMEBUFFER_COMPLETE {
                return Err(MacosGpuInteropError::GlFramebufferIncomplete { status });
            }

            gl.bind_texture(gl::TEXTURE_2D, 0);
            gl.bind_texture(GL_TEXTURE_RECTANGLE_ARB, 0);
            gl.bind_framebuffer(gl::FRAMEBUFFER, render_framebuffer_id);
            Ok(())
        })();

        if let Err(error) = result {
            cleanup_framebuffer_resources(
                gl.as_ref(),
                render_framebuffer_id,
                render_texture_id,
                depth_stencil_renderbuffer_id,
                iosurface_framebuffer_id,
                iosurface_texture_id,
            );
            return Err(error);
        }

        Ok(Self {
            gl,
            size,
            render_framebuffer_id,
            render_texture_id,
            depth_stencil_renderbuffer_id,
            iosurface_framebuffer_id,
            iosurface_texture_id,
            iosurface,
        })
    }

    fn bind(&self) {
        self.gl
            .bind_framebuffer(gl::FRAMEBUFFER, self.render_framebuffer_id);
    }

    fn copy_to_iosurface(&self) -> Result<()> {
        self.gl
            .bind_framebuffer(GL_READ_FRAMEBUFFER, self.render_framebuffer_id);
        self.gl
            .bind_framebuffer(GL_DRAW_FRAMEBUFFER, self.iosurface_framebuffer_id);
        self.gl.read_buffer(gl::COLOR_ATTACHMENT0);
        self.gl.draw_buffers(&[gl::COLOR_ATTACHMENT0]);
        self.gl.blit_framebuffer(
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
        let result = check_gl_error(self.gl.as_ref(), "glBlitFramebuffer");
        self.bind();
        result
    }

    fn native_frame(&self) -> MacosServoNativeFrame {
        MacosServoNativeFrame {
            width: self.size.width,
            height: self.size.height,
            format: ImportedFrameFormat::Bgra8Unorm,
            origin: MacosServoFrameOrigin::BottomLeft,
            surface_id: usize::try_from(self.iosurface.id()).unwrap_or(usize::MAX),
            iosurface: self.iosurface.clone(),
        }
    }
}

impl Drop for MacosServoFramebuffer {
    fn drop(&mut self) {
        cleanup_framebuffer_resources(
            self.gl.as_ref(),
            self.render_framebuffer_id,
            self.render_texture_id,
            self.depth_stencil_renderbuffer_id,
            self.iosurface_framebuffer_id,
            self.iosurface_texture_id,
        );
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

fn cleanup_framebuffer_resources(
    gl: &dyn Gl,
    render_framebuffer_id: u32,
    render_texture_id: u32,
    depth_stencil_renderbuffer_id: u32,
    iosurface_framebuffer_id: u32,
    iosurface_texture_id: u32,
) {
    gl.bind_framebuffer(gl::FRAMEBUFFER, 0);
    if render_texture_id != 0 {
        gl.delete_textures(&[render_texture_id]);
    }
    if iosurface_texture_id != 0 {
        gl.delete_textures(&[iosurface_texture_id]);
    }
    if depth_stencil_renderbuffer_id != 0 {
        gl.delete_renderbuffers(&[depth_stencil_renderbuffer_id]);
    }
    if render_framebuffer_id != 0 {
        gl.delete_framebuffers(&[render_framebuffer_id]);
    }
    if iosurface_framebuffer_id != 0 {
        gl.delete_framebuffers(&[iosurface_framebuffer_id]);
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
