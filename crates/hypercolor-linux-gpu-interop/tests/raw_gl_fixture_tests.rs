#![cfg(all(target_os = "linux", feature = "raw-gl-fixture"))]

use std::sync::mpsc;

use euclid::default::Size2D;
use glow::HasContext;
use hypercolor_linux_gpu_interop::{
    GlExternalMemoryFunctions, GlFramebufferSource, ImportedFrameFormat,
    LinuxGlFramebufferImportDescriptor, LinuxGlFramebufferImporter, LinuxGpuInteropError,
    check_wgpu_vulkan_external_memory_fd, import_gl_framebuffer_to_wgpu,
};
use surfman::{
    Connection, Context, ContextAttributeFlags, ContextAttributes, Device, Error, GLVersion,
    SurfaceAccess, SurfaceType,
};

const WIDTH: u32 = 4;
const HEIGHT: u32 = 4;
const EXPECTED_PIXEL: [u8; 4] = [0, 255, 255, 255];
const TOP_LEFT: [u8; 4] = [255, 0, 0, 255];
const TOP_RIGHT: [u8; 4] = [0, 255, 0, 255];
const BOTTOM_LEFT: [u8; 4] = [0, 0, 255, 255];
const BOTTOM_RIGHT: [u8; 4] = [255, 255, 0, 255];
const IMPORT_ITERATIONS: usize = 8;
const POOLED_IMPORT_SLOTS: usize = 2;
const RUN_FIXTURE_ENV: &str = "HYPERCOLOR_RUN_GPU_INTEROP_FIXTURE";

#[test]
fn raw_gl_solid_color_import_matches_wgpu_readback() {
    if std::env::var_os(RUN_FIXTURE_ENV).is_none() {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the raw GL import fixture");
        return;
    }

    let wgpu = WgpuFixture::new().expect("raw GL import fixture should create wgpu device");
    let raw_gl =
        RawGlFixture::new(WIDTH, HEIGHT).expect("raw GL import fixture should create GL surface");
    let gl_external_memory = raw_gl
        .load_external_memory_functions()
        .expect("raw GL import fixture should load GL external memory functions");

    let descriptor =
        LinuxGlFramebufferImportDescriptor::new(WIDTH, HEIGHT, ImportedFrameFormat::Rgba8Unorm)
            .expect("fixture dimensions should be valid");

    for _ in 0..IMPORT_ITERATIONS {
        raw_gl.clear(EXPECTED_PIXEL);
        {
            let frame = import_gl_framebuffer_to_wgpu(
                &wgpu.device,
                &raw_gl.gl,
                gl_external_memory,
                GlFramebufferSource::Framebuffer(raw_gl.framebuffer),
                descriptor,
            )
            .expect("raw GL fixture should import into wgpu");

            let pixels =
                read_texture_pixels(&wgpu.device, &wgpu.queue, &frame.texture, WIDTH, HEIGHT);
            for pixel in pixels.chunks_exact(4) {
                assert_eq!(pixel, EXPECTED_PIXEL);
            }
        }
        let _ = wgpu.device.poll(wgpu::PollType::Poll);
    }
}

#[test]
fn raw_gl_orientation_import_preserves_top_left_wgpu_readback() {
    if std::env::var_os(RUN_FIXTURE_ENV).is_none() {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the raw GL import fixture");
        return;
    }

    let wgpu = WgpuFixture::new().expect("raw GL import fixture should create wgpu device");
    let raw_gl =
        RawGlFixture::new(WIDTH, HEIGHT).expect("raw GL import fixture should create GL surface");
    let gl_external_memory = raw_gl
        .load_external_memory_functions()
        .expect("raw GL import fixture should load GL external memory functions");

    raw_gl.paint_orientation_fixture();
    let descriptor =
        LinuxGlFramebufferImportDescriptor::new(WIDTH, HEIGHT, ImportedFrameFormat::Rgba8Unorm)
            .expect("fixture dimensions should be valid");
    let frame = import_gl_framebuffer_to_wgpu(
        &wgpu.device,
        &raw_gl.gl,
        gl_external_memory,
        GlFramebufferSource::Framebuffer(raw_gl.framebuffer),
        descriptor,
    )
    .expect("raw GL orientation fixture should import into wgpu");
    let pixels = read_texture_pixels(&wgpu.device, &wgpu.queue, &frame.texture, WIDTH, HEIGHT);

    assert_eq!(
        corner_pixels(&pixels, WIDTH, HEIGHT),
        [TOP_LEFT, TOP_RIGHT, BOTTOM_LEFT, BOTTOM_RIGHT]
    );
}

