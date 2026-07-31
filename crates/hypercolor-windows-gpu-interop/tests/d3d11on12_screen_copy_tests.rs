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
    let bridge = D3d11On12ScreenBridge::new(wgpu.device.clone(), wgpu.queue.clone())
        .map_err(|error| error.to_string())?;
    let descriptor = fixture_descriptor(29, 4, 3)?;
    let plan = fixture_plan_generation(11)?;
    let first = publish_fixture(
        bridge.adapter_luid(),
        &descriptor,
        plan,
        "fixture:left",
        7,
        [10, 20, 30, 255],
    )?;
    let wrong_fixture = publish_fixture(
        bridge.adapter_luid(),
        &descriptor,
        fixture_plan_generation(12)?,
        "fixture:left",
        7,
        [11, 21, 31, 255],
    )?;
    let wrong_target = bridge
        .prepare_target(wrong_fixture.target_preparation())
        .map_err(|error| error.to_string())?;
    assert!(matches!(
        bridge.copy_publication(&wrong_target, first.publication()),
        Err(D3d11On12ScreenInteropError::PreparedTargetMismatch {
            field: "plan_generation"
        })
    ));
    drop(wrong_target);
    let target = bridge
        .prepare_target(first.target_preparation())
        .map_err(|error| error.to_string())?;
    assert_eq!(
        first.publication().opaque_handle_id(),
        first.target_preparation().slots()[0].opaque_handle_id(),
    );
    let before_first_copy = bridge.cache_stats().map_err(|error| error.to_string())?;
    let first_copy = bridge
        .copy_publication(&target, first.publication())
        .map_err(|error| error.to_string())?;
    assert_eq!(
        bridge.cache_stats().map_err(|error| error.to_string())?,
        before_first_copy,
        "first delivery must not open resources or mutate target caches",
    );
    assert_pixels(&wgpu, &first_copy, [30, 20, 10, 255])?;
    let duplicate = bridge
        .copy_publication(&target, first.publication())
        .map_err(|error| error.to_string())?;
    assert_eq!(duplicate.content_generation, first_copy.content_generation);

    let second_source = publish_fixture(
        bridge.adapter_luid(),
        &descriptor,
        plan,
        "fixture:right",
        7,
        [40, 50, 60, 255],
    )?;
    assert!(matches!(
        bridge.copy_publication(&target, second_source.publication()),
        Err(D3d11On12ScreenInteropError::PreparedTargetMismatch { field: "source_id" })
    ));
    let second_target = bridge
        .prepare_target(second_source.target_preparation())
        .map_err(|error| error.to_string())?;
    let second_copy = bridge
        .copy_publication(&second_target, second_source.publication())
        .map_err(|error| error.to_string())?;
    assert!(second_copy.content_generation > first_copy.content_generation);
    assert_pixels(&wgpu, &second_copy, [60, 50, 40, 255])?;

    let restarted = publish_fixture(
        bridge.adapter_luid(),
        &descriptor,
        plan,
        "fixture:right",
        8,
        [70, 80, 90, 255],
    )?;
    let restarted_target = bridge
        .prepare_target(restarted.target_preparation())
        .map_err(|error| error.to_string())?;
    let restarted_copy = bridge
        .copy_publication(&restarted_target, restarted.publication())
        .map_err(|error| error.to_string())?;
    assert!(restarted_copy.content_generation > second_copy.content_generation);
    assert_pixels(&wgpu, &restarted_copy, [90, 80, 70, 255])?;

    drop(target);
    assert_eq!(
        bridge
            .cache_stats()
            .map_err(|error| error.to_string())?
            .prepared_targets,
        3,
        "texture readers must retain each exact prepared target",
    );
    let duplicate_error = bridge
        .prepare_target(first.target_preparation())
        .expect_err("reader-retained target prevents a duplicate native claimant");
    assert!(matches!(
        duplicate_error,
        D3d11On12ScreenInteropError::PreparedTargetAlreadyLive
    ));
    drop(restarted_target);
    drop(second_target);
    drop(restarted_copy);
    drop(second_copy);
    drop(duplicate);
    drop(first_copy);
    let final_stats = bridge.cache_stats().map_err(|error| error.to_string())?;
    assert_eq!(final_stats.prepared_targets, 0);
    assert_eq!(final_stats.opened_surfaces, 0);
    assert_eq!(final_stats.retained_target_bytes, 0);
    Ok(())
}

#[test]
fn sequential_plan_turnover_keeps_target_and_surface_caches_bounded() -> Result<(), String> {
    if std::env::var(RUN_FIXTURE_ENV).as_deref() != Ok("1") {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the D3D11On12 fixture");
        return Ok(());
    }

    let wgpu = WgpuFixture::new_dx12("hypercolor D3D11On12 turnover fixture")?;
    let bridge = D3d11On12ScreenBridge::new(wgpu.device.clone(), wgpu.queue.clone())
        .map_err(|error| error.to_string())?;
    let descriptor = fixture_descriptor(30, 4, 3)?;

    for generation in 20..36 {
        let plan = fixture_plan_generation(generation)?;
        let fixture = publish_fixture(
            bridge.adapter_luid(),
            &descriptor,
            plan,
            "fixture:turnover",
            generation,
            [generation as u8, 90, 140, 255],
        )?;
        let target = bridge
            .prepare_target(fixture.target_preparation())
            .map_err(|error| error.to_string())?;
        let copy = bridge
            .copy_publication(&target, fixture.publication())
            .map_err(|error| error.to_string())?;
        let stats = bridge.cache_stats().map_err(|error| error.to_string())?;
        assert_eq!(stats.prepared_targets, 1);
        assert_eq!(stats.opened_surfaces, 2);

        drop(target);
        drop(copy);
        let retired = bridge.cache_stats().map_err(|error| error.to_string())?;
        assert_eq!(retired.prepared_targets, 0);
        assert_eq!(retired.opened_surfaces, 0);
        assert_eq!(retired.retained_target_bytes, 0);
    }
    Ok(())
}

fn publish_fixture(
    adapter_luid: hypercolor_windows_capture::GpuAdapterLuid,
    descriptor: &GpuSurfaceDescriptor,
    plan_generation: GpuSurfacePlanGeneration,
    source_id: &'static str,
    duplication_generation: u64,
    pixel: [u8; 4],
) -> Result<hypercolor_windows_capture::fixtures::GpuSurfaceFixture, String> {
    let bgra = pixel.repeat(4 * 3);
    publish_gpu_surface(GpuSurfaceFixtureConfig {
        adapter_luid,
        plan_generation,
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

fn fixture_plan_generation(value: u64) -> Result<GpuSurfacePlanGeneration, String> {
    let value = NonZeroU64::new(value)
        .ok_or_else(|| "fixture plan generation must be non-zero".to_owned())?;
    Ok(GpuSurfacePlanGeneration::new(value))
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
