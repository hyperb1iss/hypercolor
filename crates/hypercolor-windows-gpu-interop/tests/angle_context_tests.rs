#![cfg(all(target_os = "windows", feature = "servo-context"))]

use std::sync::Arc;

use gleam::gl;
use hypercolor_windows_gpu_interop::{
    ImportedFrameFormat, WindowsAngleRenderingContext, WindowsD3d11SharedTextureImportDescriptor,
    WindowsD3d11SharedTextureImporter, WindowsDxgiAdapterIdentity, WindowsServoFrameOrigin,
};
use paint_api::rendering_context::RenderingContext;

mod support;

use support::{WgpuFixture, assert_uniform_bgra, read_texture_pixels};

const RUN_FIXTURE_ENV: &str = "HYPERCOLOR_RUN_WINDOWS_ANGLE_CONTEXT_FIXTURE";
const WIDTH: u32 = 4;
const HEIGHT: u32 = 3;

#[test]
fn angle_context_renders_into_importable_d3d11_ring() -> Result<(), String> {
    if std::env::var(RUN_FIXTURE_ENV).as_deref() != Ok("1") {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the Windows ANGLE fixture");
        return Ok(());
    }

    let wgpu = WgpuFixture::new("hypercolor Windows ANGLE interop fixture")?;
    let context = WindowsAngleRenderingContext::new(
        WIDTH,
        HEIGHT,
        Some(WindowsDxgiAdapterIdentity {
            vendor_id: wgpu.adapter_info.vendor,
            device_id: wgpu.adapter_info.device,
        }),
    )
    .map_err(|error| error.to_string())?;
    let descriptor = WindowsD3d11SharedTextureImportDescriptor::new(
        WIDTH,
        HEIGHT,
        ImportedFrameFormat::Bgra8Unorm,
    )
    .map_err(|error| error.to_string())?;
    let mut importer = WindowsD3d11SharedTextureImporter::new(&wgpu.device, descriptor)
        .map_err(|error| error.to_string())?;

    // First frame: no present yet, so native_frame eagerly publishes the
    // current render target.
    render_color(&context, [0.25, 0.5, 0.75, 1.0])?;
    let first_native_frame = context.native_frame().map_err(|error| error.to_string())?;
    assert_eq!(first_native_frame.width, WIDTH);
    assert_eq!(first_native_frame.height, HEIGHT);
    assert_eq!(first_native_frame.format, ImportedFrameFormat::Bgra8Unorm);
    assert_eq!(
        first_native_frame.origin,
        WindowsServoFrameOrigin::BottomLeft
    );
    assert_eq!(first_native_frame.content_generation, 1);

    let first_imported = importer
        .import_servo_native_frame(&wgpu.device, first_native_frame)
        .map_err(|error| error.to_string())?;
    assert_eq!(first_imported.storage_id, 1);
    let first_pixels = read_texture_pixels(
        &wgpu.device,
        &wgpu.queue,
        first_imported.texture.as_ref(),
        WIDTH,
        HEIGHT,
    )?;
    assert_uniform_bgra(&first_pixels, [191, 128, 64, 255]);

    // Repeated fetches without a new publish return the same generation.
    let repeat_native_frame = context.native_frame().map_err(|error| error.to_string())?;
    assert_eq!(repeat_native_frame.content_generation, 1);
    assert_eq!(
        repeat_native_frame.slot_index,
        first_native_frame.slot_index
    );

    // Present publishes a new generation into a different ring slot.
    render_color(&context, [0.1, 0.8, 0.3, 1.0])?;
    context.present();
    let second_native_frame = wait_for_generation(&context, 2)?;
    assert_ne!(
        second_native_frame.slot_index,
        first_native_frame.slot_index
    );
    assert_ne!(
        second_native_frame.shared_handle,
        first_native_frame.shared_handle
    );

    let second_imported = importer
        .import_servo_native_frame(&wgpu.device, second_native_frame)
        .map_err(|error| error.to_string())?;
    assert_eq!(second_imported.storage_id, 2);
    let second_pixels = read_texture_pixels(
        &wgpu.device,
        &wgpu.queue,
        second_imported.texture.as_ref(),
        WIDTH,
        HEIGHT,
    )?;
    assert_uniform_bgra(&second_pixels, [77, 204, 26, 255]);

    // A third publish exercises ring rotation and stale-frame detection.
    render_color(&context, [0.9, 0.2, 0.6, 1.0])?;
    context.present();
    let third_native_frame = wait_for_generation(&context, 3)?;
    let third_imported = importer
        .import_servo_native_frame(&wgpu.device, third_native_frame)
        .map_err(|error| error.to_string())?;
    assert_eq!(third_imported.storage_id, 3);
    let third_pixels = read_texture_pixels(
        &wgpu.device,
        &wgpu.queue,
        third_imported.texture.as_ref(),
        WIDTH,
        HEIGHT,
    )?;
    assert_uniform_bgra(&third_pixels, [153, 51, 230, 255]);

    // Re-importing a slot the ring already published reuses the cached wgpu
    // texture while the content version advances.
    render_color(&context, [0.0, 0.0, 0.0, 1.0])?;
    context.present();
    render_color(&context, [1.0, 1.0, 1.0, 1.0])?;
    context.present();
    let wrapped_native_frame = wait_for_generation(&context, 5)?;
    let wrapped_imported = importer
        .import_servo_native_frame(&wgpu.device, wrapped_native_frame)
        .map_err(|error| error.to_string())?;
    assert_eq!(wrapped_imported.storage_id, 5);
    if wrapped_native_frame.shared_handle == first_native_frame.shared_handle {
        assert!(Arc::ptr_eq(
            &wrapped_imported.texture,
            &first_imported.texture
        ));
    }
    let wrapped_pixels = read_texture_pixels(
        &wgpu.device,
        &wgpu.queue,
        wrapped_imported.texture.as_ref(),
        WIDTH,
        HEIGHT,
    )?;
    assert_uniform_bgra(&wrapped_pixels, [255, 255, 255, 255]);

    Ok(())
}

/// Polls `native_frame` until the expected publish generation is ready.
///
/// `native_frame` only hands out slots whose blit fences have signaled, so a
/// fresh publish can briefly report the previous generation.
fn wait_for_generation(
    context: &WindowsAngleRenderingContext,
    expected_generation: u64,
) -> Result<hypercolor_windows_gpu_interop::WindowsServoNativeFrame, String> {
    let mut frame = context.native_frame().map_err(|error| error.to_string())?;
    for _ in 0..200 {
        if frame.content_generation == expected_generation {
            return Ok(frame);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
        frame = context.native_frame().map_err(|error| error.to_string())?;
    }
    Err(format!(
        "generation {expected_generation} never became ready (latest {})",
        frame.content_generation
    ))
}

fn render_color(context: &WindowsAngleRenderingContext, color: [f32; 4]) -> Result<(), String> {
    context
        .make_current()
        .map_err(|error| format!("make current failed: {error:?}"))?;
    context.prepare_for_rendering();
    let gl = context.gleam_gl_api();
    gl.viewport(0, 0, WIDTH as i32, HEIGHT as i32);
    gl.clear_color(color[0], color[1], color[2], color[3]);
    gl.clear(gl::COLOR_BUFFER_BIT);
    Ok(())
}
