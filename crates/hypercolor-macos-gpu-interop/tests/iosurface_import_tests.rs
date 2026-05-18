#![cfg(target_os = "macos")]

use std::sync::mpsc;

use hypercolor_macos_gpu_interop::{
    ImportedFrameFormat, MacosGpuInteropError, MacosIosurfaceImportDescriptor,
    MacosIosurfaceImporter, create_bgra_iosurface, write_bgra_pixels,
};

const WIDTH: u32 = 4;
const HEIGHT: u32 = 3;

#[test]
fn imports_synthetic_iosurface_into_wgpu_texture() -> Result<(), String> {
    let wgpu = WgpuFixture::new()?;
    let descriptor =
        MacosIosurfaceImportDescriptor::new(WIDTH, HEIGHT, ImportedFrameFormat::Bgra8Unorm)
            .map_err(|error| error.to_string())?;
    let iosurface = create_bgra_iosurface(WIDTH, HEIGHT).map_err(|error| error.to_string())?;
    let expected_pixels = fixture_pixels();
    write_bgra_pixels(&iosurface, WIDTH, HEIGHT, &expected_pixels)
        .map_err(|error| error.to_string())?;

    let mut importer =
        MacosIosurfaceImporter::new(&wgpu.device, descriptor).map_err(|error| error.to_string())?;
    let frame = importer
        .import_iosurface(&wgpu.device, &iosurface)
        .map_err(|error| error.to_string())?;
    let pixels = read_texture_pixels(&wgpu.device, &wgpu.queue, &frame.texture, WIDTH, HEIGHT)?;

    assert_eq!(frame.width, WIDTH);
    assert_eq!(frame.height, HEIGHT);
    assert_eq!(frame.format, ImportedFrameFormat::Bgra8Unorm);
    assert_eq!(pixels, expected_pixels);

    Ok(())
}

#[test]
fn rejects_zero_sized_descriptors() {
    assert!(
        MacosIosurfaceImportDescriptor::new(0, HEIGHT, ImportedFrameFormat::Bgra8Unorm).is_err()
    );
    assert!(
        MacosIosurfaceImportDescriptor::new(WIDTH, 0, ImportedFrameFormat::Bgra8Unorm).is_err()
    );
}

#[test]
fn rejects_mismatched_pixel_buffer_lengths() -> Result<(), String> {
    let iosurface = create_bgra_iosurface(WIDTH, HEIGHT).map_err(|error| error.to_string())?;
    let actual_len = (WIDTH * HEIGHT * 4 - 1) as usize;
    let pixels = vec![0; actual_len];
    let error = write_bgra_pixels(&iosurface, WIDTH, HEIGHT, &pixels)
        .expect_err("mismatched pixel buffers should be rejected");

    assert_eq!(
        error,
        MacosGpuInteropError::PixelBufferSizeMismatch {
            expected_len: (WIDTH * HEIGHT * 4) as usize,
            actual_len,
        }
    );

    Ok(())
}

#[test]
fn rejects_iosurface_shape_mismatches() -> Result<(), String> {
    let actual_width = WIDTH + 1;
    let iosurface =
        create_bgra_iosurface(actual_width, HEIGHT).map_err(|error| error.to_string())?;
    let pixels = fixture_pixels();
    let error = write_bgra_pixels(&iosurface, WIDTH, HEIGHT, &pixels)
        .expect_err("mismatched IOSurface shapes should be rejected");

    assert_eq!(
        error,
        MacosGpuInteropError::IosurfaceShapeMismatch {
            expected_width: WIDTH,
            expected_height: HEIGHT,
            actual_width: actual_width as usize,
            actual_height: HEIGHT as usize,
        }
    );

    Ok(())
}

struct WgpuFixture {
    _instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl WgpuFixture {
    fn new() -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .map_err(|error| format!("could not create wgpu adapter: {error}"))?;
        let adapter_info = adapter.get_info();
        if adapter_info.backend != wgpu::Backend::Metal {
            return Err(format!(
                "requires Metal wgpu backend, got {:?}",
                adapter_info.backend
            ));
        }

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("hypercolor macOS IOSurface interop fixture"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|error| format!("could not create wgpu device: {error}"))?;

        Ok(Self {
            _instance: instance,
            device,
            queue,
        })
    }
}

fn fixture_pixels() -> Vec<u8> {
    let mut pixels = vec![0; (WIDTH * HEIGHT * 4) as usize];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let index = ((y * WIDTH + x) * 4) as usize;
            pixels[index] = (x * 17 + y * 3) as u8;
            pixels[index + 1] = (x * 11 + y * 19) as u8;
            pixels[index + 2] = (x * 23 + y * 5) as u8;
            pixels[index + 3] = 255;
        }
    }
    pixels
}

fn read_texture_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let unpadded_bytes_per_row = width * 4;
    let padded_bytes_per_row = padded_bytes_per_row(width);
    let buffer_size = u64::from(padded_bytes_per_row) * u64::from(height);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hypercolor macOS IOSurface fixture readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("hypercolor macOS IOSurface fixture readback"),
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
        .map_err(|error| format!("fixture readback poll failed: {error:?}"))?;
    receiver
        .recv()
        .map_err(|error| format!("fixture readback channel failed: {error}"))?
        .map_err(|error| format!("fixture readback buffer map failed: {error:?}"))?;

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

    Ok(pixels)
}

const fn padded_bytes_per_row(width: u32) -> u32 {
    let unpadded = width * 4;
    let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    unpadded.div_ceil(alignment) * alignment
}
