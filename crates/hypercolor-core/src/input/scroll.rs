//! Exact pointer-scroll arithmetic shared by every host producer.

/// Scale factor for signed Q16.16 scroll values.
pub const Q16_16_SCALE: i64 = 1 << 16;

/// Convert a signed Q16.16 value to its floating representation.
#[must_use]
pub fn q16_16_to_f64(value: i64) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        clippy::as_conversions,
        reason = "effect payloads expose Q16.16 values as JavaScript numbers"
    )]
    {
        value as f64 / Q16_16_SCALE as f64
    }
}
