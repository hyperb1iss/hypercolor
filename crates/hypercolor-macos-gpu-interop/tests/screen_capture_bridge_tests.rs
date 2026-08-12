#![cfg(target_os = "macos")]

use std::sync::{Arc, mpsc};

use hypercolor_macos_capture::{
    MacosCaptureColorimetry, MacosCaptureFrame, MacosCaptureGeometry, MacosCapturePixelFormat,
    MacosCaptureSurface, MacosColorPrimaries, MacosColorRange, MacosPixelExtent, MacosPixelRect,
    MacosPointRect, MacosScale, MacosTransferFunction,
};
use hypercolor_macos_gpu_interop::{MacosMetalStorageMode, MacosScreenBridge};

const WIDTH: u32 = 4;
const HEIGHT: u32 = 3;

#[test]
fn bridge_imports_and_caches_complete_capture_storage_identity() -> Result<(), String> {
    let wgpu = WgpuFixture::new()?;
    let bridge = MacosScreenBridge::new(&wgpu.device).map_err(|error| error.to_string())?;
    assert_ne!(bridge.metal_registry_id(), 0);
    #[cfg(target_arch = "aarch64")]
    assert_eq!(bridge.storage_mode(), MacosMetalStorageMode::Shared);
    #[cfg(target_arch = "x86_64")]
    assert_eq!(bridge.storage_mode(), MacosMetalStorageMode::Managed);
    assert_eq!(bridge.cached_wrap_count(), 0);

    let frame = Arc::new(capture_frame()?);
    let first = bridge
        .import_bgra_frame(&wgpu.device, 11, Arc::clone(&frame))
        .map_err(|error| error.to_string())?;
    let second = bridge
        .import_bgra_frame(&wgpu.device, 11, Arc::clone(&frame))
        .map_err(|error| error.to_string())?;

    assert_eq!(first.content_sequence(), 0);
    assert_eq!(first.storage_identity().capture_session_generation, 5);
    assert_eq!(first.storage_identity().resource_generation, 11);
    assert_eq!(
        first.storage_identity().iosurface_id,
        frame.surface.iosurface_id
    );
    assert_eq!(
        first.storage_identity().bytes_per_row,
        frame.planes[0].bytes_per_row
    );
    assert!(Arc::ptr_eq(first.capture(), &frame));
    assert!(Arc::ptr_eq(first.texture(), second.texture()));
    assert!(Arc::ptr_eq(first.view(), second.view()));
    assert_eq!(bridge.cached_wrap_count(), 1);
    assert_eq!(
        read_texture_pixels(&wgpu.device, &wgpu.queue, first.texture(), WIDTH, HEIGHT,)?,
        fixture_pixels()
    );

    let next_resource = bridge
        .import_bgra_frame(&wgpu.device, 12, Arc::clone(&frame))
        .map_err(|error| error.to_string())?;
    assert!(!Arc::ptr_eq(first.texture(), next_resource.texture()));
    assert_eq!(bridge.cached_wrap_count(), 2);

    drop(frame);
    assert_eq!(Arc::strong_count(first.capture()), 3);
    assert_eq!(first.capture().surface.retained_owner_count(), 1);
    Ok(())
}

fn capture_frame() -> Result<MacosCaptureFrame, String> {
    let extent = MacosPixelExtent::new(WIDTH, HEIGHT).map_err(|error| error.to_string())?;
    let pixels = fixture_pixels();
    let (surface, plane) = MacosCaptureSurface::new_native_bgra_fixture(extent, &pixels)
        .map_err(|error| error.to_string())?;
    Ok(MacosCaptureFrame {
        epoch: 5,
        sequence: 0,
        display_time: 13,
        storage_extent: extent,
        planes: Arc::from([plane]),
        pixel_format: MacosCapturePixelFormat::Bgra8,
        color: MacosCaptureColorimetry {
            primaries: MacosColorPrimaries::Srgb,
            transfer: MacosTransferFunction::Srgb,
            matrix: None,
            range: MacosColorRange::Full,
            chroma_location: None,
        },
        geometry: MacosCaptureGeometry {
            display_scale_factor: MacosScale::display(1.0).map_err(|error| error.to_string())?,
            content_scale: MacosScale::new(1.0).map_err(|error| error.to_string())?,
            content_rect_points: MacosPointRect::new(0.0, 0.0, WIDTH.into(), HEIGHT.into())
                .map_err(|error| error.to_string())?,
            content_rect_pixels: MacosPixelRect::new(0, 0, WIDTH, HEIGHT)
                .map_err(|error| error.to_string())?,
            screen_rect_points: None,
            bounding_rect_points: None,
            bounding_rect_pixels: None,
        },
        damage: Arc::from([]),
        cursor_composed: true,
        surface,
    })
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
        if adapter.get_info().backend != wgpu::Backend::Metal {
            return Err(format!(
                "requires Metal wgpu backend, got {:?}",
                adapter.get_info().backend
            ));
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("hypercolor macOS screen bridge fixture"),
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
    [17, 43, 91, 255].repeat((WIDTH * HEIGHT) as usize)
}

fn read_texture_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let unpadded_bytes_per_row = width * 4;
    let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer_size = u64::from(padded_bytes_per_row) * u64::from(height);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hypercolor macOS screen bridge readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("hypercolor macOS screen bridge readback"),
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
        .map_err(|error| format!("screen bridge readback poll failed: {error:?}"))?;
    receiver
        .recv()
        .map_err(|error| format!("screen bridge readback callback failed: {error}"))?
        .map_err(|error| format!("screen bridge readback mapping failed: {error}"))?;
    let mapped = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((unpadded_bytes_per_row * height) as usize);
    for row in mapped.chunks_exact(padded_bytes_per_row as usize) {
        pixels.extend_from_slice(&row[..unpadded_bytes_per_row as usize]);
    }
    drop(mapped);
    buffer.unmap();
    Ok(pixels)
}
