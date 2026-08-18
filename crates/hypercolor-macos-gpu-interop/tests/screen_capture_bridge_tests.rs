#![cfg(target_os = "macos")]

use std::sync::{Arc, mpsc};

use hypercolor_macos_capture::{
    MacosCaptureColorimetry, MacosCaptureFrame, MacosCaptureGeometry, MacosCapturePixelFormat,
    MacosCaptureSurface, MacosChromaLocation, MacosColorPrimaries, MacosColorRange,
    MacosPixelExtent, MacosPixelRect, MacosPointRect, MacosScale, MacosTransferFunction,
    MacosYuvMatrix,
};
#[cfg(target_arch = "x86_64")]
use hypercolor_macos_gpu_interop::{
    ImportedFrameFormat, MacosIosurfaceImportDescriptor, MacosIosurfaceImporter,
    MacosScreenImporterCandidate, create_bgra_iosurface, qualify_macos_system_default_metal_device,
    write_bgra_pixels,
};
use hypercolor_macos_gpu_interop::{
    MacosMetalStorageMode, MacosNativeLetterboxFill, MacosNativeReducer,
    MacosNativeReductionDescriptor, MacosNativeReductionError, MacosNativeReductionFilter,
    MacosNativeTargetFormat, MacosScreenBridge,
};

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
        first.storage_identity().source_fourcc,
        MacosCapturePixelFormat::Bgra8
            .fourcc(MacosColorRange::Full)
            .expect("BGRA has one canonical full-range FourCC")
    );
    assert_eq!(
        first.storage_identity().iosurface_id,
        frame.surface.iosurface_id
    );
    assert_eq!(
        first.storage_identity().bytes_per_row,
        frame.planes[0].bytes_per_row
    );
    assert!(Arc::ptr_eq(first.capture(), &frame));
    assert_eq!(first.planes().len(), 1);
    assert_eq!(
        first.planes()[0].format(),
        hypercolor_macos_gpu_interop::ImportedMacosScreenPlaneFormat::Wgpu(
            hypercolor_macos_gpu_interop::ImportedFrameFormat::Bgra8Unorm
        )
    );
    assert_eq!(
        first.planes()[0].storage_identity(),
        first.storage_identity()
    );
    assert_eq!(
        first.storage_identities().collect::<Vec<_>>(),
        vec![first.storage_identity()]
    );
    let first_texture = first.texture().expect("BGRA import has a wgpu texture");
    let second_texture = second.texture().expect("BGRA import has a wgpu texture");
    let first_view = first.view().expect("BGRA import has a wgpu view");
    let second_view = second.view().expect("BGRA import has a wgpu view");
    assert!(Arc::ptr_eq(first_texture, second_texture));
    assert!(Arc::ptr_eq(first_view, second_view));
    assert_eq!(bridge.cached_wrap_count(), 1);
    assert_eq!(
        read_texture_pixels(&wgpu.device, &wgpu.queue, first_texture, WIDTH, HEIGHT,)?,
        fixture_pixels()
    );

    let next_resource = bridge
        .import_bgra_frame(&wgpu.device, 12, Arc::clone(&frame))
        .map_err(|error| error.to_string())?;
    let next_texture = next_resource
        .texture()
        .expect("BGRA import has a wgpu texture");
    assert!(!Arc::ptr_eq(first_texture, next_texture));
    assert_eq!(bridge.cached_wrap_count(), 2);

    drop(frame);
    assert_eq!(Arc::strong_count(first.capture()), 5);
    bridge.clear_capture_caches();
    assert_eq!(bridge.cached_wrap_count(), 0);
    assert_eq!(Arc::strong_count(first.capture()), 3);
    assert_eq!(first.capture().surface.retained_owner_count(), 1);
    Ok(())
}

