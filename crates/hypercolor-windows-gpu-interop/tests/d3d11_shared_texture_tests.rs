#![cfg(target_os = "windows")]

use std::sync::mpsc;

use hypercolor_windows_gpu_interop::{
    ImportedFrameFormat, WindowsD3d11Device, WindowsD3d11SharedTextureImportDescriptor,
    WindowsD3d11SharedTextureImporter,
};

const RUN_FIXTURE_ENV: &str = "HYPERCOLOR_RUN_WINDOWS_D3D11_FIXTURE";
const WIDTH: u32 = 4;
const HEIGHT: u32 = 3;

#[test]
fn imports_synthetic_d3d11_shared_texture_into_wgpu_texture() -> Result<(), String> {
    if std::env::var(RUN_FIXTURE_ENV).as_deref() != Ok("1") {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the D3D11 shared-texture fixture");
        return Ok(());
    }

    let wgpu = WgpuFixture::new()?;
    let descriptor = WindowsD3d11SharedTextureImportDescriptor::new(
        WIDTH,
        HEIGHT,
        ImportedFrameFormat::Bgra8Unorm,
    )
    .map_err(|error| error.to_string())?;
    let d3d11 = WindowsD3d11Device::new_for_wgpu_adapter(
        wgpu.adapter_info.vendor,
        wgpu.adapter_info.device,
    )
    .map_err(|error| error.to_string())?;
    let texture = d3d11
        .create_shared_texture(descriptor)
        .map_err(|error| error.to_string())?;
    let expected_pixels = fixture_pixels();
    d3d11
        .write_pixels(&texture, &expected_pixels)
        .map_err(|error| error.to_string())?;

    let mut importer = WindowsD3d11SharedTextureImporter::new(&wgpu.device, descriptor)
        .map_err(|error| error.to_string())?;
    // SAFETY: texture owns a live NT D3D11 shared handle matching descriptor,
    // and write_pixels flushed producer work before the import.
    let frame = unsafe { importer.import_shared_handle(&wgpu.device, texture.shared_handle(), 0) }
        .map_err(|error| error.to_string())?;
    let pixels = read_texture_pixels(
        &wgpu.device,
        &wgpu.queue,
        frame.texture.as_ref(),
        WIDTH,
        HEIGHT,
    )?;

    assert_eq!(frame.width, WIDTH);
    assert_eq!(frame.height, HEIGHT);
    assert_eq!(frame.format, ImportedFrameFormat::Bgra8Unorm);
    assert_eq!(pixels, expected_pixels);

    Ok(())
}

struct WgpuFixture {
    _instance: wgpu::Instance,
    adapter_info: wgpu::AdapterInfo,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl WgpuFixture {
    fn new() -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .map_err(|error| format!("could not create wgpu adapter: {error}"))?;
        let adapter_info = adapter.get_info();
        if adapter_info.backend != wgpu::Backend::Vulkan {
            return Err(format!(
                "requires Vulkan wgpu backend, got {:?}",
                adapter_info.backend
            ));
        }
        let required_features = wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32;
        if !adapter.features().contains(required_features) {
            return Err("wgpu adapter is missing VULKAN_EXTERNAL_MEMORY_WIN32".to_owned());
        }

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("hypercolor Windows D3D11 interop fixture"),
            required_features,
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|error| format!("could not create wgpu device: {error}"))?;

        Ok(Self {
            _instance: instance,
            adapter_info,
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
        label: Some("hypercolor Windows D3D11 fixture readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("hypercolor Windows D3D11 fixture readback"),
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
