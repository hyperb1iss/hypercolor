//! Browser-neutral WebSocket transport contract for daemon event streams.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use hypercolor_leptos_ext::prelude::current_page_location;
use hypercolor_leptos_ext::ws::transport::{
    WebSocketEventHandlers, arraybuffer_websocket, message_array_buffer,
};
use wasm_bindgen::JsValue;

use crate::api::client;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSocketConnectRequest {
    pub path: String,
    pub protocol: String,
}

#[derive(Clone)]
pub struct WebSocketBinaryFrame {
    storage: WebSocketBinaryStorage,
}

#[derive(Clone)]
enum WebSocketBinaryStorage {
    ArrayBuffer(js_sys::ArrayBuffer),
    Bytes(Vec<u8>),
}

impl WebSocketBinaryFrame {
    #[must_use]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            storage: WebSocketBinaryStorage::Bytes(bytes),
        }
    }

    #[must_use]
    pub fn to_vec(&self) -> Vec<u8> {
        match &self.storage {
            WebSocketBinaryStorage::ArrayBuffer(buffer) => js_sys::Uint8Array::new(buffer).to_vec(),
            WebSocketBinaryStorage::Bytes(bytes) => bytes.clone(),
        }
    }

    pub(super) fn from_array_buffer(buffer: js_sys::ArrayBuffer) -> Self {
        Self {
            storage: WebSocketBinaryStorage::ArrayBuffer(buffer),
        }
    }

    pub(super) fn into_array_buffer(self) -> js_sys::ArrayBuffer {
        match self.storage {
            WebSocketBinaryStorage::ArrayBuffer(buffer) => buffer,
            WebSocketBinaryStorage::Bytes(bytes) => {
                js_sys::Uint8Array::from(bytes.as_slice()).buffer()
            }
        }
    }
}

impl From<Vec<u8>> for WebSocketBinaryFrame {
    fn from(bytes: Vec<u8>) -> Self {
        Self::from_bytes(bytes)
    }
}

impl fmt::Debug for WebSocketBinaryFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("WebSocketBinaryFrame")
            .field(&self.to_vec())
            .finish()
    }
}

impl PartialEq for WebSocketBinaryFrame {
    fn eq(&self, other: &Self) -> bool {
        self.to_vec() == other.to_vec()
    }
}

