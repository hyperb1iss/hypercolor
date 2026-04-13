//! WebSocket handler — `/api/v1/ws`.
//!
//! Real-time event stream, binary frame data, and bidirectional commands.
//! Each connected client gets its own broadcast subscription with configurable
//! channel filtering. Backpressure is handled by bounded channels — slow
//! consumers get dropped frames rather than unbounded memory growth.

mod cache;
mod command;
mod preview_encode;
mod preview_scale;
mod protocol;
mod relays;
mod session;

#[cfg(test)]
mod tests;

pub(crate) use session::ws_handler;
