use anyhow::Result;
use clap::{Parser, ValueEnum};
use hypercolor_daemon::daemon::{self, DaemonRunOptions};
use hypercolor_daemon::startup::install_signal_handlers;
use hypercolor_types::config::RenderAccelerationMode;
use std::path::PathBuf;

#[cfg(target_os = "windows")]
mod windows_service;

/// Hypercolor lighting daemon — orchestrates RGB devices at up to 60fps.
#[derive(Parser, Debug)]
#[command(name = "hypercolor-daemon", about = "Hypercolor lighting daemon")]
struct DaemonArgs {
    /// Path to the configuration file.
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Address and port to bind the API server to.
    #[arg(long)]
    bind: Option<String>,

    /// Log level (trace, debug, info, warn, error).
    #[arg(long)]
    log_level: Option<String>,

    /// Override the configured compositor acceleration mode.
    #[arg(long, alias = "render-acceleration-mode", value_enum)]
    compositor_acceleration_mode: Option<RenderAccelerationModeArg>,

    /// Serve the web UI from this directory (static files with SPA fallback).
    #[arg(long)]
    ui_dir: Option<PathBuf>,

    /// Run under the Windows Service Control Manager.
    #[cfg(target_os = "windows")]
    #[arg(long, hide = true)]
    windows_service: bool,
}

impl DaemonArgs {
    fn into_run_options(self) -> DaemonRunOptions {
        DaemonRunOptions {
            config: self.config,
            bind: self.bind,
            log_level: self.log_level,
            compositor_acceleration_mode: self.compositor_acceleration_mode.map(Into::into),
            ui_dir: self.ui_dir,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RenderAccelerationModeArg {
    Cpu,
    Auto,
    Gpu,
}

impl From<RenderAccelerationModeArg> for RenderAccelerationMode {
    fn from(value: RenderAccelerationModeArg) -> Self {
        match value {
            RenderAccelerationModeArg::Cpu => Self::Cpu,
            RenderAccelerationModeArg::Auto => Self::Auto,
            RenderAccelerationModeArg::Gpu => Self::Gpu,
        }
    }
}

fn main() -> Result<()> {
    let args = DaemonArgs::parse();

    #[cfg(target_os = "windows")]
    if args.windows_service {
        return windows_service::run(args.into_run_options());
    }

    let runtime = daemon::build_main_runtime()?;
    runtime.block_on(async move {
        let shutdown_rx = install_signal_handlers();
        daemon::run(args.into_run_options(), shutdown_rx).await
    })
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{DaemonArgs, RenderAccelerationModeArg};
    use hypercolor_types::config::{HypercolorConfig, RenderAccelerationMode};

    #[test]
    fn compositor_acceleration_mode_cli_override_updates_config() {
        let args = DaemonArgs::try_parse_from([
            "hypercolor-daemon",
            "--compositor-acceleration-mode",
            "gpu",
        ])
        .expect("CLI override should parse");
        let mut config = HypercolorConfig::default();

        if let Some(mode) = args.compositor_acceleration_mode {
            config.effect_engine.compositor_acceleration_mode = mode.into();
        }

        assert_eq!(
            config.effect_engine.compositor_acceleration_mode,
            RenderAccelerationMode::Gpu
        );
    }

    #[test]
    fn legacy_render_acceleration_mode_cli_alias_updates_config() {
        let args =
            DaemonArgs::try_parse_from(["hypercolor-daemon", "--render-acceleration-mode", "gpu"])
                .expect("legacy CLI override should parse");
        let mut config = HypercolorConfig::default();

        if let Some(mode) = args.compositor_acceleration_mode {
            config.effect_engine.compositor_acceleration_mode = mode.into();
        }

        assert_eq!(
            config.effect_engine.compositor_acceleration_mode,
            RenderAccelerationMode::Gpu
        );
    }

    #[test]
    fn render_acceleration_arg_maps_all_modes() {
        assert_eq!(
            RenderAccelerationMode::from(RenderAccelerationModeArg::Cpu),
            RenderAccelerationMode::Cpu
        );
        assert_eq!(
            RenderAccelerationMode::from(RenderAccelerationModeArg::Auto),
            RenderAccelerationMode::Auto
        );
        assert_eq!(
            RenderAccelerationMode::from(RenderAccelerationModeArg::Gpu),
            RenderAccelerationMode::Gpu
        );
    }
}
