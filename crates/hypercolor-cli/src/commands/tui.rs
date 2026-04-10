use clap::Args;

/// Arguments for the `tui` subcommand.
#[derive(Args)]
pub struct TuiArgs {
    /// Log level for the TUI session (error, warn, info, debug, trace).
    #[arg(long, default_value = "warn")]
    pub log_level: String,
}
