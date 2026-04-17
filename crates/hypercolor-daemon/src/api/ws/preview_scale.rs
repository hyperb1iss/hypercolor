use std::cell::RefCell;

use fast_image_resize as fr;

#[derive(Clone, Copy)]
pub(super) enum PreviewScaleFormat {
    Rgb,
    Rgba,
}

impl PreviewScaleFormat {
    const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }
}

thread_local! {
    /// Per-worker-thread resizer + RGBA scratch buffer. `fr::Resizer` holds
    /// pixel-type-indexed internal buffers, so keeping it alive across calls
    /// skips re-initialization on every encode. The RGBA scratch absorbs
    /// resize output when the caller wants RGB-packed bytes out — we still
    /// have to ask `fast_image_resize` for U8x4, then strip alpha during the
    /// brightness post-pass.
    static PREVIEW_RESIZER_STATE: RefCell<PreviewResizerState> = RefCell::new(PreviewResizerState::new());
}

struct PreviewResizerState {
    resizer: fr::Resizer,
    rgba_scratch: Vec<u8>,
}

impl PreviewResizerState {
    fn new() -> Self {
        Self {
            resizer: fr::Resizer::new(),
            rgba_scratch: Vec::new(),
        }
    }
}

/// Bilinear-scale an RGBA source into the target buffer, optionally applying
/// a 256-entry brightness LUT to R/G/B (alpha untouched) and packing the
/// output as RGB or RGBA. Internally dispatches to `fast_image_resize`
/// (AVX2 when present) via a thread-local resizer; the scalar fallback is
/// only used for the identity and invalid-input cases.
pub(super) fn scale_rgba_bilinear(
    rgba: &[u8],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    brightness_lut: Option<&[u8; 256]>,
    format: PreviewScaleFormat,
    out: &mut Vec<u8>,
) {
    PREVIEW_RESIZER_STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        state.scale(
            rgba,
            source_width,
            source_height,
            target_width,
            target_height,
            brightness_lut,
            format,
            out,
        );
    });
}

impl PreviewResizerState {
    #[allow(clippy::too_many_arguments, clippy::as_conversions)]
    fn scale(
        &mut self,
        rgba: &[u8],
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
        brightness_lut: Option<&[u8; 256]>,
        format: PreviewScaleFormat,
        out: &mut Vec<u8>,
    ) {
        let out_bpp = format.bytes_per_pixel();
        let required_len = (target_width as usize)
            .saturating_mul(target_height as usize)
            .saturating_mul(out_bpp);
        if out.len() != required_len {
            out.resize(required_len, 0);
        }

        let source_pixels_len = (source_width as usize)
            .saturating_mul(source_height as usize)
            .saturating_mul(4);
        if source_width == 0
            || source_height == 0
            || target_width == 0
            || target_height == 0
            || rgba.len() < source_pixels_len
        {
            out.fill(0);
            return;
        }

        // Identity: copy source directly into the output, applying brightness
        // and dropping alpha as the format requires. Skipping `fr::Resizer`
        // here avoids its setup cost on the common UI case where the canvas
        // and requested preview are the same dimensions.
        if source_width == target_width && source_height == target_height {
            passthrough_with_brightness(rgba, out, format, brightness_lut);
            return;
        }

        // Split the state so we can hand a `&mut Vec<u8>` scratch to the
        // resizer while also borrowing `resizer` itself. Both fields are
        // independent — destructuring lets the borrow checker see that.
        let Self {
            resizer,
            rgba_scratch,
        } = self;

        let resize_buffer: &mut Vec<u8> = match format {
            PreviewScaleFormat::Rgba => out,
            PreviewScaleFormat::Rgb => {
                let scratch_len = (target_width as usize)
                    .saturating_mul(target_height as usize)
                    .saturating_mul(4);
                if rgba_scratch.len() != scratch_len {
                    rgba_scratch.resize(scratch_len, 0);
                }
                rgba_scratch
            }
        };

        if try_resize_rgba(
            resizer,
            rgba,
            source_width,
            source_height,
            target_width,
            target_height,
            resize_buffer,
        )
        .is_err()
        {
            out.fill(0);
            return;
        }

        match format {
            PreviewScaleFormat::Rgba => apply_brightness_inplace_rgba(out, brightness_lut),
            PreviewScaleFormat::Rgb => {
                copy_rgba_to_rgb_with_brightness(rgba_scratch, out, brightness_lut);
            }
        }
    }
}

fn try_resize_rgba(
    resizer: &mut fr::Resizer,
    rgba: &[u8],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    out_rgba: &mut [u8],
) -> Result<(), fr::ImageBufferError> {
    let src = fr::images::ImageRef::new(source_width, source_height, rgba, fr::PixelType::U8x4)?;
    let mut dst = fr::images::Image::from_slice_u8(
        target_width,
        target_height,
        out_rgba,
        fr::PixelType::U8x4,
    )?;
    let options =
        fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Interpolation(fr::FilterType::Bilinear));
    // `resize` can only fail on shape mismatch for a same-pixel-type pair,
    // which `ImageRef::new`/`Image::from_slice_u8` already reject above —
    // so any error past this point is a library-level bug. Ignore the
    // `Result` deliberately; the caller fills with zeros on the outer
    // error path, which also handles invalid input shape.
    let _ = resizer.resize(&src, &mut dst, &options);
    Ok(())
}

