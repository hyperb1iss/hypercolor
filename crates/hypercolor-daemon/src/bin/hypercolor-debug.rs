use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::{
    DeviceRegistry, DiscoveredDevice, DiscoveryOrchestrator, DiscoveryReport, ScannerScanReport,
    TransportScanner, UsbHotplugEvent, UsbHotplugMonitor,
};
use hypercolor_driver_api::{
    CredentialStore, DiscoveryRequest, DriverConfigView, DriverCredentialStore,
    DriverDiscoveredDevice, DriverDiscoveryState, DriverHost, DriverModule, DriverRuntimeActions,
    DriverTrackedDevice,
};
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::{DriverConfigEntry, HypercolorConfig};
use hypercolor_types::device::DeviceInfo;
use serde_json::Value;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "hypercolor-debug",
    version,
    about = "Local discovery target debug tools for discovery and USB hotplug"
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum DebugTarget {
    HostTransport(String),
    Driver(String),
}

impl DebugTarget {
    fn host_transport(target_id: &str) -> Self {
        Self::HostTransport(target_id.to_owned())
    }

    fn is_usb(&self) -> bool {
        matches!(self, Self::HostTransport(target_id) if target_id == "usb")
    }
}

#[derive(Debug, Args)]
struct DetectArgs {
    /// Discovery targets to scan (repeat or comma-separate values).
    #[arg(long, value_delimiter = ',', default_values_t = ["usb".to_owned(), "smbus".to_owned()])]
    targets: Vec<String>,

    /// Periodic scan interval in seconds.
    #[arg(long, default_value_t = 5)]
    interval_secs: u64,

    /// Stop automatically after this many seconds.
    #[arg(long)]
    duration_secs: Option<u64>,

    /// Disable USB hotplug-triggered rescans.
    #[arg(long, default_value_t = false)]
    no_hotplug: bool,

    /// Discovery timeout for selected scanners.
    #[arg(long, default_value_t = 10_000)]
    timeout_ms: u64,
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
    hypercolor_daemon::startup::logging::install(env_filter);
}