#[cfg(target_arch = "x86_64")]
#[test]
fn intel_runner_qualification_requires_native_device_and_both_import_candidates()
-> Result<(), String> {
    let qualification =
        qualify_macos_system_default_metal_device().map_err(|error| error.to_string())?;
    println!(
        "Intel Metal qualification: device={} registry_id={} apple_family={}",
        qualification.device_name, qualification.registry_id, qualification.apple_family
    );
    if qualification.apple_family {
        return Err(
            "Intel runner qualification requires a non-Apple-family Metal device".to_owned(),
        );
    }

    let wgpu = WgpuFixture::new()?;
    let pixels = fixture_pixels();
    let iosurface = create_bgra_iosurface(WIDTH, HEIGHT).map_err(|error| error.to_string())?;
    write_bgra_pixels(&iosurface, WIDTH, HEIGHT, &pixels).map_err(|error| error.to_string())?;
    let descriptor =
        MacosIosurfaceImportDescriptor::new(WIDTH, HEIGHT, ImportedFrameFormat::Bgra8Unorm)
            .map_err(|error| error.to_string())?;
    let mut importer =
        MacosIosurfaceImporter::new(&wgpu.device, descriptor).map_err(|error| error.to_string())?;
    if importer.metal_registry_id() != qualification.registry_id {
        return Err(format!(
            "wgpu Metal device {} does not match system-default device {}",
            importer.metal_registry_id(),
            qualification.registry_id
        ));
    }
    let imported = importer
        .import_iosurface_for_test(&wgpu.device, &iosurface)
        .map_err(|error| error.to_string())?;
    assert_eq!(
        read_texture_pixels(&wgpu.device, &wgpu.queue, &imported.texture, WIDTH, HEIGHT)?,
        pixels
    );

    let frame = Arc::new(capture_frame()?);
    let bridge = MacosScreenBridge::new(&wgpu.device).map_err(|error| error.to_string())?;
    let direct = bridge
        .import_frame_via_candidate_for_test(
            MacosScreenImporterCandidate::DirectIosurface,
            &wgpu.device,
            31,
            Arc::clone(&frame),
        )
        .map_err(|error| error.to_string())?;
    assert!(!direct.planes()[0].uses_core_video_texture_cache());
    let direct_texture = direct
        .texture()
        .expect("BGRA direct import has a wgpu texture");
    assert_eq!(
        read_texture_pixels(&wgpu.device, &wgpu.queue, direct_texture, WIDTH, HEIGHT)?,
        fixture_pixels()
    );

    let core_video = bridge
        .import_frame_via_candidate_for_test(
            MacosScreenImporterCandidate::CoreVideoTextureCache,
            &wgpu.device,
            32,
            frame,
        )
        .map_err(|error| error.to_string())?;
    assert!(core_video.planes()[0].uses_core_video_texture_cache());
    let core_video_texture = core_video
        .texture()
        .expect("BGRA Core Video import has a wgpu texture");
    assert_eq!(
        read_texture_pixels(&wgpu.device, &wgpu.queue, core_video_texture, WIDTH, HEIGHT)?,
        fixture_pixels()
    );
    Ok(())
}

