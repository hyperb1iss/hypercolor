#![cfg(target_os = "macos")]

use dpi::PhysicalSize;
use gleam::gl;
use hypercolor_macos_gpu_interop::{
    ImportedFrameFormat, MacosHardwareRenderingContext, MacosServoFrameOrigin,
};
use objc2_io_surface::{IOSurfaceLockOptions, IOSurfaceRef};
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
    context.present();

    let native_frame = context.native_frame().map_err(|error| error.to_string())?;
    assert_eq!(native_frame.width, WIDTH);
    assert_eq!(native_frame.height, HEIGHT);
    assert_eq!(native_frame.format, ImportedFrameFormat::Bgra8Unorm);
    assert_eq!(native_frame.origin, MacosServoFrameOrigin::BottomLeft);
    assert_ne!(native_frame.surface_id, 0);
    assert_ne!(native_frame.content_generation, 0);
    assert_eq!(native_frame.iosurface.width(), WIDTH as usize);
    assert_eq!(native_frame.iosurface.height(), HEIGHT as usize);
    assert_iosurface_uniform_bgra(&native_frame.iosurface, WIDTH, HEIGHT, [191, 128, 64, 255])?;

    let same_surface = context.native_frame().map_err(|error| error.to_string())?;
    assert_eq!(same_surface.surface_id, native_frame.surface_id);
    assert_eq!(
        same_surface.content_generation,
        native_frame.content_generation
    );
    assert_eq!(
        same_surface.iosurface.pixel_format(),
        native_frame.iosurface.pixel_format()
    );

    let image = context
        .read_to_image(DeviceIntRect::from_origin_and_size(
            DeviceIntPoint::new(0, 0),
            DeviceIntSize::new(WIDTH as i32, HEIGHT as i32),
        ))
        .ok_or_else(|| "hardware context readback returned no image".to_owned())?;
    assert_eq!(image.width(), WIDTH);
    assert_eq!(image.height(), HEIGHT);
    assert_uniform_rgba(image.as_raw(), [64, 128, 191, 255]);

    context.resize(PhysicalSize::new(WIDTH + 1, HEIGHT + 1));
    let resized_frame = context.native_frame().map_err(|error| error.to_string())?;
    assert_eq!(resized_frame.width, WIDTH + 1);
    assert_eq!(resized_frame.height, HEIGHT + 1);
    assert_ne!(resized_frame.surface_id, native_frame.surface_id);
    assert!(
        resized_frame.content_generation > same_surface.content_generation,
        "content generation must continue monotonically across a ring rebuild"
    );

    Ok(())
}

#[test]
fn iosurface_ring_rotates_with_monotonic_generations() -> Result<(), String> {
    let context =
        MacosHardwareRenderingContext::new(WIDTH, HEIGHT).map_err(|error| error.to_string())?;
    context
        .make_current()
        .map_err(|error| format!("make current failed: {error:?}"))?;
    let gl = context.gleam_gl_api();

    let mut surface_ids = Vec::new();
    let mut generations = Vec::new();
    for cycle in 0..3u32 {
        context.prepare_for_rendering();
        gl.viewport(0, 0, WIDTH as i32, HEIGHT as i32);
        let level = (cycle + 1) as f32 / 4.0;
        gl.clear_color(level, level, level, 1.0);
        gl.clear(gl::COLOR_BUFFER_BIT);
        context.present();
        // Test-only sync so the freshest publish fence is signaled before
        // native_frame polls the ring.
        gl.finish();
        let frame = context.native_frame().map_err(|error| error.to_string())?;
        surface_ids.push(frame.surface_id);
        generations.push(frame.content_generation);
    }

    let distinct_surfaces: std::collections::HashSet<usize> = surface_ids.iter().copied().collect();
    assert!(
        distinct_surfaces.len() >= 2,
        "ring must rotate across publishes, got surface ids {surface_ids:?}"
    );
    assert!(
        generations.windows(2).all(|pair| pair[1] > pair[0]),
        "content generations must strictly increase, got {generations:?}"
    );

    Ok(())
}

#[test]
fn native_frame_without_new_publish_returns_same_generation() -> Result<(), String> {
    let context =
        MacosHardwareRenderingContext::new(WIDTH, HEIGHT).map_err(|error| error.to_string())?;
    context
        .make_current()
        .map_err(|error| format!("make current failed: {error:?}"))?;
    context.prepare_for_rendering();
    let gl = context.gleam_gl_api();
    gl.viewport(0, 0, WIDTH as i32, HEIGHT as i32);
    gl.clear_color(0.5, 0.25, 0.125, 1.0);
    gl.clear(gl::COLOR_BUFFER_BIT);
    context.present();

    let first = context.native_frame().map_err(|error| error.to_string())?;
    let second = context.native_frame().map_err(|error| error.to_string())?;
    assert_eq!(second.surface_id, first.surface_id);
    assert_eq!(second.content_generation, first.content_generation);

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

fn assert_iosurface_uniform_bgra(
    iosurface: &IOSurfaceRef,
    width: u32,
    height: u32,
    expected: [u8; 4],
) -> Result<(), String> {
    let _lock = IosurfaceReadLock::lock(iosurface)?;
    let bytes_per_row = iosurface.bytes_per_row();
    let row_len = width as usize * 4;
    let base_address = iosurface.base_address().as_ptr().cast::<u8>();
    for row in 0..height as usize {
        // SAFETY: the IOSurface is locked read-only, and row_len is bounded
        // by the surface dimensions verified by the caller.
        let row_bytes =
            unsafe { std::slice::from_raw_parts(base_address.add(row * bytes_per_row), row_len) };
        for pixel in row_bytes.chunks_exact(4) {
            assert!(
                pixel
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| actual.abs_diff(expected) <= 1),
                "IOSurface pixel {pixel:?} did not match {expected:?}"
            );
        }
    }
    Ok(())
}

struct IosurfaceReadLock<'a> {
    iosurface: &'a IOSurfaceRef,
}

impl<'a> IosurfaceReadLock<'a> {
    fn lock(iosurface: &'a IOSurfaceRef) -> Result<Self, String> {
        // SAFETY: null seed is allowed by IOSurfaceLock.
        let code = unsafe { iosurface.lock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut()) };
        if code == 0 {
            Ok(Self { iosurface })
        } else {
            Err(format!("IOSurface read lock failed with {code}"))
        }
    }
}

impl Drop for IosurfaceReadLock<'_> {
    fn drop(&mut self) {
        // SAFETY: null seed is allowed by IOSurfaceUnlock.
        let _ = unsafe {
            self.iosurface
                .unlock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut())
        };
    }
}
