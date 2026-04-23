#![cfg(feature = "axum")]

use hypercolor_leptos_ext::axum::HYPERCOLOR_WS_PROTOCOL;

#[test]
fn hypercolor_ws_protocol_matches_daemon_subprotocol() {
    assert_eq!(HYPERCOLOR_WS_PROTOCOL, "hypercolor-v1");
}
