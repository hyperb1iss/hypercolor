#![cfg(all(target_os = "windows", feature = "servo-context"))]

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

    render_color(&context, [0.25, 0.5, 0.75, 1.0])?;
    let first_native_frame = context
        .publish_current_frame()
        .map_err(|error| error.to_string())?;
    assert!(first_native_frame.is_none());

    render_color(&context, [0.1, 0.8, 0.3, 1.0])?;
    let first_native_frame = context
        .publish_current_frame()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "first frame was not ready after one ring rotation".to_owned())?;
    assert_eq!(first_native_frame.width, WIDTH);
    assert_eq!(first_native_frame.height, HEIGHT);
    assert_eq!(first_native_frame.format, ImportedFrameFormat::Bgra8Unorm);
    assert_eq!(
        first_native_frame.origin,
        WindowsServoFrameOrigin::BottomLeft
    );
    assert_eq!(first_native_frame.slot_index, 0);

    let first_imported = importer
        .import_servo_native_frame(&wgpu.device, first_native_frame)
        .map_err(|error| error.to_string())?;
    let first_pixels = read_texture_pixels(
        &wgpu.device,
        &wgpu.queue,
        first_imported.texture.as_ref(),
        WIDTH,
        HEIGHT,
    )?;
    assert_uniform_bgra(&first_pixels, [191, 128, 64, 255]);

    render_color(&context, [0.9, 0.2, 0.6, 1.0])?;
    let second_native_frame = context
        .publish_current_frame()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "second frame was not ready after one ring rotation".to_owned())?;
    assert_eq!(second_native_frame.slot_index, 1);
    assert_ne!(
        second_native_frame.shared_handle,
        first_native_frame.shared_handle
    );

    let second_imported = importer
        .import_servo_native_frame(&wgpu.device, second_native_frame)
        .map_err(|error| error.to_string())?;
    assert_ne!(second_imported.storage_id, first_imported.storage_id);
    let second_pixels = read_texture_pixels(
        &wgpu.device,
        &wgpu.queue,
        second_imported.texture.as_ref(),
        WIDTH,
        HEIGHT,
    )?;
    assert_uniform_bgra(&second_pixels, [77, 204, 26, 255]);

    Ok(())
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
