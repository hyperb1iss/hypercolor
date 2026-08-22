//! HTTP client for daemon communication.
//!
//! Builds and sends requests to the Hypercolor daemon's REST API.
//! When the daemon is not running, all requests return a descriptive error
//! rather than panicking.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use hypercolor_types::api::envelope::ApiErrorBody;
use serde::Serialize;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{Message, http};

type DaemonWebSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
const WEBSOCKET_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const WEBSOCKET_ACKNOWLEDGMENT_TIMEOUT: Duration = Duration::from_secs(5);

/// HTTP client for the Hypercolor daemon REST API.
#[derive(Debug, Clone)]
pub struct DaemonClient {
    /// Base URL for the daemon (e.g., `http://localhost:9420`).
    base_url: String,
    /// Optional API key sent as a bearer token.
    api_key: Option<String>,
    /// Inner `reqwest` async client.
    http: reqwest::Client,
}

impl DaemonClient {
    /// Create a new client targeting the given host and port.
    #[must_use]
    pub fn new(host: &str, port: u16, api_key: Option<&str>) -> Self {
        let base_url = format!("http://{host}:{port}");
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("CLI HTTP client should build");
        Self {
            base_url,
            api_key: api_key.map(ToOwned::to_owned),
            http,
        }
    }

    /// Send a GET request to the daemon and parse the JSON response.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon is unreachable or returns a non-success
    /// status code.
    pub async fn get(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .with_auth(self.http.get(&url))
            .send()
            .await
            .with_context(|| {
                format!("Failed to connect to daemon at {url}. Is the daemon running?")
            })?;
        parse_api_response(response).await
    }

    /// Subscribe to the daemon's event channel.
    ///
    /// The returned stream is acknowledged before this method completes, so
    /// callers can fetch an authoritative REST snapshot without an event gap.
    ///
    /// # Errors
    ///
    /// Returns an error if the WebSocket cannot connect, the subscription is
    /// rejected, or the connection closes before acknowledgment.
    pub async fn subscribe_events(&self) -> Result<DaemonEventSubscription> {
        DaemonEventSubscription::connect(&self.base_url, self.api_key.as_deref()).await
    }

    /// Send a GET request to a path mounted outside the `/api/v1` prefix,
    /// such as the top-level `/health` probe.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon is unreachable or returns a non-success
    /// status code.
    pub async fn get_unversioned(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}{path}", self.base_url);
        let response = self
            .with_auth(self.http.get(&url))
            .send()
            .await
            .with_context(|| {
                format!("Failed to connect to daemon at {url}. Is the daemon running?")
            })?;
        parse_api_response(response).await
    }

    /// Send a POST request with a JSON body and parse the response.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon is unreachable, the body cannot be
    /// serialized, or the daemon returns a non-success status code.
    pub async fn post(&self, path: &str, body: &impl Serialize) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .with_auth(self.http.post(&url))
            .json(body)
            .send()
            .await
            .with_context(|| {
                format!("Failed to connect to daemon at {url}. Is the daemon running?")
            })?;
        parse_api_response(response).await
    }

    /// Send a POST request with a JSON body, extra headers, and parse the response.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon is unreachable, the body cannot be
    /// serialized, or the daemon returns a non-success status code.
    pub async fn post_with_headers(
        &self,
        path: &str,
        body: &impl Serialize,
        headers: &[(&str, &str)],
    ) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1{path}", self.base_url);
        let request = headers.iter().fold(
            self.with_auth(self.http.post(&url)),
            |request, (name, value)| request.header(*name, *value),
        );
        let response = request.json(body).send().await.with_context(|| {
            format!("Failed to connect to daemon at {url}. Is the daemon running?")
        })?;
        parse_api_response(response).await
    }

    /// Send a PUT request with a JSON body and parse the response.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon is unreachable, the body cannot be
    /// serialized, or the daemon returns a non-success status code.
    pub async fn put(&self, path: &str, body: &impl Serialize) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .with_auth(self.http.put(&url))
            .json(body)
            .send()
            .await
            .with_context(|| {
                format!("Failed to connect to daemon at {url}. Is the daemon running?")
            })?;
        parse_api_response(response).await
    }

    /// Send a PATCH request with a JSON body and parse the response.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon is unreachable, the body cannot be
    /// serialized, or the daemon returns a non-success status code.
    pub async fn patch(&self, path: &str, body: &impl Serialize) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .with_auth(self.http.patch(&url))
            .json(body)
            .send()
            .await
            .with_context(|| {
                format!("Failed to connect to daemon at {url}. Is the daemon running?")
            })?;
        parse_api_response(response).await
    }

    /// Send a DELETE request and parse the response.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon is unreachable or returns a non-success
    /// status code.
    pub async fn delete(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}/api/v1{path}", self.base_url);
        let response = self
            .with_auth(self.http.delete(&url))
            .send()
            .await
            .with_context(|| {
                format!("Failed to connect to daemon at {url}. Is the daemon running?")
            })?;
        parse_api_response(response).await
    }

    fn with_auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(api_key) = &self.api_key {
            request.bearer_auth(api_key)
        } else {
            request
        }
    }
}