#[test]
fn raw_gl_pooled_importer_reuses_slots_and_matches_wgpu_readback() {
    if std::env::var_os(RUN_FIXTURE_ENV).is_none() {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the raw GL import fixture");
        return;
    }

    let wgpu = WgpuFixture::new().expect("raw GL import fixture should create wgpu device");
    let raw_gl =
        RawGlFixture::new(WIDTH, HEIGHT).expect("raw GL import fixture should create GL surface");
    let gl_external_memory = raw_gl
        .load_external_memory_functions()
        .expect("raw GL import fixture should load GL external memory functions");
    let descriptor =
        LinuxGlFramebufferImportDescriptor::new(WIDTH, HEIGHT, ImportedFrameFormat::Rgba8Unorm)
            .expect("fixture dimensions should be valid");
    let mut importer = LinuxGlFramebufferImporter::new(
        &wgpu.device,
        &raw_gl.gl,
        gl_external_memory,
        descriptor,
        POOLED_IMPORT_SLOTS,
    )
    .expect("raw GL fixture should create pooled importer");

    assert_eq!(importer.descriptor(), descriptor);
    assert_eq!(importer.slot_count(), POOLED_IMPORT_SLOTS);

    for expected in [[255, 0, 128, 255], [0, 255, 255, 255], [32, 64, 255, 255]] {
        raw_gl.clear(expected);
        let frame = importer
            .import_framebuffer(
                &raw_gl.gl,
                GlFramebufferSource::Framebuffer(raw_gl.framebuffer),
            )
            .expect("pooled raw GL fixture should import into wgpu");
        let pixels = read_texture_pixels(&wgpu.device, &wgpu.queue, &frame.texture, WIDTH, HEIGHT);
        for pixel in pixels.chunks_exact(4) {
            assert_eq!(pixel, expected);
        }
    }

    importer.destroy_gl_resources(&raw_gl.gl);
}

#[test]
fn raw_gl_pooled_importer_reports_exhaustion_when_slots_are_held() {
    if std::env::var_os(RUN_FIXTURE_ENV).is_none() {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the raw GL import fixture");
        return;
    }

    let wgpu = WgpuFixture::new().expect("raw GL import fixture should create wgpu device");
    let raw_gl =
        RawGlFixture::new(WIDTH, HEIGHT).expect("raw GL import fixture should create GL surface");
    let gl_external_memory = raw_gl
        .load_external_memory_functions()
        .expect("raw GL import fixture should load GL external memory functions");
    let descriptor =
        LinuxGlFramebufferImportDescriptor::new(WIDTH, HEIGHT, ImportedFrameFormat::Rgba8Unorm)
            .expect("fixture dimensions should be valid");
    let mut importer = LinuxGlFramebufferImporter::new(
        &wgpu.device,
        &raw_gl.gl,
        gl_external_memory,
        descriptor,
        POOLED_IMPORT_SLOTS,
    )
    .expect("raw GL fixture should create pooled importer");

    let mut held_frames = Vec::new();
    for expected in [[255, 0, 0, 255], [0, 255, 0, 255]] {
        raw_gl.clear(expected);
        held_frames.push(
            importer
                .import_framebuffer(
                    &raw_gl.gl,
                    GlFramebufferSource::Framebuffer(raw_gl.framebuffer),
                )
                .expect("pooled raw GL fixture should fill an available slot"),
        );
    }

    raw_gl.clear([0, 0, 255, 255]);
    let result = importer.import_framebuffer(
        &raw_gl.gl,
        GlFramebufferSource::Framebuffer(raw_gl.framebuffer),
    );
    assert!(matches!(
        result,
        Err(LinuxGpuInteropError::ImportSlotsExhausted {
            slot_count: POOLED_IMPORT_SLOTS
        })
    ));

    drop(held_frames);
    let _ = wgpu.device.poll(wgpu::PollType::Poll);
    importer.destroy_gl_resources(&raw_gl.gl);
}

