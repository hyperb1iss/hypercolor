use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use hypercolor_cloud_client::DaemonConnectRequest;
use hypercolor_cloud_client::daemon_link::{
    ChannelName, DaemonCapabilities, Frame, FrameKind, HelloFrame, PROTOCOL_VERSION, WelcomeFrame,
    frame::TunnelResume,
};
use serde_json::Value;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{
    HeaderName, InvalidHeaderName, InvalidHeaderValue,
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use ulid::Ulid;

use crate::cloud_connection::{CloudConnectionRuntime, CloudConnectionRuntimeState};

#[derive(Debug, Error)]
pub enum CloudSocketError {
    #[error("no prepared cloud connection request is staged")]
    MissingPreparedRequest,
    #[error("invalid cloud connection header name: {0}")]
    InvalidHeaderName(#[from] InvalidHeaderName),
    #[error("invalid cloud connection header value: {0}")]
    InvalidHeaderValue(#[from] InvalidHeaderValue),
    #[error("cloud WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("cloud link frame serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("cloud closed the connection before sending welcome")]
    ClosedBeforeWelcome,
    #[error("cloud sent non-text welcome frame")]
    NonTextWelcome,
}

const MISSED_HEARTBEAT_LIMIT: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudReconnectPolicy {
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub backoff_factor: f64,
    pub jitter: f64,
}

impl Default for CloudReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_factor: 2.0,
            jitter: 0.25,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudReconnectDelay {
    pub attempt_index: u32,
    pub base_delay: Duration,
    pub retry_delay: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CloudReconnectState {
    attempt: u32,
    policy: CloudReconnectPolicy,
}

impl Default for CloudReconnectState {
    fn default() -> Self {
        Self::new(CloudReconnectPolicy::default())
    }
}

impl CloudReconnectState {
    #[must_use]
    pub const fn new(policy: CloudReconnectPolicy) -> Self {
        Self { attempt: 0, policy }
    }

    #[must_use]
    pub const fn next_attempt(&self) -> u32 {
        self.attempt
    }

    pub const fn reset(&mut self) {
        self.attempt = 0;
    }

    #[must_use]
    pub fn next_delay(&mut self) -> CloudReconnectDelay {
        self.next_delay_with_jitter_sample(rand::random_range(-1.0..=1.0))
    }

    #[must_use]
    pub fn next_delay_with_jitter_sample(&mut self, jitter_sample: f64) -> CloudReconnectDelay {
        let attempt_index = self.attempt;
        let base_delay = self.base_delay();
        let retry_delay = self.jittered_delay(base_delay, jitter_sample);
        self.attempt = self.attempt.saturating_add(1);

        CloudReconnectDelay {
            attempt_index,
            base_delay,
            retry_delay,
        }
    }

    fn base_delay(&self) -> Duration {
        let factor = self
            .policy
            .backoff_factor
            .powf(f64::from(self.attempt))
            .max(1.0);
        let base_secs = (self.policy.initial_delay.as_secs_f64() * factor)
            .min(self.policy.max_delay.as_secs_f64());

        Duration::from_secs_f64(base_secs)
    }

    fn jittered_delay(&self, base_delay: Duration, jitter_sample: f64) -> Duration {
        let jitter = jitter_sample.clamp(-1.0, 1.0) * self.policy.jitter.clamp(0.0, 1.0);
        let retry_secs = (base_delay.as_secs_f64() * (1.0 + jitter))
            .max(0.1)
            .min(self.policy.max_delay.as_secs_f64());

        Duration::from_secs_f64(retry_secs)
    }
}

#[derive(Debug, Error)]
pub enum CloudSocketStartError {
    #[error("cloud socket is already running")]
    AlreadyRunning,
    #[error(transparent)]
    Connect(#[from] CloudSocketError),
}

#[derive(Debug, Clone)]
pub struct CloudSocketHelloInput {
    pub entitlement_jwt: Option<String>,
    pub tunnel_resume: Option<TunnelResume>,
    pub studio_preview: bool,
}

#[derive(Debug, Default)]
pub struct CloudSocketRuntime {
    task: Option<JoinHandle<()>>,
}

impl CloudSocketRuntime {
    #[must_use]
    pub fn is_running(&mut self) -> bool {
        self.prune_finished();
        self.task.is_some()
    }

    pub async fn spawn_prepared_session(
        &mut self,
        runtime: Arc<RwLock<CloudConnectionRuntime>>,
        hello: CloudSocketHelloInput,
    ) -> Result<(), CloudSocketStartError> {
        self.prune_finished();
        if self.task.is_some() {
            return Err(CloudSocketStartError::AlreadyRunning);
        }

        let request = take_prepared_request(&runtime).await?;
        self.task = Some(tokio::spawn(async move {
            match connect_request_once(Arc::clone(&runtime), request, hello).await {
                Ok(session) => run_session_until_close(session, runtime).await,
                Err(error) => tracing::warn!(error = %error, "cloud daemon connect failed"),
            }
        }));
        Ok(())
    }

    pub async fn shutdown(&mut self, runtime: &Arc<RwLock<CloudConnectionRuntime>>) {
        if let Some(task) = self.task.take() {
            task.abort();
            if let Err(error) = task.await {
                if error.is_cancelled() {
                    let mut runtime = runtime.write().await;
                    if runtime.snapshot().runtime_state != CloudConnectionRuntimeState::Backoff {
                        runtime.mark_idle();
                    }
                } else {
                    tracing::warn!(error = %error, "cloud socket task failed");
                    runtime
                        .write()
                        .await
                        .mark_backoff("cloud socket task failed");
                }
            }
        }
    }

    pub async fn disconnect(&mut self, runtime: &Arc<RwLock<CloudConnectionRuntime>>) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
        runtime.write().await.mark_idle();
    }

    fn prune_finished(&mut self) {
        if self.task.as_ref().is_some_and(JoinHandle::is_finished) {
            self.task = None;
        }
    }
}

#[derive(Debug)]
pub struct CloudSocketSession {
    welcome: WelcomeFrame,
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl CloudSocketSession {
    #[must_use]
    pub const fn welcome(&self) -> &WelcomeFrame {
        &self.welcome
    }

    #[must_use]
    pub fn into_socket(self) -> WebSocketStream<MaybeTlsStream<TcpStream>> {
        self.socket
    }
}

pub async fn connect_prepared_once(
    runtime: &Arc<RwLock<CloudConnectionRuntime>>,
    hello: CloudSocketHelloInput,
) -> Result<CloudSocketSession, CloudSocketError> {
    let request = take_prepared_request(runtime).await?;
    connect_request_once(Arc::clone(runtime), request, hello).await
}

async fn connect_request_once(
    runtime: Arc<RwLock<CloudConnectionRuntime>>,
    request: DaemonConnectRequest,
    hello: CloudSocketHelloInput,
) -> Result<CloudSocketSession, CloudSocketError> {
    let result = connect_with_request(request, hello_frame(hello)).await;

    match result {
        Ok(session) => {
            runtime.write().await.mark_connected(session.welcome());
            Ok(session)
        }
        Err(error) => {
            runtime.write().await.mark_backoff(error.runtime_message());
            Err(error)
        }
    }
}

async fn run_session_until_close(
    session: CloudSocketSession,
    runtime: Arc<RwLock<CloudConnectionRuntime>>,
) {
    let mut heartbeat = HeartbeatState::new(session.welcome());
    let mut heartbeat_tick =
        tokio::time::interval_at(Instant::now() + heartbeat.interval, heartbeat.interval);
    heartbeat_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut socket = session.into_socket();
    loop {
        tokio::select! {
            frame = socket.next() => {
                match frame {
                    Some(Ok(Message::Close(_))) | None => {
                        runtime.write().await.mark_backoff("cloud websocket closed");
                        break;
                    }
                    Some(Ok(Message::Text(text))) => {
                        match handle_text_frame(&text, &mut heartbeat) {
                            Ok(TextFrameAction::Reply(reply)) => {
                                if socket.send(Message::Text(reply.into())).await.is_err() {
                                    mark_socket_failed(&runtime).await;
                                    break;
                                }
                            }
                            Ok(TextFrameAction::Disconnect(reason)) => {
                                let _ = socket.send(Message::Close(None)).await;
                                runtime.write().await.mark_backoff(reason);
                                break;
                            }
                            Ok(TextFrameAction::Ignore) => {}
                            Err(error) => tracing::warn!(error = %error, "cloud daemon text frame failed"),
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            mark_socket_failed(&runtime).await;
                            break;
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        tracing::warn!(error = %error, "cloud daemon socket failed");
                        mark_socket_failed(&runtime).await;
                        break;
                    }
                }
            }
            _ = heartbeat_tick.tick() => {
                if heartbeat.record_ping_due() {
                    let _ = socket.send(Message::Close(None)).await;
                    runtime.write().await.mark_backoff("cloud heartbeat missed");
                    break;
                }
                match control_frame_text(FrameKind::Ping, None) {
                    Ok(frame) => {
                        if socket.send(Message::Text(frame.into())).await.is_err() {
                            mark_socket_failed(&runtime).await;
                            break;
                        }
                        heartbeat.mark_ping_sent();
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "cloud heartbeat serialization failed");
                        mark_socket_failed(&runtime).await;
                        break;
                    }
                }
            }
        }
    }
}

async fn mark_socket_failed(runtime: &Arc<RwLock<CloudConnectionRuntime>>) {
    runtime
        .write()
        .await
        .mark_backoff("cloud websocket connection failed");
}

fn handle_text_frame(
    text: &str,
    heartbeat: &mut HeartbeatState,
) -> Result<TextFrameAction, serde_json::Error> {
    let frame: Frame<Value> = serde_json::from_str(text)?;
    if frame.channel != ChannelName::Control {
        return Ok(TextFrameAction::Ignore);
    }

    match frame.kind {
        FrameKind::Pong => {
            heartbeat.mark_pong_received();
            Ok(TextFrameAction::Ignore)
        }
        FrameKind::Ping => {
            control_frame_text(FrameKind::Pong, Some(frame.msg_id)).map(TextFrameAction::Reply)
        }
        FrameKind::Msg => Ok(control_msg_action(&frame.payload)),
        _ => Ok(TextFrameAction::Ignore),
    }
}

fn control_msg_action(payload: &Value) -> TextFrameAction {
    match payload.get("kind").and_then(Value::as_str) {
        Some("force.disconnect") => {
            TextFrameAction::Disconnect(control_reason(payload, "cloud requested disconnect"))
        }
        Some("force.relogin") => {
            TextFrameAction::Disconnect(control_reason(payload, "cloud requested relogin"))
        }
        _ => TextFrameAction::Ignore,
    }
}

fn control_reason(payload: &Value, fallback: &str) -> String {
    payload.get("reason").and_then(Value::as_str).map_or_else(
        || fallback.to_owned(),
        |reason| format!("{fallback}: {reason}"),
    )
}

fn control_frame_text(
    kind: FrameKind,
    in_reply_to: Option<Ulid>,
) -> Result<String, serde_json::Error> {
    let mut frame = Frame::new(
        ChannelName::Control,
        kind,
        Ulid::new(),
        Value::Object(serde_json::Map::default()),
    );
    frame.in_reply_to = in_reply_to;
    serde_json::to_string(&frame)
}

#[derive(Debug)]
enum TextFrameAction {
    Ignore,
    Reply(String),
    Disconnect(String),
}

#[derive(Debug)]
struct HeartbeatState {
    interval: Duration,
    awaiting_pong: bool,
    missed_pongs: u8,
}

impl HeartbeatState {
    fn new(welcome: &WelcomeFrame) -> Self {
        Self {
            interval: Duration::from_secs(welcome.heartbeat_interval_s.max(1)),
            awaiting_pong: false,
            missed_pongs: 0,
        }
    }

    fn record_ping_due(&mut self) -> bool {
        if self.awaiting_pong {
            self.missed_pongs = self.missed_pongs.saturating_add(1);
        }
        self.missed_pongs >= MISSED_HEARTBEAT_LIMIT
    }

    const fn mark_ping_sent(&mut self) {
        self.awaiting_pong = true;
    }

    const fn mark_pong_received(&mut self) {
        self.awaiting_pong = false;
        self.missed_pongs = 0;
    }
}

#[must_use]
pub fn hello_frame(input: CloudSocketHelloInput) -> HelloFrame {
    HelloFrame {
        protocol_version: PROTOCOL_VERSION,
        daemon_capabilities: DaemonCapabilities {
            sync: true,
            relay: true,
            entitlement_refresh: true,
            telemetry: false,
            studio_preview: input.studio_preview,
        },
        entitlement_jwt: input.entitlement_jwt,
        tunnel_resume: input.tunnel_resume,
    }
}

async fn take_prepared_request(
    runtime: &Arc<RwLock<CloudConnectionRuntime>>,
) -> Result<DaemonConnectRequest, CloudSocketError> {
    let mut runtime = runtime.write().await;
    let request = runtime
        .take_prepared_connect()
        .ok_or(CloudSocketError::MissingPreparedRequest)?;
    runtime.mark_connecting();
    Ok(request)
}

async fn connect_with_request(
    connect: DaemonConnectRequest,
    hello: HelloFrame,
) -> Result<CloudSocketSession, CloudSocketError> {
    let mut request = connect.url.as_str().into_client_request()?;
    for (name, value) in connect.headers.pairs() {
        request
            .headers_mut()
            .insert(HeaderName::from_bytes(name.as_bytes())?, value.parse()?);
    }

    let (mut socket, _) = connect_async(request).await?;
    socket
        .send(Message::Text(serde_json::to_string(&hello)?.into()))
        .await?;

    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                let welcome = serde_json::from_str(&text)?;
                return Ok(CloudSocketSession { welcome, socket });
            }
            Some(Ok(Message::Close(_))) | None => {
                return Err(CloudSocketError::ClosedBeforeWelcome);
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
            Some(Ok(_)) => return Err(CloudSocketError::NonTextWelcome),
            Some(Err(error)) => return Err(CloudSocketError::WebSocket(error)),
        }
    }
}

impl CloudSocketError {
    fn runtime_message(&self) -> &'static str {
        match self {
            Self::MissingPreparedRequest => "missing prepared cloud connection request",
            Self::InvalidHeaderName(_) | Self::InvalidHeaderValue(_) => {
                "invalid prepared cloud connection request"
            }
            Self::WebSocket(_) => "cloud websocket connection failed",
            Self::Json(_) => "invalid cloud link frame",
            Self::ClosedBeforeWelcome => "cloud closed before welcome",
            Self::NonTextWelcome => "cloud sent non-text welcome frame",
        }
    }
}
