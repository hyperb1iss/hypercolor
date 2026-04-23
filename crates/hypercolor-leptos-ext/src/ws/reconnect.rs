use async_trait::async_trait;
use std::time::Duration;

use crate::MaybeSend;

use super::transport::CinderTransport;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconnectOutcome {
    ConnectFailure,
    SendFailure,
    RecvFailure,
    RemoteClosed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Jitter {
    None,
    Equal(f64),
}

pub trait ReconnectPolicy: MaybeSend + 'static {
    fn next_delay(&mut self, attempt: u32, outcome: ReconnectOutcome) -> Option<Duration>;
    fn reset(&mut self);
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

    pub fn delay_for_attempt(&self, attempt: u32) -> Option<Duration> {
        self.delay_for_attempt_with_sample(attempt, 0.5)
    }

    pub fn delay_for_attempt_with_sample(&self, attempt: u32, sample: f64) -> Option<Duration> {
        if self.base.is_zero() || self.max.is_zero() || self.multiplier <= 0.0 {
            return None;
        }

        let exponent = if attempt > i32::MAX as u32 {
            i32::MAX
        } else {
            attempt as i32
        };
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

impl ReconnectPolicy for ExponentialBackoff {
    fn next_delay(&mut self, attempt: u32, _outcome: ReconnectOutcome) -> Option<Duration> {
        self.delay_for_attempt(attempt)
    }

    fn reset(&mut self) {}
}

#[async_trait(?Send)]
pub trait Connector: MaybeSend + 'static {
    type Transport: CinderTransport;
    type Error: std::error::Error + MaybeSend + Sync + 'static;

    async fn connect(&mut self) -> Result<Self::Transport, Self::Error>;
}

#[async_trait(?Send)]
impl<F, Fut, T, E> Connector for F
where
    F: FnMut() -> Fut + MaybeSend + 'static,
    Fut: core::future::Future<Output = Result<T, E>> + MaybeSend,
    T: CinderTransport,
    E: std::error::Error + MaybeSend + Sync + 'static,
{
    type Transport = T;
    type Error = E;

    async fn connect(&mut self) -> Result<Self::Transport, Self::Error> {
        self().await
    }
}