#[test]
fn native_reducer_compiles_and_reads_back_spatially_reduced_rgba() -> Result<(), String> {
    let wgpu = WgpuFixture::new()?;
    let bridge = MacosScreenBridge::new(&wgpu.device).map_err(|error| error.to_string())?;
    let reducer = MacosNativeReducer::new(&wgpu.device).map_err(|error| error.to_string())?;
    let gradient_row = [
        0_u8, 0, 0, 255, 20, 40, 60, 255, 40, 80, 120, 255, 60, 120, 180, 255,
    ];
    let gradient = gradient_row.repeat(HEIGHT as usize);
    let extent = MacosPixelExtent::new(WIDTH, HEIGHT).map_err(|error| error.to_string())?;
    let color = MacosCaptureColorimetry {
        primaries: MacosColorPrimaries::Srgb,
        transfer: MacosTransferFunction::Srgb,
        matrix: None,
        range: MacosColorRange::Full,
        chroma_location: None,
    };
    let imported = bridge
        .import_frame(
            &wgpu.device,
            11,
            Arc::new(native_capture_frame(
                extent,
                MacosCapturePixelFormat::Bgra8,
                color,
                &[gradient],
                1,
            )?),
        )
        .map_err(|error| error.to_string())?;
    let target = reducer
        .create_target(&wgpu.device, 2, 1, MacosNativeTargetFormat::Rgba8)
        .map_err(|error| error.to_string())?;
    let descriptor = MacosNativeReductionDescriptor::new(
        [2, 1],
        [0, 0, 2, 1],
        [0.0, 0.0, WIDTH as f32, HEIGHT as f32],
        MacosNativeReductionFilter::Area,
        None,
    )
    .map_err(|error| error.to_string())?;
    let mut encoder = wgpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("hypercolor macOS native reduction fixture"),
        });
    reducer
        .encode(&imported, &target, descriptor, &mut encoder)
        .map_err(|error| error.to_string())?;
    let _ = wgpu.queue.submit(Some(encoder.finish()));

    assert_eq!(
        read_texture_pixels(&wgpu.device, &wgpu.queue, target.texture(), 2, 1)?,
        [30, 20, 10, 255, 150, 100, 50, 255]
    );

    let transparent = reducer
        .create_target(&wgpu.device, 4, 3, MacosNativeTargetFormat::Rgba8)
        .map_err(|error| error.to_string())?;
    let solid = reducer
        .create_target(&wgpu.device, 4, 3, MacosNativeTargetFormat::Rgba8)
        .map_err(|error| error.to_string())?;
    let mut encoder = wgpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("hypercolor macOS native materialization fixture"),
        });
    reducer
        .encode_materialization(
            &target,
            &transparent,
            [1, 1, 2, 1],
            MacosNativeLetterboxFill::Transparent,
            &mut encoder,
        )
        .map_err(|error| error.to_string())?;
    reducer
        .encode_materialization(
            &target,
            &solid,
            [1, 1, 2, 1],
            MacosNativeLetterboxFill::Solid([
                7.0 / 255.0,
                11.0 / 255.0,
                13.0 / 255.0,
                17.0 / 255.0,
            ]),
            &mut encoder,
        )
        .map_err(|error| error.to_string())?;
    let _ = wgpu.queue.submit(Some(encoder.finish()));
    let mut transparent_expected = vec![0; 4 * 3 * 4];
    transparent_expected[20..28].copy_from_slice(&[30, 20, 10, 255, 150, 100, 50, 255]);
    assert_eq!(
        read_texture_pixels(&wgpu.device, &wgpu.queue, transparent.texture(), 4, 3,)?,
        transparent_expected
    );
    let mut solid_expected = [7, 11, 13, 17].repeat(12);
    solid_expected[20..28].copy_from_slice(&[30, 20, 10, 255, 150, 100, 50, 255]);
    assert_eq!(
        read_texture_pixels(&wgpu.device, &wgpu.queue, solid.texture(), 4, 3)?,
        solid_expected
    );
    Ok(())
}

#[test]
fn every_native_format_matches_the_scalar_source_oracle() -> Result<(), String> {
    let wgpu = WgpuFixture::new()?;
    let bridge = MacosScreenBridge::new(&wgpu.device).map_err(|error| error.to_string())?;
    let reducer = MacosNativeReducer::new(&wgpu.device).map_err(|error| error.to_string())?;
    let extent = MacosPixelExtent::new(3, 3).map_err(|error| error.to_string())?;
    let fixtures = native_format_vectors();

    for (index, (format, color, planes)) in fixtures.into_iter().enumerate() {
        let frame = Arc::new(native_capture_frame(
            extent,
            format,
            color,
            &planes,
            u64::try_from(index + 1).map_err(|error| error.to_string())?,
        )?);
        let expected = scalar_rgba8(&frame)?;
        let imported = bridge
            .import_frame(&wgpu.device, 17, frame)
            .map_err(|error| error.to_string())?;
        let target = reducer
            .create_target(
                &wgpu.device,
                extent.width,
                extent.height,
                MacosNativeTargetFormat::Rgba8,
            )
            .map_err(|error| error.to_string())?;
        let descriptor = MacosNativeReductionDescriptor::new(
            [extent.width, extent.height],
            [0, 0, extent.width, extent.height],
            [0.0, 0.0, extent.width as f32, extent.height as f32],
            MacosNativeReductionFilter::Nearest,
            None,
        )
        .map_err(|error| error.to_string())?;
        let mut encoder = wgpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("hypercolor macOS native format parity fixture"),
            });
        reducer
            .encode(&imported, &target, descriptor, &mut encoder)
            .map_err(|error| error.to_string())?;
        let _ = wgpu.queue.submit(Some(encoder.finish()));
        let actual = read_texture_pixels(
            &wgpu.device,
            &wgpu.queue,
            target.texture(),
            extent.width,
            extent.height,
        )?;
        let tolerance = match format {
            MacosCapturePixelFormat::Yuv420VideoRange
            | MacosCapturePixelFormat::Yuv420FullRange
            | MacosCapturePixelFormat::Yuv44410BiPlanar => 1,
            MacosCapturePixelFormat::Bgra8
            | MacosCapturePixelFormat::Argb2101010
            | MacosCapturePixelFormat::Rgba16Float => 0,
        };
        assert_eq!(actual.len(), expected.len(), "{format:?} output length");
        for (byte_index, (actual, expected)) in actual.into_iter().zip(expected).enumerate() {
            assert!(
                actual.abs_diff(expected) <= tolerance,
                "{format:?} scalar parity at byte {byte_index}: actual {actual}, expected {expected}, tolerance {tolerance}"
            );
        }
    }
    Ok(())
}

