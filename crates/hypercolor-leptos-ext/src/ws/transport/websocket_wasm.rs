use async_trait::async_trait;
use bytes::Bytes;
use futures_channel::{mpsc, oneshot};
use futures_util::StreamExt;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::task::{Context, Poll};
use thiserror::Error;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket};

use super::CinderTransport;

type ConnectSender = Rc<RefCell<Option<oneshot::Sender<Result<(), WebSocketTransportError>>>>>;

pub struct WebSocketTransport {
    ws: WebSocket,
    recv_rx: mpsc::UnboundedReceiver<Result<Bytes, WebSocketTransportError>>,
    callbacks: WebSocketEventHandlers,
    state: Rc<Cell<WebSocketTransportState>>,
}

impl WebSocketTransport {
    pub async fn connect(url: impl Into<String>) -> Result<Self, WebSocketTransportError> {
        Self::connect_with_protocols(url, &[]).await
    }

    pub async fn connect_with_protocols(
        url: impl Into<String>,
        protocols: &[&str],
    ) -> Result<Self, WebSocketTransportError> {
        let url = url.into();
        let ws = create_websocket(&url, protocols)?;
        ws.set_binary_type(BinaryType::Arraybuffer);

        let (recv_tx, recv_rx) = mpsc::unbounded();
        let (connect_tx, connect_rx) = oneshot::channel();
        let connect_tx = Rc::new(RefCell::new(Some(connect_tx)));
        let state = Rc::new(Cell::new(WebSocketTransportState::Connecting));

        let callbacks = install_callbacks(&ws, recv_tx, Rc::clone(&connect_tx), Rc::clone(&state));

        match connect_rx.await {
            Ok(Ok(())) => Ok(Self {
                ws,
                recv_rx,
                callbacks,
                state,
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(WebSocketTransportError::Connect {
                message: "websocket connection callback was dropped".to_owned(),
            }),
        }
    }

    #[must_use]
    pub fn inner(&self) -> &WebSocket {
        &self.ws
    }

    #[must_use]
    pub fn state(&self) -> WebSocketTransportState {
        match self.ws.ready_state() {
            WebSocket::CONNECTING => WebSocketTransportState::Connecting,
            WebSocket::OPEN => WebSocketTransportState::Open,
            WebSocket::CLOSING => WebSocketTransportState::Closing,
            WebSocket::CLOSED => WebSocketTransportState::Closed,
            _ => self.state.get(),
        }
    }

    #[must_use]
    pub fn into_inner(self) -> WebSocket {
        self.ws.clone()
    }
}

impl Drop for WebSocketTransport {
    fn drop(&mut self) {
        self.callbacks.detach_from(&self.ws);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSocketTransportState {
    Connecting,
    Open,
    Closing,
    Closed,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WebSocketTransportError {
    #[error("failed to connect websocket: {message}")]
    Connect { message: String },
    #[error("websocket is not open")]
    NotOpen,
    #[error("failed to send websocket frame: {message}")]
    Send { message: String },
    #[error("websocket reported an error event")]
    ErrorEvent,
    #[error("websocket message payload is not binary")]
    NonBinaryMessage,
}

pub struct WebSocketEventHandlers {
    _on_open: Closure<dyn FnMut(Event)>,
    _on_close: Closure<dyn FnMut(CloseEvent)>,
    _on_error: Closure<dyn FnMut(Event)>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
}

impl WebSocketEventHandlers {
    #[must_use]
    pub fn attach<OnOpen, OnClose, OnError, OnMessage>(
        ws: &WebSocket,
        on_open: OnOpen,
        on_close: OnClose,
        on_error: OnError,
        on_message: OnMessage,
    ) -> Self
    where
        OnOpen: FnMut(Event) + 'static,
        OnClose: FnMut(CloseEvent) + 'static,
        OnError: FnMut(Event) + 'static,
        OnMessage: FnMut(MessageEvent) + 'static,
    {
        let on_open = Closure::<dyn FnMut(Event)>::new(on_open);
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));

        let on_close = Closure::<dyn FnMut(CloseEvent)>::new(on_close);
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));

        let on_error = Closure::<dyn FnMut(Event)>::new(on_error);
        ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));

        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(on_message);
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

        Self {
            _on_open: on_open,
            _on_close: on_close,
            _on_error: on_error,
            _on_message: on_message,
        }
    }

    pub fn detach_from(&self, ws: &WebSocket) {
        ws.set_onopen(None);
        ws.set_onclose(None);
        ws.set_onerror(None);
        ws.set_onmessage(None);
    }
}