/// Acknowledged daemon event-channel subscription.
pub struct DaemonEventSubscription {
    stream: DaemonWebSocket,
}

impl DaemonEventSubscription {
    async fn connect(base_url: &str, api_key: Option<&str>) -> Result<Self> {
        let request = websocket_request(base_url, api_key)?;
        let (stream, _) = tokio::time::timeout(
            WEBSOCKET_CONNECT_TIMEOUT,
            tokio_tungstenite::connect_async(request),
        )
        .await
        .context("Timed out connecting to daemon event stream")?
        .context("Failed to connect to daemon event stream")?;
        let mut subscription = Self { stream };
        subscription
            .stream
            .send(Message::Text(
                serde_json::json!({
                    "type": "subscribe",
                    "topics": [{"topic": "events"}]
                })
                .to_string()
                .into(),
            ))
            .await
            .context("Failed to subscribe to daemon events")?;
        tokio::time::timeout(
            WEBSOCKET_ACKNOWLEDGMENT_TIMEOUT,
            subscription.wait_for_acknowledgment(),
        )
        .await
        .context("Timed out waiting for daemon event subscription acknowledgment")??;
        Ok(subscription)
    }

    async fn wait_for_acknowledgment(&mut self) -> Result<()> {
        while let Some(message) = self.next_message().await? {
            let Message::Text(text) = message else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            if value.get("type").and_then(serde_json::Value::as_str) == Some("error") {
                let reason = value
                    .get("message")
                    .or_else(|| value.get("error"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unspecified protocol error");
                anyhow::bail!("Daemon rejected event subscription: {reason}");
            }
            if value.get("type").and_then(serde_json::Value::as_str) == Some("subscribed")
                && value
                    .get("topics")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|topics| {
                        topics
                            .iter()
                            .any(|entry| entry.get("topic").is_some_and(|topic| topic == "events"))
                    })
            {
                return Ok(());
            }
        }
        anyhow::bail!("Daemon event stream closed before subscription acknowledgment")
    }

    /// Wait for the next safe daemon event.
    ///
    /// # Errors
    ///
    /// Returns an error for WebSocket transport failures.
    pub async fn next_event(&mut self) -> Result<Option<serde_json::Value>> {
        while let Some(message) = self.next_message().await? {
            let Message::Text(text) = message else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            if value.get("type").and_then(serde_json::Value::as_str) == Some("event") {
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    async fn next_message(&mut self) -> Result<Option<Message>> {
        loop {
            let Some(message) = self.stream.next().await else {
                return Ok(None);
            };
            let message = message.context("Daemon event stream failed")?;
            match message {
                Message::Close(_) => return Ok(None),
                Message::Ping(payload) => {
                    self.stream
                        .send(Message::Pong(payload))
                        .await
                        .context("Failed to answer daemon event-stream ping")?;
                }
                message => return Ok(Some(message)),
            }
        }
    }

    /// Close the event subscription gracefully.
    pub async fn close(mut self) {
        let _ = self.stream.send(Message::Close(None)).await;
    }
}

fn websocket_url(base_url: &str) -> String {
    let base = base_url.strip_prefix("https://").map_or_else(
        || {
            base_url
                .strip_prefix("http://")
                .map(|authority| format!("ws://{authority}"))
                .unwrap_or_else(|| format!("ws://{base_url}"))
        },
        |authority| format!("wss://{authority}"),
    );
    format!("{base}/api/v1/ws")
}

fn websocket_request(base_url: &str, api_key: Option<&str>) -> Result<http::Request<()>> {
    let mut request = websocket_url(base_url)
        .into_client_request()
        .context("Failed to construct daemon event-stream request")?;
    if let Some(api_key) = api_key {
        let authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))
            .context("API key cannot be represented in an authorization header")?;
        request
            .headers_mut()
            .insert(http::header::AUTHORIZATION, authorization);
    }
    Ok(request)
}

async fn parse_api_response(response: reqwest::Response) -> Result<serde_json::Value> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("{}", describe_error_body(status, &body));
    }

    let json: serde_json::Value = response
        .json()
        .await
        .context("Failed to parse daemon response as JSON")?;

