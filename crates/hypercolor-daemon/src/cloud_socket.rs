use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use hypercolor_cloud_client::DaemonConnectRequest;
use hypercolor_cloud_client::daemon_link::{
    DaemonCapabilities, HelloFrame, PROTOCOL_VERSION, WelcomeFrame, frame::TunnelResume,
};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{
    HeaderName, InvalidHeaderName, InvalidHeaderValue,
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::cloud_connection::CloudConnectionRuntime;

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
            let _ = task.await;
            runtime.write().await.mark_idle();
        }
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
    let mut socket = session.into_socket();
    loop {
        match socket.next().await {
            Some(Ok(Message::Close(_))) | None => {
                runtime.write().await.mark_backoff("cloud websocket closed");
                break;
            }
            Some(Ok(Message::Ping(payload))) => {
                if socket.send(Message::Pong(payload)).await.is_err() {
                    runtime
                        .write()
                        .await
                        .mark_backoff("cloud websocket connection failed");
                    break;
                }
            }
            Some(Ok(_)) => {}
            Some(Err(error)) => {
                tracing::warn!(error = %error, "cloud daemon socket failed");
                runtime
                    .write()
                    .await
                    .mark_backoff("cloud websocket connection failed");
                break;
            }
        }
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
