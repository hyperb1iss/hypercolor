use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use dpi::PhysicalSize;
use gleam::gl::{self, Gl};
use servo::{DeviceIntRect, RenderingContext, RgbaImage};
use surfman::chains::{PreserveBuffer, SwapChain};
use surfman::{
    Connection, Context, ContextAttributeFlags, ContextAttributes, Device, Error, GLApi, Surface,
    SurfaceAccess, SurfaceInfo, SurfaceTexture, SurfaceType,
};

/// Linux Servo software context with access to the current surfman FBO.
pub struct LinuxServoRenderingContext {
    size: Cell<PhysicalSize<u32>>,
    gleam_gl: Rc<dyn Gl>,
    glow_gl: Arc<glow::Context>,
    device: RefCell<Device>,
    context: RefCell<Context>,
    swap_chain: SwapChain<Device>,
}

impl LinuxServoRenderingContext {
    /// Creates a Servo-compatible software rendering context.
    ///
    /// This mirrors Servo's software context, but keeps the surfman context
    /// visible so the GPU import path can bind the actual surface FBO.
    pub fn new_software(width: u32, height: u32) -> Result<Self, Error> {
        let size = PhysicalSize::new(width, height);
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
            size: Cell::new(size),
            gleam_gl,
            glow_gl,
            device: RefCell::new(device),
            context: RefCell::new(context),
            swap_chain,
        })
    }

    /// Returns the surfman framebuffer backing the current render surface.
    #[must_use]
    pub fn framebuffer(&self) -> Option<glow::NativeFramebuffer> {
        self.surface_info().and_then(|info| info.framebuffer_object)
    }

    /// Returns the current surfman surface snapshot.
    #[must_use]
    pub fn surface_snapshot(&self) -> Option<LinuxServoSurfaceSnapshot> {
        self.surface_info()
            .map(LinuxServoSurfaceSnapshot::from_surface_info)
    }

    fn framebuffer_id(&self) -> u32 {
        self.framebuffer()
            .map_or(0, |framebuffer| framebuffer.0.into())
    }

    fn surface_info(&self) -> Option<SurfaceInfo> {
        let device = self.device.borrow();
        let context = self.context.borrow();
        device.context_surface_info(&context).ok().flatten()
    }

    fn bind_framebuffer(&self) {
        self.gleam_gl
            .bind_framebuffer(gl::FRAMEBUFFER, self.framebuffer_id());
    }

    fn read_framebuffer_to_image(
        &self,
        framebuffer_id: u32,
        source_rectangle: DeviceIntRect,
    ) -> Option<RgbaImage> {
        self.gleam_gl
            .bind_framebuffer(gl::FRAMEBUFFER, framebuffer_id);
        self.gleam_gl.bind_vertex_array(0);

        let mut pixels = self.gleam_gl.read_pixels(
            source_rectangle.min.x,
            source_rectangle.min.y,
            source_rectangle.width(),
            source_rectangle.height(),
            gl::RGBA,
            gl::UNSIGNED_BYTE,
        );
        if self.gleam_gl.get_error() != gl::NO_ERROR {
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

impl Drop for LinuxServoRenderingContext {
    fn drop(&mut self) {
        let device = &mut self.device.borrow_mut();
        let context = &mut self.context.borrow_mut();
        let _ = self.swap_chain.destroy(device, context);
        let _ = device.destroy_context(context);
    }
}

impl RenderingContext for LinuxServoRenderingContext {
    fn prepare_for_rendering(&self) {
        self.bind_framebuffer();
    }

    fn read_to_image(&self, source_rectangle: DeviceIntRect) -> Option<RgbaImage> {
        self.read_framebuffer_to_image(self.framebuffer_id(), source_rectangle)
    }

    fn size(&self) -> PhysicalSize<u32> {
        self.size.get()
    }

    fn resize(&self, size: PhysicalSize<u32>) {
        if self.size.get() == size {
            return;
        }

        let surfman_size = euclid::default::Size2D::new(size.width as i32, size.height as i32);
        let device = &mut self.device.borrow_mut();
        let context = &mut self.context.borrow_mut();
        if self
            .swap_chain
            .resize(device, context, surfman_size)
            .is_ok()
        {
            self.size.set(size);
        }
    }

    fn present(&self) {
        let device = &mut self.device.borrow_mut();
        let context = &mut self.context.borrow_mut();
        let _ = self
            .swap_chain
            .swap_buffers(device, context, PreserveBuffer::No);
    }

    fn make_current(&self) -> Result<(), Error> {
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

    fn connection(&self) -> Option<Connection> {
        Some(self.device.borrow().connection())
    }
}

/// Snapshot of the current Linux Servo surfman surface.
#[derive(Debug, Clone, Copy)]
pub struct LinuxServoSurfaceSnapshot {
    /// Surfman surface id.
    pub surface_id: usize,
    /// Width in physical pixels.
    pub width: i32,
    /// Height in physical pixels.
    pub height: i32,
    /// Current framebuffer object name, or zero for the default FBO.
    pub framebuffer: u32,
}

impl LinuxServoSurfaceSnapshot {
    fn from_surface_info(info: SurfaceInfo) -> Self {
        Self {
            surface_id: info.id.0,
            width: info.size.width,
            height: info.size.height,
            framebuffer: info
                .framebuffer_object
                .map_or(0, |framebuffer| framebuffer.0.into()),
        }
    }
}