struct WgpuFixture {
    _instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl WgpuFixture {
    fn new() -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })) {
                Ok(adapter) => adapter,
                Err(error) => return Err(format!("could not create wgpu adapter: {error}")),
            };
        let adapter_info = adapter.get_info();
        if adapter_info.backend != wgpu::Backend::Vulkan {
            return Err(format!(
                "requires Vulkan wgpu backend, got {:?}",
                adapter_info.backend
            ));
        }

        let (device, queue) =
            match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("hypercolor raw GL interop fixture"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })) {
                Ok(result) => result,
                Err(error) => return Err(format!("could not create wgpu device: {error}")),
            };

        if let Err(error) = check_wgpu_vulkan_external_memory_fd(&device) {
            return Err(format!("missing Vulkan external memory: {error}"));
        }

        Ok(Self {
            _instance: instance,
            device,
            queue,
        })
    }
}

struct RawGlFixture {
    device: Device,
    surfman_context: Context,
    gl: glow::Context,
    framebuffer: Option<glow::NativeFramebuffer>,
    width: u32,
    height: u32,
}

impl RawGlFixture {
    fn new(width: u32, height: u32) -> Result<Self, String> {
        let connection = match Connection::new() {
            Ok(connection) => connection,
            Err(error) => return Err(format!("could not create surfman connection: {error:?}")),
        };
        let adapter = match connection.create_hardware_adapter() {
            Ok(adapter) => adapter,
            Err(error) => return Err(format!("could not create hardware adapter: {error:?}")),
        };
        let device = match connection.create_device(&adapter) {
            Ok(device) => device,
            Err(Error::RequiredExtensionUnavailable) => {
                return Err("missing required surfman extension".to_string());
            }
            Err(error) => return Err(format!("could not create surfman device: {error:?}")),
        };

        let context_descriptor = match device.create_context_descriptor(&ContextAttributes {
            version: GLVersion::new(3, 3),
            flags: ContextAttributeFlags::empty(),
        }) {
            Ok(context_descriptor) => context_descriptor,
            Err(error) => return Err(format!("could not create context descriptor: {error:?}")),
        };
        let mut context = match device.create_context(&context_descriptor, None) {
            Ok(context) => context,
            Err(error) => return Err(format!("could not create GL context: {error:?}")),
        };
        let surface = match device.create_surface(
            &context,
            SurfaceAccess::GPUOnly,
            SurfaceType::Generic {
                size: Size2D::new(width as i32, height as i32),
            },
        ) {
            Ok(surface) => surface,
            Err(error) => {
                let _ = device.destroy_context(&mut context);
                return Err(format!("could not create surface: {error:?}"));
            }
        };
        if let Err((error, mut surface)) = device.bind_surface_to_context(&mut context, surface) {
            let _ = device.destroy_surface(&mut context, &mut surface);
            let _ = device.destroy_context(&mut context);
            return Err(format!("could not bind surface: {error:?}"));
        }
        if let Err(error) = device.make_context_current(&context) {
            destroy_bound_context(&device, &mut context);
            return Err(format!("could not make context current: {error:?}"));
        }

        let framebuffer = match device.context_surface_info(&context) {
            Ok(Some(surface_info)) => surface_info
                .framebuffer_object
                .map(|framebuffer| glow::NativeFramebuffer(framebuffer.0)),
            Ok(None) => None,
            Err(error) => {
                destroy_bound_context(&device, &mut context);
                return Err(format!("could not inspect surface info: {error:?}"));
            }
        };
        // SAFETY: surfman returned the loader for the current context, and
        // the glow handle is only used while that context remains current.
        let gl = unsafe {
            glow::Context::from_loader_function(|symbol| device.get_proc_address(&context, symbol))
        };

        Ok(Self {
            device,
            surfman_context: context,
            gl,
            framebuffer,
            width,
            height,
        })
    }

    fn load_external_memory_functions(&self) -> Result<GlExternalMemoryFunctions, String> {
        GlExternalMemoryFunctions::load_from(|symbol| {
            let symbol = symbol.to_str().unwrap_or_default();
            self.device.get_proc_address(&self.surfman_context, symbol)
        })
        .map_err(|error| format!("missing GL external memory support: {error}"))
    }

