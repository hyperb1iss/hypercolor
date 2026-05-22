#![cfg(target_os = "windows")]

use std::sync::Arc;

use hypercolor_windows_gpu_interop::{
    ImportedFrameFormat, WindowsD3d11Device, WindowsD3d11SharedTextureImportDescriptor,
    WindowsD3d11SharedTextureImporter,
};

mod support;

use support::{WgpuFixture, patterned_bgra_pixels, read_texture_pixels};

const RUN_FIXTURE_ENV: &str = "HYPERCOLOR_RUN_WINDOWS_D3D11_FIXTURE";
const WIDTH: u32 = 4;
const HEIGHT: u32 = 3;

#[test]
fn imports_synthetic_d3d11_shared_texture_into_wgpu_texture() -> Result<(), String> {
    if std::env::var(RUN_FIXTURE_ENV).as_deref() != Ok("1") {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the D3D11 shared-texture fixture");
        return Ok(());
    }

    let wgpu = WgpuFixture::new("hypercolor Windows D3D11 interop fixture")?;
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
    let expected_pixels = patterned_bgra_pixels(WIDTH, HEIGHT);
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

    // SAFETY: the texture and producer synchronization remain valid.
    let cached_frame =
        unsafe { importer.import_shared_handle(&wgpu.device, texture.shared_handle(), 123) }
            .map_err(|error| error.to_string())?;
    assert_eq!(cached_frame.storage_id, frame.storage_id);
    assert!(Arc::ptr_eq(&cached_frame.texture, &frame.texture));
    assert!(Arc::ptr_eq(&cached_frame.view, &frame.view));
    assert_eq!(cached_frame.timings.wrap_us, 0);
    assert_eq!(cached_frame.timings.sync_us, 123);

    Ok(())
}