    Ok(json.get("data").cloned().unwrap_or(json))
}

/// Render a failed response as one line a human can act on.
///
/// The daemon answers every error with the canonical envelope
/// `{ error: { code, message, details }, meta }`, so the code and message
/// are what a user needs; the raw JSON is the fallback for the surfaces
/// that bypass it (Axum's own rejections, binary routes).
fn describe_error_body(status: reqwest::StatusCode, body: &str) -> String {
    let Ok(envelope) = serde_json::from_str::<ApiErrorBody>(body) else {
        let trimmed = body.trim();
        return if trimmed.is_empty() {
            format!("Daemon returned {status}")
        } else {
            format!("Daemon returned {status}: {trimmed}")
        };
    };

    let code = envelope.error.code;
    let message = envelope.error.message;
    match envelope.error.details {
        Some(details) => format!("Daemon returned {status} ({code}): {message} [{details}]"),
        None => format!("Daemon returned {status} ({code}): {message}"),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;

    use super::DaemonClient;
    use super::{http, websocket_request, websocket_url};

    #[test]
    fn websocket_request_preserves_transport_and_protects_credentials() {
        assert_eq!(
            websocket_url("http://localhost:9420"),
            "ws://localhost:9420/api/v1/ws"
        );
        assert_eq!(
            websocket_url("https://example.test"),
            "wss://example.test/api/v1/ws"
        );
        let request = websocket_request("http://localhost:9420", Some("hc key/1"))
            .expect("reserved characters should remain valid in a bearer header");
        assert_eq!(request.uri().query(), None);
        assert_eq!(
            request.headers()[http::header::AUTHORIZATION],
            "Bearer hc key/1"
        );
    }

    #[tokio::test]
    async fn event_subscription_is_acknowledged_before_delivery() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("fixture listener should bind");
        let port = listener
            .local_addr()
            .expect("fixture address should resolve")
            .port();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("fixture should accept");
            let mut stream = tokio_tungstenite::accept_async(socket)
                .await
                .expect("fixture WebSocket should accept");
            let message = stream
                .next()
                .await
                .expect("subscription should arrive")
                .expect("subscription should decode");
            let Message::Text(text) = message else {
                panic!("subscription should be text");
            };
            let request: serde_json::Value =
                serde_json::from_str(&text).expect("subscription should be JSON");
            assert_eq!(request["type"], "subscribe");
            assert_eq!(request["topics"], serde_json::json!([{"topic": "events"}]));

            stream
                .send(Message::Text(
                    serde_json::json!({
                        "type": "subscribed",
                        "topics": [{"topic": "events"}],
                        "config": {},
                        "preview_transport": "none"
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .expect("acknowledgment should send");
            stream
                .send(Message::Text(
                    serde_json::json!({
                        "type": "event",
                        "event": "macos_daemon_ownership_changed",
                        "timestamp": "2026-08-12T00:00:00Z",
                        "data": {"owner_epoch": 4}
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .expect("event should send");
            let close = stream
                .next()
                .await
                .expect("close should arrive")
                .expect("close should decode");
            assert!(matches!(close, Message::Close(_)));
        });

        let client = DaemonClient::new("127.0.0.1", port, None);
        let mut subscription = client
            .subscribe_events()
            .await
            .expect("event subscription should be acknowledged");
        let event = subscription
            .next_event()
            .await
            .expect("event stream should remain valid")
            .expect("fixture event should arrive");

        assert_eq!(event["event"], "macos_daemon_ownership_changed");
        assert_eq!(event["data"]["owner_epoch"], 4);
        subscription.close().await;
        server.await.expect("fixture server should finish");
    }

    #[tokio::test]
    async fn event_subscription_surfaces_protocol_rejection_immediately() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("fixture listener should bind");
        let port = listener
            .local_addr()
            .expect("fixture address should resolve")
            .port();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("fixture should accept");
            let mut stream = tokio_tungstenite::accept_async(socket)
                .await
                .expect("fixture WebSocket should accept");
            stream
                .next()
                .await
                .expect("subscription should arrive")
                .expect("subscription should decode");
            stream
                .send(Message::Text(
                    serde_json::json!({
                        "type": "error",
                        "message": "events are unavailable"
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .expect("rejection should send");
        });

        let client = DaemonClient::new("127.0.0.1", port, None);
        let result = tokio::time::timeout(Duration::from_secs(1), client.subscribe_events())
            .await
            .expect("protocol rejection should not wait for the acknowledgment timeout");
        let Err(error) = result else {
            panic!("protocol rejection should fail the subscription");
        };
        assert!(error.to_string().contains("events are unavailable"));
        server.await.expect("fixture server should finish");
    }
}
