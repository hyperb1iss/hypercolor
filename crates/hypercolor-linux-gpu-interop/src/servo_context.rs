use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;

use dpi::PhysicalSize;
use gleam::gl::{self, Gl};
use servo::{DeviceIntRect, RenderingContext, RgbaImage};
use surfman::chains::SwapChain;
use surfman::{
    Connection, Context, ContextAttributeFlags, ContextAttributes, Device, Error, GLApi, Surface,
    SurfaceAccess, SurfaceInfo, SurfaceTexture, SurfaceType,
};

/// Shared Linux Servo software GL context used by all offscreen render targets.
pub struct LinuxServoRenderDevice {
    gleam_gl: Rc<dyn Gl>,
    glow_gl: Arc<glow::Context>,
    device: RefCell<Device>,
    context: RefCell<Context>,
    swap_chain: RefCell<SwapChain<Device>>,
}

impl LinuxServoRenderDevice {
    /// Creates a Servo-compatible software GL context that can host many render targets.
    pub fn new_software(width: u32, height: u32) -> Result<Self, Error> {
        let connection = Connection::new()?;
        let adapter = connection.create_software_adapter()?;
        let device = connection.create_device(&adapter)?;

        let flags = ContextAttributeFlags::ALPHA
            | ContextAttributeFlags::DEPTH
            | ContextAttributeFlags::STENCIL;
        let gl_api = connection.gl_api();
        let version = match gl_api {
            GLApi::GLES => surfman::GLVersion { major: 3, minor: 0 },
            GLApi::GL => surfman::GLVersion { major: 3, minor: 2 },
        };
        let context_descriptor =
            device.create_context_descriptor(&ContextAttributes { flags, version })?;
        let mut context = device.create_context(&context_descriptor, None)?;

        let gleam_gl: Rc<dyn Gl> = match gl_api {
            GLApi::GL => {
                // SAFETY: surfman returns function pointers for this current
                // context and gleam uses the OpenGL ABI for GL contexts.
                unsafe {
                    gl::GlFns::load_with(|func_name| device.get_proc_address(&context, func_name))
                }
            }
            GLApi::GLES => {
                // SAFETY: surfman returns function pointers for this current
                // context and gleam uses the GLES ABI for GLES contexts.
                unsafe {
                    gl::GlesFns::load_with(|func_name| device.get_proc_address(&context, func_name))
                }
            }
        };

        // SAFETY: surfman returns function pointers for this GL context and
        // the glow handle is used only while the context remains alive.
        let glow_gl = Arc::new(unsafe {
            glow::Context::from_loader_function(|function_name| {
                device.get_proc_address(&context, function_name)
            })
        });

        let surfman_size = euclid::default::Size2D::new(width as i32, height as i32);
        let surface = device.create_surface(
            &context,
            SurfaceAccess::GPUOnly,
            SurfaceType::Generic { size: surfman_size },
        )?;
        device
            .bind_surface_to_context(&mut context, surface)
            .map_err(|(error, mut surface)| {
                let _ = device.destroy_surface(&mut context, &mut surface);
                error
            })?;
        device.make_context_current(&context)?;

        let swap_chain = SwapChain::create_attached(&device, &mut context, SurfaceAccess::GPUOnly)?;

        Ok(Self {
            gleam_gl,
            glow_gl,
            device: RefCell::new(device),
            context: RefCell::new(context),
            swap_chain: RefCell::new(swap_chain),
        })
    }

    /// Creates an offscreen Servo rendering context backed by an FBO in this GL context.
    pub fn create_rendering_context(
        self: &Rc<Self>,
        width: u32,
        height: u32,
    ) -> Result<LinuxServoRenderingContext, Error> {
        self.make_current()?;
        Ok(LinuxServoRenderingContext {
            parent: Rc::clone(self),
            size: Cell::new(PhysicalSize::new(width, height)),
            framebuffer: RefCell::new(LinuxServoFramebuffer::new(
                Rc::clone(&self.gleam_gl),
                PhysicalSize::new(width, height),
            )),
        })
    }

    fn make_current(&self) -> Result<(), Error> {
        let device = self.device.borrow();
        let context = self.context.borrow();
        device.make_context_current(&context)
    }

