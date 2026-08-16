use thiserror::Error;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket};

pub fn arraybuffer_websocket(
    url: &str,
    protocol: &str,
) -> Result<WebSocket, WebSocketTransportError> {
    let ws = create_websocket(url, &[protocol])?;
    ws.set_binary_type(BinaryType::Arraybuffer);
    Ok(ws)
}

pub fn send_websocket_text(ws: &WebSocket, message: &str) -> Result<(), WebSocketTransportError> {
    ws.send_with_str(message)
        .map_err(|error| WebSocketTransportError::Send {
            message: js_error_message(&error),
        })
}

pub fn send_websocket_json(
    ws: &WebSocket,
    message: &serde_json::Value,
) -> Result<(), WebSocketTransportError> {
    send_websocket_text(ws, &message.to_string())
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WebSocketTransportError {
    #[error("failed to connect websocket: {message}")]
    Connect { message: String },
    #[error("failed to send websocket frame: {message}")]
    Send { message: String },
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

fn js_error_message(error: &JsValue) -> String {
    error
        .as_string()
        .or_else(|| js_sys::JSON::stringify(error).ok()?.as_string())
        .unwrap_or_else(|| "unknown JavaScript error".to_owned())
}
