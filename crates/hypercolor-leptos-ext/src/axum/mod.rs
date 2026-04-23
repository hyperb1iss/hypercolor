use ::axum::extract::ws::{WebSocket, WebSocketUpgrade};
use ::axum::response::Response;
use std::future::Future;

pub use crate::ws::HYPERCOLOR_WS_PROTOCOL;

pub fn upgrade_handler<F, Fut>(ws: WebSocketUpgrade, on_connect: F) -> Response
where
    F: FnOnce(WebSocket) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    ws.protocols([HYPERCOLOR_WS_PROTOCOL])
        .on_upgrade(on_connect)
}
