//! `hypercolor-windows-helper` — single signed elevated helper for all
//! Hypercolor Windows privileged operations.
//!
//! The request-file authorization protocol, threat model, and verb allowlist
//! are specified in the Windows experience execution roadmap §7.4
//! (`hypercolor.lighting/docs/internal/windows-experience-roadmap.md`).
//!
//! This crate is **only buildable on Windows**. On other platforms `main`
//! prints a diagnostic and exits non-zero so build tooling that compiles the
//! whole workspace still produces a binary, but accidental execution makes
//! the platform mismatch obvious.

#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use std::process::ExitCode;

#[cfg(target_os = "windows")]
mod cli;
#[cfg(target_os = "windows")]
mod request;
#[cfg(target_os = "windows")]
mod verbs;

#[cfg(target_os = "windows")]
fn main() -> ExitCode {
    init_logging();
    match cli::parse() {
        Ok(invocation) => match run(invocation) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => emit_error(&err),
        },
        Err(err) => emit_error(&HelperError::new("cli_invalid", err.to_string())),
    }
}

#[cfg(not(target_os = "windows"))]
fn main() -> ExitCode {
    eprintln!(
        "{{\"verb\":\"<none>\",\"error_kind\":\"platform_unsupported\",\"detail\":\"hypercolor-windows-helper only runs on Windows\"}}"
    );
    ExitCode::from(2)
}

#[cfg(target_os = "windows")]
fn init_logging() {
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("hypercolor_windows_helper=info,warn"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .without_time()
        .try_init();
}

#[cfg(target_os = "windows")]
fn run(invocation: cli::Invocation) -> Result<(), HelperError> {
    let request = request::load_and_validate(&invocation.request_file_path)
        .map_err(|err| HelperError::new("request_invalid", err.to_string()))?;

    tracing::info!(verb = %request.verb, nonce = request.nonce, "helper invocation accepted");
    verbs::dispatch(&request).map_err(|err| HelperError::new(err.kind(), err.to_string()))
}

#[cfg(target_os = "windows")]
fn emit_error(err: &HelperError) -> ExitCode {
    let payload = serde_json::json!({
        "verb": err.verb.as_deref().unwrap_or("<unknown>"),
        "error_kind": err.kind,
        "detail": err.detail,
    });
    eprintln!("{payload}");
    tracing::error!(kind = %err.kind, detail = %err.detail, "helper invocation failed");
    ExitCode::from(1)
}

#[cfg(not(target_os = "windows"))]
fn emit_error(_err: &HelperError) -> ExitCode {
    ExitCode::from(1)
}

/// Structured helper failure used to populate the stderr JSON envelope the
/// parent app parses (see §8 Hardware Setup Failure Modes in the roadmap).
#[derive(Debug)]
struct HelperError {
    verb: Option<String>,
    kind: String,
    detail: String,
}

impl HelperError {
    fn new(kind: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            verb: None,
            kind: kind.into(),
            detail: detail.into(),
        }
    }
}
