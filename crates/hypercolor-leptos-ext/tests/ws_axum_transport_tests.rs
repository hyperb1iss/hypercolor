#![cfg(feature = "axum")]

use hypercolor_leptos_ext::ws::transport::{AxumWebSocketTransport, CinderTransport};

fn assert_transport<T: CinderTransport>() {}

#[test]
fn axum_websocket_transport_implements_transport_trait() {
    assert_transport::<AxumWebSocketTransport>();
}
