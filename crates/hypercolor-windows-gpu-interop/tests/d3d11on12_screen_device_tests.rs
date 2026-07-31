#![cfg(all(target_os = "windows", feature = "screen-capture-fixtures"))]
#![allow(
    dead_code,
    reason = "shared fixture support is compiled independently by each integration test"
)]

use hypercolor_windows_capture::fixtures::{GpuSurfaceFixtureConfig, publish_gpu_surface};
use hypercolor_windows_capture::{
    CaptureExtent, CaptureRegion, DisplayRotation, GPU_SURFACE_ALGORITHM_REVISION,
    GpuSurfaceColorPipeline, GpuSurfaceCoordinateSpace, GpuSurfaceCursorPolicy,
    GpuSurfaceDescriptor, GpuSurfaceDescriptorConfig, GpuSurfaceDescriptorId, GpuSurfaceFilter,
    GpuSurfaceFormat, GpuSurfacePlanGeneration, GpuSurfaceSourceColorSpace,
};
use hypercolor_windows_gpu_interop::{
    D3d11On12ScreenBridge, D3d11On12ScreenDevice, D3d11On12ScreenInteropError,
};
use std::num::NonZeroU64;
use std::sync::{Arc, Barrier};
use std::time::Duration;

mod support;

use support::WgpuFixture;

const RUN_FIXTURE_ENV: &str = "HYPERCOLOR_RUN_WINDOWS_D3D11ON12_FIXTURE";

#[test]
fn binds_d3d11on12_to_the_renderer_dx12_queue() -> Result<(), String> {
    if std::env::var(RUN_FIXTURE_ENV).as_deref() != Ok("1") {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the D3D11On12 fixture");
        return Ok(());
    }

    let wgpu = WgpuFixture::new_dx12("hypercolor D3D11On12 device fixture")?;
    let interop =
        D3d11On12ScreenDevice::new(&wgpu.device, &wgpu.queue).map_err(|error| error.to_string())?;

    assert_ne!(
        (
            interop.adapter_luid().low_part(),
            interop.adapter_luid().high_part()
        ),
        (0, 0),
        "renderer adapter should expose a concrete DXGI LUID",
    );
    Ok(())
}

#[test]
fn rejects_a_duplicate_live_renderer_target() -> Result<(), String> {
    if std::env::var(RUN_FIXTURE_ENV).as_deref() != Ok("1") {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the D3D11On12 fixture");
        return Ok(());
    }

    let wgpu = WgpuFixture::new_dx12("hypercolor D3D11On12 target fixture")?;
    let bridge =
        D3d11On12ScreenBridge::new(wgpu.device, wgpu.queue).map_err(|error| error.to_string())?;
    let descriptor = fixture_descriptor(17, 641, 359)?;
    let plan = fixture_plan_generation(11)?;
    let fixture = publish_fixture(
        bridge.adapter_luid(),
        &descriptor,
        plan,
        "fixture:independent",
        1,
    )?;
    let quote = bridge
        .quote_target_bytes(fixture.target_preparation())
        .map_err(|error| error.to_string())?;
    let retained_quote = bridge
        .quote_target_retained_bytes(fixture.target_preparation())
        .map_err(|error| error.to_string())?;

    let first = bridge
        .prepare_target(fixture.target_preparation())
        .map_err(|error| error.to_string())?;
    let duplicate = bridge
        .prepare_target(fixture.target_preparation())
        .expect_err("one exact route has one live renderer target");

    assert!(matches!(
        duplicate,
        D3d11On12ScreenInteropError::PreparedTargetAlreadyLive
    ));
    assert_eq!(first.width(), 641);
    assert_eq!(first.height(), 359);
    let logical_bytes = 641 * 359 * 4;
    assert!(quote >= logical_bytes);
    assert_ne!(quote, logical_bytes);
    assert_eq!(quote % (64 * 1024), 0);
    assert_eq!(first.retained_bytes(), quote);
    assert_eq!(first.total_retained_bytes(), retained_quote);
    assert_eq!(first.total_retained_bytes(), quote + first.metadata_bytes());
    assert!(
        first.metadata_bytes()
            >= u64::try_from(
                std::mem::size_of::<wgpu::Texture>() + std::mem::size_of::<wgpu::TextureView>()
            )
            .expect("wgpu handle payload sizes must fit u64")
    );
    assert_eq!(
        bridge
            .cache_stats()
            .map_err(|error| error.to_string())?
            .prepared_targets,
        1
    );
    Ok(())
}

