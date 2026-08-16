//! WebSocket handler — `/api/v1/ws`.
//!
//! Real-time event stream, binary frame data, and bidirectional commands.
//! Each connected client gets its own broadcast subscription with configurable
//! channel filtering. Backpressure is handled by bounded channels — slow
//! consumers get dropped frames rather than unbounded memory growth.

mod cache;
mod command;
mod interactive_preview_relay;
mod preview_encode;
mod preview_scale;
mod protocol;
mod relays;
mod session;

#[cfg(test)]
mod tests;

pub(crate) use protocol::MAX_WS_MESSAGE_BYTES;
pub(crate) use session::{spawn_trusted_local_socket, ws_handler};

/// Daemon-owned binary encoders whose byte layouts are frozen by the golden
/// fixtures in `tests/ws_golden_tests.rs`.
///
/// The preview family (`0x03`–`0x11`) and the RPC pair (`0x80`/`0x81`) already
/// encode through public `hypercolor-leptos-ext` codecs; the `frames` (`0x01`)
/// and `spectrum` (`0x02`) tags are written here instead, so they are exported
/// for the fixture suite to assert against the production encoder rather than a
/// copy of it.
pub mod wire {
    pub use super::cache::{encode_frame_binary_selected, encode_spectrum_binary};
    pub use super::protocol::FrameZoneSelection;
}
