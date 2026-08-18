//! Trusted in-process access to the daemon API.
//!
//! Downstream daemon extensions use this surface when an already-authenticated
//! transport needs to execute the same API handlers as a local client. The
//! executor grants control authority to every request, so callers must finish
//! authentication and admission before invoking it.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::ws::Message;
use axum::http::{Request, Response, Uri};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use super::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TrustedLocalApiError {
    #[error("trusted local requests are limited to /api/v1 routes")]
    UnsupportedPath,
    #[error("trusted local websocket session is closed")]
    WebSocketClosed,
    #[error("trusted local websocket message exceeds the daemon limit")]
    WebSocketMessageTooLarge,
    #[error("trusted local websocket requires a Tokio runtime")]
    RuntimeUnavailable,
}

pub const MAX_TRUSTED_LOCAL_WEBSOCKET_MESSAGE_BYTES: usize = super::ws::MAX_WS_MESSAGE_BYTES;

/// Control-authorized access to the daemon's real Axum router.
#[derive(Clone)]
pub struct TrustedLocalApi {
    state: Arc<AppState>,
    router: Router,
}

impl TrustedLocalApi {
    #[must_use]
    pub fn new(state: Arc<AppState>) -> Self {
        let router = super::build_router(Arc::clone(&state), None);
        Self { state, router }
    }

    pub async fn execute(
        &self,
        mut request: Request<Body>,
    ) -> Result<Response<Body>, TrustedLocalApiError> {
        validate_http_uri(request.uri())?;
        super::security::mark_trusted_local_control(&mut request);
        Ok(self
            .router
            .clone()
            .oneshot(request)
            .await
            .unwrap_or_else(|never| match never {}))
    }

    pub fn open_websocket(
        &self,
        path: &str,
    ) -> Result<TrustedLocalWebSocket, TrustedLocalApiError> {
        validate_websocket_uri(path)?;
        let runtime =
            Handle::try_current().map_err(|_| TrustedLocalApiError::RuntimeUnavailable)?;
        Ok(super::ws::spawn_trusted_local_socket(
            Arc::clone(&self.state),
            &runtime,
        ))
    }
}

fn validate_http_uri(uri: &Uri) -> Result<(), TrustedLocalApiError> {
    let path = uri.path();
    if uri.scheme().is_some()
        || uri.authority().is_some()
        || !path.starts_with("/api/v1/")
        || path == "/api/v1/ws"
    {
        return Err(TrustedLocalApiError::UnsupportedPath);
    }
    Ok(())
}

fn validate_websocket_uri(raw: &str) -> Result<(), TrustedLocalApiError> {
    let uri = raw
        .parse::<Uri>()
        .map_err(|_| TrustedLocalApiError::UnsupportedPath)?;
    if uri.scheme().is_some() || uri.authority().is_some() || uri.path() != "/api/v1/ws" {
        return Err(TrustedLocalApiError::UnsupportedPath);
    }
    Ok(())
}

const TRUSTED_LOCAL_SOCKET_BUFFER: usize = 64;

pub struct TrustedLocalWebSocket {
    inbound: mpsc::Sender<Message>,
    outbound: mpsc::Receiver<Message>,
    shutdown: CancellationToken,
    completion: Option<oneshot::Receiver<()>>,
}

impl TrustedLocalWebSocket {
    pub async fn send(&self, message: Message) -> Result<(), TrustedLocalApiError> {
        if websocket_message_len(&message) > MAX_TRUSTED_LOCAL_WEBSOCKET_MESSAGE_BYTES {
            return Err(TrustedLocalApiError::WebSocketMessageTooLarge);
        }
        tokio::select! {
            () = self.shutdown.cancelled() => Err(TrustedLocalApiError::WebSocketClosed),
            result = self.inbound.send(message) => {
                result.map_err(|_| TrustedLocalApiError::WebSocketClosed)
            }
        }
    }

    pub async fn recv(&mut self) -> Option<Message> {
        loop {
            match self.outbound.recv().await? {
                Message::Ping(payload) => {
                    self.send(Message::Pong(payload)).await.ok()?;
                }
                message => return Some(message),
            }
        }
    }

    pub async fn shutdown(mut self) {
        self.shutdown.cancel();
        if let Some(completion) = self.completion.take() {
            let _ = completion.await;
        }
    }
}

fn websocket_message_len(message: &Message) -> usize {
    match message {
        Message::Text(text) => text.len(),
        Message::Binary(bytes) | Message::Ping(bytes) | Message::Pong(bytes) => bytes.len(),
        Message::Close(Some(frame)) => frame.reason.len().saturating_add(2),
        Message::Close(None) => 0,
    }
}

impl Drop for TrustedLocalWebSocket {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

pub(crate) struct TrustedLocalSocketTransport {
    inbound: mpsc::Receiver<Message>,
    outbound: mpsc::Sender<Message>,
    shutdown: CancellationToken,
    completion: Option<oneshot::Sender<()>>,
}

impl TrustedLocalSocketTransport {
    pub(crate) async fn send(&mut self, message: Message) -> Result<(), ()> {
        tokio::select! {
            () = self.shutdown.cancelled() => Err(()),
            result = self.outbound.send(message) => result.map_err(|_| ()),
        }
    }

