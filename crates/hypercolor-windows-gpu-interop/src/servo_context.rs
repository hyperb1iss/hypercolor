use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem;
use std::ptr;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use dpi::PhysicalSize;
use euclid::default::Size2D;
use gleam::gl::{self, Gl};
use image::RgbaImage;
use paint_api::rendering_context::RenderingContext;
use surfman::{
    Adapter, Connection, Context, ContextAttributeFlags, ContextAttributes, Device, Error, GLApi,
    Surface, SurfaceInfo, SurfaceTexture,
};
use webrender_api::units::DeviceIntRect;
use winapi::Interface;
use winapi::shared::dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1,
};
use winapi::shared::winerror::{DXGI_ERROR_NOT_FOUND, SUCCEEDED};
use wio::com::ComPtr;

use crate::{
    ImportedEffectFrame, ImportedFrameFormat, Result, WindowsD3d11Device,
    WindowsD3d11SharedTexture, WindowsD3d11SharedTextureImportDescriptor,
    WindowsD3d11SharedTextureImporter, WindowsGpuInteropError,
};

const WINDOWS_SERVO_RING_DEPTH: usize = 3;

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
    /// Time spent waiting for producer GL work to finish.
    pub sync_us: u64,
}

/// Windows Servo rendering context backed by ANGLE D3D11 shared textures.
pub struct WindowsAngleRenderingContext {
    size: Cell<PhysicalSize<u32>>,
    gleam_gl: Rc<dyn Gl>,
    glow_gl: Arc<glow::Context>,
    device: RefCell<Device>,
    context: RefCell<Context>,
    ring: RefCell<Vec<WindowsServoTextureSlot>>,
    current_slot: Cell<usize>,
}

struct WindowsServoTextureSlot {
    texture: WindowsD3d11SharedTexture,
    surface: Option<Surface>,
}

impl WindowsAngleRenderingContext {
    /// Creates an ANGLE context with a D3D11 shared-texture ring.
    pub fn new(
        width: u32,
        height: u32,
        adapter_identity: Option<WindowsDxgiAdapterIdentity>,
    ) -> Result<Self> {
        let size = validate_size(width, height)?;
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
        device
            .make_context_current(&context)
            .map_err(context_error("make context current"))?;

        let native_device = device.native_device();
        // SAFETY: native_device.d3d11_device is an owned AddRef returned by Surfman.
        let d3d11 = unsafe {
            WindowsD3d11Device::from_owned_raw_d3d11_device(
                native_device.d3d11_device.cast::<c_void>(),
            )?
        };
        let descriptor = WindowsD3d11SharedTextureImportDescriptor::new(
            width,
            height,
            ImportedFrameFormat::Bgra8Unorm,
        )?;
        let surfman_size = Size2D::new(width as i32, height as i32);
        let mut ring = create_texture_ring(&device, &context, &d3d11, descriptor, surfman_size)?;
        let first_surface = ring[0]
            .surface
            .take()
            .ok_or(WindowsGpuInteropError::WindowsImportStaleFrame)?;
        bind_surface(&device, &mut context, first_surface)?;

        let gleam_gl = load_gleam_gl(&device, &context, connection.gl_api());
        let glow_gl = load_glow_gl(&device, &context);

        Ok(Self {
            size: Cell::new(size),
            gleam_gl,
            glow_gl: Arc::new(glow_gl),
            device: RefCell::new(device),
            context: RefCell::new(context),
            ring: RefCell::new(ring),
            current_slot: Cell::new(0),
        })
    }

    /// Finishes Servo GL work and exposes the current D3D11 shared texture.
    pub fn finish_current_frame(&self) -> Result<WindowsServoNativeFrame> {
        self.make_current()
            .map_err(context_error("make current for shared frame"))?;
        self.prepare_for_rendering();
        let sync_start = Instant::now();
        self.gleam_gl.finish();
        let sync_us = elapsed_micros(sync_start);
        let slot_index = self.current_slot.get();
        let ring = self.ring.borrow();
        let texture = &ring
            .get(slot_index)
            .ok_or(WindowsGpuInteropError::WindowsImportStaleFrame)?
            .texture;
        let descriptor = texture.descriptor();
        Ok(WindowsServoNativeFrame {
            width: descriptor.width,
            height: descriptor.height,
            format: descriptor.format,
            origin: WindowsServoFrameOrigin::BottomLeft,
            slot_index,
            shared_handle: texture.shared_handle(),
            sync_us,
        })
    }

    /// Rotates the ring so Servo paints into a different slot next frame.
    pub fn rotate_after_import(&self) -> Result<()> {
        let device = self.device.borrow();
        let mut context = self.context.borrow_mut();
        let mut ring = self.ring.borrow_mut();
        let current_slot = self.current_slot.get();
        let next_slot = (current_slot + 1) % ring.len();
        let next_surface = ring[next_slot]
            .surface
            .take()
            .ok_or(WindowsGpuInteropError::WindowsImportStaleFrame)?;
        let current_surface = device
            .unbind_surface_from_context(&mut context)
            .map_err(context_error("unbind current shared surface"))?
            .ok_or(WindowsGpuInteropError::WindowsImportStaleFrame)?;

        match device.bind_surface_to_context(&mut context, next_surface) {
            Ok(()) => {
                ring[current_slot].surface = Some(current_surface);
                self.current_slot.set(next_slot);
                device
                    .make_context_current(&context)
                    .map_err(context_error("make rotated context current"))?;
                Ok(())
            }
            Err((error, next_surface)) => {
                ring[next_slot].surface = Some(next_surface);
                let _ = device.bind_surface_to_context(&mut context, current_surface);
                Err(context_error("bind next shared surface")(error))
            }
        }
    }