#[test]
fn native_reducer_rejects_missing_yuv_color_metadata() -> Result<(), String> {
    let wgpu = WgpuFixture::new()?;
    let bridge = MacosScreenBridge::new(&wgpu.device).map_err(|error| error.to_string())?;
    let reducer = MacosNativeReducer::new(&wgpu.device).map_err(|error| error.to_string())?;
    let extent = MacosPixelExtent::new(1, 1).map_err(|error| error.to_string())?;
    let valid_color = MacosCaptureColorimetry {
        primaries: MacosColorPrimaries::Rec2020,
        transfer: MacosTransferFunction::Pq,
        matrix: Some(MacosYuvMatrix::Bt2020),
        range: MacosColorRange::Video,
        chroma_location: Some(MacosChromaLocation::Center),
    };
    let mut frame = native_capture_frame(
        extent,
        MacosCapturePixelFormat::Yuv420VideoRange,
        valid_color,
        &[vec![128], vec![64, 192]],
        1,
    )?;
    frame.color.matrix = None;
    let imported = bridge
        .import_frame(&wgpu.device, 31, Arc::new(frame))
        .map_err(|error| error.to_string())?;
    let target = reducer
        .create_target(&wgpu.device, 1, 1, MacosNativeTargetFormat::Rgba8)
        .map_err(|error| error.to_string())?;
    let descriptor = MacosNativeReductionDescriptor::new(
        [1, 1],
        [0, 0, 1, 1],
        [0.0, 0.0, 1.0, 1.0],
        MacosNativeReductionFilter::Nearest,
        None,
    )
    .map_err(|error| error.to_string())?;
    let mut encoder = wgpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("hypercolor invalid YUV metadata fixture"),
        });
    assert!(matches!(
        reducer.encode(&imported, &target, descriptor, &mut encoder),
        Err(MacosNativeReductionError::InvalidPlanes(_))
    ));
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

fn native_capture_frame(
    extent: MacosPixelExtent,
    format: MacosCapturePixelFormat,
    color: MacosCaptureColorimetry,
    planes: &[Vec<u8>],
    sequence: u64,
) -> Result<MacosCaptureFrame, String> {
    let borrowed = planes.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let (surface, planes) =
        MacosCaptureSurface::new_native_fixture(extent, format, color, &borrowed)
            .map_err(|error| error.to_string())?;
    Ok(MacosCaptureFrame {
        epoch: 5,
        sequence,
        display_time: 13 + sequence,
        storage_extent: extent,
        planes: Arc::from(planes),
        pixel_format: format,
        color,
        geometry: MacosCaptureGeometry {
            display_scale_factor: MacosScale::display(1.0).map_err(|error| error.to_string())?,
            content_scale: MacosScale::new(1.0).map_err(|error| error.to_string())?,
            content_rect_points: MacosPointRect::new(
                0.0,
                0.0,
                extent.width.into(),
                extent.height.into(),
            )
            .map_err(|error| error.to_string())?,
            content_rect_pixels: MacosPixelRect::new(0, 0, extent.width, extent.height)
                .map_err(|error| error.to_string())?,
            screen_rect_points: None,
            bounding_rect_points: None,
            bounding_rect_pixels: None,
        },
        damage: Arc::from([]),
        cursor_composed: false,
        surface,
    })
}