impl Eq for WebSocketBinaryFrame {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebSocketMessage {
    Text(String),
    Binary(WebSocketBinaryFrame),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebSocketEvent {
    Opened,
    Message(WebSocketMessage),
    Closed { code: u16, reason: String },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSocketTransportError {
    pub message: String,
}

impl fmt::Display for WebSocketTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WebSocketTransportError {}

pub trait WebSocketConnection {
    /// Send one logical frame without applying daemon routing or credentials.
    fn send(&self, message: WebSocketMessage) -> Result<(), WebSocketTransportError>;

    /// Begin closing the connection. Implementations should tolerate repeats.
    fn close(&self) -> Result<(), WebSocketTransportError>;
}

pub type WebSocketEventHandler = Rc<dyn Fn(WebSocketEvent)>;

pub trait WebSocketTransport {
    /// Open a connection for a daemon-relative path and deliver its events.
    ///
    /// Event delivery may begin before this method returns. The UI buffers
    /// those events until it owns the returned connection.
    fn connect(
        &self,
        request: WebSocketConnectRequest,
        events: WebSocketEventHandler,
    ) -> Result<Rc<dyn WebSocketConnection>, WebSocketTransportError>;
}

#[derive(Default)]
struct WebSocketTransportState {
    provider: Option<Rc<dyn WebSocketTransport>>,
    used: bool,
}

thread_local! {
    static WEB_SOCKET_TRANSPORT: RefCell<WebSocketTransportState> =
        RefCell::new(WebSocketTransportState::default());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSocketTransportInstallError {
    AlreadyInstalled,
    AlreadyUsed,
}

impl fmt::Display for WebSocketTransportInstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInstalled => {
                formatter.write_str("a WebSocket transport is already installed")
            }
            Self::AlreadyUsed => {
                formatter.write_str("the WebSocket transport has already been used")
            }
        }
    }
}

impl std::error::Error for WebSocketTransportInstallError {}

pub fn install_websocket_transport(
    transport: Rc<dyn WebSocketTransport>,
) -> Result<(), WebSocketTransportInstallError> {
    WEB_SOCKET_TRANSPORT.with_borrow_mut(|state| {
        if state.used {
            return Err(WebSocketTransportInstallError::AlreadyUsed);
        }
        if state.provider.is_some() {
            return Err(WebSocketTransportInstallError::AlreadyInstalled);
        }
        state.provider = Some(transport);
        Ok(())
    })
}

pub(super) fn connect(
    request: WebSocketConnectRequest,
    events: WebSocketEventHandler,
) -> Result<Rc<dyn WebSocketConnection>, WebSocketTransportError> {
    validate_relative_path(&request.path)?;
    let provider = WEB_SOCKET_TRANSPORT.with_borrow_mut(|state| {
        state.used = true;
        state
            .provider
            .clone()
            .unwrap_or_else(|| Rc::new(BrowserWebSocketTransport))
    });
    provider.connect(request, events)
}

pub(super) fn send_json(
    connection: &dyn WebSocketConnection,
    message: &serde_json::Value,
) -> Result<(), WebSocketTransportError> {
    connection.send(WebSocketMessage::Text(message.to_string()))
}

fn validate_relative_path(path: &str) -> Result<(), WebSocketTransportError> {
    if path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
    {
        return Ok(());
    }
    Err(WebSocketTransportError {
        message: "authenticated daemon WebSocket URLs must be relative".to_owned(),
    })
}

struct BrowserWebSocketTransport;

impl WebSocketTransport for BrowserWebSocketTransport {
    fn connect(
        &self,
        request: WebSocketConnectRequest,
        events: WebSocketEventHandler,
    ) -> Result<Rc<dyn WebSocketConnection>, WebSocketTransportError> {
        let url = build_ws_url(&request.path).ok_or_else(|| WebSocketTransportError {
            message: "verified daemon WebSocket route is unavailable".to_owned(),
        })?;
        let socket = arraybuffer_websocket(&url, &request.protocol).map_err(|error| {
            WebSocketTransportError {
                message: error.to_string(),
            }
        })?;
        let connection = Rc::new(BrowserWebSocketConnection {
            socket,
            handlers: RefCell::new(None),
        });

        let opened_events = Rc::clone(&events);
        let closed_events = Rc::clone(&events);
        let error_events = Rc::clone(&events);
        let message_events = events;
        let handlers = WebSocketEventHandlers::attach(
            &connection.socket,
            move |_| opened_events(WebSocketEvent::Opened),
            move |event| {
                closed_events(WebSocketEvent::Closed {
                    code: event.code(),
                    reason: event.reason(),
                });
            },
            move |_| {
                error_events(WebSocketEvent::Error {
                    message: "browser WebSocket transport error".to_owned(),
                });
            },
            move |event| {
                if let Some(buffer) = message_array_buffer(&event) {
                    message_events(WebSocketEvent::Message(WebSocketMessage::Binary(
                        WebSocketBinaryFrame::from_array_buffer(buffer),
                    )));
                } else if let Some(text) = event.data().as_string() {
                    message_events(WebSocketEvent::Message(WebSocketMessage::Text(text)));
                }
            },
        );
        connection.handlers.replace(Some(handlers));
        Ok(connection)
    }
}

struct BrowserWebSocketConnection {
    socket: web_sys::WebSocket,
    handlers: RefCell<Option<WebSocketEventHandlers>>,
}

impl BrowserWebSocketConnection {
    fn detach(&self) {
        if let Some(handlers) = self.handlers.borrow_mut().take() {
            handlers.detach_from(&self.socket);
        }
    }
}

impl WebSocketConnection for BrowserWebSocketConnection {
    fn send(&self, message: WebSocketMessage) -> Result<(), WebSocketTransportError> {
        let result = match message {
            WebSocketMessage::Text(text) => self.socket.send_with_str(&text),
            WebSocketMessage::Binary(frame) => self
                .socket
                .send_with_array_buffer(&frame.into_array_buffer()),
        };
        result.map_err(|error| WebSocketTransportError {
            message: js_error_message(&error),
        })
    }