#[test]
fn aborted_preparation_releases_its_target() -> Result<(), String> {
    if std::env::var(RUN_FIXTURE_ENV).as_deref() != Ok("1") {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the D3D11On12 fixture");
        return Ok(());
    }

    let wgpu = WgpuFixture::new_dx12("hypercolor D3D11On12 abort fixture")?;
    let bridge =
        D3d11On12ScreenBridge::new(wgpu.device, wgpu.queue).map_err(|error| error.to_string())?;
    let descriptor = fixture_descriptor(18, 320, 180)?;
    let plan = fixture_plan_generation(12)?;
    let fixture = publish_fixture(bridge.adapter_luid(), &descriptor, plan, "fixture:abort", 1)?;
    let aborted = bridge
        .prepare_target(fixture.target_preparation())
        .map_err(|error| error.to_string())?;
    let aborted_storage = aborted.storage_id();
    drop(aborted);

    assert_eq!(
        bridge
            .cache_stats()
            .map_err(|error| error.to_string())?
            .prepared_targets,
        0
    );
    let retried = bridge
        .prepare_target(fixture.target_preparation())
        .map_err(|error| error.to_string())?;
    assert_ne!(retried.storage_id(), aborted_storage);
    Ok(())
}

#[test]
fn released_targets_leave_no_dynamic_cache_ownership() -> Result<(), String> {
    if std::env::var(RUN_FIXTURE_ENV).as_deref() != Ok("1") {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the D3D11On12 fixture");
        return Ok(());
    }

    let wgpu = WgpuFixture::new_dx12("hypercolor D3D11On12 cache churn fixture")?;
    let bridge =
        D3d11On12ScreenBridge::new(wgpu.device, wgpu.queue).map_err(|error| error.to_string())?;
    for generation in 1_u32..=64 {
        let identity = 100 + u64::from(generation);
        let descriptor = fixture_descriptor(identity, 64 + generation, 36)?;
        let fixture = publish_fixture(
            bridge.adapter_luid(),
            &descriptor,
            fixture_plan_generation(identity)?,
            "fixture:cache-churn",
            u64::from(generation),
        )?;
        let target = bridge
            .prepare_target(fixture.target_preparation())
            .map_err(|error| error.to_string())?;
        drop(target);
        let stats = bridge.cache_stats().map_err(|error| error.to_string())?;
        assert_eq!(stats.prepared_targets, 0);
        assert_eq!(stats.opened_surfaces, 0);
        assert_eq!(stats.retained_target_bytes, 0);
    }
    Ok(())
}

#[test]
fn overlapping_plan_generations_keep_exact_targets_independent() -> Result<(), String> {
    if std::env::var(RUN_FIXTURE_ENV).as_deref() != Ok("1") {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the D3D11On12 fixture");
        return Ok(());
    }

    let wgpu = WgpuFixture::new_dx12("hypercolor D3D11On12 overlap fixture")?;
    let bridge =
        D3d11On12ScreenBridge::new(wgpu.device, wgpu.queue).map_err(|error| error.to_string())?;
    let descriptor = fixture_descriptor(19, 800, 450)?;
    let active_fixture = publish_fixture(
        bridge.adapter_luid(),
        &descriptor,
        fixture_plan_generation(13)?,
        "fixture:overlap",
        1,
    )?;
    let candidate_fixture = publish_fixture(
        bridge.adapter_luid(),
        &descriptor,
        fixture_plan_generation(14)?,
        "fixture:overlap",
        1,
    )?;
    let active = bridge
        .prepare_target(active_fixture.target_preparation())
        .map_err(|error| error.to_string())?;
    let candidate = bridge
        .prepare_target(candidate_fixture.target_preparation())
        .map_err(|error| error.to_string())?;

    assert_ne!(active.storage_id(), candidate.storage_id());
    assert_eq!(
        bridge
            .cache_stats()
            .map_err(|error| error.to_string())?
            .prepared_targets,
        2
    );
    drop(active);
    let stats = bridge.cache_stats().map_err(|error| error.to_string())?;
    assert_eq!(stats.prepared_targets, 1);
    assert_eq!(stats.retained_target_bytes, candidate.retained_bytes());
    Ok(())
}