async fn run_detect(args: DetectArgs) -> Result<()> {
    let registry = DeviceRegistry::new();
    let config = HypercolorConfig::default();
    let credential_store = Arc::new(
        CredentialStore::open_blocking(&ConfigManager::data_dir())
            .context("failed to open driver credential store")?,
    );
    let driver_registry =
        hypercolor_daemon::network::build_builtin_driver_module_registry(&config, credential_store)
            .context("failed to build debug driver registry")?;
    let driver_host = Arc::new(DebugDriverHost);
    let discovery_timeout = Duration::from_millis(args.timeout_ms.max(100));
    let periodic_targets = normalize_targets(&args.targets, &driver_registry)?;
    if periodic_targets.is_empty() {
        anyhow::bail!("no discovery targets selected");
    }

    let hotplug_enabled = !args.no_hotplug && periodic_targets.iter().any(DebugTarget::is_usb);
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
        targets = ?periodic_targets,
        interval_secs = args.interval_secs.max(1),
        timeout_ms = discovery_timeout.as_millis(),
        hotplug = hotplug_enabled,
        "starting debug detection loop"
    );

    run_scan(
        &registry,
        &driver_registry,
        Arc::clone(&driver_host),
        &config,
        &periodic_targets,
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
                    &driver_registry,
                    Arc::clone(&driver_host),
                    &config,
                    &periodic_targets,
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
                            &driver_registry,
                            Arc::clone(&driver_host),
                            &config,
                            &[DebugTarget::host_transport("usb")],
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
    driver_registry: &DriverModuleRegistry,
    driver_host: Arc<DebugDriverHost>,
    config: &HypercolorConfig,
    targets: &[DebugTarget],
    _args: &DetectArgs,
    timeout: Duration,
    trigger: &str,
) -> Result<()> {
    let mut orchestrator = DiscoveryOrchestrator::new(registry.clone());
    for target in targets {
        match target {
            DebugTarget::HostTransport(target_id) => {
                add_host_transport_scanner(&mut orchestrator, driver_registry, config, target_id)?;
            }
            DebugTarget::Driver(driver_id) => add_driver_scanner(
                &mut orchestrator,
                driver_registry,
                Arc::clone(&driver_host),
                config,
                driver_id,
                timeout,
            )?,
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
                "  + {} [{}] output_backend={}",
                tracked.info.name,
                id,
                output_backend_id(&tracked.info),
            );
        }
    }

    for id in &report.reappeared_devices {
        if let Some(tracked) = registry.get(id).await {
            println!(
                "  ~ {} [{}] output_backend={}",
                tracked.info.name,
                id,
                output_backend_id(&tracked.info),
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

fn normalize_targets(
    targets: &[String],
    driver_registry: &DriverModuleRegistry,
) -> Result<Vec<DebugTarget>> {
    let mut out = Vec::with_capacity(targets.len());
    for target in targets
        .iter()
        .map(|target| target.trim().to_ascii_lowercase())
    {
        let target = match target.as_str() {
            target_id if hypercolor_daemon::network::is_host_transport_target(target_id) => {
                DebugTarget::HostTransport(target_id.to_owned())
            }
            driver_id => {
                let Some(driver) = driver_registry.get(driver_id) else {
                    let mut supported = hypercolor_daemon::network::HOST_TRANSPORT_TARGET_IDS
                        .iter()
                        .map(|target_id| (*target_id).to_owned())
                        .collect::<Vec<_>>();
                    supported.extend(driver_registry.ids());
                    anyhow::bail!(
                        "unknown discovery target '{driver_id}'. Supported targets: {}",
                        supported.join(", ")
                    );
                };
                if driver.discovery().is_none() {
                    anyhow::bail!("driver '{driver_id}' does not support discovery");
                }
                DebugTarget::Driver(driver_id.to_owned())
            }
        };
        if !out.contains(&target) {
            out.push(target);
        }
    }
    Ok(out)
}

fn output_backend_id(info: &DeviceInfo) -> &str {
    info.output_backend_id()
}

fn add_host_transport_scanner(
    orchestrator: &mut DiscoveryOrchestrator,
    driver_registry: &DriverModuleRegistry,
    config: &HypercolorConfig,
    target_id: &str,
) -> Result<()> {
    let scanner =
        hypercolor_daemon::network::host_transport_scanner(target_id, driver_registry, config)
            .with_context(|| format!("host discovery target '{target_id}' is not registered"))?;
    orchestrator.add_scanner(scanner);
    Ok(())
}

fn add_driver_scanner(
    orchestrator: &mut DiscoveryOrchestrator,
    driver_registry: &DriverModuleRegistry,
    host: Arc<DebugDriverHost>,
    config: &HypercolorConfig,
    driver_id: &str,
    timeout: Duration,
) -> Result<()> {
    let driver = driver_registry
        .get(driver_id)
        .with_context(|| format!("debug driver '{driver_id}' is not registered"))?;
    let driver_config = hypercolor_daemon::network::driver_config_entry(config, driver_id);
    orchestrator.add_scanner(Box::new(DebugDriverScanner {
        driver,
        driver_id: driver_id.to_owned(),
        config: driver_config,
        host,
        request: DiscoveryRequest {
            timeout,
            mdns_enabled: config.discovery.mdns_enabled,
        },
    }));
    Ok(())
}

struct DebugDriverScanner {
    driver: Arc<dyn DriverModule>,
    driver_id: String,
    config: DriverConfigEntry,
    host: Arc<DebugDriverHost>,
    request: DiscoveryRequest,
}

#[async_trait::async_trait]
impl TransportScanner for DebugDriverScanner {
    fn name(&self) -> &str {
        self.driver.descriptor().display_name
    }

    async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
        let Some(capability) = self.driver.discovery() else {
            return Ok(Vec::new());
        };
        let config = DriverConfigView {
            driver_id: &self.driver_id,
            entry: &self.config,
        };
        let result = capability
            .discover(self.host.as_ref(), &self.request, config)
            .await?;
        Ok(result
            .devices
            .into_iter()
            .map(driver_discovered_to_device)
            .collect())
    }
}

fn driver_discovered_to_device(device: DriverDiscoveredDevice) -> DiscoveredDevice {
    DiscoveredDevice {
        connection_type: device.info.connection_type,
        origin: device.info.origin.clone(),
        name: device.info.name.clone(),
        family: device.info.family.clone(),
        fingerprint: device.fingerprint,
        connect_behavior: device.connect_behavior,
        info: device.info,
        metadata: device.metadata,
    }
}

struct DebugDriverHost;

#[async_trait::async_trait]
impl DriverCredentialStore for DebugDriverHost {
    async fn get_json(&self, _driver_id: &str, _key: &str) -> Result<Option<Value>> {
        Ok(None)
    }

    async fn set_json(&self, _driver_id: &str, _key: &str, _value: Value) -> Result<()> {
        Ok(())
    }

    async fn remove(&self, _driver_id: &str, _key: &str) -> Result<()> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl DriverRuntimeActions for DebugDriverHost {
    async fn activate_device(
        &self,
        _device_id: hypercolor_types::device::DeviceId,
        _backend_id: &str,
    ) -> Result<bool> {
        Ok(false)
    }

    async fn disconnect_device(
        &self,
        _device_id: hypercolor_types::device::DeviceId,
        _backend_id: &str,
        _will_retry: bool,
    ) -> Result<bool> {
        Ok(false)
    }
}

#[async_trait::async_trait]
impl DriverDiscoveryState for DebugDriverHost {
    async fn tracked_devices(&self, _driver_id: &str) -> Vec<DriverTrackedDevice> {
        Vec::new()
    }

    fn load_cached_json(&self, _driver_id: &str, _key: &str) -> Result<Option<Value>> {
        Ok(None)
    }
}

impl DriverHost for DebugDriverHost {
    fn credentials(&self) -> &dyn DriverCredentialStore {
        self
    }

    fn runtime(&self) -> &dyn DriverRuntimeActions {
        self
    }

    fn discovery_state(&self) -> &dyn DriverDiscoveryState {
        self
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
