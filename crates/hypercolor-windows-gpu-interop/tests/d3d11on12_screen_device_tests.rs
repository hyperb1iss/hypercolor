#![cfg(all(target_os = "windows", feature = "screen-capture"))]
#![allow(
    dead_code,
    reason = "shared fixture support is compiled independently by each integration test"
)]

use hypercolor_windows_capture::{
    CaptureExtent, CaptureRegion, DisplayRotation, GPU_SURFACE_ALGORITHM_REVISION,
    GpuSurfaceColorPipeline, GpuSurfaceCoordinateSpace, GpuSurfaceCursorPolicy,
    GpuSurfaceDescriptor, GpuSurfaceDescriptorConfig, GpuSurfaceDescriptorId, GpuSurfaceFilter,
    GpuSurfaceFormat, GpuSurfaceSourceColorSpace,
};
use hypercolor_windows_gpu_interop::D3d11On12ScreenBridge;
use hypercolor_windows_gpu_interop::D3d11On12ScreenDevice;
use std::num::NonZeroU64;
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
fn prepares_and_reuses_exact_renderer_target() -> Result<(), String> {
    if std::env::var(RUN_FIXTURE_ENV).as_deref() != Ok("1") {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the D3D11On12 fixture");
        return Ok(());
    }

    let wgpu = WgpuFixture::new_dx12("hypercolor D3D11On12 target fixture")?;
    let mut bridge =
        D3d11On12ScreenBridge::new(wgpu.device, wgpu.queue).map_err(|error| error.to_string())?;
    let descriptor = fixture_descriptor(17, 641, 359)?;

    let first = bridge
        .prepare_target(&descriptor)
        .map_err(|error| error.to_string())?;
    let reused = bridge
        .prepare_target(&descriptor)
        .map_err(|error| error.to_string())?;

    assert_eq!(first, reused);
    assert_eq!(first.width, 641);
    assert_eq!(first.height, 359);
    assert_eq!(first.retained_bytes, 641 * 359 * 4);
    Ok(())
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
