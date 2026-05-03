//! Shared modules for the unified Hypercolor desktop app.

pub const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:9420";

pub mod cli;
pub mod daemon_client;
pub mod logging;
pub mod state;
pub mod supervisor;
pub mod tray;
pub mod window;
