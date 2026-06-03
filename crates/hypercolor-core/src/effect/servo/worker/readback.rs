use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use hypercolor_types::canvas::Canvas;
use tracing::{trace, warn};

use super::ServoSession;

const MAX_GL_ERROR_BATCH: usize = 8;

static SERVO_READBACK_GL_WARNING_EMITTED: AtomicBool = AtomicBool::new(false);

/// Read Servo's composited framebuffer into a reusable `Canvas` buffer.
///
/// Bypasses `servo-paint-api::Framebuffer::read_framebuffer_to_image`, which
/// allocates, calls `glReadPixels`, clones the whole `Vec<u8>` so it can
/// flip rows into the original via `clone_from_slice`. On a 640x480x4 frame
/// that's two extra full-buffer passes over about 1.2 MB beyond the
/// unavoidable `glReadPixels` DMA. Profile attributed that pair to roughly
/// 45% of the Servo worker thread as raw libc memmove.
///
/// Here we read directly into retired canvas storage when downstream has
/// released it, then swap rows in place. Each byte moves at most once after
/// the GL readback, and the hot path stops allocating a new RGBA buffer every
/// rendered frame. The `bind_vertex_array(0)` call is the OSMesa workaround
/// that Servo's upstream implementation keeps for its own reasons; preserve
/// it so the headless adapter stays honest.
pub(super) fn read_framebuffer_into_canvas(
    session: &mut ServoSession,
    width: i32,
    height: i32,
) -> Result<Canvas> {
    use gleam::gl;

    if width <= 0 || height <= 0 {
        bail!("Servo readback rectangle has non-positive dimensions ({width}x{height})");
    }

    let width_u32 = u32::try_from(width).context("servo readback width overflow")?;
    let height_u32 = u32::try_from(height).context("servo readback height overflow")?;

    session.rendering_context.prepare_for_rendering();
    let gl = session.rendering_context.gleam_gl_api();
    let stale_errors = collect_gl_errors_until_clear(gl::NO_ERROR, || gl.get_error());
    if !stale_errors.is_empty() {
        trace!(
            errors = %format_gl_error_codes(&stale_errors),
            "Cleared stale GL errors before Servo framebuffer readback"
        );
    }
    gl.bind_vertex_array(0);

    let stride = usize::try_from(width)
        .ok()
        .and_then(|w| w.checked_mul(4))
        .context("servo readback row stride overflow")?;
    let expected_len = stride
        .checked_mul(usize::try_from(height).context("servo readback height overflow")?)
        .context("servo readback buffer length overflow")?;

    let mut pixels = session.readback_buffers.take_buffer(expected_len);
    gl.read_pixels_into_buffer(
        0,
        0,
        width,
        height,
        gl::RGBA,
        gl::UNSIGNED_BYTE,
        &mut pixels,
    );
    let gl_errors = collect_gl_errors_until_clear(gl::NO_ERROR, || gl.get_error());
    log_servo_readback_gl_errors(&gl_errors);

    flip_rows_in_place(&mut pixels, stride);

    Ok(Canvas::from_vec(pixels, width_u32, height_u32))
}

fn collect_gl_errors_until_clear(no_error: u32, mut next_error: impl FnMut() -> u32) -> Vec<u32> {
    let mut errors = Vec::new();
    for _ in 0..MAX_GL_ERROR_BATCH {
        let error = next_error();
        if error == no_error {
            break;
        }
        errors.push(error);
    }
    errors
}

