#![cfg(target_os = "macos")]

use gleam::gl;
use hypercolor_macos_gpu_interop::{
    ImportedFrameFormat, MacosHardwareRenderingContext, MacosServoFrameOrigin,
};
use paint_api::rendering_context::RenderingContext;
use webrender_api::units::{DeviceIntPoint, DeviceIntRect, DeviceIntSize};

const WIDTH: u32 = 4;
const HEIGHT: u32 = 3;

#[test]
fn hardware_context_reads_back_and_exposes_iosurface() -> Result<(), String> {
    let context =
        MacosHardwareRenderingContext::new(WIDTH, HEIGHT).map_err(|error| error.to_string())?;
    context
        .make_current()
        .map_err(|error| format!("make current failed: {error:?}"))?;
    context.prepare_for_rendering();
    let gl = context.gleam_gl_api();
    gl.viewport(0, 0, WIDTH as i32, HEIGHT as i32);
    gl.clear_color(0.25, 0.5, 0.75, 1.0);
    gl.clear(gl::COLOR_BUFFER_BIT);

    let native_frame = context.native_frame().map_err(|error| error.to_string())?;
    assert_eq!(native_frame.width, WIDTH);
    assert_eq!(native_frame.height, HEIGHT);
    assert_eq!(native_frame.format, ImportedFrameFormat::Bgra8Unorm);
    assert_eq!(native_frame.origin, MacosServoFrameOrigin::BottomLeft);
    assert_ne!(native_frame.surface_id, 0);
    assert_eq!(native_frame.iosurface.width(), WIDTH as usize);
    assert_eq!(native_frame.iosurface.height(), HEIGHT as usize);

    let image = context
        .read_to_image(DeviceIntRect::from_origin_and_size(
            DeviceIntPoint::new(0, 0),
            DeviceIntSize::new(WIDTH as i32, HEIGHT as i32),
        ))
        .ok_or_else(|| "hardware context readback returned no image".to_owned())?;
    assert_eq!(image.width(), WIDTH);
    assert_eq!(image.height(), HEIGHT);
    assert_uniform_rgba(image.as_raw(), [64, 128, 191, 255]);

    Ok(())
}

fn assert_uniform_rgba(pixels: &[u8], expected: [u8; 4]) {
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
