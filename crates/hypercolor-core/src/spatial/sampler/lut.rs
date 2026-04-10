//! sRGB-to-linear and linear-to-sRGB lookup tables.
//!
//! Built once on first access via `LazyLock`. All pixel sampling flows through
//! these tables so that interpolation happens in linear light, avoiding
//! perceptual banding artifacts.

use std::sync::LazyLock;

use hypercolor_types::canvas::{linear_to_srgb, srgb_to_linear};

pub(crate) const BILINEAR_ONE: u32 = 256;
pub(crate) const BILINEAR_SHIFT: u32 = 16;
pub(crate) const ATTENUATION_ONE: u16 = 256;

static SRGB_TO_LINEAR_LUT: LazyLock<[u16; 256]> = LazyLock::new(build_srgb_to_linear_lut);
static LINEAR_TO_SRGB_LUT: LazyLock<Vec<u8>> = LazyLock::new(build_linear_to_srgb_lut);

#[must_use]
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "the LUT is built once and clamps each decoded channel into the byte range"
)]
fn build_srgb_to_linear_lut() -> [u16; 256] {
    let mut table = [0_u16; 256];
    for (index, value) in table.iter_mut().enumerate() {
        let srgb = index as f32 / 255.0;
        *value = (srgb_to_linear(srgb) * 65535.0).round().clamp(0.0, 65535.0) as u16;
    }
    table
}

#[must_use]
pub(super) fn decode_srgb_byte(channel: u8) -> u16 {
    SRGB_TO_LINEAR_LUT[usize::from(channel)]
}

#[must_use]
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "the LUT is built once and clamps each encoded channel into the byte range"
)]
fn build_linear_to_srgb_lut() -> Vec<u8> {
    let mut table = Vec::with_capacity(usize::from(u16::MAX) + 1);
    for index in 0..=u16::MAX {
        let linear = f32::from(index) / 65535.0;
        table.push((linear_to_srgb(linear) * 255.0).round().clamp(0.0, 255.0) as u8);
    }
    table
}

#[must_use]
pub(super) fn encode_linear_byte(channel: u16) -> u8 {
    LINEAR_TO_SRGB_LUT[usize::from(channel)]
}