    fn close(&self) -> Result<(), WebSocketTransportError> {
        self.socket
            .close()
            .map_err(|error| WebSocketTransportError {
                message: js_error_message(&error),
            })
    }
}

impl Drop for BrowserWebSocketConnection {
    fn drop(&mut self) {
        self.detach();
    }
}

fn build_ws_url(path: &str) -> Option<String> {
    let routed = client::daemon_url(path)?;
    let native_base = native_websocket_url(&routed);
    let location = current_page_location();
    let protocol = location.websocket_protocol();
    let host = if cfg!(debug_assertions) {
        format!("{}:9420", location.hostname)
    } else {
        location.host()
    };
    let base = native_base.unwrap_or_else(|| format!("{protocol}//{host}{path}"));
    Some(authenticated_websocket_url(
        base,
        client::authorization_token().as_deref(),
    ))
}

fn authenticated_websocket_url(base: String, token: Option<&str>) -> String {
    token.map_or(base.clone(), |token| {
        format!("{base}?token={}", percent_encode(token))
    })
}

fn native_websocket_url(routed: &str) -> Option<String> {
    routed
        .strip_prefix("https://")
        .map(|rest| format!("wss://{rest}"))
        .or_else(|| {
            routed
                .strip_prefix("http://")
                .map(|rest| format!("ws://{rest}"))
        })
}

fn percent_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            encoded.push(char::from(byte));
        } else {
            let _ = fmt::Write::write_fmt(&mut encoded, format_args!("%{byte:02X}"));
        }
    }
    encoded
}

