use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use dpi::PhysicalSize;
use euclid::default::Size2D;
use gleam::gl::{self, Gl};
use glow::NativeFramebuffer;
use image::RgbaImage;
use objc2_core_foundation::CFRetained;
use objc2_io_surface::IOSurfaceRef;
use paint_api::rendering_context::RenderingContext;
use surfman::{
    Connection, Context, ContextAttributeFlags, ContextAttributes, Device, Error, GLApi, Surface,
    SurfaceAccess, SurfaceInfo, SurfaceTexture, SurfaceType,
};
use webrender_api::units::DeviceIntRect;

use crate::{ImportedFrameFormat, MacosGpuInteropError, Result};

const IOSURFACE_PIXEL_FORMAT_BGRA: u32 = u32::from_be_bytes(*b"BGRA");

/// Origin convention for a native macOS Servo frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MacosServoFrameOrigin {
    /// The first row in native framebuffer coordinates is the bottom row.
    BottomLeft,
}

/// Native IOSurface-backed frame exposed by the macOS Servo hardware context.
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
    /// Surfman surface identity for diagnostics and cache comparisons.
    pub surface_id: usize,
    /// Retained IOSurface backing the Servo render target.
    pub iosurface: CFRetained<IOSurfaceRef>,
}

/// macOS hardware Servo rendering context backed by a Surfman generic surface.
pub struct MacosHardwareRenderingContext {
    size: Cell<PhysicalSize<u32>>,
    gleam_gl: Rc<dyn Gl>,
    glow_gl: Arc<glow::Context>,
    device: RefCell<Device>,
    context: RefCell<Context>,
}

impl MacosHardwareRenderingContext {
    /// Creates a hardware OpenGL context with an IOSurface-backed render target.
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
            .map_err(context_error("create IOSurface-backed surface"))?;
        bind_surface(&device, &mut context, surface)?;
        device
            .make_context_current(&context)
            .map_err(context_error("make context current"))?;

        Ok(Self {
            size: Cell::new(size),
            gleam_gl,
            glow_gl: Arc::new(glow_gl),
            device: RefCell::new(device),
            context: RefCell::new(context),
        })
    }

    /// Returns the retained native IOSurface for the currently bound surface.
    pub fn native_frame(&self) -> Result<MacosServoNativeFrame> {
        let device = self.device.borrow();
        let mut context = self.context.borrow_mut();
        let surface = device
            .unbind_surface_from_context(&mut context)
            .map_err(context_error("unbind surface for native frame"))?
            .ok_or(MacosGpuInteropError::MissingServoSurface)?;
        let info = device.surface_info(&surface);
        let native_surface = device.native_surface(&surface);
        let actual_format = native_surface.0.pixel_format();
        let frame = if actual_format == IOSURFACE_PIXEL_FORMAT_BGRA {
            Ok(MacosServoNativeFrame {
                width: info.size.width as u32,
                height: info.size.height as u32,
                format: ImportedFrameFormat::Bgra8Unorm,
                origin: MacosServoFrameOrigin::BottomLeft,
                surface_id: info.id.0,
                iosurface: native_surface.0,
            })
        } else {
            Err(MacosGpuInteropError::IosurfacePixelFormatMismatch {
                expected: IOSURFACE_PIXEL_FORMAT_BGRA,
                actual: actual_format,
            })
        };
        bind_surface(&device, &mut context, surface)?;
        frame
    }

    fn framebuffer(&self) -> Option<NativeFramebuffer> {
        let device = self.device.borrow();
        let context = self.context.borrow();
        device
            .context_surface_info(&context)
            .unwrap_or(None)
            .and_then(|info| info.framebuffer_object)
    }

    fn resize_surface(&self, size: PhysicalSize<u32>) -> Result<()> {
        let device = self.device.borrow();
        let mut context = self.context.borrow_mut();
        let old_surface = device
            .unbind_surface_from_context(&mut context)
            .map_err(context_error("unbind surface for resize"))?
            .ok_or(MacosGpuInteropError::MissingServoSurface)?;
        let new_surface = match device.create_surface(
            &context,
            SurfaceAccess::GPUOnly,
            SurfaceType::Generic {
                size: Size2D::new(size.width as i32, size.height as i32),
            },
        ) {
            Ok(surface) => surface,
            Err(error) => {
                let error = context_error("create resized IOSurface-backed surface")(error);
                bind_surface(&device, &mut context, old_surface)?;
                return Err(error);
            }
        };

        if let Err((error, new_surface)) = device.bind_surface_to_context(&mut context, new_surface)
        {
            destroy_surface_or_forget(&device, &mut context, new_surface);
            bind_surface(&device, &mut context, old_surface)?;
            return Err(context_error("bind resized IOSurface-backed surface")(
                error,
            ));
        }
        destroy_surface_or_forget(&device, &mut context, old_surface);
        Ok(())
    }

    fn present_bound_surface(&self) -> Result<()> {
        let device = self.device.borrow();
        let mut context = self.context.borrow_mut();
        let mut surface = device
            .unbind_surface_from_context(&mut context)
            .map_err(context_error("unbind surface for present"))?
            .ok_or(MacosGpuInteropError::MissingServoSurface)?;
        if let Err(error) = device.present_surface(&context, &mut surface) {
            let error = context_error("present IOSurface-backed surface")(error);
            bind_surface(&device, &mut context, surface)?;
            return Err(error);
        }
        bind_surface(&device, &mut context, surface)?;
        Ok(())
    }
}

impl Drop for MacosHardwareRenderingContext {
    fn drop(&mut self) {
        let device = self.device.get_mut();
        let context = self.context.get_mut();
        if let Ok(Some(surface)) = device.unbind_surface_from_context(context) {
            destroy_surface_or_forget(device, context, surface);
        }
        let _ = device.destroy_context(context);
    }
}

impl RenderingContext for MacosHardwareRenderingContext {
    fn prepare_for_rendering(&self) {
        let framebuffer_id = self
            .framebuffer()
            .map_or(0, |framebuffer| framebuffer.0.into());
        self.gleam_gl
            .bind_framebuffer(gl::FRAMEBUFFER, framebuffer_id);
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
        match self.resize_surface(size) {
            Ok(()) => self.size.set(size),
            Err(error) => tracing::warn!(%error, "failed to resize macOS Servo hardware context"),
        }
    }

    fn present(&self) {
        if let Err(error) = self.present_bound_surface() {
            tracing::warn!(%error, "failed to present macOS Servo hardware context");
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
