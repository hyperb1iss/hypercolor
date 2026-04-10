//! Hypercolor Terminal UI — a live instrument for controlling light.

pub mod action;
pub mod app;
pub mod bridge;
pub mod chrome;
pub mod client;
pub mod component;
pub mod event;
pub mod motion;
pub mod screen;
pub mod state;
pub mod theme;
pub mod theme_picker;
pub mod views;
pub mod widgets;

/// Boot the TUI, taking over the terminal until the user quits.
///
/// Tracing is redirected to a log file so it doesn't corrupt ratatui's
/// alternate screen. The caller must NOT initialize a tracing subscriber
/// before calling this.
pub async fn launch(
    host: String,
    port: u16,
    theme: Option<String>,
    log_level: &str,
) -> anyhow::Result<()> {
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("hypercolor-tui.log"))
        .expect("failed to create log file");

    tracing_subscriber::fmt()
        .with_env_filter(log_level)
        .with_writer(std::sync::Mutex::new(log_file))
        .with_ansi(false)
        .init();

    let persisted = theme_picker::load_config().theme;
    let theme_name = theme.as_deref().or(persisted.as_deref());
    theme::initialize(theme_name);

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        ratatui::restore();
        original_hook(panic_info);
    }));

    let mut app = app::App::new(host, port);
    app.run().await
}
