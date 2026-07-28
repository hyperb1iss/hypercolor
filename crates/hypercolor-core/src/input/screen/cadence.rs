use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use thiserror::Error;

const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// Highest cadence whose every frame interval is representable by [`Duration`].
pub const MAX_REPRESENTABLE_CAPTURE_FPS: u32 = NANOS_PER_SECOND as u32;

/// A capture cadence admitted against the scheduler clock's resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureCadence {
    frames_per_second: NonZeroU32,
}

impl CaptureCadence {
    /// Admit a capture cadence without imposing a product-specific ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureCadenceError::Zero`] for zero and
    /// [`CaptureCadenceError::ClockResolutionExceeded`] when a positive frame
    /// interval cannot be represented by [`Duration`].
    pub const fn new(frames_per_second: u32) -> Result<Self, CaptureCadenceError> {
        let Some(frames_per_second) = NonZeroU32::new(frames_per_second) else {
            return Err(CaptureCadenceError::Zero);
        };
        if frames_per_second.get() > MAX_REPRESENTABLE_CAPTURE_FPS {
            return Err(CaptureCadenceError::ClockResolutionExceeded {
                requested_fps: frames_per_second.get(),
                maximum_fps: MAX_REPRESENTABLE_CAPTURE_FPS,
            });
        }
        Ok(Self { frames_per_second })
    }

    /// Requested frames per second.
    #[must_use]
    pub const fn frames_per_second(self) -> u32 {
        self.frames_per_second.get()
    }

    /// Smallest duration that covers `intervals` requested frame intervals.
    #[must_use]
    pub const fn interval_window(self, intervals: NonZeroU32) -> Duration {
        let numerator = NANOS_PER_SECOND * intervals.get() as u64;
        let denominator = self.frames_per_second.get() as u64;
        Duration::from_nanos(numerator.div_ceil(denominator))
    }

    /// Freshness deadline two requested frame intervals after capture.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureCadenceError::InstantOverflow`] when the deadline is
    /// outside the platform monotonic clock range.
    pub fn freshness_deadline(self, captured_at: Instant) -> Result<Instant, CaptureCadenceError> {
        let two = NonZeroU32::new(2).expect("two is non-zero");
        captured_at
            .checked_add(self.interval_window(two))
            .ok_or(CaptureCadenceError::InstantOverflow)
    }

    /// Create a phase-preserving scheduler for this cadence.
    #[must_use]
    pub const fn pacer(self) -> CapturePacer {
        CapturePacer {
            cadence: self,
            remainder_phase: 0,
        }
    }
}

/// Rational cadence scheduler that does not accumulate integer-nanosecond drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapturePacer {
    cadence: CaptureCadence,
    remainder_phase: u32,
}

impl CapturePacer {
    /// Produce the next positive interval while preserving exact average cadence.
    pub fn next_interval(&mut self) -> Duration {
        let frames_per_second = self.cadence.frames_per_second.get();
        let base_nanos = NANOS_PER_SECOND / u64::from(frames_per_second);
        let remainder = (NANOS_PER_SECOND % u64::from(frames_per_second)) as u32;
        let phase = u64::from(self.remainder_phase) + u64::from(remainder);
        let extra_nanos = u64::from(phase >= u64::from(frames_per_second));
        self.remainder_phase = (phase % u64::from(frames_per_second)) as u32;
        Duration::from_nanos(base_nanos + extra_nanos)
    }

    /// Advance an existing deadline without retaining lateness as future drift.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureCadenceError::InstantOverflow`] when the next deadline
    /// is outside the platform monotonic clock range.
    pub fn advance_deadline(
        &mut self,
        deadline: Instant,
        now: Instant,
    ) -> Result<Instant, CaptureCadenceError> {
        deadline
            .max(now)
            .checked_add(self.next_interval())
            .ok_or(CaptureCadenceError::InstantOverflow)
    }
}

/// Capture cadence cannot be represented faithfully by the scheduler clock.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum CaptureCadenceError {
    /// A cadence must request at least one frame per second.
    #[error("capture cadence must be non-zero")]
    Zero,
    /// The scheduler cannot represent a positive interval at this cadence.
    #[error(
        "capture cadence {requested_fps} FPS exceeds the scheduler clock limit of {maximum_fps} FPS"
    )]
    ClockResolutionExceeded {
        /// Requested cadence.
        requested_fps: u32,
        /// Highest cadence with a positive representable interval.
        maximum_fps: u32,
    },
    /// A scheduler deadline cannot be represented by the platform clock.
    #[error("capture cadence deadline exceeds the platform monotonic clock range")]
    InstantOverflow,
}