fn native_format_vectors() -> Vec<(
    MacosCapturePixelFormat,
    MacosCaptureColorimetry,
    Vec<Vec<u8>>,
)> {
    let rgb = MacosCaptureColorimetry {
        primaries: MacosColorPrimaries::Srgb,
        transfer: MacosTransferFunction::Srgb,
        matrix: None,
        range: MacosColorRange::Full,
        chroma_location: None,
    };
    let linear = MacosCaptureColorimetry {
        transfer: MacosTransferFunction::Linear,
        ..rgb
    };
    let yuv = |range, matrix, chroma_location| MacosCaptureColorimetry {
        primaries: MacosColorPrimaries::Rec2020,
        transfer: MacosTransferFunction::Pq,
        matrix: Some(matrix),
        range,
        chroma_location: Some(chroma_location),
    };
    let bgra = (0..9_u8)
        .flat_map(|value| [value * 17, 255 - value * 13, value * 23, 255])
        .collect();
    let l10r: Vec<u8> = [
        (0_u32, 0_u32, 0_u32, 3_u32),
        (1_023, 0, 0, 3),
        (0, 1_023, 0, 3),
        (0, 0, 1_023, 3),
        (512, 256, 768, 2),
        (128, 900, 64, 1),
        (900, 128, 512, 3),
        (1023, 1023, 1023, 3),
        (64, 32, 16, 0),
    ]
    .into_iter()
    .flat_map(|(r, g, b, a): (u32, u32, u32, u32)| {
        ((a << 30) | (r << 20) | (g << 10) | b).to_le_bytes()
    })
    .collect();
    let rgha = [
        [0x0000, 0x0000, 0x0000, 0x3c00],
        [0x3c00, 0x0000, 0x0000, 0x3c00],
        [0x0000, 0x3c00, 0x0000, 0x3c00],
        [0x0000, 0x0000, 0x3c00, 0x3c00],
        [0x4000, 0x3800, 0xbc00, 0x3800],
        [0x3400, 0x3a00, 0x3e00, 0x3c00],
        [0x3b00, 0x3900, 0x3700, 0x3c00],
        [0x3c00, 0x3c00, 0x3c00, 0x3c00],
        [0x2c00, 0x3000, 0x3400, 0x0000],
    ]
    .into_iter()
    .flatten()
    .flat_map(u16::to_le_bytes)
    .collect();
    let luma_video = vec![16, 64, 128, 192, 235, 96, 32, 160, 224];
    let luma_full = vec![0, 31, 63, 95, 127, 159, 191, 223, 255];
    let chroma = vec![16, 240, 128, 128, 240, 16, 64, 192];
    let xf_luma = [0_u16, 64, 256, 512, 768, 876, 940, 1023, 128]
        .into_iter()
        .flat_map(|code| (code << 6).to_le_bytes())
        .collect();
    let xf_chroma: Vec<u8> = [
        (512_u16, 512_u16),
        (64, 960),
        (960, 64),
        (256, 768),
        (768, 256),
        (512, 960),
        (960, 512),
        (128, 128),
        (896, 896),
    ]
    .into_iter()
    .flat_map(|(cb, cr): (u16, u16)| [(cb << 6).to_le_bytes(), (cr << 6).to_le_bytes()])
    .flatten()
    .collect();
    vec![
        (MacosCapturePixelFormat::Bgra8, rgb, vec![bgra]),
        (MacosCapturePixelFormat::Argb2101010, linear, vec![l10r]),
        (MacosCapturePixelFormat::Rgba16Float, linear, vec![rgha]),
        (
            MacosCapturePixelFormat::Yuv420VideoRange,
            yuv(
                MacosColorRange::Video,
                MacosYuvMatrix::Bt709,
                MacosChromaLocation::Left,
            ),
            vec![luma_video, chroma.clone()],
        ),
        (
            MacosCapturePixelFormat::Yuv420FullRange,
            yuv(
                MacosColorRange::Full,
                MacosYuvMatrix::Bt2020,
                MacosChromaLocation::TopLeft,
            ),
            vec![luma_full, chroma],
        ),
        (
            MacosCapturePixelFormat::Yuv44410BiPlanar,
            yuv(
                MacosColorRange::Video,
                MacosYuvMatrix::Bt601,
                MacosChromaLocation::Center,
            ),
            vec![xf_luma, xf_chroma],
        ),
    ]
}

fn scalar_rgba8(frame: &MacosCaptureFrame) -> Result<Vec<u8>, String> {
    frame
        .with_cpu_source(|source| {
            let mut output = Vec::new();
            for y in 0..source.extent().height {
                for x in 0..source.extent().width {
                    let pixel = source
                        .sample_rgba32f(x, y)
                        .map_err(|error| error.to_string())?;
                    output.extend(
                        pixel.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8),
                    );
                }
            }
            Ok(output)
        })
        .map_err(|error| error.to_string())?
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