pub fn message_array_buffer(event: &MessageEvent) -> Option<js_sys::ArrayBuffer> {
    event.data().dyn_into().ok()
}

#[async_trait(?Send)]
impl CinderTransport for WebSocketTransport {
    type SendError = WebSocketTransportError;
    type RecvError = WebSocketTransportError;

    async fn send(&mut self, frame: Bytes) -> Result<(), Self::SendError> {
        if self.state() != WebSocketTransportState::Open {
            return Err(WebSocketTransportError::NotOpen);
        }

        self.ws
            .send_with_u8_array(&frame)
            .map_err(|error| WebSocketTransportError::Send {
                message: js_error_message(&error),
            })
    }

    async fn recv(&mut self) -> Result<Option<Bytes>, Self::RecvError> {
        match self.recv_rx.next().await {
            Some(Ok(frame)) => Ok(Some(frame)),
            Some(Err(error)) => Err(error),
            None => Ok(None),
        }
    }

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::SendError>> {
        if self.state() == WebSocketTransportState::Open {
            Poll::Ready(Ok(()))
        } else {
            Poll::Ready(Err(WebSocketTransportError::NotOpen))
        }
    }

    async fn close(&mut self) -> Result<(), Self::SendError> {
        self.state.set(WebSocketTransportState::Closing);
        self.ws
            .close()
            .map_err(|error| WebSocketTransportError::Send {
                message: js_error_message(&error),
            })
    }
}

fn create_websocket(url: &str, protocols: &[&str]) -> Result<WebSocket, WebSocketTransportError> {
    let result = match protocols {
        [] => WebSocket::new(url),
        [protocol] => WebSocket::new_with_str(url, protocol),
        protocols => {
            let array = js_sys::Array::new();
            for protocol in protocols {
                array.push(&JsValue::from_str(protocol));
            }
            WebSocket::new_with_str_sequence(url, &array.into())
        }
    };

    result.map_err(|error| WebSocketTransportError::Connect {
        message: js_error_message(&error),
    })
}

fn install_callbacks(
    ws: &WebSocket,
    recv_tx: mpsc::UnboundedSender<Result<Bytes, WebSocketTransportError>>,
    connect_tx: ConnectSender,
    state: Rc<Cell<WebSocketTransportState>>,
) -> WebSocketEventHandlers {
    let open_state = Rc::clone(&state);
    let open_connect_tx = Rc::clone(&connect_tx);
    let on_open = move |_| {
        open_state.set(WebSocketTransportState::Open);
        if let Some(sender) = open_connect_tx.borrow_mut().take() {
            let _ = sender.send(Ok(()));
        }
    };

    let close_state = Rc::clone(&state);
    let close_connect_tx = Rc::clone(&connect_tx);
    let close_recv_tx = recv_tx.clone();
    let on_close = move |_| {
        close_state.set(WebSocketTransportState::Closed);
        if let Some(sender) = close_connect_tx.borrow_mut().take() {
            let _ = sender.send(Err(WebSocketTransportError::Connect {
                message: "websocket closed before opening".to_owned(),
            }));
        }
        close_recv_tx.close_channel();
    };

    let error_connect_tx = Rc::clone(&connect_tx);
    let error_recv_tx = recv_tx.clone();
    let on_error = move |_| {
        if let Some(sender) = error_connect_tx.borrow_mut().take() {
            let _ = sender.send(Err(WebSocketTransportError::Connect {
                message: "websocket error before opening".to_owned(),
            }));
        } else {
            let _ = error_recv_tx.unbounded_send(Err(WebSocketTransportError::ErrorEvent));
        }
    };

    let message_recv_tx = recv_tx;
    let on_message = move |event: MessageEvent| {
        let data = event.data();
        if data.is_instance_of::<js_sys::ArrayBuffer>() {
            let buffer = data.unchecked_into::<js_sys::ArrayBuffer>();
            let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
            let _ = message_recv_tx.unbounded_send(Ok(Bytes::from(bytes)));
        } else if data.is_instance_of::<js_sys::Uint8Array>() {
            let bytes = data.unchecked_into::<js_sys::Uint8Array>().to_vec();
            let _ = message_recv_tx.unbounded_send(Ok(Bytes::from(bytes)));
        } else {
            let _ = message_recv_tx.unbounded_send(Err(WebSocketTransportError::NonBinaryMessage));
        }
    };

    WebSocketEventHandlers::attach(ws, on_open, on_close, on_error, on_message)
}

fn js_error_message(error: &JsValue) -> String {
    error
        .as_string()
        .or_else(|| js_sys::JSON::stringify(error).ok()?.as_string())
        .unwrap_or_else(|| "unknown JavaScript error".to_owned())
}
