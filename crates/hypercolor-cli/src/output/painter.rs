//! Opaline-backed terminal painter for the Hypercolor CLI.
//!
//! All colored output flows through this module. Semantic helpers map domain
//! concepts to opaline theme tokens so themes can change centrally without
//! touching individual command handlers.

use opaline::adapters::owo_colors::OwoThemeExt;
use owo_colors::OwoColorize;

// ── Painter ─────────────────────────────────────────────────────────────

/// Semantic colorizer backed by an opaline theme.
///
/// Holds a loaded theme and an enabled flag. When disabled (e.g. `--no-color`,
/// `NO_COLOR`, non-TTY), every helper returns its input unchanged.
pub struct Painter {
    theme: opaline::Theme,
    enabled: bool,
}

impl std::fmt::Debug for Painter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Painter")
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

impl Painter {
    /// Construct a painter from CLI options.
    pub fn new(theme_name: Option<&str>, enabled: bool) -> Self {
        Self {
            theme: load_theme(theme_name),
            enabled,
        }
    }

    /// Plain (no-color) painter for non-interactive use.
    pub fn plain() -> Self {
        Self {
            theme: load_theme(None),
            enabled: false,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn paint(&self, text: &str, token: &str) -> String {
        if self.enabled {
            format!("{}", text.style(self.theme.owo_fg(token)))
        } else {
            text.to_string()
        }
    }

    fn paint_style(&self, text: &str, style_name: &str) -> String {
        if self.enabled {
            format!("{}", text.style(self.theme.owo_style(style_name)))
        } else {
            text.to_string()
        }
    }

    // ── Semantic helpers ────────────────────────────────────────────────

    /// Names, labels, identifiers (neon cyan).
    pub fn name(&self, text: &str) -> String {
        self.paint(text, "accent.secondary")
    }

    /// Keywords, emphasis (electric purple, bold).
    pub fn keyword(&self, text: &str) -> String {
        self.paint_style(text, "keyword")
    }

    /// Numeric values, counts, ports (coral).
    pub fn number(&self, text: &str) -> String {
        self.paint(text, "code.number")
    }

    /// UUIDs, hardware IDs (dim).
    pub fn id(&self, text: &str) -> String {
        self.paint(text, "text.dim")
    }

    /// Types, categories, secondary info (muted).
    pub fn muted(&self, text: &str) -> String {
        self.paint(text, "text.muted")
    }

    /// Success states (neon green).
    pub fn success(&self, text: &str) -> String {
        self.paint(text, "success")
    }

    /// Error states (red).
    pub fn error(&self, text: &str) -> String {
        self.paint(text, "error")
    }

    /// Warning states (yellow).
    pub fn warning(&self, text: &str) -> String {
        self.paint(text, "warning")
    }

    // ── Domain helpers ─────────────────────────────────────────────────

    /// Device connection state.
    pub fn device_state(&self, state: &str) -> String {
        match state.to_ascii_lowercase().as_str() {
            "online" | "connected" | "ready" => self.success(state),
            "offline" | "disconnected" | "missing" => self.error(state),
            "paired" | "pairing" => self.warning(state),
            _ => self.muted(state),
        }
    }

    /// Effect activity state.
    pub fn effect_state(&self, state: &str) -> String {
        match state.to_ascii_lowercase().as_str() {
            "running" | "active" => self.success(state),
            "stopped" | "idle" => self.muted(state),
            "error" | "failed" => self.error(state),
            _ => self.warning(state),
        }
    }

    /// Boolean display with colored yes/no.
    pub fn yesno(&self, value: bool) -> String {
        if value {
            self.success("yes")
        } else {
            self.error("no")
        }
    }

    /// Diagnostic check result.
    pub fn check_status(&self, status: &str) -> String {
        match status {
            "pass" => self.success(status),
            "warning" => self.warning(status),
            "fail" => self.error(status),
            _ => self.muted(status),
        }
    }

    // ── Status icons ───────────────────────────────────────────────────

    /// Success icon: ✦ (green).
    pub fn success_icon(&self) -> String {
        self.paint("\u{2726}", "success")
    }

    /// Error icon: ✗ (red).
    pub fn error_icon(&self) -> String {
        self.paint("\u{2717}", "error")
    }

    /// Warning icon: ! (yellow).
    pub fn warning_icon(&self) -> String {
        self.paint("!", "warning")
    }

    /// Check-pass icon: ✓ (green).
    pub fn check_pass_icon(&self) -> String {
        self.paint("\u{2713}", "success")
    }

    /// Check-fail icon: ✗ (red).
    pub fn check_fail_icon(&self) -> String {
        self.paint("\u{2717}", "error")
    }

    /// Running status dot: ● (green or red).
    pub fn status_dot(&self, running: bool) -> String {
        if running {
            self.paint("\u{25cf}", "success")
        } else {
            self.paint("\u{25cf}", "error")
        }
    }

    /// Diagnostic icon for check results.
    pub fn diagnose_icon(&self, status: &str) -> String {
        if !self.enabled {
            return match status {
                "pass" => "[OK]".to_string(),
                "warning" => "[!!]".to_string(),
                "fail" => "[FAIL]".to_string(),
                _ => "[??]".to_string(),
            };
        }
        match status {
            "pass" => self.check_pass_icon(),
            "warning" => self.warning_icon(),
            "fail" => self.check_fail_icon(),
            _ => "?".to_string(),
        }
    }
}

// ── Theme loading ──────────────────────────────────────────────────────

/// Load a theme by name, falling back to the SilkCircuit Neon default.
fn load_theme(name: Option<&str>) -> opaline::Theme {
    let resolved = name.unwrap_or("silkcircuit-neon");
    opaline::load_by_name(resolved).unwrap_or_else(|| {
        opaline::load_by_name("silkcircuit-neon")
            .expect("builtin silkcircuit-neon theme must exist")
    })
}

// ── Clap help styling ──────────────────────────────────────────────────

/// Render arbitrary text through the brand gradient (Purple → Coral → Cyan).
///
/// Returns plain text when the painter is disabled. Use for hero text like
/// banners, splash headers, or emphasized titles.
pub fn gradient_brand(text: &str, enabled: bool) -> String {
    if !enabled {
        return text.to_string();
    }
    use opaline::{Gradient, OpalineColor};
    let gradient = Gradient::new(vec![
        OpalineColor {
            r: 225,
            g: 53,
            b: 255,
        }, // Electric Purple
        OpalineColor {
            r: 255,
            g: 106,
            b: 193,
        }, // Coral
        OpalineColor {
            r: 128,
            g: 255,
            b: 234,
        }, // Neon Cyan
    ]);
    opaline::adapters::owo_colors::gradient_string(text, &gradient)
}

impl Painter {
    /// Render the brand title "H Y P E R C O L O R" with the brand gradient.
    ///
    /// Respects the painter's enabled flag. Used by both the clap help banner
    /// and the `hyper status` header.
    pub fn help_banner_title(&self) -> String {
        gradient_brand("H Y P E R C O L O R", self.enabled)
    }
}

/// Gradient-colored banner for the top of `--help` output.
///
/// Composes the brand title with a muted separator line. Used by clap's
/// `before_help` hook, so its color gating must match clap's own detection
/// (NO_COLOR / CLICOLOR_FORCE / stdout TTY).
pub fn help_banner() -> String {
    let use_color = should_color_banner();
    let title = gradient_brand("H Y P E R C O L O R", use_color);
    let sep = if use_color {
        format!("\x1b[38;2;130;135;159m{}\x1b[0m", "\u{2500}".repeat(21))
    } else {
        "\u{2500}".repeat(21)
    };
    format!("  {title}\n  {sep}")
}

fn should_color_banner() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var_os("CLICOLOR_FORCE").is_some_and(|v| v != "0") {
        return true;
    }
    std::io::IsTerminal::is_terminal(&std::io::stdout())
}

/// SilkCircuit-themed styles for clap help output.
///
/// Maps the project's visual identity onto clap's help categories:
///   header/usage → Electric Purple (bold)
///   literal      → Neon Cyan
///   placeholder  → Text Muted
///   valid        → Success Green
///   invalid/error→ Error Red
///
/// Clap respects `NO_COLOR` and non-TTY detection automatically, so
/// these styles are stripped when color output is suppressed.
pub fn help_styles() -> clap::builder::Styles {
    use clap::builder::styling::{Color, RgbColor, Style, Styles};

    let purple = Some(Color::Rgb(RgbColor(225, 53, 255)));
    let cyan = Some(Color::Rgb(RgbColor(128, 255, 234)));
    let coral = Some(Color::Rgb(RgbColor(255, 106, 193)));
    let muted = Some(Color::Rgb(RgbColor(130, 135, 159)));
    let green = Some(Color::Rgb(RgbColor(80, 250, 123)));
    let red = Some(Color::Rgb(RgbColor(255, 99, 99)));

    Styles::styled()
        .header(Style::new().fg_color(purple).bold())
        .usage(Style::new().fg_color(purple).bold())
        .literal(Style::new().fg_color(cyan))
        .placeholder(Style::new().fg_color(muted))
        .valid(Style::new().fg_color(green))
        .invalid(Style::new().fg_color(red))
        .error(Style::new().fg_color(coral).bold())
}
