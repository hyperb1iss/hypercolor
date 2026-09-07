//! sRGB transfer functions (IEC 61966-2-1) — scalar forms and the
//! precomputed lookup tables used by pixel hot paths.

use std::sync::LazyLock;

/// Convert one sRGB gamma-encoded channel to linear light.
///
/// For u8-indexed hot paths prefer [`lut::srgb_u8_to_linear`], which reads
/// from a precomputed table.
#[must_use]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Convert one linear-light channel to sRGB gamma-encoded.
///
/// For byte-output hot paths prefer [`lut::linear_to_srgb_u8`], which reads
/// from a precomputed table.
#[must_use]
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Precomputed transfer-function tables.
///
/// The byte→linear table has one entry per byte. The linear→byte table
/// uses 4096 bins (12-bit quantization) — empirically the smallest size at
/// which `srgb_u8_to_linear` → `linear_to_srgb_u8` round-trips every byte
/// exactly.
pub mod lut {
    use super::{LazyLock, linear_to_srgb, srgb_to_linear};

    const LINEAR_TO_SRGB_U8_LUT_SIZE: usize = 4096;

    static SRGB_TO_LINEAR_LUT: LazyLock<[f32; 256]> = LazyLock::new(|| {
        let mut table = [0.0_f32; 256];
        for (index, entry) in table.iter_mut().enumerate() {
            *entry = srgb_to_linear(index as f32 / 255.0);
        }
        table
    });

    static LINEAR_TO_SRGB_U8_LUT: LazyLock<[u8; LINEAR_TO_SRGB_U8_LUT_SIZE]> =
        LazyLock::new(|| {
            let mut table = [0_u8; LINEAR_TO_SRGB_U8_LUT_SIZE];
            let max_index_f = (LINEAR_TO_SRGB_U8_LUT_SIZE - 1) as f32;
            for (index, entry) in table.iter_mut().enumerate() {
                let linear = index as f32 / max_index_f;
                *entry = (linear_to_srgb(linear) * 255.0).round().clamp(0.0, 255.0) as u8;
            }
            table
        });

    /// Convert one stored sRGB byte to linear-light float via the
    /// 256-entry table. Constant-time in pixel hot paths.
    #[inline]
    #[must_use]
    pub fn srgb_u8_to_linear(c: u8) -> f32 {
        SRGB_TO_LINEAR_LUT[usize::from(c)]
    }

    /// Convert one linear-light float to an sRGB byte via the 4096-entry
    /// table. The input is clamped to `[0, 1]` and NaN maps to 0,
    /// matching the scalar round-clamp semantics.
    #[inline]
    #[must_use]
    pub fn linear_to_srgb_u8(c: f32) -> u8 {
        let clamped = if c.is_nan() { 0.0 } else { c.clamp(0.0, 1.0) };
        let max_index_f = (LINEAR_TO_SRGB_U8_LUT_SIZE - 1) as f32;
        let index = (clamped * max_index_f).round() as usize;
        // `clamped` is in [0, 1] so the scaled index cannot exceed the
        // table; the `.min` guards f32 rounding landing exactly at the
        // length.
        LINEAR_TO_SRGB_U8_LUT[index.min(LINEAR_TO_SRGB_U8_LUT_SIZE - 1)]
    }
}
