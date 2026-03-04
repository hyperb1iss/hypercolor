use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use hypercolor_core::device::openrgb::{OpenRgbScanner, ScannerConfig as OpenRgbScannerConfig};
use hypercolor_core::device::wled::WledScanner;
use hypercolor_core::device::{
    DeviceRegistry, DiscoveryOrchestrator, DiscoveryReport, ScannerScanReport, UsbHotplugEvent,
    UsbHotplugMonitor, UsbScanner,
};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "hypercolor-debug",
    version,
    about = "Local backend debug tools for discovery and USB hotplug"
)]
struct DebugCli {
    /// Log level (trace, debug, info, warn, error).
    #[arg(long, default_value = "info")]
    log_level: String,

    #[command(subcommand)]
    command: DebugCommand,
}

#[derive(Debug, Subcommand)]
enum DebugCommand {
    /// Run discovery sweeps and optionally react to USB hotplug events.
    Detect(DetectArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum DebugBackend {
    Usb,
    Wled,
    Openrgb,
}

#[derive(Debug, Args)]
struct DetectArgs {
    /// Backends to scan (repeat or comma-separate values).
    #[arg(long, value_enum, value_delimiter = ',', default_values_t = [DebugBackend::Usb])]
    backends: Vec<DebugBackend>,

    /// Periodic scan interval in seconds.
    #[arg(long, default_value_t = 5)]
    interval_secs: u64,

    /// Stop automatically after this many seconds.
    #[arg(long)]
    duration_secs: Option<u64>,

    /// Disable USB hotplug-triggered rescans.
    #[arg(long, default_value_t = false)]
    no_hotplug: bool,

    /// Discovery timeout for network scanners.
    #[arg(long, default_value_t = 10_000)]
    timeout_ms: u64,

    /// `OpenRGB` SDK host used when `openrgb` backend is enabled.
    #[arg(long, default_value = "127.0.0.1")]
    openrgb_host: String,

    /// `OpenRGB` SDK port used when `openrgb` backend is enabled.
    #[arg(long, default_value_t = 6742)]
    openrgb_port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = DebugCli::parse();
    init_tracing(&cli.log_level);

    match cli.command {
        DebugCommand::Detect(args) => run_detect(args).await,
    }
}

fn init_tracing(log_level: &str) {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));
    tracing_subscriber::fmt().with_env_filter(env_filter).init();
}

