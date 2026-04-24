use async_trait::async_trait;
use std::time::Duration;
use thiserror::Error;

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

pub struct Reconnecting<C, P>
where
    C: Connector,
    P: ReconnectPolicy,
{
    connector: C,
    policy: P,
    transport: Option<C::Transport>,
    attempt: u32,
    last_delay: Option<Duration>,
}

impl<C, P> Reconnecting<C, P>
where
    C: Connector,
    P: ReconnectPolicy,
{
    #[must_use]
    pub const fn new(connector: C, policy: P) -> Self {
        Self {
            connector,
            policy,
            transport: None,
            attempt: 0,
            last_delay: None,
        }
    }

    pub async fn connect(&mut self) -> Result<(), ReconnectError<C::Error>> {
        let transport = self
            .connector
            .connect()
            .await
            .map_err(ReconnectError::Connect)?;
        self.transport = Some(transport);
        self.attempt = 0;
        self.policy.reset();
        Ok(())
    }

    pub async fn reconnect(
        &mut self,
        outcome: ReconnectOutcome,
    ) -> Result<(), ReconnectError<C::Error>> {
        let delay =
            self.policy
                .next_delay(self.attempt, outcome)
                .ok_or(ReconnectError::Exhausted {
                    attempt: self.attempt,
                    outcome,
                })?;
        self.attempt = self.attempt.saturating_add(1);
        self.last_delay = Some(delay);

        self.connect().await
    }

    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.transport.is_some()
    }

    #[must_use]
    pub fn last_delay(&self) -> Option<Duration> {
        self.last_delay
    }

    pub fn transport(&self) -> Option<&C::Transport> {
        self.transport.as_ref()
    }

    pub fn transport_mut(&mut self) -> Option<&mut C::Transport> {
        self.transport.as_mut()
    }

    pub fn into_parts(self) -> (C, P, Option<C::Transport>) {
        (self.connector, self.policy, self.transport)
    }
}

#[derive(Debug, Error)]
pub enum ReconnectError<E>
where
    E: std::error::Error + 'static,
{
    #[error(transparent)]
    Connect(E),
    #[error("reconnect policy exhausted after attempt {attempt} for {outcome:?}")]
    Exhausted {
        attempt: u32,
        outcome: ReconnectOutcome,
    },
}

#[derive(Debug, Error)]
pub enum ReconnectSendError<C, S>
where
    C: std::error::Error + 'static,
    S: std::error::Error + 'static,
{
    #[error("transport is not connected")]
    NotConnected,
    #[error(transparent)]
    Reconnect(#[from] ReconnectError<C>),
    #[error(transparent)]
    Transport(S),
}

#[derive(Debug, Error)]
pub enum ReconnectRecvError<C, R>
where
    C: std::error::Error + 'static,
    R: std::error::Error + 'static,
{
    #[error("transport is not connected")]
    NotConnected,
    #[error(transparent)]
    Reconnect(#[from] ReconnectError<C>),
    #[error(transparent)]
    Transport(R),
}

#[async_trait(?Send)]
impl<C, P> CinderTransport for Reconnecting<C, P>
where
    C: Connector,
    P: ReconnectPolicy,
{
    type SendError = ReconnectSendError<C::Error, <C::Transport as CinderTransport>::SendError>;
    type RecvError = ReconnectRecvError<C::Error, <C::Transport as CinderTransport>::RecvError>;

    async fn send(&mut self, frame: bytes::Bytes) -> Result<(), Self::SendError> {
        if self.transport.is_none() {
            self.reconnect(ReconnectOutcome::SendFailure).await?;
        }

        let result = self
            .transport
            .as_mut()
            .ok_or(ReconnectSendError::NotConnected)?
            .send(frame)
            .await;

        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                self.reconnect(ReconnectOutcome::SendFailure).await?;
                Err(ReconnectSendError::Transport(error))
            }
        }
    }

    async fn recv(&mut self) -> Result<Option<bytes::Bytes>, Self::RecvError> {
        if self.transport.is_none() {
            self.reconnect(ReconnectOutcome::RecvFailure).await?;
        }

        let result = self
            .transport
            .as_mut()
            .ok_or(ReconnectRecvError::NotConnected)?
            .recv()
            .await;

        match result {
            Ok(Some(frame)) => Ok(Some(frame)),
            Ok(None) => {
                self.reconnect(ReconnectOutcome::RemoteClosed).await?;
                Ok(None)
            }
            Err(error) => {
                self.reconnect(ReconnectOutcome::RecvFailure).await?;
                Err(ReconnectRecvError::Transport(error))
            }
        }
    }

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::SendError>> {
        match self.transport.as_mut() {
            Some(transport) => transport
                .poll_ready(cx)
                .map_err(ReconnectSendError::Transport),
            None => std::task::Poll::Ready(Err(ReconnectSendError::NotConnected)),
        }
    }

    async fn close(&mut self) -> Result<(), Self::SendError> {
        match self.transport.as_mut() {
            Some(transport) => transport
                .close()
                .await
                .map_err(ReconnectSendError::Transport),
            None => Ok(()),
        }
    }
}
