//! Logging setup for the unified desktop app.

use std::{io::IsTerminal, path::PathBuf};

use anyhow::{Context, Result};
use hypercolor_core::config::paths::data_dir;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

const DEFAULT_FILTER: &str = "hypercolor_app=debug,tauri=info,wry=warn";

/// Prefix used for app log files.
pub const LOG_FILE_PREFIX: &str = "hypercolor-app.log";

/// Guard that keeps the non-blocking file logger alive.
#[must_use]
pub struct LogGuard {
    _file_guard: WorkerGuard,
}

/// Resolve the app log directory.
#[must_use]
pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Install stderr and rolling-file tracing subscribers.
pub fn init() -> Result<LogGuard> {
    let log_dir = log_dir();
    std::fs::create_dir_all(&log_dir)
        .with_context(|| format!("failed to create app log directory {}", log_dir.display()))?;

    let file_appender = tracing_appender::rolling::daily(&log_dir, LOG_FILE_PREFIX);
    let (file_writer, file_guard) = tracing_appender::non_blocking(file_appender);
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(stderr_supports_ansi()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(file_writer)
                .with_ansi(false),
        )
        .try_init()
        .context("failed to install app tracing subscriber")?;

    Ok(LogGuard {
        _file_guard: file_guard,
    })
}

fn stderr_supports_ansi() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal()
}
