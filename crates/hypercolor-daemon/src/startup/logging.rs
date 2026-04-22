//! SilkCircuit-tinted tracing formatter for the daemon console.
//!
//! Replaces the `tracing_subscriber` default palette with the app's
//! electric neon scheme — one color per level, coral module paths,
//! dimmed field keys, and timestamps in electric yellow. Pure ANSI
//! 24-bit truecolor; disables itself automatically when stdout isn't
//! a TTY so piped/redirected logs stay plain.
//!
//! Matches the palette in `~/dev/conventions/shared/STYLE_GUIDE.md` and
//! the terminal output conventions in `CLAUDE.md`.

use std::fmt;
use std::io::IsTerminal;
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::{Event, Level, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

// ── SilkCircuit palette — 24-bit ANSI ─────────────────────────────────────

#[allow(
    dead_code,
    reason = "full palette kept on-hand so adjacent features can reach for SilkCircuit tokens without re-deriving them"
)]
mod color {
    pub const ELECTRIC_PURPLE: &str = "\x1b[38;2;225;53;255m";
    pub const NEON_CYAN: &str = "\x1b[38;2;128;255;234m";
    pub const CORAL: &str = "\x1b[38;2;255;106;193m";
    pub const ELECTRIC_YELLOW: &str = "\x1b[38;2;241;250;140m";
    pub const SUCCESS_GREEN: &str = "\x1b[38;2;80;250;123m";
    pub const ERROR_RED: &str = "\x1b[38;2;255;99;99m";
    pub const MUTED: &str = "\x1b[38;2;139;133;160m";
    pub const BOLD: &str = "\x1b[1m";
    pub const DIM: &str = "\x1b[2m";
    pub const RESET: &str = "\x1b[0m";
}

/// `(color, 5-char label)` — fixed-width so adjacent lines line up.
fn level_style(level: Level) -> (&'static str, &'static str) {
    match level {
        Level::ERROR => (color::ERROR_RED, "ERROR"),
        Level::WARN => (color::ELECTRIC_YELLOW, " WARN"),
        Level::INFO => (color::SUCCESS_GREEN, " INFO"),
        Level::DEBUG => (color::NEON_CYAN, "DEBUG"),
        Level::TRACE => (color::ELECTRIC_PURPLE, "TRACE"),
    }
}

/// Wall-clock HH:MM:SS.mmm — short enough to keep log lines compact.
fn write_timestamp(writer: &mut Writer<'_>, ansi: bool) -> fmt::Result {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Local-time offset detection would pull in chrono; local machine
    // time is close enough for developer logs, so we render the current
    // wall-clock seconds-of-day modulo 24h plus milliseconds.
    let secs = elapsed.as_secs();
    let millis = elapsed.subsec_millis();
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    if ansi {
        write!(
            writer,
            "{}{h:02}:{m:02}:{s:02}{}{}.{millis:03}{} ",
            color::ELECTRIC_YELLOW,
            color::DIM,
            color::ELECTRIC_YELLOW,
            color::RESET
        )
    } else {
        write!(writer, "{h:02}:{m:02}:{s:02}.{millis:03} ")
    }
}

/// SilkCircuit-themed event formatter.
pub struct SilkFormat {
    ansi: bool,
}

impl SilkFormat {
    #[must_use]
    pub fn new(ansi: bool) -> Self {
        Self { ansi }
    }
}

impl Default for SilkFormat {
    fn default() -> Self {
        Self {
            ansi: std::io::stdout().is_terminal(),
        }
    }
}

impl<S, N> FormatEvent<S, N> for SilkFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let ansi = self.ansi && writer.has_ansi_escapes();
        let meta = event.metadata();

        write_timestamp(&mut writer, ansi)?;

        let (lvl_color, lvl_label) = level_style(*meta.level());
        if ansi {
            write!(
                writer,
                "{}{}{lvl_label}{} ",
                color::BOLD,
                lvl_color,
                color::RESET
            )?;
        } else {
            write!(writer, "{lvl_label} ")?;
        }

        // Target / module path — coral, muted so the message stays focal.
        if ansi {
            write!(
                writer,
                "{}{}{}{} ",
                color::CORAL,
                color::DIM,
                meta.target(),
                color::RESET
            )?;
        } else {
            write!(writer, "{} ", meta.target())?;
        }

        // Field rendering + message — defer to the registered field
        // formatter so structured fields keep their native shape.
        ctx.field_format().format_fields(writer.by_ref(), event)?;

        writeln!(writer)
    }
}

/// Install the SilkCircuit-themed subscriber with the given env filter.
/// Called from every daemon entry point that wants colored console logs.
pub fn install(env_filter: EnvFilter) {
    let ansi = std::io::stdout().is_terminal();
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_ansi(ansi)
        .event_format(SilkFormat::new(ansi))
        .init();
}
