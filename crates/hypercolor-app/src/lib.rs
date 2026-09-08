//! Shared modules for the unified Hypercolor desktop app.

pub const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:9420";

/// Environment override for the daemon the app shell talks to.
pub const DAEMON_URL_ENV: &str = "HYPERCOLOR_URL";

/// The daemon base URL for this app process: `HYPERCOLOR_URL` when set,
/// otherwise [`DEFAULT_DAEMON_URL`].
#[must_use]
pub fn daemon_base_url() -> String {
    std::env::var(DAEMON_URL_ENV).unwrap_or_else(|_| DEFAULT_DAEMON_URL.to_string())
}

pub mod cli;
pub mod daemon_client;
pub mod diagnostics;
pub mod first_run;
pub mod helper_client;
pub mod linux_webkit;
pub mod logging;
pub mod ownership;
pub mod process_ext;
pub mod state;
pub mod supervisor;
pub mod support;
pub mod tray;
pub mod window;
