//! Output formatting for the Hypercolor CLI.
//!
//! Supports three output modes: styled tables for humans, raw JSON for
//! machine consumption, and plain text for piping.

pub mod painter;
mod table;

use std::fmt::Write as _;
use std::io::Write;

use clap::ValueEnum;

pub use painter::Painter;

// ── OutputFormat ─────────────────────────────────────────────────────────

/// Output format for CLI commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Styled, aligned table output (default).
    Table,
    /// Machine-readable JSON.
    Json,
    /// Plain text, one value per line, no decoration.
    Plain,
}

// ── OutputContext ─────────────────────────────────────────────────────────

/// Shared formatting state threaded through all command handlers.
#[derive(Debug)]
pub struct OutputContext {
    /// Which format to render.
    pub format: OutputFormat,
    /// Whether to suppress non-essential output.
    pub quiet: bool,
    /// Semantic colorizer backed by the active opaline theme.
    pub painter: Painter,
}

impl OutputContext {
    /// Create a new output context from CLI flags.
    #[must_use]
    pub fn new(
        format: OutputFormat,
        json: bool,
        quiet: bool,
        no_color: bool,
        theme: Option<&str>,
    ) -> Self {
        let format = if json { OutputFormat::Json } else { format };

        let color_enabled = if no_color || is_no_color_env() {
            false
        } else {
            match hypercolor_color_env().as_deref() {
                Some("always") => true,
                Some("never") => false,
                _ => cli_color_force() || atty_stdout(),
            }
        };

        Self {
            format,
            quiet,
            painter: Painter::new(theme, color_enabled),
        }
    }

    /// Print a success message (suppressed in quiet mode).
    pub fn success(&self, msg: &str) {
        if self.quiet {
            return;
        }
        println!("  {} {msg}", self.painter.success_icon());
    }

    /// Print an error message (never suppressed).
    pub fn error(&self, msg: &str) {
        eprintln!("  {} {msg}", self.painter.error_icon());
    }

    /// Print a warning message (suppressed in quiet mode).
    pub fn warning(&self, msg: &str) {
        if self.quiet {
            return;
        }
        eprintln!("  {} {msg}", self.painter.warning_icon());
    }

    /// Print an info line (suppressed in quiet mode).
    pub fn info(&self, msg: &str) {
        if self.quiet {
            return;
        }
        println!("  {msg}");
    }

    /// Print JSON output to stdout.
    #[expect(clippy::unused_self)]
    pub fn print_json(&self, value: &serde_json::Value) -> anyhow::Result<()> {
        let output = serde_json::to_string_pretty(value)?;
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{output}")?;
        Ok(())
    }

    /// Print a simple table with headers and rows.
    pub fn print_table(&self, headers: &[&str], rows: &[Vec<String>]) {
        table::print_table(headers, rows, self.quiet, &self.painter);
    }
}

// ── Shared Helpers ──────────────────────────────────────────────────────

/// Extract a string field from a JSON value, returning "?" if missing.
pub fn extract_str(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?")
        .to_string()
}

/// Simple percent-encoding for URL path segments.
pub fn urlencoded(s: &str) -> String {
    percent_encode_component(s)
}

fn is_no_color_env() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

fn hypercolor_color_env() -> Option<String> {
    std::env::var("HYPERCOLOR_COLOR")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
}

fn atty_stdout() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdout())
}

fn cli_color_force() -> bool {
    std::env::var_os("CLICOLOR_FORCE").is_some_and(|v| v != "0")
}

fn percent_encode_component(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}
