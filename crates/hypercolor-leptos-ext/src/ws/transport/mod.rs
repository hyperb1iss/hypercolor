#[cfg(feature = "ws-client-wasm")]
mod websocket_wasm;

#[cfg(feature = "ws-client-wasm")]
pub use websocket_wasm::{
    WebSocketEventHandlers, WebSocketTransportError, arraybuffer_websocket, message_array_buffer,
    send_websocket_json, send_websocket_text,
};
