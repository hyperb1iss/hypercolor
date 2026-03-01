//! Output formatting for the Hypercolor CLI.
//!
//! Supports three output modes: styled tables for humans, raw JSON for
//! machine consumption, and plain text for piping.

use std::io::Write;

use clap::ValueEnum;

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
#[derive(Debug, Clone)]
pub struct OutputContext {
    /// Which format to render.
    pub format: OutputFormat,
    /// Whether to suppress non-essential output.
    pub quiet: bool,
    /// Whether ANSI color codes are enabled.
    pub color: bool,
}

impl OutputContext {
    /// Create a new output context from CLI flags.
    #[must_use]
    pub fn new(format: OutputFormat, json: bool, quiet: bool, no_color: bool) -> Self {
        // --json flag overrides --format
        let format = if json { OutputFormat::Json } else { format };

        // Color is enabled unless explicitly disabled or not a TTY
        let color = !no_color && !is_no_color_env() && atty_stdout();

        Self {
            format,
            quiet,
            color,
        }
    }

    /// Print a success message (suppressed in quiet mode).
    pub fn success(&self, msg: &str) {
        if self.quiet {
            return;
        }
        if self.color {
            println!("  \x1b[38;2;80;250;123m\u{2726}\x1b[0m {msg}");
        } else {
            println!("  * {msg}");
        }
    }

    /// Print an error message (never suppressed).
    pub fn error(&self, msg: &str) {
        if self.color {
            eprintln!("  \x1b[38;2;255;99;99m\u{2717}\x1b[0m {msg}");
        } else {
            eprintln!("  ERROR: {msg}");
        }
    }

    /// Print a warning message (suppressed in quiet mode).
    pub fn warning(&self, msg: &str) {
        if self.quiet {
            return;
        }
        if self.color {
            eprintln!("  \x1b[38;2;241;250;140m!\x1b[0m {msg}");
        } else {
            eprintln!("  WARNING: {msg}");
        }
    }

    /// Print an info line (suppressed in quiet mode).
    pub fn info(&self, msg: &str) {
        if self.quiet {
            return;
        }
        println!("  {msg}");
    }

    /// Print JSON output to stdout.
    ///
    /// The `&self` receiver is kept for API consistency with other output
    /// methods, even though the current implementation doesn't use it.
    #[expect(clippy::unused_self)]
    pub fn print_json(&self, value: &serde_json::Value) -> anyhow::Result<()> {
        let output = serde_json::to_string_pretty(value)?;
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{output}")?;
        Ok(())
    }

    /// Print a simple table with headers and rows.
    ///
    /// Each row is a slice of column values. The formatter auto-aligns
    /// columns based on the widest value in each column.
    pub fn print_table(&self, headers: &[&str], rows: &[Vec<String>]) {
        if rows.is_empty() && self.quiet {
            return;
        }

        // Calculate column widths
        let col_count = headers.len();
        let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
        for row in rows {
            for (i, cell) in row.iter().enumerate() {
                if i < col_count {
                    widths[i] = widths[i].max(cell.len());
                }
            }
        }

        // Print header
        let header_line: String = headers
            .iter()
            .enumerate()
            .map(|(i, h)| format!("{h:<width$}", width = widths[i]))
            .collect::<Vec<_>>()
            .join("  ");
        println!("  {header_line}");

        // Print separator
        let sep_width: usize = widths.iter().sum::<usize>() + (col_count.saturating_sub(1)) * 2;
        let separator = "\u{2500}".repeat(sep_width);
        println!("  {separator}");

        // Print rows
        for row in rows {
            let line: String = row
                .iter()
                .enumerate()
                .map(|(i, cell)| {
                    let w = widths.get(i).copied().unwrap_or(cell.len());
                    format!("{cell:<w$}")
                })
                .collect::<Vec<_>>()
                .join("  ");
            println!("  {line}");
        }
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
    s.replace(' ', "%20")
}

/// Check if the `NO_COLOR` environment variable is set.
fn is_no_color_env() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

/// Check if stdout is a TTY. Returns `false` when piped.
fn atty_stdout() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdout())
}