fn js_error_message(error: &JsValue) -> String {
    error
        .as_string()
        .or_else(|| js_sys::JSON::stringify(error).ok()?.as_string())
        .unwrap_or_else(|| "unknown JavaScript error".to_owned())
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use super::{
        WebSocketConnectRequest, WebSocketConnection, WebSocketEvent, WebSocketEventHandler,
        WebSocketMessage, WebSocketTransport, WebSocketTransportError,
        WebSocketTransportInstallError,
    };

    #[derive(Default)]
    struct FakeConnection {
        messages: RefCell<Vec<WebSocketMessage>>,
        close_count: Cell<u32>,
    }

    impl WebSocketConnection for FakeConnection {
        fn send(&self, message: WebSocketMessage) -> Result<(), WebSocketTransportError> {
            self.messages.borrow_mut().push(message);
            Ok(())
        }

        fn close(&self) -> Result<(), WebSocketTransportError> {
            self.close_count.set(self.close_count.get() + 1);
            Ok(())
        }
    }

    struct FakeTransport {
        requests: RefCell<Vec<WebSocketConnectRequest>>,
        handlers: RefCell<Vec<WebSocketEventHandler>>,
        connection: Rc<FakeConnection>,
    }

    impl WebSocketTransport for FakeTransport {
        fn connect(
            &self,
            request: WebSocketConnectRequest,
            events: WebSocketEventHandler,
        ) -> Result<Rc<dyn WebSocketConnection>, WebSocketTransportError> {
            self.requests.borrow_mut().push(request);
            self.handlers.borrow_mut().push(events);
            Ok(self.connection.clone())
        }
    }

    #[test]
    fn injected_transport_preserves_the_logical_websocket_contract() {
        let connection = Rc::new(FakeConnection::default());
        let transport = Rc::new(FakeTransport {
            requests: RefCell::new(Vec::new()),
            handlers: RefCell::new(Vec::new()),
            connection: Rc::clone(&connection),
        });
        let received = Rc::new(RefCell::new(Vec::new()));
        let received_for_handler = Rc::clone(&received);
        let events: WebSocketEventHandler = Rc::new(move |event| {
            received_for_handler.borrow_mut().push(event);
        });

        crate::api::client::install_verified_daemon_connection(
            "http://127.0.0.1:9420",
            Some("local-secret"),
        );
        super::install_websocket_transport(transport.clone()).expect("first install succeeds");
        assert_eq!(
            super::install_websocket_transport(transport.clone()),
            Err(WebSocketTransportInstallError::AlreadyInstalled)
        );

        let opened = super::connect(
            WebSocketConnectRequest {
                path: "/api/v1/ws".to_owned(),
                protocol: "hypercolor.v1".to_owned(),
            },
            events,
        )
        .expect("fake transport connects");
        assert_eq!(
            super::install_websocket_transport(transport.clone()),
            Err(WebSocketTransportInstallError::AlreadyUsed)
        );
        assert!(
            super::connect(
                WebSocketConnectRequest {
                    path: "https://attacker.example/steal".to_owned(),
                    protocol: "hypercolor.v1".to_owned(),
                },
                Rc::new(|_| {}),
            )
            .is_err()
        );
        assert!(
            super::connect(
                WebSocketConnectRequest {
                    path: "//attacker.example/steal".to_owned(),
                    protocol: "hypercolor.v1".to_owned(),
                },
                Rc::new(|_| {}),
            )
            .is_err()
        );
        for path in [
            "/\\attacker.example/steal",
            "/\n/attacker.example/steal",
            "/\t/attacker.example/steal",
        ] {
            assert!(
                super::connect(
                    WebSocketConnectRequest {
                        path: path.to_owned(),
                        protocol: "hypercolor.v1".to_owned(),
                    },
                    Rc::new(|_| {}),
                )
                .is_err()
            );
        }

        let requests = transport.requests.borrow();
        assert_eq!(
            requests.as_slice(),
            [WebSocketConnectRequest {
                path: "/api/v1/ws".to_owned(),
                protocol: "hypercolor.v1".to_owned(),
            }]
        );
        assert!(!requests[0].path.contains("local-secret"));
        drop(requests);

        let handler = transport.handlers.borrow()[0].clone();
        handler(WebSocketEvent::Opened);
        handler(WebSocketEvent::Message(WebSocketMessage::Text(
            r#"{"type":"hello"}"#.to_owned(),
        )));
        handler(WebSocketEvent::Message(WebSocketMessage::Binary(
            vec![0, 1, 2, 255].into(),
        )));
        handler(WebSocketEvent::Closed {
            code: 1000,
            reason: "complete".to_owned(),
        });
        assert_eq!(
            received.borrow().as_slice(),
            [
                WebSocketEvent::Opened,
                WebSocketEvent::Message(WebSocketMessage::Text(r#"{"type":"hello"}"#.to_owned())),
                WebSocketEvent::Message(WebSocketMessage::Binary(vec![0, 1, 2, 255].into())),
                WebSocketEvent::Closed {
                    code: 1000,
                    reason: "complete".to_owned(),
                },
            ]
        );

        super::send_json(opened.as_ref(), &serde_json::json!({ "type": "subscribe" }))
            .expect("JSON serializes to one text frame");
        opened
            .send(WebSocketMessage::Binary(vec![5, 4, 3].into()))
            .expect("binary send reaches the provider");
        opened.close().expect("close reaches the provider");
        assert_eq!(
            connection.messages.borrow().as_slice(),
            [
                WebSocketMessage::Text(r#"{"type":"subscribe"}"#.to_owned()),
                WebSocketMessage::Binary(vec![5, 4, 3].into()),
            ]
        );
        assert_eq!(connection.close_count.get(), 1);
        crate::api::client::reset_daemon_transport_for_test();
    }

    #[test]
    fn native_daemon_routes_convert_http_schemes_for_websocket() {
        assert_eq!(
            super::native_websocket_url("http://127.0.0.1:9420/api/v1/ws").as_deref(),
            Some("ws://127.0.0.1:9420/api/v1/ws")
        );
        assert_eq!(
            super::native_websocket_url("https://daemon.test/api/v1/ws").as_deref(),
            Some("wss://daemon.test/api/v1/ws")
        );
        assert!(super::native_websocket_url("/api/v1/ws").is_none());
    }

    #[test]
    fn every_connection_uses_the_current_verified_websocket_token() {
        let base = "ws://127.0.0.1:9420/api/v1/ws".to_owned();
        let first = super::authenticated_websocket_url(base.clone(), Some("session-one"));
        let rotated = super::authenticated_websocket_url(base, Some("session-two"));
        assert_eq!(first, "ws://127.0.0.1:9420/api/v1/ws?token=session-one");
        assert_eq!(rotated, "ws://127.0.0.1:9420/api/v1/ws?token=session-two");
        assert_ne!(first, rotated);
    }

    #[test]
    fn health_verified_native_base_enables_websocket_without_session_token() {
        crate::api::client::reset_daemon_transport_for_test();
        crate::api::client::begin_native_daemon_verification();
        assert!(crate::api::client::daemon_url("/api/v1/ws").is_none());

        crate::api::client::install_verified_daemon_connection("https://daemon.lan:19420", None);
        let routed = crate::api::client::daemon_url("/api/v1/ws")
            .expect("health-proven native route should exist");
        let websocket =
            super::native_websocket_url(&routed).expect("HTTPS daemon route should convert to WSS");
        assert_eq!(
            super::authenticated_websocket_url(websocket, None),
            "wss://daemon.lan:19420/api/v1/ws"
        );
        crate::api::client::reset_daemon_transport_for_test();
    }
}