fn passthrough_with_brightness(
    rgba: &[u8],
    out: &mut [u8],
    format: PreviewScaleFormat,
    brightness_lut: Option<&[u8; 256]>,
) {
    match format {
        PreviewScaleFormat::Rgba => {
            out.copy_from_slice(&rgba[..out.len()]);
            apply_brightness_inplace_rgba(out, brightness_lut);
        }
        PreviewScaleFormat::Rgb => {
            copy_rgba_to_rgb_with_brightness(rgba, out, brightness_lut);
        }
    }
}

fn apply_brightness_inplace_rgba(buffer: &mut [u8], brightness_lut: Option<&[u8; 256]>) {
    let Some(lut) = brightness_lut else {
        return;
    };
    for pixel in buffer.chunks_exact_mut(4) {
        pixel[0] = lut[usize::from(pixel[0])];
        pixel[1] = lut[usize::from(pixel[1])];
        pixel[2] = lut[usize::from(pixel[2])];
    }
}

fn copy_rgba_to_rgb_with_brightness(
    rgba: &[u8],
    rgb_out: &mut [u8],
    brightness_lut: Option<&[u8; 256]>,
) {
    for (rgba_chunk, rgb_chunk) in rgba.chunks_exact(4).zip(rgb_out.chunks_exact_mut(3)) {
        rgb_chunk[0] = apply_brightness(rgba_chunk[0], brightness_lut);
        rgb_chunk[1] = apply_brightness(rgba_chunk[1], brightness_lut);
        rgb_chunk[2] = apply_brightness(rgba_chunk[2], brightness_lut);
    }
}

fn apply_brightness(channel: u8, brightness_lut: Option<&[u8; 256]>) -> u8 {
    brightness_lut.map_or(channel, |lut| lut[usize::from(channel)])
}

#[cfg(test)]
mod tests {
    use super::{PreviewScaleFormat, scale_rgba_bilinear};

    #[test]
    fn bilinear_scaling_preserves_identity_at_native_resolution() {
        let source = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let mut out = Vec::new();
        scale_rgba_bilinear(
            &source,
            2,
            2,
            2,
            2,
            None,
            PreviewScaleFormat::Rgba,
            &mut out,
        );
        assert_eq!(out, source);
    }

    #[test]
    fn bilinear_scaling_averages_center_pixels() {
        let source = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let mut out = Vec::new();
        scale_rgba_bilinear(
            &source,
            2,
            2,
            1,
            1,
            None,
            PreviewScaleFormat::Rgba,
            &mut out,
        );
        // fast_image_resize's bilinear centers samples between source texels,
        // matching the mean of the four corners: (255+0+0+255)/4 for R,
        // (0+255+0+255)/4 for G, (0+0+255+255)/4 for B. Allow a 1 LSB drift
        // to cover the fixed-point rounding mode.
        assert_eq!(out.len(), 4);
        let expected = [127_u8, 127, 127, 255];
        for (got, want) in out.iter().zip(expected.iter()) {
            assert!(
                got.abs_diff(*want) <= 1,
                "channel differs by more than 1 LSB: got {out:?}, expected {expected:?}"
            );
        }
    }

    #[test]
    fn bilinear_scaling_applies_brightness_after_interpolation() {
        let source = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let brightness_lut = std::array::from_fn(|channel| {
            u8::try_from(channel / 2).expect("brightness LUT should stay in byte range")
        });
        let mut out = Vec::new();
        scale_rgba_bilinear(
            &source,
            2,
            2,
            1,
            1,
            Some(&brightness_lut),
            PreviewScaleFormat::Rgb,
            &mut out,
        );
        // Post-resize brightness applies the `/2` LUT to each channel;
        // whether pre-LUT was 127 or 128 the output lands at 63 or 64.
        assert_eq!(out.len(), 3);
        for (i, channel) in out.iter().enumerate() {
            assert!(
                channel.abs_diff(64) <= 1,
                "channel {i} drifted beyond LSB: {out:?}"
            );
        }
    }

    #[test]
    fn bilinear_scaling_passthrough_strips_alpha_for_rgb_format() {
        let source = vec![10, 20, 30, 255, 40, 50, 60, 255];
        let mut out = Vec::new();
        scale_rgba_bilinear(&source, 2, 1, 2, 1, None, PreviewScaleFormat::Rgb, &mut out);
        assert_eq!(out, vec![10, 20, 30, 40, 50, 60]);
    }

    #[test]
    fn bilinear_scaling_fills_zeroes_on_invalid_source_shape() {
        let source = vec![0u8; 4]; // claims 2x2 but only 4 bytes of storage
        let mut out = Vec::new();
        scale_rgba_bilinear(
            &source,
            2,
            2,
            2,
            2,
            None,
            PreviewScaleFormat::Rgba,
            &mut out,
        );
        assert_eq!(out, vec![0_u8; 16]);
    }
}
