#![cfg(target_os = "windows")]

use std::sync::mpsc;

pub struct WgpuFixture {
    _instance: wgpu::Instance,
    pub adapter_info: wgpu::AdapterInfo,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl WgpuFixture {
    pub fn new(label: &str) -> Result<Self, String> {
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
            label: Some(label),
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

#[allow(dead_code)]
pub fn patterned_bgra_pixels(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = vec![0; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let index = ((y * width + x) * 4) as usize;
            pixels[index] = (x * 17 + y * 3) as u8;
            pixels[index + 1] = (x * 11 + y * 19) as u8;
            pixels[index + 2] = (x * 23 + y * 5) as u8;
            pixels[index + 3] = 255;
        }
    }
    pixels
}

pub fn read_texture_pixels(
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
        label: Some("hypercolor Windows interop fixture readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("hypercolor Windows interop fixture readback"),
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

#[allow(dead_code)]
pub fn assert_uniform_bgra(pixels: &[u8], expected: [u8; 4]) {
    for pixel in pixels.chunks_exact(4) {
        assert!(
            pixel
                .iter()
                .zip(expected)
                .all(|(actual, expected)| actual.abs_diff(expected) <= 1),
            "pixel {pixel:?} did not match {expected:?}"
        );
    }
}

const fn padded_bytes_per_row(width: u32) -> u32 {
    let unpadded = width * 4;
    let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    unpadded.div_ceil(alignment) * alignment
}