    fn framebuffer(&self) -> Option<glow::NativeFramebuffer> {
        let device = self.device.borrow();
        let context = self.context.borrow();
        device
            .context_surface_info(&context)
            .unwrap_or(None)
            .and_then(|info| info.framebuffer_object)
    }

    fn recreate_ring(&self, size: PhysicalSize<u32>) -> Result<()> {
        let device = self.device.borrow();
        let mut context = self.context.borrow_mut();
        let mut ring = self.ring.borrow_mut();
        if let Ok(Some(surface)) = device.unbind_surface_from_context(&mut context) {
            ring[self.current_slot.get()].surface = Some(surface);
        }
        for slot in ring.iter_mut() {
            if let Some(mut surface) = slot.surface.take() {
                destroy_surface_or_forget(&device, &mut context, &mut surface);
            }
        }

        let native_device = device.native_device();
        // SAFETY: native_device.d3d11_device is an owned AddRef returned by Surfman.
        let d3d11 = unsafe {
            WindowsD3d11Device::from_owned_raw_d3d11_device(
                native_device.d3d11_device.cast::<c_void>(),
            )?
        };
        let descriptor = WindowsD3d11SharedTextureImportDescriptor::new(
            size.width,
            size.height,
            ImportedFrameFormat::Bgra8Unorm,
        )?;
        let surfman_size = Size2D::new(size.width as i32, size.height as i32);
        let mut next_ring =
            create_texture_ring(&device, &context, &d3d11, descriptor, surfman_size)?;
        let first_surface = next_ring[0]
            .surface
            .take()
            .ok_or(WindowsGpuInteropError::WindowsImportStaleFrame)?;
        bind_surface(&device, &mut context, first_surface)?;
        *ring = next_ring;
        self.current_slot.set(0);
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
        // SAFETY: WindowsServoNativeFrame is only produced by the ANGLE
        // context after finish_current_frame synchronizes producer GL work.
        unsafe { self.import_shared_handle(device, frame.shared_handle, frame.sync_us) }
    }
}

impl Drop for WindowsAngleRenderingContext {
    fn drop(&mut self) {
        let device = self.device.get_mut();
        let context = self.context.get_mut();
        let ring = self.ring.get_mut();
        if let Ok(Some(surface)) = device.unbind_surface_from_context(context) {
            ring[self.current_slot.get()].surface = Some(surface);
        }
        for slot in ring {
            if let Some(mut surface) = slot.surface.take() {
                destroy_surface_or_forget(device, context, &mut surface);
            }
        }
        let _ = device.destroy_context(context);
    }
}

impl RenderingContext for WindowsAngleRenderingContext {
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
        match validate_size(size.width, size.height).and_then(|_| self.recreate_ring(size)) {
            Ok(()) => self.size.set(size),
            Err(error) => tracing::warn!(%error, "failed to resize Windows ANGLE Servo context"),
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
                let mut surface = surface;
                destroy_surface_or_forget(&device, &mut context, &mut surface);
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

fn create_texture_ring(
    device: &Device,
    context: &Context,
    d3d11: &WindowsD3d11Device,
    descriptor: WindowsD3d11SharedTextureImportDescriptor,
    size: Size2D<i32>,
) -> Result<Vec<WindowsServoTextureSlot>> {
    let mut ring = Vec::with_capacity(WINDOWS_SERVO_RING_DEPTH);
    for _ in 0..WINDOWS_SERVO_RING_DEPTH {
        let texture = d3d11.create_shared_texture(descriptor)?;
        // SAFETY: texture stays alive in the ring while the Surfman surface exists.
        let surfman_texture = unsafe { texture.to_surfman_texture() };
        // SAFETY: the surface is used on this Servo worker thread, and the
        // D3D11 texture remains owned by the slot for at least as long.
        let surface = unsafe {
            device
                .create_surface_from_texture(context, &size, surfman_texture)
                .map_err(context_error("create ANGLE client-buffer surface"))?
        };
        ring.push(WindowsServoTextureSlot {
            texture,
            surface: Some(surface),
        });
    }
    Ok(ring)
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
        .map_err(|(error, mut surface)| {
            destroy_surface_or_forget(device, context, &mut surface);
            context_error("bind ANGLE shared surface")(error)
        })
}

fn destroy_surface_or_forget(device: &Device, context: &mut Context, surface: &mut Surface) {
    if device.destroy_surface(context, surface).is_err() {
        // SAFETY: destroy_surface failed, so Surfman's Drop would panic; move
        // the surface value out and intentionally leak the platform handle.
        unsafe {
            std::mem::forget(std::ptr::read(surface));
        }
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