#[test]
fn cloned_bridges_admit_one_concurrent_exact_target() -> Result<(), String> {
    if std::env::var(RUN_FIXTURE_ENV).as_deref() != Ok("1") {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the D3D11On12 fixture");
        return Ok(());
    }

    let wgpu = WgpuFixture::new_dx12("hypercolor D3D11On12 concurrent fixture")?;
    let bridge =
        D3d11On12ScreenBridge::new(wgpu.device, wgpu.queue).map_err(|error| error.to_string())?;
    let descriptor = fixture_descriptor(20, 512, 288)?;
    let plan = fixture_plan_generation(15)?;
    let fixture = publish_fixture(
        bridge.adapter_luid(),
        &descriptor,
        plan,
        "fixture:concurrent",
        1,
    )?;
    let preparation = fixture.target_preparation();
    let barrier = Arc::new(Barrier::new(2));
    let left_bridge = bridge.clone();
    let left_preparation = preparation;
    let left_barrier = Arc::clone(&barrier);
    let right_bridge = bridge.clone();
    let right_barrier = Arc::clone(&barrier);

    let (left, right) = std::thread::scope(|scope| {
        let left = scope.spawn(move || {
            left_barrier.wait();
            left_bridge.prepare_target(left_preparation)
        });
        let right = scope.spawn(move || {
            right_barrier.wait();
            right_bridge.prepare_target(preparation)
        });
        (
            left.join().expect("left preparation thread must not panic"),
            right
                .join()
                .expect("right preparation thread must not panic"),
        )
    });
    let target = match (left, right) {
        (Ok(target), Err(D3d11On12ScreenInteropError::PreparedTargetAlreadyLive))
        | (Err(D3d11On12ScreenInteropError::PreparedTargetAlreadyLive), Ok(target)) => target,
        (left, right) => {
            return Err(format!(
                "concurrent preparation returned unexpected results: left={left:?}, right={right:?}"
            ));
        }
    };
    assert_eq!(
        bridge
            .cache_stats()
            .map_err(|error| error.to_string())?
            .prepared_targets,
        1,
    );
    drop(target);
    Ok(())
}

#[test]
fn same_plan_and_descriptor_ids_across_sources_keep_independent_targets() -> Result<(), String> {
    if std::env::var(RUN_FIXTURE_ENV).as_deref() != Ok("1") {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the D3D11On12 fixture");
        return Ok(());
    }

    let wgpu = WgpuFixture::new_dx12("hypercolor D3D11On12 source identity fixture")?;
    let bridge =
        D3d11On12ScreenBridge::new(wgpu.device, wgpu.queue).map_err(|error| error.to_string())?;
    let descriptor = fixture_descriptor(21, 64, 36)?;
    let plan = fixture_plan_generation(16)?;
    let left = publish_fixture(bridge.adapter_luid(), &descriptor, plan, "fixture:left", 1)?;
    let right = publish_fixture(bridge.adapter_luid(), &descriptor, plan, "fixture:right", 1)?;

    let left_target = bridge
        .prepare_target(left.target_preparation())
        .map_err(|error| error.to_string())?;
    let right_target = bridge
        .prepare_target(right.target_preparation())
        .map_err(|error| error.to_string())?;

    assert_ne!(left_target.storage_id(), right_target.storage_id());
    assert_eq!(
        bridge
            .cache_stats()
            .map_err(|error| error.to_string())?
            .prepared_targets,
        2,
    );
    Ok(())
}

#[test]
fn bridge_and_prepared_handles_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<D3d11On12ScreenBridge>();
    assert_send_sync::<hypercolor_windows_gpu_interop::PreparedScreenCopyTarget>();
}

fn fixture_plan_generation(value: u64) -> Result<GpuSurfacePlanGeneration, String> {
    let value = NonZeroU64::new(value)
        .ok_or_else(|| "fixture plan generation must be non-zero".to_owned())?;
    Ok(GpuSurfacePlanGeneration::new(value))
}

fn publish_fixture(
    adapter_luid: hypercolor_windows_capture::GpuAdapterLuid,
    descriptor: &GpuSurfaceDescriptor,
    plan_generation: GpuSurfacePlanGeneration,
    source_id: &'static str,
    duplication_generation: u64,
) -> Result<hypercolor_windows_capture::fixtures::GpuSurfaceFixture, String> {
    let extent = descriptor.output_extent();
    let pixel_count = usize::try_from(u64::from(extent.width()) * u64::from(extent.height()))
        .map_err(|_| "fixture pixel count exceeds usize".to_owned())?;
    publish_gpu_surface(GpuSurfaceFixtureConfig {
        adapter_luid,
        plan_generation,
        source_id: Arc::from(source_id),
        topology_generation: 3,
        duplication_generation,
        descriptor: descriptor.clone(),
        bgra: [10, 20, 30, 255].repeat(pixel_count),
        width: extent.width(),
        height: extent.height(),
    })
    .map_err(|error| error.to_string())
}

fn fixture_descriptor(id: u64, width: u32, height: u32) -> Result<GpuSurfaceDescriptor, String> {
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
        freshness: Duration::from_millis(100),
    }))
}
