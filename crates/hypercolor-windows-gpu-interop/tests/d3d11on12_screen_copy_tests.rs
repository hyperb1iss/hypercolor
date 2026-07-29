#![cfg(all(target_os = "windows", feature = "screen-capture-fixtures"))]
#![allow(
    dead_code,
    reason = "shared fixture support is compiled independently by each integration test"
)]

use std::num::NonZeroU64;
use std::sync::Arc;

use hypercolor_windows_capture::fixtures::{GpuSurfaceFixtureConfig, publish_gpu_surface};
use hypercolor_windows_capture::{
    CaptureExtent, CaptureRegion, GpuSurfaceDescriptor, GpuSurfacePlanGeneration,
};
use hypercolor_windows_gpu_interop::{D3d11On12ScreenBridge, D3d11On12ScreenInteropError};

mod support;

use support::{WgpuFixture, read_texture_pixels};

const RUN_FIXTURE_ENV: &str = "HYPERCOLOR_RUN_WINDOWS_D3D11ON12_FIXTURE";

#[test]
fn copies_exact_pixels_and_separates_native_resource_incarnations() -> Result<(), String> {
    if std::env::var(RUN_FIXTURE_ENV).as_deref() != Ok("1") {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the D3D11On12 fixture");
        return Ok(());
    }

    let wgpu = WgpuFixture::new_dx12("hypercolor D3D11On12 copy fixture")?;
    let mut bridge = D3d11On12ScreenBridge::new(wgpu.device.clone(), wgpu.queue.clone())
        .map_err(|error| error.to_string())?;
    let descriptor = fixture_descriptor(29, 4, 3)?;
    let first = publish_fixture(
        bridge.adapter_luid(),
        &descriptor,
        "fixture:left",
        7,
        [10, 20, 30, 255],
    )?;
    assert!(matches!(
        bridge.copy_publication(first.publication()),
        Err(D3d11On12ScreenInteropError::TargetNotPrepared { .. })
    ));
    bridge
        .prepare_target(&descriptor)
        .map_err(|error| error.to_string())?;
    let first_copy = bridge
        .copy_publication(first.publication())
        .map_err(|error| error.to_string())?;
    assert_pixels(&wgpu, &first_copy, [30, 20, 10, 255])?;
    let duplicate = bridge
        .copy_publication(first.publication())
        .map_err(|error| error.to_string())?;
    assert_eq!(duplicate.content_generation, first_copy.content_generation);

    let second_source = publish_fixture(
        bridge.adapter_luid(),
        &descriptor,
        "fixture:right",
        7,
        [40, 50, 60, 255],
    )?;
    let second_copy = bridge
        .copy_publication(second_source.publication())
        .map_err(|error| error.to_string())?;
    assert!(second_copy.content_generation > first_copy.content_generation);
    assert_pixels(&wgpu, &second_copy, [60, 50, 40, 255])?;

    let restarted = publish_fixture(
        bridge.adapter_luid(),
        &descriptor,
        "fixture:right",
        8,
        [70, 80, 90, 255],
    )?;
    let restarted_copy = bridge
        .copy_publication(restarted.publication())
        .map_err(|error| error.to_string())?;
    assert!(restarted_copy.content_generation > second_copy.content_generation);
    assert_pixels(&wgpu, &restarted_copy, [90, 80, 70, 255])?;

    bridge.retire_source("fixture:right", 3, fixture_plan_generation());
    assert!(matches!(
        bridge.copy_publication(restarted.publication()),
        Err(D3d11On12ScreenInteropError::Capture(_))
    ));
    Ok(())
}

fn publish_fixture(
    adapter_luid: hypercolor_windows_capture::GpuAdapterLuid,
    descriptor: &GpuSurfaceDescriptor,
    source_id: &'static str,
    duplication_generation: u64,
    pixel: [u8; 4],
) -> Result<hypercolor_windows_capture::fixtures::GpuSurfaceFixture, String> {
    let bgra = pixel.repeat(4 * 3);
    publish_gpu_surface(GpuSurfaceFixtureConfig {
        adapter_luid,
        plan_generation: fixture_plan_generation(),
        source_id: Arc::from(source_id),
        topology_generation: 3,
        duplication_generation,
        descriptor: descriptor.clone(),
        bgra,
        width: 4,
        height: 3,
    })
    .map_err(|error| error.to_string())
}

fn fixture_plan_generation() -> GpuSurfacePlanGeneration {
    GpuSurfacePlanGeneration::new(NonZeroU64::new(11).expect("fixture plan generation is non-zero"))
}

fn fixture_descriptor(id: u64, width: u32, height: u32) -> Result<GpuSurfaceDescriptor, String> {
    use std::time::Duration;

    use hypercolor_windows_capture::{
        DisplayRotation, GPU_SURFACE_ALGORITHM_REVISION, GpuSurfaceColorPipeline,
        GpuSurfaceCoordinateSpace, GpuSurfaceCursorPolicy, GpuSurfaceDescriptorConfig,
        GpuSurfaceDescriptorId, GpuSurfaceFilter, GpuSurfaceFormat, GpuSurfaceSourceColorSpace,
    };

    let extent = CaptureExtent::try_new(width, height).map_err(|error| error.to_string())?;
    let region = CaptureRegion::new(0, 0, width, height)
        .ok_or_else(|| "fixture capture region must be non-empty".to_owned())?;
    let id =
        NonZeroU64::new(id).ok_or_else(|| "fixture descriptor id must be non-zero".to_owned())?;
    Ok(GpuSurfaceDescriptor::new(GpuSurfaceDescriptorConfig {
        id: GpuSurfaceDescriptorId::new(id),
        source_region: region,
        coordinate_space: GpuSurfaceCoordinateSpace::LogicalDisplay,
        source_rotation: DisplayRotation::Identity,
        source_color_space: GpuSurfaceSourceColorSpace::RgbFullG22P709,
        output_extent: extent,
        filter: GpuSurfaceFilter::Nearest,
        format: GpuSurfaceFormat::Rgba8Unorm,
        color_pipeline: GpuSurfaceColorPipeline::PreserveEncoded,
        cursor: GpuSurfaceCursorPolicy::Exclude,
        algorithm_revision: GPU_SURFACE_ALGORITHM_REVISION,
        freshness: Duration::from_secs(1),
    }))
}

fn assert_pixels(
    wgpu: &WgpuFixture,
    copied: &hypercolor_windows_gpu_interop::ScreenTextureCopy,
    expected: [u8; 4],
) -> Result<(), String> {
    let pixels = read_texture_pixels(
        &wgpu.device,
        &wgpu.queue,
        &copied.texture,
        copied.width,
        copied.height,
    )?;
    for pixel in pixels.chunks_exact(4) {
        if pixel != expected {
            return Err(format!("copied pixel {pixel:?} did not match {expected:?}"));
        }
    }
    Ok(())
}