    pub(crate) async fn recv(&mut self) -> Option<Message> {
        tokio::select! {
            () = self.shutdown.cancelled() => None,
            message = self.inbound.recv() => message,
        }
    }

    pub(crate) fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }
}

impl Drop for TrustedLocalSocketTransport {
    fn drop(&mut self) {
        if let Some(completion) = self.completion.take() {
            let _ = completion.send(());
        }
    }
}

pub(crate) fn trusted_local_socket_pair() -> (TrustedLocalWebSocket, TrustedLocalSocketTransport) {
    let (inbound_tx, inbound_rx) = mpsc::channel(TRUSTED_LOCAL_SOCKET_BUFFER);
    let (outbound_tx, outbound_rx) = mpsc::channel(TRUSTED_LOCAL_SOCKET_BUFFER);
    let shutdown = CancellationToken::new();
    let (completion_tx, completion_rx) = oneshot::channel();
    (
        TrustedLocalWebSocket {
            inbound: inbound_tx,
            outbound: outbound_rx,
            shutdown: shutdown.clone(),
            completion: Some(completion_rx),
        },
        TrustedLocalSocketTransport {
            inbound: inbound_rx,
            outbound: outbound_tx,
            shutdown,
            completion: Some(completion_tx),
        },
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::{Body, to_bytes};
    use axum::extract::ws::Message;
    use axum::http::{Request, StatusCode};
    use hypercolor_core::input::{BrowserInputSource, InputGraphHandle, InputSource};
    use hypercolor_types::config::InteractionRoutePolicy;

    use super::{MAX_TRUSTED_LOCAL_WEBSOCKET_MESSAGE_BYTES, TrustedLocalApi, TrustedLocalApiError};
    use crate::api::AppState;
    use crate::api::security::SecurityState;
    use crate::interaction_routing::InteractionRoutingControl;
    use crate::interactive_preview::{
        InteractivePreviewAcceleration, InteractivePreviewContext, InteractivePreviewExecutor,
    };
    use crate::render_thread::InputPublicationDemandHandle;

    fn secured_api() -> TrustedLocalApi {
        let mut state = AppState::new();
        state.security_state =
            SecurityState::with_keys(Some("hc_ak_control_test"), Some("hc_ak_r_read_test"));
        TrustedLocalApi::new(std::sync::Arc::new(state))
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("trusted local response body should be bounded");
        serde_json::from_slice(&bytes).expect("trusted local response should be JSON")
    }

    fn message_json(message: Message) -> serde_json::Value {
        let Message::Text(text) = message else {
            panic!("trusted local socket should emit a text protocol message");
        };
        serde_json::from_str(text.as_str()).expect("trusted local socket text should be JSON")
    }

    #[tokio::test]
    async fn trusted_http_executes_control_routes_without_exporting_api_keys() {
        let api = secured_api();
        let response = api
            .execute(
                Request::builder()
                    .method("PATCH")
                    .uri("/api/v1/output")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"brightness":0.37}"#))
                    .expect("trusted local request should build"),
            )
            .await
            .expect("trusted local API path should be accepted");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["data"]["brightness"], 0.37);
    }

    #[tokio::test]
    async fn trusted_http_rejects_non_api_and_websocket_paths() {
        let api = secured_api();
        for path in ["/health", "/api/v1/ws", "https://example.com/api/v1/status"] {
            let error = api
                .execute(
                    Request::builder()
                        .uri(path)
                        .body(Body::empty())
                        .expect("invalid trusted local request should still build"),
                )
                .await
                .expect_err("trusted local executor must not escape the HTTP API surface");
            assert_eq!(error, TrustedLocalApiError::UnsupportedPath);
        }
    }

    #[tokio::test]
    async fn trusted_websocket_runs_the_real_control_session_and_joins_shutdown() {
        let api = secured_api();
        let mut socket = api
            .open_websocket("/api/v1/ws")
            .expect("canonical trusted local websocket path should open");

        let hello = socket
            .recv()
            .await
            .expect("trusted local websocket should emit its hello barrier");
        assert_eq!(message_json(hello)["type"], "hello");

        socket
            .send(Message::Text(
                serde_json::json!({
                    "type": "command",
                    "id": "set_brightness",
                    "method": "PATCH",
                    "path": "/output",
                    "body": {"brightness": 0.41}
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("trusted local websocket should accept a command frame");
        let response = socket
            .recv()
            .await
            .expect("trusted local websocket should emit the command response barrier");
        let response = message_json(response);
        assert_eq!(response["type"], "response");
        assert_eq!(response["id"], "set_brightness");
        assert_eq!(response["status"], 200);
        assert_eq!(response["data"]["brightness"], 0.41);

        socket.shutdown().await;
    }

    #[tokio::test]
    async fn trusted_websocket_shutdown_joins_active_preview_cleanup() {
        let mut source = BrowserInputSource::new();
        source.start().expect("browser input source should start");
        let browser_input = source.handle();
        let interaction_routing = InteractionRoutingControl::new(
            browser_input.registry(),
            1,
            InteractionRoutePolicy::Host,
            InteractionRoutePolicy::Browser,
        );
        let mut state = AppState::new();
        state.browser_input = browser_input;
        state.interaction_routing = interaction_routing;
        let state = Arc::new(state);
        let executor = Arc::new(
            InteractivePreviewExecutor::start_cpu(InteractivePreviewContext {
                scene_manager: Arc::clone(&state.scene_manager),
                effect_registry: Arc::clone(&state.effect_registry),
                asset_library: Some(Arc::clone(&state.asset_library)),
                event_bus: Arc::clone(&state.event_bus),
                input_graph: InputGraphHandle::default(),
                sensor_snapshots: None,
                interaction_routing: state.interaction_routing.clone(),
                input_demands: InputPublicationDemandHandle::new(),
                canvas_width: 64,
                canvas_height: 64,
                acceleration: InteractivePreviewAcceleration::cpu(),
                resource_capacity_bytes: 64 * 1024 * 1024,
            })
            .await
            .expect("CPU interactive preview executor should start"),
        );
        state
            .preview_runtime
            .install_interactive_executor(Arc::clone(&executor));
        let api = TrustedLocalApi::new(Arc::clone(&state));
        let mut socket = api
            .open_websocket("/api/v1/ws")
            .expect("canonical trusted local websocket path should open");

        assert_eq!(
            message_json(
                socket
                    .recv()
                    .await
                    .expect("trusted local websocket should emit hello")
            )["type"],
            "hello"
        );
        socket
            .send(Message::Text(
                serde_json::json!({
                    "type": "subscribe",
                    "topics": [{
                        "topic": "interactive_preview",
                        "key": "remote",
                        "config": {
                            "fps": 30,
                            "width": 64,
                            "height": 64,
                            "format": "rgba"
                        }
                    }]
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("trusted local websocket should accept a preview subscription");
        let opened = message_json(
            socket
                .recv()
                .await
                .expect("trusted local websocket should acknowledge the preview barrier"),
        );
        assert_eq!(opened["type"], "subscribed", "{opened}");
        let preview = opened["topics"]
            .as_array()
            .expect("the acknowledgment lists live subscriptions")
            .iter()
            .find(|entry| entry["topic"] == "interactive_preview")
            .expect("the interactive preview subscription is live");
        assert_eq!(preview["key"], "remote", "{opened}");
        assert!(
            preview["publication_id"].as_u64().is_some_and(|id| id > 0),
            "the acknowledgment carries the lane's publication: {opened}"
        );
        assert_eq!(executor.lane_count(), 1);

        socket.shutdown().await;

        assert_eq!(executor.lane_count(), 0);
        assert!(
            state
                .browser_input
                .registry()
                .snapshot()
                .children()
                .is_empty()
        );
        assert_eq!(
            executor
                .resource_snapshot()
                .used
                .total_bytes()
                .expect("preview resource usage should remain representable"),
            0
        );
    }

    #[tokio::test]
    async fn trusted_websocket_rejects_oversized_messages_before_dispatch() {
        let api = secured_api();
        let socket = api
            .open_websocket("/api/v1/ws")
            .expect("canonical trusted local websocket path should open");
        let error = socket
            .send(Message::Binary(
                vec![0; MAX_TRUSTED_LOCAL_WEBSOCKET_MESSAGE_BYTES + 1].into(),
            ))
            .await
            .expect_err("trusted local websocket must preserve the network frame bound");
        assert_eq!(error, TrustedLocalApiError::WebSocketMessageTooLarge);
        socket.shutdown().await;
    }

    #[tokio::test]
    async fn trusted_websocket_answers_heartbeats_before_exposing_protocol_messages() {
        let (mut socket, mut transport) = super::trusted_local_socket_pair();
        transport
            .send(Message::Ping(vec![1, 2, 3].into()))
            .await
            .expect("daemon transport should send a heartbeat");
        transport
            .send(Message::Text(r#"{"type":"barrier"}"#.into()))
            .await
            .expect("daemon transport should send the protocol barrier");

        assert_eq!(
            socket
                .recv()
                .await
                .expect("trusted local socket should expose the protocol barrier"),
            Message::Text(r#"{"type":"barrier"}"#.into())
        );
        assert_eq!(
            transport
                .recv()
                .await
                .expect("daemon transport should receive the automatic heartbeat reply"),
            Message::Pong(vec![1, 2, 3].into())
        );
    }

    #[test]
    fn trusted_websocket_rejects_noncanonical_paths() {
        let api = secured_api();
        for path in ["/api/v1/status", "ws://localhost/api/v1/ws"] {
            let error = api
                .open_websocket(path)
                .err()
                .expect("trusted local websocket must stay on the canonical API path");
            assert_eq!(error, TrustedLocalApiError::UnsupportedPath);
        }
    }

    #[test]
    fn trusted_websocket_reports_a_missing_runtime_without_panicking() {
        let error = secured_api()
            .open_websocket("/api/v1/ws")
            .err()
            .expect("opening outside Tokio should return an error");
        assert_eq!(error, TrustedLocalApiError::RuntimeUnavailable);
    }
}
