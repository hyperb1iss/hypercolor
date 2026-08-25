use std::num::NonZeroU32;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct MacosDisplayClock {
    anchor_ticks: u64,
    anchor_instant: Instant,
    timebase_numerator: NonZeroU32,
    timebase_denominator: NonZeroU32,
}

impl MacosDisplayClock {
    pub fn new(
        anchor_ticks: u64,
        anchor_instant: Instant,
        timebase_numerator: u32,
        timebase_denominator: u32,
    ) -> Result<Self, MacosDisplayClockError> {
        let timebase_numerator =
            NonZeroU32::new(timebase_numerator).ok_or(MacosDisplayClockError::InvalidTimebase {
                numerator: timebase_numerator,
                denominator: timebase_denominator,
            })?;
        let timebase_denominator = NonZeroU32::new(timebase_denominator).ok_or(
            MacosDisplayClockError::InvalidTimebase {
                numerator: timebase_numerator.get(),
                denominator: timebase_denominator,
            },
        )?;
        Ok(Self {
            anchor_ticks,
            anchor_instant,
            timebase_numerator,
            timebase_denominator,
        })
    }

    #[cfg(target_os = "macos")]
    pub fn system() -> Result<Self, MacosDisplayClockError> {
        #[repr(C)]
        struct MachTimebaseInfo {
            numerator: u32,
            denominator: u32,
        }
        unsafe extern "C" {
            fn mach_absolute_time() -> u64;
            fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
        }
        let mut timebase = MachTimebaseInfo {
            numerator: 0,
            denominator: 0,
        };
        // SAFETY: mach_timebase_info initializes the provided plain-data
        // structure and retains no pointer after returning.
        let result = unsafe { mach_timebase_info(&raw mut timebase) };
        if result != 0 {
            return Err(MacosDisplayClockError::TimebaseQueryFailed(result));
        }
        let anchor_instant = Instant::now();
        // SAFETY: mach_absolute_time has no preconditions or retained state.
        let anchor_ticks = unsafe { mach_absolute_time() };
        Self::new(
            anchor_ticks,
            anchor_instant,
            timebase.numerator,
            timebase.denominator,
        )
    }

    /// Reports that the mach display clock is unavailable on this platform.
    ///
    /// # Errors
    ///
    /// Always returns the unsupported-platform error.
    #[cfg(not(target_os = "macos"))]
    pub fn system() -> Result<Self, MacosDisplayClockError> {
        Err(MacosDisplayClockError::UnsupportedPlatform)
    }

    pub fn timestamp(&self, display_time: u64) -> Result<Instant, MacosDisplayClockError> {
        if display_time >= self.anchor_ticks {
            let elapsed = self.duration(display_time - self.anchor_ticks)?;
            self.anchor_instant
                .checked_add(elapsed)
                .ok_or(MacosDisplayClockError::InstantOutOfRange)
        } else {
            let elapsed = self.duration(self.anchor_ticks - display_time)?;
            self.anchor_instant
                .checked_sub(elapsed)
                .ok_or(MacosDisplayClockError::InstantOutOfRange)
        }
    }

    fn duration(&self, ticks: u64) -> Result<Duration, MacosDisplayClockError> {
        let nanoseconds = u128::from(ticks)
            .checked_mul(u128::from(self.timebase_numerator.get()))
            .ok_or(MacosDisplayClockError::DurationOutOfRange)?
            / u128::from(self.timebase_denominator.get());
        let seconds = u64::try_from(nanoseconds / 1_000_000_000)
            .map_err(|_| MacosDisplayClockError::DurationOutOfRange)?;
        let subsecond_nanos = u32::try_from(nanoseconds % 1_000_000_000)
            .map_err(|_| MacosDisplayClockError::DurationOutOfRange)?;
        Ok(Duration::new(seconds, subsecond_nanos))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum MacosDisplayClockError {
    #[error("mach timebase query failed with status {0}")]
    TimebaseQueryFailed(i32),
    #[error("invalid mach timebase {numerator}/{denominator}")]
    InvalidTimebase { numerator: u32, denominator: u32 },
    #[error("mach display-time duration exceeds the monotonic clock range")]
    DurationOutOfRange,
    #[error("mach display time falls outside the monotonic clock range")]
    InstantOutOfRange,
    #[error("the mach display clock is unavailable on this platform")]
    UnsupportedPlatform,
}