    fn create_texture(
        &self,
        surface: Surface,
    ) -> Option<(SurfaceTexture, u32, euclid::default::Size2D<i32>)> {
        let device = self.device.borrow();
        let context = &mut self.context.borrow_mut();
        let SurfaceInfo { size, .. } = device.surface_info(&surface);
        let surface_texture = device.create_surface_texture(context, surface).ok()?;
        let gl_texture = device
            .surface_texture_object(&surface_texture)
            .map(|texture| texture.0.get())
            .unwrap_or(0);
        Some((surface_texture, gl_texture, size))
    }

    fn destroy_texture(&self, surface_texture: SurfaceTexture) -> Option<Surface> {
        let device = self.device.borrow();
        let context = &mut self.context.borrow_mut();
        device
            .destroy_surface_texture(context, surface_texture)
            .map_err(|(error, _surface_texture)| error)
            .ok()
    }

    fn connection(&self) -> Connection {
        self.device.borrow().connection()
    }
}

impl Drop for LinuxServoRenderDevice {
    fn drop(&mut self) {
        let device = &mut self.device.borrow_mut();
        let context = &mut self.context.borrow_mut();
        let _ = self.swap_chain.borrow_mut().destroy(device, context);
        let _ = device.destroy_context(context);
    }
}

/// Linux Servo offscreen target with an importable framebuffer.
pub struct LinuxServoRenderingContext {
    parent: Rc<LinuxServoRenderDevice>,
    size: Cell<PhysicalSize<u32>>,
    framebuffer: RefCell<LinuxServoFramebuffer>,
}

impl LinuxServoRenderingContext {
    /// Creates a standalone Servo-compatible software rendering context.
    pub fn new_software(width: u32, height: u32) -> Result<Self, Error> {
        let parent = Rc::new(LinuxServoRenderDevice::new_software(width, height)?);
        parent.create_rendering_context(width, height)
    }

    /// Returns the framebuffer backing this render target.
    #[must_use]
    pub fn framebuffer(&self) -> Option<glow::NativeFramebuffer> {
        self.framebuffer.borrow().native_framebuffer()
    }

    /// Returns the current render target snapshot.
    #[must_use]
    pub fn target_snapshot(&self) -> LinuxServoRenderTargetSnapshot {
        LinuxServoRenderTargetSnapshot::from_framebuffer(&self.framebuffer.borrow(), self.size())
    }

    fn read_framebuffer_to_image(
        &self,
        framebuffer_id: u32,
        source_rectangle: DeviceIntRect,
    ) -> Option<RgbaImage> {
        self.parent
            .gleam_gl
            .bind_framebuffer(gl::FRAMEBUFFER, framebuffer_id);
        self.parent.gleam_gl.bind_vertex_array(0);

        let mut pixels = self.parent.gleam_gl.read_pixels(
            source_rectangle.min.x,
            source_rectangle.min.y,
            source_rectangle.width(),
            source_rectangle.height(),
            gl::RGBA,
            gl::UNSIGNED_BYTE,
        );
        if self.parent.gleam_gl.get_error() != gl::NO_ERROR {
            return None;
        }

        let source_rectangle = source_rectangle.to_usize();
        let stride = source_rectangle.width() * 4;
        let original_pixels = pixels.clone();
        for y in 0..source_rectangle.height() {
            let dst_start = y * stride;
            let src_start = (source_rectangle.height() - y - 1) * stride;
            pixels[dst_start..dst_start + stride]
                .copy_from_slice(&original_pixels[src_start..src_start + stride]);
        }

        RgbaImage::from_raw(
            source_rectangle.width() as u32,
            source_rectangle.height() as u32,
            pixels,
        )
    }
}

impl RenderingContext for LinuxServoRenderingContext {
    fn prepare_for_rendering(&self) {
        self.framebuffer.borrow().bind();
    }

    fn read_to_image(&self, source_rectangle: DeviceIntRect) -> Option<RgbaImage> {
        self.read_framebuffer_to_image(self.framebuffer.borrow().framebuffer_id, source_rectangle)
    }

    fn size(&self) -> PhysicalSize<u32> {
        self.size.get()
    }

