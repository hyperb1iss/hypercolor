use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use hypercolor_daemon::daemon::{self, DaemonRunOptions};
use hypercolor_daemon::startup::install_signal_handlers;
use hypercolor_types::config::{RenderAccelerationMode, ServoGpuImportMode};
use single_instance::SingleInstance;
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

    /// Host/interface to bind using the configured daemon port.
    #[arg(long, alias = "listen-host", alias = "host", conflicts_with = "bind")]
    listen: Option<String>,

    /// Listen on every IPv4 network interface.
    #[arg(long, alias = "lan", alias = "all-interfaces", conflicts_with_all = ["bind", "listen"])]
    listen_all: bool,

    /// Log level (trace, debug, info, warn, error).
    #[arg(long)]
    log_level: Option<String>,

    /// Override the configured compositor acceleration mode.
    #[arg(long, alias = "render-acceleration-mode", value_enum)]
    compositor_acceleration_mode: Option<RenderAccelerationModeArg>,

    /// Override the configured Servo Linux GPU import mode.
    #[arg(long, value_enum)]
    servo_gpu_import_mode: Option<ServoGpuImportModeArg>,

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
            listen_address: self.listen,
            listen_all: self.listen_all,
            log_level: self.log_level,
            compositor_acceleration_mode: self.compositor_acceleration_mode.map(Into::into),
            servo_gpu_import_mode: self.servo_gpu_import_mode.map(Into::into),
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

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ServoGpuImportModeArg {
    Off,
    Auto,
    On,
}

impl From<ServoGpuImportModeArg> for ServoGpuImportMode {
    fn from(value: ServoGpuImportModeArg) -> Self {
        match value {
            ServoGpuImportModeArg::Off => Self::Off,
            ServoGpuImportModeArg::Auto => Self::Auto,
            ServoGpuImportModeArg::On => Self::On,
        }
    }
}

fn main() -> Result<()> {
    let args = DaemonArgs::parse();
    let instance = SingleInstance::new(&daemon_instance_name())
        .context("failed to acquire daemon single-instance guard")?;
    if !instance.is_single() {
        eprintln!("hypercolor-daemon is already running; exiting");
        return Ok(());
    }

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

fn daemon_instance_name() -> String {
    #[cfg(target_os = "macos")]
    {
        std::env::temp_dir()
            .join("hypercolor-daemon.lock")
            .display()
            .to_string()
    }

    #[cfg(not(target_os = "macos"))]
    {
        "hypercolor-daemon".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{
        DaemonArgs, RenderAccelerationModeArg, ServoGpuImportModeArg, daemon_instance_name,
    };
    use hypercolor_types::config::{HypercolorConfig, RenderAccelerationMode, ServoGpuImportMode};

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
    fn servo_gpu_import_mode_cli_override_updates_config() {
        let args =
            DaemonArgs::try_parse_from(["hypercolor-daemon", "--servo-gpu-import-mode", "auto"])
                .expect("Servo GPU import CLI override should parse");
        let mut config = HypercolorConfig::default();

        if let Some(mode) = args.servo_gpu_import_mode {
            config.rendering.servo_gpu_import.mode = mode.into();
        }

        assert_eq!(
            config.rendering.servo_gpu_import.mode,
            ServoGpuImportMode::Auto
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

    #[test]
    fn servo_gpu_import_arg_maps_all_modes() {
        assert_eq!(
            ServoGpuImportMode::from(ServoGpuImportModeArg::Off),
            ServoGpuImportMode::Off
        );
        assert_eq!(
            ServoGpuImportMode::from(ServoGpuImportModeArg::Auto),
            ServoGpuImportMode::Auto
        );
        assert_eq!(
            ServoGpuImportMode::from(ServoGpuImportModeArg::On),
            ServoGpuImportMode::On
        );
    }

    #[test]
    fn daemon_instance_name_is_stable() {
        let name = daemon_instance_name();

        assert!(name.contains("hypercolor-daemon"));
    }
}
