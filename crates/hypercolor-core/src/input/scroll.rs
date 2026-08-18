//! Exact pointer-scroll arithmetic shared by every host producer.

/// Scale factor for signed Q16.16 scroll values.
pub const Q16_16_SCALE: i64 = 1 << 16;

/// Per-source projector from exact line scroll to the legacy integral wheel signal.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LegacyWheelProjector {
    remainder_q16_16: i64,
}

impl LegacyWheelProjector {
    /// Project vertical `Line120` motion while retaining signed fractions.
    #[must_use]
    pub fn project(&mut self, delta_y_q16_16: i64) -> i32 {
        let total = i128::from(self.remainder_q16_16) + i128::from(delta_y_q16_16);
        let integral = total / i128::from(Q16_16_SCALE);
        let remainder = total % i128::from(Q16_16_SCALE);
        self.remainder_q16_16 =
            i64::try_from(remainder).expect("a Q16.16 remainder always fits in i64");
        i32::try_from(integral).unwrap_or_else(|_| {
            if integral.is_negative() {
                i32::MIN
            } else {
                i32::MAX
            }
        })
    }

    /// Signed fractional motion retained for the next event.
    #[must_use]
    pub const fn remainder_q16_16(self) -> i64 {
        self.remainder_q16_16
    }

    /// Clear fractional state after a source gap or generation change.
    pub fn reset(&mut self) {
        self.remainder_q16_16 = 0;
    }
}

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