async fn run_detect(args: DetectArgs) -> Result<()> {
    let registry = DeviceRegistry::new();
    let discovery_timeout = Duration::from_millis(args.timeout_ms.max(100));
    let periodic_backends = normalize_backends(&args.backends);
    if periodic_backends.is_empty() {
        anyhow::bail!("no backends selected");
    }

    let hotplug_enabled = !args.no_hotplug && periodic_backends.contains(&DebugBackend::Usb);
    let hotplug_monitor = hotplug_enabled.then(|| UsbHotplugMonitor::new(256));
    let mut hotplug_rx = hotplug_monitor.as_ref().map(UsbHotplugMonitor::subscribe);
    let mut hotplug_task = if let Some(monitor) = hotplug_monitor.as_ref() {
        Some(
            monitor
                .start()
                .context("failed to start USB hotplug watcher")?,
        )
    } else {
        None
    };

    info!(
        backends = ?periodic_backends,
        interval_secs = args.interval_secs.max(1),
        timeout_ms = discovery_timeout.as_millis(),
        hotplug = hotplug_enabled,
        "starting debug detection loop"
    );

    run_scan(
        &registry,
        &periodic_backends,
        &args,
        discovery_timeout,
        "initial",
    )
    .await?;

    let mut ticker = tokio::time::interval(Duration::from_secs(args.interval_secs.max(1)));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;

    let end_at = args
        .duration_secs
        .map(|seconds| Instant::now() + Duration::from_secs(seconds));

    loop {
        if let Some(deadline) = end_at
            && Instant::now() >= deadline
        {
            println!(
                "[{}] duration elapsed, stopping detection loop",
                timestamp_now()
            );
            break;
        }

        tokio::select! {
            _ = ticker.tick() => {
                run_scan(
                    &registry,
                    &periodic_backends,
                    &args,
                    discovery_timeout,
                    "periodic",
                ).await?;
            }
            hotplug_event = recv_hotplug(hotplug_rx.as_mut()), if hotplug_enabled => {
                match hotplug_event {
                    HotplugRecv::Event(event) => {
                        log_hotplug_event(&event);
                        run_scan(
                            &registry,
                            &[DebugBackend::Usb],
                            &args,
                            discovery_timeout,
                            "usb-hotplug",
                        ).await?;
                    }
                    HotplugRecv::Lagged(skipped) => {
                        println!("[{}] hotplug receiver lagged (skipped={skipped})", timestamp_now());
                    }
                    HotplugRecv::Closed => {
                        println!("[{}] hotplug channel closed", timestamp_now());
                        hotplug_rx = None;
                        hotplug_task.take();
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!("[{}] ctrl-c received, stopping detection loop", timestamp_now());
                break;
            }
        }
    }

    if let Some(task) = hotplug_task.take() {
        task.abort();
    }

    Ok(())
}

enum HotplugRecv {
    Event(UsbHotplugEvent),
    Lagged(u64),
    Closed,
}

async fn recv_hotplug(
    receiver: Option<&mut tokio::sync::broadcast::Receiver<UsbHotplugEvent>>,
) -> HotplugRecv {
    let Some(receiver) = receiver else {
        return HotplugRecv::Closed;
    };

    match receiver.recv().await {
        Ok(event) => HotplugRecv::Event(event),
        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
            HotplugRecv::Lagged(skipped)
        }
        Err(tokio::sync::broadcast::error::RecvError::Closed) => HotplugRecv::Closed,
    }
}

fn log_hotplug_event(event: &UsbHotplugEvent) {
    match event {
        UsbHotplugEvent::Arrived {
            vendor_id,
            product_id,
            descriptor,
        } => {
            println!(
                "[{}] hotplug arrived {:04X}:{:04X} {}",
                timestamp_now(),
                vendor_id,
                product_id,
                descriptor.name
            );
        }
        UsbHotplugEvent::Removed {
            vendor_id,
            product_id,
        } => {
            println!(
                "[{}] hotplug removed {:04X}:{:04X}",
                timestamp_now(),
                vendor_id,
                product_id
            );
        }
    }
}

async fn run_scan(
    registry: &DeviceRegistry,
    backends: &[DebugBackend],
    args: &DetectArgs,
    timeout: Duration,
    trigger: &str,
) -> Result<()> {
    let mut orchestrator = DiscoveryOrchestrator::new(registry.clone());
    for backend in backends {
        match backend {
            DebugBackend::Usb => orchestrator.add_scanner(Box::new(UsbScanner::new())),
            DebugBackend::Wled => {
                orchestrator.add_scanner(Box::new(WledScanner::with_timeout(timeout)));
            }
            DebugBackend::Openrgb => {
                let probe_timeout =
                    timeout.clamp(Duration::from_millis(250), Duration::from_secs(2));
                orchestrator.add_scanner(Box::new(OpenRgbScanner::new(OpenRgbScannerConfig {
                    host: args.openrgb_host.clone(),
                    port: args.openrgb_port,
                    probe_timeout,
                })));
            }
        }
    }

    let report = orchestrator.full_scan().await;
    print_scan_report(registry, trigger, &report).await;
    Ok(())
}

async fn print_scan_report(registry: &DeviceRegistry, trigger: &str, report: &DiscoveryReport) {
    println!(
        "[{}] scan trigger={} new={} reappeared={} vanished={} total={} duration_ms={}",
        timestamp_now(),
        trigger,
        report.new_devices.len(),
        report.reappeared_devices.len(),
        report.vanished_devices.len(),
        report.total_known,
        report.scan_duration.as_millis(),
    );

    for scanner in &report.scanner_reports {
        print_scanner_report(scanner);
    }

    for id in &report.new_devices {
        if let Some(tracked) = registry.get(id).await {
            println!(
                "  + {} [{}] backend_hint={}",
                tracked.info.name,
                id,
                backend_hint(&tracked.info.family),
            );
        }
    }

    for id in &report.reappeared_devices {
        if let Some(tracked) = registry.get(id).await {
            println!(
                "  ~ {} [{}] backend_hint={}",
                tracked.info.name,
                id,
                backend_hint(&tracked.info.family),
            );
        }
    }

    for id in &report.vanished_devices {
        println!("  - {id}");
    }
}

fn print_scanner_report(report: &ScannerScanReport) {
    match &report.error {
        Some(error) => println!(
            "  scanner={} status=error discovered={} duration_ms={} error={}",
            report.scanner,
            report.discovered,
            report.duration.as_millis(),
            error
        ),
        None => println!(
            "  scanner={} status=ok discovered={} duration_ms={}",
            report.scanner,
            report.discovered,
            report.duration.as_millis(),
        ),
    }
}

fn normalize_backends(backends: &[DebugBackend]) -> Vec<DebugBackend> {
    let mut out = Vec::with_capacity(backends.len());
    for backend in backends {
        if !out.contains(backend) {
            out.push(*backend);
        }
    }
    out
}

fn backend_hint(family: &hypercolor_types::device::DeviceFamily) -> &'static str {
    match family {
        hypercolor_types::device::DeviceFamily::Razer
        | hypercolor_types::device::DeviceFamily::Corsair
        | hypercolor_types::device::DeviceFamily::LianLi
        | hypercolor_types::device::DeviceFamily::PrismRgb => "usb",
        hypercolor_types::device::DeviceFamily::Wled => "wled",
        hypercolor_types::device::DeviceFamily::OpenRgb => "openrgb",
        hypercolor_types::device::DeviceFamily::Hue => "hue",
        hypercolor_types::device::DeviceFamily::Custom(_) => "custom",
    }
}

fn timestamp_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let seconds = now.as_secs() % 86_400;
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let secs = seconds % 60;
    let millis = now.subsec_millis();
    format!("{hours:02}:{minutes:02}:{secs:02}.{millis:03}")
}