    fn resize(&self, size: PhysicalSize<u32>) {
        if self.size.get() == size {
            return;
        }

        if self.make_current().is_ok() {
            *self.framebuffer.borrow_mut() =
                LinuxServoFramebuffer::new(Rc::clone(&self.parent.gleam_gl), size);
            self.size.set(size);
        }
    }

    fn present(&self) {}

    fn make_current(&self) -> Result<(), Error> {
        self.parent.make_current()
    }

    fn gleam_gl_api(&self) -> Rc<dyn Gl> {
        Rc::clone(&self.parent.gleam_gl)
    }

    fn glow_gl_api(&self) -> Arc<glow::Context> {
        Arc::clone(&self.parent.glow_gl)
    }

    fn create_texture(
        &self,
        surface: Surface,
    ) -> Option<(SurfaceTexture, u32, euclid::default::Size2D<i32>)> {
        self.parent.create_texture(surface)
    }

    fn destroy_texture(&self, surface_texture: SurfaceTexture) -> Option<Surface> {
        self.parent.destroy_texture(surface_texture)
    }

    fn connection(&self) -> Option<Connection> {
        Some(self.parent.connection())
    }
}

struct LinuxServoFramebuffer {
    gl: Rc<dyn Gl>,
    framebuffer_id: u32,
    renderbuffer_id: u32,
    texture_id: u32,
}

impl LinuxServoFramebuffer {
    fn new(gl: Rc<dyn Gl>, size: PhysicalSize<u32>) -> Self {
        let framebuffer_ids = gl.gen_framebuffers(1);
        gl.bind_framebuffer(gl::FRAMEBUFFER, framebuffer_ids[0]);

        let texture_ids = gl.gen_textures(1);
        gl.bind_texture(gl::TEXTURE_2D, texture_ids[0]);
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
        gl.framebuffer_texture_2d(
            gl::FRAMEBUFFER,
            gl::COLOR_ATTACHMENT0,
            gl::TEXTURE_2D,
            texture_ids[0],
            0,
        );
        gl.bind_texture(gl::TEXTURE_2D, 0);

        let renderbuffer_ids = gl.gen_renderbuffers(1);
        let depth_rb = renderbuffer_ids[0];
        gl.bind_renderbuffer(gl::RENDERBUFFER, depth_rb);
        gl.renderbuffer_storage(
            gl::RENDERBUFFER,
            gl::DEPTH_COMPONENT24,
            size.width as gl::GLsizei,
            size.height as gl::GLsizei,
        );
        gl.framebuffer_renderbuffer(
            gl::FRAMEBUFFER,
            gl::DEPTH_ATTACHMENT,
            gl::RENDERBUFFER,
            depth_rb,
        );

        Self {
            gl,
            framebuffer_id: framebuffer_ids[0],
            renderbuffer_id: renderbuffer_ids[0],
            texture_id: texture_ids[0],
        }
    }

    fn bind(&self) {
        self.gl
            .bind_framebuffer(gl::FRAMEBUFFER, self.framebuffer_id);
    }

    fn native_framebuffer(&self) -> Option<glow::NativeFramebuffer> {
        NonZeroU32::new(self.framebuffer_id).map(glow::NativeFramebuffer)
    }
}

impl Drop for LinuxServoFramebuffer {
    fn drop(&mut self) {
        self.gl.bind_framebuffer(gl::FRAMEBUFFER, 0);
        self.gl.delete_textures(&[self.texture_id]);
        self.gl.delete_renderbuffers(&[self.renderbuffer_id]);
        self.gl.delete_framebuffers(&[self.framebuffer_id]);
    }
}

/// Snapshot of the current Linux Servo offscreen render target.
#[derive(Debug, Clone, Copy)]
pub struct LinuxServoRenderTargetSnapshot {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// Current framebuffer object name.
    pub framebuffer: u32,
    /// Color texture attached to the framebuffer.
    pub texture: u32,
    /// Depth renderbuffer attached to the framebuffer.
    pub renderbuffer: u32,
}

impl LinuxServoRenderTargetSnapshot {
    fn from_framebuffer(framebuffer: &LinuxServoFramebuffer, size: PhysicalSize<u32>) -> Self {
        Self {
            width: size.width,
            height: size.height,
            framebuffer: framebuffer.framebuffer_id,
            texture: framebuffer.texture_id,
            renderbuffer: framebuffer.renderbuffer_id,
        }
    }
}