fn format_gl_error_codes(errors: &[u32]) -> String {
    errors
        .iter()
        .map(|error| format!("0x{error:x}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn log_servo_readback_gl_errors(errors: &[u32]) {
    if errors.is_empty() {
        return;
    }

    let formatted = format_gl_error_codes(errors);
    if SERVO_READBACK_GL_WARNING_EMITTED.swap(true, Ordering::AcqRel) {
        trace!(
            errors = %formatted,
            "Repeated GL errors during Servo framebuffer readback"
        );
    } else {
        warn!(
            errors = %formatted,
            "GL errors raised during Servo framebuffer readback; suppressing repeated warnings"
        );
    }
}

/// Swap pairs of rows in a row-major RGBA buffer to flip it vertically.
///
/// OpenGL's `glReadPixels` places (0,0) at the bottom-left of the source
/// framebuffer, but `Canvas` expects top-left origin. Walking from both
/// ends with `swap_with_slice` lets each byte move exactly once: no scratch
/// buffer, no per-row clone.
fn flip_rows_in_place(pixels: &mut [u8], stride: usize) {
    if stride == 0 {
        return;
    }
    let row_count = pixels.len() / stride;
    if row_count < 2 {
        return;
    }
    let mut top = 0;
    let mut bottom = row_count - 1;
    while top < bottom {
        let top_start = top * stride;
        let bottom_start = bottom * stride;
        let (upper, lower) = pixels.split_at_mut(bottom_start);
        upper[top_start..top_start + stride].swap_with_slice(&mut lower[..stride]);
        top += 1;
        bottom -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flip_rows_in_place_inverts_row_order() {
        let mut pixels = vec![
            0x11, 0x11, 0x11, 0xff, 0x22, 0x22, 0x22, 0xff, // row 0
            0x33, 0x33, 0x33, 0xff, 0x44, 0x44, 0x44, 0xff, // row 1
            0x55, 0x55, 0x55, 0xff, 0x66, 0x66, 0x66, 0xff, // row 2
        ];
        flip_rows_in_place(&mut pixels, 8);
        assert_eq!(
            pixels,
            vec![
                0x55, 0x55, 0x55, 0xff, 0x66, 0x66, 0x66, 0xff, 0x33, 0x33, 0x33, 0xff, 0x44, 0x44,
                0x44, 0xff, 0x11, 0x11, 0x11, 0xff, 0x22, 0x22, 0x22, 0xff,
            ]
        );
    }

    #[test]
    fn flip_rows_in_place_handles_even_row_count() {
        let mut pixels = vec![
            0xaa, 0xaa, 0xaa, 0xff, // row 0
            0xbb, 0xbb, 0xbb, 0xff, // row 1
            0xcc, 0xcc, 0xcc, 0xff, // row 2
            0xdd, 0xdd, 0xdd, 0xff, // row 3
        ];
        flip_rows_in_place(&mut pixels, 4);
        assert_eq!(
            pixels,
            vec![
                0xdd, 0xdd, 0xdd, 0xff, 0xcc, 0xcc, 0xcc, 0xff, 0xbb, 0xbb, 0xbb, 0xff, 0xaa, 0xaa,
                0xaa, 0xff,
            ]
        );
    }

    #[test]
    fn flip_rows_in_place_is_a_noop_for_degenerate_buffers() {
        let mut single_row = vec![0x01, 0x02, 0x03, 0xff];
        flip_rows_in_place(&mut single_row, 4);
        assert_eq!(single_row, vec![0x01, 0x02, 0x03, 0xff]);

        let mut empty: Vec<u8> = Vec::new();
        flip_rows_in_place(&mut empty, 4);
        assert!(empty.is_empty());

        let mut pixels = vec![0u8; 16];
        flip_rows_in_place(&mut pixels, 0);
        assert_eq!(pixels, vec![0u8; 16]);
    }

    #[test]
    fn collect_gl_errors_stops_after_no_error() {
        let mut errors = vec![0x502, 0x501, 0].into_iter();
        let collected = collect_gl_errors_until_clear(0, || {
            errors
                .next()
                .expect("test iterator should have enough entries")
        });

        assert_eq!(collected, vec![0x502, 0x501]);
    }

    #[test]
    fn collect_gl_errors_caps_the_batch_size() {
        let mut calls = 0usize;
        let collected = collect_gl_errors_until_clear(0, || {
            calls += 1;
            0x502
        });

        assert_eq!(collected.len(), MAX_GL_ERROR_BATCH);
        assert_eq!(calls, MAX_GL_ERROR_BATCH);
    }
}
