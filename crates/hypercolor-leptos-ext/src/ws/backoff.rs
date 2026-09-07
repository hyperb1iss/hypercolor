use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Jitter {
    None,
    Equal(f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExponentialBackoff {
    pub base: Duration,
    pub max: Duration,
    pub multiplier: f64,
    pub jitter: Jitter,
}

impl ExponentialBackoff {
    pub const HYPERCOLOR_DEFAULT: Self = Self {
        base: Duration::from_millis(500),
        max: Duration::from_secs(15),
        multiplier: 2.0,
        jitter: Jitter::Equal(0.25),
    };

    /// Delay before `attempt`, with `sample` picking a point in the jitter
    /// window: 0.0 is its floor, 0.5 the unjittered delay, 1.0 its ceiling.
    ///
    /// Returns `None` when the schedule cannot produce a delay at all, which
    /// callers read as "fall back to your own floor" rather than "stop trying".
    pub fn delay_for_attempt_with_sample(&self, attempt: u32, sample: f64) -> Option<Duration> {
        if self.base.is_zero() || self.max.is_zero() || self.multiplier <= 0.0 {
            return None;
        }

        let exponent = i32::try_from(attempt).unwrap_or(i32::MAX);
        let scaled_secs = self.base.as_secs_f64() * self.multiplier.powi(exponent);
        let capped_secs = scaled_secs.min(self.max.as_secs_f64());
        let jittered_secs = match self.jitter {
            Jitter::None => capped_secs,
            Jitter::Equal(ratio) => {
                let clamped_ratio = ratio.clamp(0.0, 1.0);
                let clamped_sample = sample.clamp(0.0, 1.0);
                let spread = capped_secs * clamped_ratio;
                let min_secs = (capped_secs - spread).max(0.0);
                let max_secs = capped_secs + spread;

                min_secs + ((max_secs - min_secs) * clamped_sample)
            }
        };

        Some(Duration::from_secs_f64(jittered_secs))
    }
}