    fn clear(&self, pixel: [u8; 4]) {
        let [red, green, blue, alpha] = pixel.map(|channel| f32::from(channel) / 255.0);
        // SAFETY: the surfman context is current for this thread while the
        // fixture is alive, and the framebuffer belongs to that context.
        unsafe {
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, self.framebuffer);
            self.gl
                .viewport(0, 0, self.width as i32, self.height as i32);
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.clear_color(red, green, blue, alpha);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
            self.gl.finish();
        }
    }

    fn paint_orientation_fixture(&self) {
        let half_width = self.width / 2;
        let half_height = self.height / 2;
        unsafe {
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, self.framebuffer);
            self.gl
                .viewport(0, 0, self.width as i32, self.height as i32);
            self.gl.enable(glow::SCISSOR_TEST);
            self.clear_rect(
                0,
                half_height,
                half_width,
                self.height - half_height,
                TOP_LEFT,
            );
            self.clear_rect(
                half_width,
                half_height,
                self.width - half_width,
                self.height - half_height,
                TOP_RIGHT,
            );
            self.clear_rect(0, 0, half_width, half_height, BOTTOM_LEFT);
            self.clear_rect(
                half_width,
                0,
                self.width - half_width,
                half_height,
                BOTTOM_RIGHT,
            );
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.finish();
        }
    }

    fn clear_rect(&self, x: u32, y: u32, width: u32, height: u32, pixel: [u8; 4]) {
        let [red, green, blue, alpha] = pixel.map(|channel| f32::from(channel) / 255.0);
        unsafe {
            self.gl.scissor(
                i32::try_from(x).expect("fixture x should fit i32"),
                i32::try_from(y).expect("fixture y should fit i32"),
                i32::try_from(width).expect("fixture width should fit i32"),
                i32::try_from(height).expect("fixture height should fit i32"),
            );
            self.gl.clear_color(red, green, blue, alpha);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }
}

impl Drop for RawGlFixture {
    fn drop(&mut self) {
        destroy_bound_context(&self.device, &mut self.surfman_context);
    }
}

fn destroy_bound_context(device: &Device, context: &mut Context) {
    if let Ok(Some(mut surface)) = device.unbind_surface_from_context(context) {
        let _ = device.destroy_surface(context, &mut surface);
    }
    let _ = device.destroy_context(context);
}

fn read_texture_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let unpadded_bytes_per_row = width * 4;
    let padded_bytes_per_row = padded_bytes_per_row(width);
    let buffer_size = u64::from(padded_bytes_per_row) * u64::from(height);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hypercolor raw GL interop fixture readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("hypercolor raw GL interop fixture readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let submission = queue.submit(Some(encoder.finish()));

    let slice = buffer.slice(..buffer_size);
    let (sender, receiver) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })
        .expect("fixture readback poll should complete");
    receiver
        .recv()
        .expect("fixture readback channel should receive map result")
        .expect("fixture readback buffer should map");

    let mapped = slice.get_mapped_range();
    let mut pixels = vec![0; (height * unpadded_bytes_per_row) as usize];
    for (target, source) in pixels
        .chunks_exact_mut(unpadded_bytes_per_row as usize)
        .zip(
            mapped
                .chunks(padded_bytes_per_row as usize)
                .take(height as usize),
        )
    {
        target.copy_from_slice(&source[..unpadded_bytes_per_row as usize]);
    }
    drop(mapped);
    buffer.unmap();

    pixels
}

const fn padded_bytes_per_row(width: u32) -> u32 {
    let unpadded = width * 4;
    let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    unpadded.div_ceil(alignment) * alignment
}

fn corner_pixels(pixels: &[u8], width: u32, height: u32) -> [[u8; 4]; 4] {
    [
        pixel_at(pixels, width, 0, 0),
        pixel_at(pixels, width, width - 1, 0),
        pixel_at(pixels, width, 0, height - 1),
        pixel_at(pixels, width, width - 1, height - 1),
    ]
}

fn pixel_at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let index = usize::try_from(y * width + x).expect("fixture pixel index should fit usize");
    let offset = index * 4;
    pixels[offset..offset + 4]
        .try_into()
        .expect("fixture pixel should contain four channels")
}
