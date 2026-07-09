//! OpenRGB fallback Bridge driver for Hypercolor.
//!
//! This driver talks to a user-managed OpenRGB SDK server over the clean
//! `hypercolor-openrgb-sdk` crate. It does not supervise or bundle OpenRGB.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Context, Error, Result, bail};
use async_trait::async_trait;
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceFrameSink,
    DiscoveredDevice, DiscoveryCapability, DiscoveryConnectBehavior, DiscoveryRequest,
    DiscoveryResult, DriverConfigProvider, DriverConfigView, DriverDescriptor,
    DriverDiscoveredDevice, DriverHost, DriverModule, DriverPresentationProvider, HealthStatus,
};
use hypercolor_openrgb_sdk::{
    ControllerData, ControllerMode, ControllerZone, DeviceType, ModeFlagPolicy, OpenRgbClient,
    OpenRgbClientConfig, PacketId, RgbColor,
};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceClassHint, DeviceColorFormat, DeviceColorSpace,
    DeviceFamily, DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin,
    DeviceTopologyHint, DriverCapabilitySet, DriverModuleDescriptor, DriverModuleKind,
    DriverPresentation, DriverTransportKind, ZoneInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{Mutex, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep};
use tracing::debug;

/// OpenRGB driver descriptor.
pub static DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "openrgb",
    "OpenRGB Fallback",
    DriverTransportKind::Bridge,
    true,
    false,
);

const FIELD_ENDPOINTS: &str = "endpoints";
const FIELD_ALLOW_INSECURE_REMOTE: &str = "allow_insecure_remote";
const FIELD_CONNECT_TIMEOUT_MS: &str = "connect_timeout_ms";
const FIELD_READ_TIMEOUT_MS: &str = "read_timeout_ms";
const FIELD_WRITE_TIMEOUT_MS: &str = "write_timeout_ms";
const FIELD_STARTUP_RESCAN: &str = "startup_rescan";
const FIELD_AUTO_CONNECT: &str = "auto_connect";
const FIELD_OWNERSHIP: &str = "ownership";
const FIELD_DETECTOR_PARTITION_CONFIRMED: &str = "detector_partition_confirmed";
const FIELD_DEFAULT_TARGET_FPS: &str = "default_target_fps";
const FIELD_CONTROLLER_FPS: &str = "controller_fps";
const FIELD_MODE_PER_LED_MASK: &str = "mode_per_led_mask";
const FIELD_MODE_PERSISTENT_MASK: &str = "mode_persistent_mask";
const FIELD_TEARDOWN_POLICY: &str = "teardown_policy";

const METADATA_ENDPOINT: &str = "endpoint";
const METADATA_CONTROLLER_INDEX: &str = "controller_index";
const METADATA_FINGERPRINT: &str = "fingerprint";
const METADATA_IDENTITY_CONFIDENCE: &str = "identity_confidence";
const METADATA_DETECTOR_CLASS: &str = "detector_class";
const METADATA_OUTPUT_ENABLED: &str = "output_enabled";
const METADATA_DISABLED_REASON: &str = "disabled_reason";
const METADATA_PROTOCOL_VERSION: &str = "protocol_version";

const DEFAULT_OPENRGB_PORT: u16 = 6742;
const DEFAULT_TIMEOUT_MS: u64 = 750;
const DEFAULT_TARGET_FPS: u32 = 30;
const MAX_TIMEOUT_MS: u64 = 10_000;
const MAX_CONTROLLERS_PER_ENDPOINT: u32 = 1024;
const INITIAL_RECONNECT_BACKOFF: Duration = Duration::from_millis(250);
const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(5);
const OUTPUT_WORKER_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const DELIVERY_PENDING: u8 = 0;
const DELIVERY_STARTED: u8 = 1;
const DELIVERY_REJECTED: u8 = 2;

/// Driver configuration for the OpenRGB fallback bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenRgbConfig {
    #[serde(default = "default_endpoints")]
    pub endpoints: Vec<SocketAddr>,
    #[serde(default)]
    pub allow_insecure_remote: bool,
    #[serde(default = "default_timeout_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_timeout_ms")]
    pub read_timeout_ms: u64,
    #[serde(default = "default_timeout_ms")]
    pub write_timeout_ms: u64,
    #[serde(default)]
    pub startup_rescan: bool,
    #[serde(default = "default_auto_connect")]
    pub auto_connect: bool,
    #[serde(default)]
    pub ownership: OpenRgbOwnership,
    #[serde(default)]
    pub detector_partition_confirmed: bool,
    #[serde(default = "default_target_fps")]
    pub default_target_fps: u32,
    #[serde(default)]
    pub controller_fps: BTreeMap<String, u32>,
    #[serde(default = "default_per_led_mask")]
    pub mode_per_led_mask: u32,
    #[serde(default)]
    pub mode_persistent_mask: u32,
    #[serde(default)]
    pub teardown_policy: OpenRgbTeardownPolicy,
}

impl Default for OpenRgbConfig {
    fn default() -> Self {
        Self {
            endpoints: default_endpoints(),
            allow_insecure_remote: false,
            connect_timeout_ms: default_timeout_ms(),
            read_timeout_ms: default_timeout_ms(),
            write_timeout_ms: default_timeout_ms(),
            startup_rescan: false,
            auto_connect: default_auto_connect(),
            ownership: OpenRgbOwnership::default(),
            detector_partition_confirmed: false,
            default_target_fps: default_target_fps(),
            controller_fps: BTreeMap::new(),
            mode_per_led_mask: default_per_led_mask(),
            mode_persistent_mask: 0,
            teardown_policy: OpenRgbTeardownPolicy::default(),
        }
    }
}

/// Static detector-class partition for OpenRGB ownership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenRgbOwnership {
    #[serde(default)]
    pub mode: OpenRgbOwnershipMode,
    #[serde(default)]
    pub allowed_detector_classes: Vec<String>,
    #[serde(default)]
    pub native_claimed_detector_classes: Vec<String>,
    #[serde(default)]
    pub allow_low_confidence: bool,
}

impl Default for OpenRgbOwnership {
    fn default() -> Self {
        Self {
            mode: OpenRgbOwnershipMode::Disabled,
            allowed_detector_classes: Vec::new(),
            native_claimed_detector_classes: Vec::new(),
            allow_low_confidence: false,
        }
    }
}

/// OpenRGB ownership mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OpenRgbOwnershipMode {
    /// Do not expose OpenRGB output devices.
    #[default]
    Disabled,
    /// Expose only configured detector classes.
    DetectorPartitioned,
    /// OpenRGB owns every detector class it reports.
    OpenRgbOwned,
}

/// Disconnect behavior for controllers left in OpenRGB direct mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OpenRgbTeardownPolicy {
    /// Restore the pre-connect mode when known, otherwise leave the last frame.
    #[default]
    RestorePreviousOrLeave,
    /// Restore the pre-connect mode when known, otherwise write black.
    RestorePreviousOrBlackout,
    /// Always write black before disconnecting.
    Blackout,
    /// Leave whatever frame OpenRGB last received.
    LeaveLastFrame,
}

/// Confidence assigned to OpenRGB identity data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityConfidence {
    High,
    Medium,
    Low,
}

impl IdentityConfidence {
    const fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

/// OpenRGB driver module.
#[derive(Debug, Clone, Default)]
pub struct OpenRgbDriverModule;

impl DriverModule for OpenRgbDriverModule {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn module_descriptor(&self) -> DriverModuleDescriptor {
        let mut descriptor = self.descriptor().module_descriptor();
        descriptor.module_kind = DriverModuleKind::Bridge;
        descriptor.transports = vec![DriverTransportKind::Bridge];
        descriptor.capabilities = DriverCapabilitySet {
            config: true,
            discovery: true,
            output_backend: true,
            presentation: true,
            ..DriverCapabilitySet::empty()
        };
        descriptor.default_enabled = false;
        descriptor
    }

    fn has_output_backend(&self) -> bool {
        true
    }

    fn build_output_backend(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        Ok(Some(Box::new(OpenRgbBackend::new(
            config.parse_settings::<OpenRgbConfig>()?,
        )?)))
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }

    fn config(&self) -> Option<&dyn DriverConfigProvider> {
        Some(self)
    }

    fn presentation(&self) -> Option<&dyn DriverPresentationProvider> {
        Some(self)
    }
}

impl DriverPresentationProvider for OpenRgbDriverModule {
    fn presentation(&self) -> DriverPresentation {
        DriverPresentation {
            label: "OpenRGB Fallback".to_owned(),
            short_label: Some("OpenRGB".to_owned()),
            accent_rgb: Some([128, 255, 234]),
            secondary_rgb: Some([225, 53, 255]),
            icon: Some("bridge".to_owned()),
            default_device_class: Some(DeviceClassHint::Controller),
        }
    }
}

impl DriverConfigProvider for OpenRgbDriverModule {
    fn default_config(&self) -> DriverConfigEntry {
        DriverConfigEntry::disabled(openrgb_config_settings(&OpenRgbConfig::default()))
    }

    fn validate_config(&self, config: &DriverConfigEntry) -> Result<()> {
        let config = DriverConfigView {
            driver_id: DESCRIPTOR.id,
            entry: config,
        }
        .parse_settings::<OpenRgbConfig>()?;
        validate_openrgb_config(&config)
    }
}

#[async_trait]
impl DiscoveryCapability for OpenRgbDriverModule {
    async fn discover(
        &self,
        _host: &dyn DriverHost,
        _request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> Result<DiscoveryResult> {
        let config = config.parse_settings::<OpenRgbConfig>()?;
        validate_openrgb_config(&config)?;
        let routes = discover_routes(&config).await?;
        Ok(DiscoveryResult {
            devices: routes
                .into_iter()
                .map(DiscoveredDevice::from)
                .map(DriverDiscoveredDevice::from)
                .collect(),
        })
    }
}

/// Runtime backend for OpenRGB-proxied output.
pub struct OpenRgbBackend {
    config: OpenRgbConfig,
    discovered: HashMap<DeviceId, ControllerRoute>,
    connected: HashMap<DeviceId, ConnectedOutput>,
}

impl OpenRgbBackend {
    /// Create an OpenRGB backend.
    ///
    /// # Errors
    ///
    /// Returns an error when configuration is invalid.
    pub fn new(config: OpenRgbConfig) -> Result<Self> {
        validate_openrgb_config(&config)?;
        Ok(Self {
            config,
            discovered: HashMap::new(),
            connected: HashMap::new(),
        })
    }
}

#[async_trait]
impl DeviceBackend for OpenRgbBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: DESCRIPTOR.id.to_owned(),
            name: "OpenRGB Fallback".to_owned(),
            description: "Out-of-process OpenRGB SDK bridge for fallback hardware coverage"
                .to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        let mut routes = discover_routes(&self.config).await?;
        self.preserve_connected_previous_modes(&mut routes).await;
        self.discovered = routes
            .iter()
            .cloned()
            .map(|route| (route.info.id, route))
            .collect();
        Ok(routes.into_iter().map(|route| route.info).collect())
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        let route = self
            .discovered
            .get(id)
            .cloned()
            .with_context(|| format!("OpenRGB controller {id} is not discovered"))?;
        if let Some(reason) = &route.disabled_reason {
            bail!("OpenRGB controller {id} is output-disabled: {reason}");
        }

        let mut client = connect_openrgb_client(&self.config, route.endpoint).await?;
        let route = find_current_route(
            &mut client,
            route.endpoint,
            &route.fingerprint,
            &self.config,
        )
        .await?;
        ensure_route_output_enabled(&route)?;
        configure_controller_output(&mut client, &route, &self.config).await?;

        let controller = Arc::new(Mutex::new(ConnectedController {
            previous_mode: route.previous_mode.clone(),
            route,
            client,
            config: self.config.clone(),
            accepting_frames: true,
            consecutive_failures: 0,
            reconnect_backoff: ReconnectBackoff::default(),
        }));
        self.connected
            .insert(*id, ConnectedOutput::spawn(controller));
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        if let Some(output) = self.connected.remove(id) {
            let controller = output.stop().await;
            let mut controller = controller.lock().await;
            controller.accepting_frames = false;
            if let Err(error) = teardown_connected_controller(&mut controller).await {
                debug!(
                    device_id = %id,
                    error = %error,
                    "OpenRGB teardown failed during disconnect"
                );
            }
        }
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let Some(output) = self.connected.get(id) else {
            bail!("OpenRGB controller {id} is not connected");
        };
        output.enqueue_colors(Arc::new(colors.to_vec()))
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        self.connected
            .get(id)
            .and_then(|output| {
                output
                    .controller
                    .try_lock()
                    .ok()
                    .map(|controller| controller.route.target_fps)
            })
            .or_else(|| self.discovered.get(id).map(|route| route.target_fps))
    }

    async fn health_check(&self, id: &DeviceId) -> Result<HealthStatus> {
        if let Some(output) = self.connected.get(id) {
            let controller = output.controller.lock().await;
            if controller.consecutive_failures == 0 {
                return Ok(HealthStatus::Healthy);
            }
            return Ok(HealthStatus::Degraded);
        }
        if self.discovered.contains_key(id) {
            return Ok(HealthStatus::Degraded);
        }
        Ok(HealthStatus::Unreachable)
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.connected
            .get(id)
            .map(|output| output.frame_sink() as Arc<dyn DeviceFrameSink>)
    }
}

impl OpenRgbBackend {
    async fn preserve_connected_previous_modes(&self, routes: &mut [ControllerRoute]) {
        for route in routes {
            let Some(controller) = self.connected.get(&route.info.id) else {
                continue;
            };
            let controller = controller.controller.lock().await;
            route.previous_mode.clone_from(&controller.previous_mode);
        }
    }
}

struct ConnectedOutput {
    controller: Arc<Mutex<ConnectedController>>,
    frame_tx: watch::Sender<Option<Arc<OpenRgbFramePayload>>>,
    io_task: Option<JoinHandle<()>>,
    active: Arc<AtomicBool>,
    last_async_error: Arc<StdMutex<Option<String>>>,
}

impl ConnectedOutput {
    fn spawn(controller: Arc<Mutex<ConnectedController>>) -> Self {
        let (frame_tx, frame_rx) = watch::channel(None::<Arc<OpenRgbFramePayload>>);
        let active = Arc::new(AtomicBool::new(true));
        let last_async_error = Arc::new(StdMutex::new(None::<String>));
        let io_task = tokio::spawn(run_openrgb_output_worker(
            Arc::clone(&controller),
            frame_rx,
            Arc::clone(&active),
            Arc::clone(&last_async_error),
        ));

        Self {
            controller,
            frame_tx,
            io_task: Some(io_task),
            active,
            last_async_error,
        }
    }

    fn frame_sink(&self) -> Arc<OpenRgbFrameSink> {
        Arc::new(OpenRgbFrameSink {
            frame_tx: self.frame_tx.clone(),
            active: Arc::clone(&self.active),
            last_async_error: Arc::clone(&self.last_async_error),
        })
    }

    fn enqueue_colors(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<()> {
        enqueue_openrgb_payload(
            &self.frame_tx,
            &self.active,
            &self.last_async_error,
            Arc::new(OpenRgbFramePayload::untracked(colors)),
        )
    }

    async fn stop(mut self) -> Arc<Mutex<ConnectedController>> {
        self.active.store(false, Ordering::Release);
        {
            let mut controller = self.controller.lock().await;
            controller.accepting_frames = false;
        }
        let controller = Arc::clone(&self.controller);
        if let Some(pending) = self.frame_tx.send_replace(None) {
            pending.reject_pending("OpenRGB controller disconnected before transport started");
        }
        if let Some(mut io_task) = self.io_task.take() {
            tokio::select! {
                result = &mut io_task => {
                    if let Err(error) = result {
                        debug!(error = %error, "OpenRGB output worker did not stop cleanly");
                    }
                }
                () = sleep(OUTPUT_WORKER_STOP_TIMEOUT) => {
                    io_task.abort();
                    let _ = io_task.await;
                    debug!(
                        timeout_ms = OUTPUT_WORKER_STOP_TIMEOUT.as_millis(),
                        "OpenRGB output worker stop timed out"
                    );
                }
            }
        }
        controller
    }
}

impl Drop for ConnectedOutput {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        if let Some(pending) = self.frame_tx.send_replace(None) {
            pending.reject_pending("OpenRGB output worker stopped before transport started");
        }
        if let Some(io_task) = &self.io_task {
            io_task.abort();
        }
    }
}

#[derive(Debug)]
struct OpenRgbFramePayload {
    colors: Arc<Vec<[u8; 3]>>,
    delivery_id: Option<DeviceDeliveryId>,
    delivery_tx: StdMutex<Option<oneshot::Sender<DeviceDeliveryAck>>>,
    delivery_state: AtomicU8,
}

impl OpenRgbFramePayload {
    fn untracked(colors: Arc<Vec<[u8; 3]>>) -> Self {
        Self {
            colors,
            delivery_id: None,
            delivery_tx: StdMutex::new(None),
            delivery_state: AtomicU8::new(DELIVERY_PENDING),
        }
    }

    fn tracked(
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> (Self, oneshot::Receiver<DeviceDeliveryAck>) {
        let (delivery_tx, delivery_rx) = oneshot::channel();
        (
            Self {
                colors,
                delivery_id: Some(id),
                delivery_tx: StdMutex::new(Some(delivery_tx)),
                delivery_state: AtomicU8::new(DELIVERY_PENDING),
            },
            delivery_rx,
        )
    }

    fn mark_transport_started(&self) -> bool {
        self.delivery_id.is_none()
            || self
                .delivery_state
                .compare_exchange(
                    DELIVERY_PENDING,
                    DELIVERY_STARTED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
    }

    fn acknowledge(&self, ack: DeviceDeliveryAck) {
        if let Ok(mut delivery_tx) = self.delivery_tx.lock()
            && let Some(delivery_tx) = delivery_tx.take()
        {
            let _ = delivery_tx.send(ack);
        }
    }

    fn reject_pending(&self, error: impl Into<String>) {
        let Some(id) = self.delivery_id else {
            return;
        };
        if self
            .delivery_state
            .compare_exchange(
                DELIVERY_PENDING,
                DELIVERY_REJECTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return;
        }
        self.acknowledge(DeviceDeliveryAck::rejected(id, error));
    }
}

struct OpenRgbFrameSink {
    frame_tx: watch::Sender<Option<Arc<OpenRgbFramePayload>>>,
    active: Arc<AtomicBool>,
    last_async_error: Arc<StdMutex<Option<String>>>,
}

#[async_trait]
impl DeviceFrameSink for OpenRgbFrameSink {
    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<()> {
        enqueue_openrgb_payload(
            &self.frame_tx,
            &self.active,
            &self.last_async_error,
            Arc::new(OpenRgbFramePayload::untracked(colors)),
        )
    }

    async fn deliver_colors_shared(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> DeviceDeliveryAck {
        let (payload, delivery_rx) = OpenRgbFramePayload::tracked(id, colors);
        if let Err(error) = enqueue_openrgb_payload(
            &self.frame_tx,
            &self.active,
            &self.last_async_error,
            Arc::new(payload),
        ) {
            return DeviceDeliveryAck::rejected(id, error.to_string());
        }

        delivery_rx.await.unwrap_or_else(|_| {
            DeviceDeliveryAck::rejected(
                id,
                "OpenRGB output worker terminated before acknowledging delivery",
            )
        })
    }
}

fn enqueue_openrgb_payload(
    frame_tx: &watch::Sender<Option<Arc<OpenRgbFramePayload>>>,
    active: &AtomicBool,
    last_async_error: &StdMutex<Option<String>>,
    payload: Arc<OpenRgbFramePayload>,
) -> Result<()> {
    if !active.load(Ordering::Acquire) {
        bail!("OpenRGB controller is disconnected");
    }
    if let Some(error) = last_async_error
        .lock()
        .map_err(|_| anyhow::anyhow!("OpenRGB async error state lock poisoned"))?
        .take()
    {
        bail!("{error}");
    }
    if let Some(previous) = frame_tx.send_replace(Some(payload)) {
        previous.reject_pending("OpenRGB frame was superseded before transport started");
    }
    Ok(())
}

async fn run_openrgb_output_worker(
    controller: Arc<Mutex<ConnectedController>>,
    mut frame_rx: watch::Receiver<Option<Arc<OpenRgbFramePayload>>>,
    active: Arc<AtomicBool>,
    last_async_error: Arc<StdMutex<Option<String>>>,
) {
    loop {
        if frame_rx.changed().await.is_err() {
            break;
        }
        if !active.load(Ordering::Acquire) {
            break;
        }
        let Some(mut frame) = frame_rx.borrow_and_update().clone() else {
            break;
        };
        while frame_rx.has_changed().unwrap_or(false) {
            if frame_rx.changed().await.is_err() {
                return;
            }
            if !active.load(Ordering::Acquire) {
                return;
            }
            let Some(latest) = frame_rx.borrow_and_update().clone() else {
                return;
            };
            frame = latest;
        }

        if !frame.mark_transport_started() {
            continue;
        }
        let transport_started_at = Instant::now();
        match write_controller_colors(&controller, frame.colors.as_slice()).await {
            Ok(()) => {
                if let Some(id) = frame.delivery_id {
                    frame.acknowledge(DeviceDeliveryAck::completed(
                        id,
                        frame.colors.len().saturating_mul(3),
                        transport_started_at.elapsed(),
                    ));
                }
                if let Ok(mut error) = last_async_error.lock() {
                    *error = None;
                }
            }
            Err(error) => {
                let error = error.to_string();
                if let Some(id) = frame.delivery_id {
                    frame.acknowledge(DeviceDeliveryAck::failed(
                        id,
                        true,
                        transport_started_at.elapsed(),
                        error,
                    ));
                    if let Ok(mut last_error) = last_async_error.lock() {
                        *last_error = None;
                    }
                } else if let Ok(mut last_error) = last_async_error.lock() {
                    *last_error = Some(error);
                }
            }
        }
    }
}

async fn write_controller_colors(
    controller: &Arc<Mutex<ConnectedController>>,
    colors: &[[u8; 3]],
) -> Result<()> {
    let mut controller = controller.lock().await;
    if !controller.accepting_frames {
        bail!("OpenRGB controller is disconnected");
    }
    prepare_controller_for_write(&mut controller).await?;

    let colors = colors
        .iter()
        .map(|[red, green, blue]| RgbColor::new(*red, *green, *blue))
        .collect::<Vec<_>>();
    match write_prepared_controller_colors(&mut controller, &colors).await {
        Ok(()) => {
            controller.consecutive_failures = 0;
            Ok(())
        }
        Err(error) => {
            controller.consecutive_failures = controller.consecutive_failures.saturating_add(1);
            let write_error = Error::new(error).context("OpenRGB update_leds failed");
            reconnect_connected_controller(&mut controller)
                .await
                .map_err(|reconnect_error| {
                    write_error.context(format!("OpenRGB reconnect failed: {reconnect_error}"))
                })?;
            write_prepared_controller_colors(&mut controller, &colors)
                .await
                .map_err(|retry_error| {
                    controller.consecutive_failures =
                        controller.consecutive_failures.saturating_add(1);
                    Error::new(retry_error).context("OpenRGB update_leds failed after reconnect")
                })?;
            controller.consecutive_failures = 0;
            Ok(())
        }
    }
}

async fn prepare_controller_for_write(controller: &mut ConnectedController) -> Result<()> {
    let packets = match controller.client.drain_pending_packets() {
        Ok(packets) => packets,
        Err(error) => {
            controller.consecutive_failures = controller.consecutive_failures.saturating_add(1);
            reconnect_connected_controller(controller)
                .await
                .with_context(|| format!("OpenRGB notification drain failed: {error}"))?;
            return Ok(());
        }
    };
    if packets
        .iter()
        .any(|packet| packet.header.packet_id == PacketId::DeviceListUpdated)
        && let Err(error) = refresh_connected_route(controller).await
    {
        controller.consecutive_failures = controller.consecutive_failures.saturating_add(1);
        return Err(error);
    }
    Ok(())
}

async fn write_prepared_controller_colors(
    controller: &mut ConnectedController,
    colors: &[RgbColor],
) -> hypercolor_openrgb_sdk::Result<()> {
    controller
        .client
        .update_leds(controller.route.controller_index, colors)
        .await
}

async fn reconnect_connected_controller(controller: &mut ConnectedController) -> Result<()> {
    let now = Instant::now();
    if !controller.reconnect_backoff.can_attempt(now) {
        bail!(
            "OpenRGB reconnect backoff is active for {} ms",
            controller.reconnect_backoff.remaining(now).as_millis()
        );
    }

    let mut client =
        match connect_openrgb_client(&controller.config, controller.route.endpoint).await {
            Ok(client) => client,
            Err(error) => {
                controller.reconnect_backoff.record_failure(now);
                return Err(error);
            }
        };
    let route_result = find_current_route(
        &mut client,
        controller.route.endpoint,
        &controller.route.fingerprint,
        &controller.config,
    )
    .await
    .and_then(|route| {
        ensure_route_output_enabled(&route)?;
        Ok(route)
    });
    let route = match route_result {
        Ok(route) => route,
        Err(error) => {
            controller.reconnect_backoff.record_failure(now);
            return Err(error);
        }
    };
    if let Err(error) = configure_controller_output(&mut client, &route, &controller.config).await {
        controller.reconnect_backoff.record_failure(now);
        return Err(error);
    }
    controller.client = client;
    controller.route = route;
    controller.consecutive_failures = 0;
    controller.reconnect_backoff.reset();
    Ok(())
}

async fn refresh_connected_route(controller: &mut ConnectedController) -> Result<()> {
    let route = find_current_route(
        &mut controller.client,
        controller.route.endpoint,
        &controller.route.fingerprint,
        &controller.config,
    )
    .await?;
    ensure_route_output_enabled(&route)?;
    configure_controller_output(&mut controller.client, &route, &controller.config).await?;
    controller.route = route;
    controller.consecutive_failures = 0;
    Ok(())
}

struct ConnectedController {
    previous_mode: Option<(u32, ControllerMode)>,
    route: ControllerRoute,
    client: OpenRgbClient,
    config: OpenRgbConfig,
    accepting_frames: bool,
    consecutive_failures: u32,
    reconnect_backoff: ReconnectBackoff,
}

#[derive(Debug, Clone)]
struct ReconnectBackoff {
    next_attempt_at: Option<Instant>,
    next_delay: Duration,
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self {
            next_attempt_at: None,
            next_delay: INITIAL_RECONNECT_BACKOFF,
        }
    }
}

impl ReconnectBackoff {
    fn can_attempt(&self, now: Instant) -> bool {
        self.next_attempt_at.is_none_or(|deadline| now >= deadline)
    }

    fn remaining(&self, now: Instant) -> Duration {
        self.next_attempt_at
            .and_then(|deadline| deadline.checked_duration_since(now))
            .unwrap_or_default()
    }

    fn record_failure(&mut self, now: Instant) {
        self.next_attempt_at = Some(now + self.next_delay);
        self.next_delay = self
            .next_delay
            .checked_mul(2)
            .map_or(MAX_RECONNECT_BACKOFF, |delay| {
                delay.min(MAX_RECONNECT_BACKOFF)
            });
    }

    fn reset(&mut self) {
        self.next_attempt_at = None;
        self.next_delay = INITIAL_RECONNECT_BACKOFF;
    }
}

#[derive(Debug, Clone)]
struct ControllerRoute {
    endpoint: SocketAddr,
    controller_index: u32,
    fingerprint: DeviceFingerprint,
    confidence: IdentityConfidence,
    detector_class: String,
    disabled_reason: Option<String>,
    writable_mode: Option<(u32, ControllerMode)>,
    previous_mode: Option<(u32, ControllerMode)>,
    target_fps: u32,
    auto_connect: bool,
    info: DeviceInfo,
    protocol_version: u32,
}

impl From<ControllerRoute> for DiscoveredDevice {
    fn from(route: ControllerRoute) -> Self {
        let mut metadata = HashMap::from([
            (METADATA_ENDPOINT.to_owned(), route.endpoint.to_string()),
            (
                METADATA_CONTROLLER_INDEX.to_owned(),
                route.controller_index.to_string(),
            ),
            (METADATA_FINGERPRINT.to_owned(), route.fingerprint.0.clone()),
            (
                METADATA_IDENTITY_CONFIDENCE.to_owned(),
                route.confidence.as_str().to_owned(),
            ),
            (METADATA_DETECTOR_CLASS.to_owned(), route.detector_class),
            (
                METADATA_OUTPUT_ENABLED.to_owned(),
                route.disabled_reason.is_none().to_string(),
            ),
            (
                METADATA_PROTOCOL_VERSION.to_owned(),
                route.protocol_version.to_string(),
            ),
        ]);
        if let Some(reason) = &route.disabled_reason {
            metadata.insert(METADATA_DISABLED_REASON.to_owned(), reason.clone());
        }
        Self {
            fingerprint: route.fingerprint,
            connect_behavior: if route.disabled_reason.is_none() && route.auto_connect {
                DiscoveryConnectBehavior::AutoConnect
            } else {
                DiscoveryConnectBehavior::Deferred
            },
            info: route.info,
            metadata,
        }
    }
}

async fn discover_routes(config: &OpenRgbConfig) -> Result<Vec<ControllerRoute>> {
    if config.ownership.mode == OpenRgbOwnershipMode::Disabled {
        return Ok(Vec::new());
    }

    let mut routes = Vec::new();
    for endpoint in &config.endpoints {
        match discover_endpoint(*endpoint, config).await {
            Ok(mut endpoint_routes) => routes.append(&mut endpoint_routes),
            Err(error) => {
                debug!(endpoint = %endpoint, error = %error, "OpenRGB endpoint discovery failed");
            }
        }
    }
    routes.sort_by_key(|route| route.info.id.to_string());
    Ok(routes)
}

async fn discover_endpoint(
    endpoint: SocketAddr,
    config: &OpenRgbConfig,
) -> Result<Vec<ControllerRoute>> {
    let mut client = connect_openrgb_client(config, endpoint).await?;

    let protocol_version = client.protocol_version();
    let count = client.controller_count().await?;
    let count = if count > MAX_CONTROLLERS_PER_ENDPOINT {
        debug!(
            endpoint = %endpoint,
            reported = count,
            max = MAX_CONTROLLERS_PER_ENDPOINT,
            "OpenRGB endpoint reported excessive controller count"
        );
        MAX_CONTROLLERS_PER_ENDPOINT
    } else {
        count
    };
    let mut routes = Vec::new();
    for controller_index in 0..count {
        let controller = match client.controller_data(controller_index).await {
            Ok(controller) => controller,
            Err(error) => {
                debug!(
                    endpoint = %endpoint,
                    controller_index,
                    error = %error,
                    "OpenRGB controller data parse failed"
                );
                continue;
            }
        };
        routes.push(build_route(
            endpoint,
            controller_index,
            protocol_version,
            controller,
            config,
        ));
    }
    disambiguate_duplicate_fingerprints(&mut routes);
    Ok(routes)
}

async fn connect_openrgb_client(
    config: &OpenRgbConfig,
    endpoint: SocketAddr,
) -> Result<OpenRgbClient> {
    let mut client = OpenRgbClient::connect(endpoint, client_config(config)).await?;
    if config.startup_rescan && client.supports_device_rescan() {
        client.request_rescan().await?;
    }
    Ok(client)
}

async fn find_current_route(
    client: &mut OpenRgbClient,
    endpoint: SocketAddr,
    fingerprint: &DeviceFingerprint,
    config: &OpenRgbConfig,
) -> Result<ControllerRoute> {
    let protocol_version = client.protocol_version();
    let count = client.controller_count().await?;
    let count = count.min(MAX_CONTROLLERS_PER_ENDPOINT);
    let mut routes = Vec::new();
    for controller_index in 0..count {
        let controller = match client.controller_data(controller_index).await {
            Ok(controller) => controller,
            Err(error) => {
                debug!(
                    endpoint = %endpoint,
                    controller_index,
                    error = %error,
                    "OpenRGB controller data parse failed during remap"
                );
                continue;
            }
        };
        routes.push(build_route(
            endpoint,
            controller_index,
            protocol_version,
            controller,
            config,
        ));
    }
    disambiguate_duplicate_fingerprints(&mut routes);
    if let Some(route) = routes
        .into_iter()
        .find(|route| route.fingerprint == *fingerprint)
    {
        return Ok(route);
    }
    bail!(
        "OpenRGB controller fingerprint '{}' disappeared",
        fingerprint.0
    )
}

async fn configure_controller_output(
    client: &mut OpenRgbClient,
    route: &ControllerRoute,
    config: &OpenRgbConfig,
) -> Result<()> {
    client.set_custom_mode(route.controller_index).await?;
    if let Some((mode_index, mode)) = route.writable_mode.clone() {
        client
            .update_mode(route.controller_index, mode_index, &mode)
            .await?;
    }
    verify_controller_output_mode(client, route, mode_flag_policy(config)).await?;
    Ok(())
}

async fn verify_controller_output_mode(
    client: &mut OpenRgbClient,
    route: &ControllerRoute,
    policy: ModeFlagPolicy,
) -> Result<()> {
    let controller = match client.controller_data(route.controller_index).await {
        Ok(controller) => controller,
        Err(error) => {
            debug!(
                controller_index = route.controller_index,
                error = %error,
                "OpenRGB active mode readback unavailable after output setup"
            );
            return Ok(());
        }
    };
    let Some((mode_index, mode)) = active_mode_snapshot(&controller) else {
        bail!(
            "OpenRGB controller {} has no active mode after output setup",
            route.info.id
        );
    };
    if mode.is_realtime_writable(policy) {
        return Ok(());
    }
    bail!(
        "OpenRGB controller {} active mode {mode_index} is not approved for realtime output",
        route.info.id
    )
}

async fn teardown_connected_controller(controller: &mut ConnectedController) -> Result<()> {
    match controller.config.teardown_policy {
        OpenRgbTeardownPolicy::RestorePreviousOrLeave => {
            if let Err(error) = try_restore_previous_mode(controller).await {
                debug!(
                    controller_index = controller.route.controller_index,
                    error = %error,
                    "OpenRGB previous mode restore failed; leaving last frame"
                );
            }
        }
        OpenRgbTeardownPolicy::RestorePreviousOrBlackout => {
            match try_restore_previous_mode(controller).await {
                Ok(true) => {}
                Ok(false) => {
                    blackout_controller(controller).await?;
                }
                Err(error) => {
                    debug!(
                        controller_index = controller.route.controller_index,
                        error = %error,
                        "OpenRGB previous mode restore failed; blacking out"
                    );
                    blackout_controller(controller).await?;
                }
            }
        }
        OpenRgbTeardownPolicy::Blackout => {
            blackout_controller(controller).await?;
        }
        OpenRgbTeardownPolicy::LeaveLastFrame => {}
    }
    Ok(())
}

async fn try_restore_previous_mode(controller: &mut ConnectedController) -> Result<bool> {
    let Some((mode_index, mode)) = controller.previous_mode.clone() else {
        return Ok(false);
    };
    controller
        .client
        .update_mode(controller.route.controller_index, mode_index, &mode)
        .await
        .context("OpenRGB previous mode restore failed")?;
    Ok(true)
}

async fn blackout_controller(controller: &mut ConnectedController) -> Result<()> {
    let led_count = usize::try_from(controller.route.info.capabilities.led_count)
        .context("OpenRGB LED count does not fit usize")?;
    let colors = vec![RgbColor::new(0, 0, 0); led_count];
    controller
        .client
        .update_leds(controller.route.controller_index, &colors)
        .await
        .context("OpenRGB blackout teardown failed")
}

fn ensure_route_output_enabled(route: &ControllerRoute) -> Result<()> {
    if let Some(reason) = &route.disabled_reason {
        bail!(
            "OpenRGB controller {} is output-disabled after refresh: {reason}",
            route.info.id
        );
    }
    Ok(())
}

fn disambiguate_duplicate_fingerprints(routes: &mut [ControllerRoute]) {
    let mut counts = HashMap::<DeviceFingerprint, usize>::new();
    for route in routes.iter() {
        *counts.entry(route.fingerprint.clone()).or_default() += 1;
    }

    let mut seen = HashMap::<DeviceFingerprint, usize>::new();
    for route in routes.iter_mut() {
        if counts.get(&route.fingerprint).copied().unwrap_or_default() <= 1 {
            continue;
        }
        let ordinal = seen.entry(route.fingerprint.clone()).or_default();
        *ordinal += 1;
        let base = route.fingerprint.0.clone();
        route.fingerprint = DeviceFingerprint(format!("{base}:duplicate:{ordinal}"));
        route.info.id = route.fingerprint.stable_device_id();
        route.info.capabilities.supports_direct = false;
        route.disabled_reason = Some(
            "OpenRGB identity collides with another controller; assign a stable identity"
                .to_owned(),
        );
    }
}

fn build_route(
    endpoint: SocketAddr,
    controller_index: u32,
    protocol_version: u32,
    controller: ControllerData,
    config: &OpenRgbConfig,
) -> ControllerRoute {
    let confidence = identity_confidence(&controller);
    let detector_class = detector_class(&controller.device_type).to_owned();
    let writable_mode = select_writable_mode(&controller, mode_flag_policy(config));
    let disabled_reason = output_disabled_reason(
        &config.ownership,
        confidence,
        &detector_class,
        writable_mode.as_ref(),
    )
    .or_else(|| topology_disabled_reason(&controller));
    let previous_mode = previous_mode_snapshot(&controller, writable_mode.as_ref());
    let fingerprint = controller_fingerprint(endpoint, &controller, confidence, controller_index);
    let device_id = fingerprint.stable_device_id();
    let target_fps = target_fps(config, &fingerprint.0, &detector_class);
    let info = build_device_info(
        device_id,
        &controller,
        &fingerprint,
        confidence,
        &detector_class,
        target_fps,
        disabled_reason.is_none(),
    );

    ControllerRoute {
        endpoint,
        controller_index,
        fingerprint,
        confidence,
        detector_class,
        disabled_reason,
        writable_mode,
        previous_mode,
        target_fps,
        auto_connect: config.auto_connect,
        info,
        protocol_version,
    }
}

/// Classify controller identity confidence.
#[must_use]
pub fn identity_confidence(controller: &ControllerData) -> IdentityConfidence {
    let has_vendor_name =
        !controller.vendor.trim().is_empty() && !controller.name.trim().is_empty();
    if (!controller.serial.trim().is_empty() || !controller.location.trim().is_empty())
        && has_vendor_name
    {
        IdentityConfidence::High
    } else if has_vendor_name && !controller.zones.is_empty() && !controller.leds.is_empty() {
        IdentityConfidence::Medium
    } else {
        IdentityConfidence::Low
    }
}

fn controller_fingerprint(
    endpoint: SocketAddr,
    controller: &ControllerData,
    confidence: IdentityConfidence,
    controller_index: u32,
) -> DeviceFingerprint {
    let identity = if !controller.serial.trim().is_empty() {
        format!("serial:{}", controller.serial.trim())
    } else if !controller.location.trim().is_empty() {
        format!("location:{}", controller.location.trim())
    } else if confidence == IdentityConfidence::Medium {
        format!(
            "shape:{}:{}:{}:{}",
            controller.vendor.trim(),
            controller.name.trim(),
            controller.zones.len(),
            controller.leds.len()
        )
    } else {
        format!("unstable-index:{controller_index}")
    };
    DeviceFingerprint(format!("bridge:openrgb:{endpoint}:{identity}"))
}

fn select_writable_mode(
    controller: &ControllerData,
    policy: ModeFlagPolicy,
) -> Option<(u32, ControllerMode)> {
    controller
        .modes
        .iter()
        .enumerate()
        .find(|(_, mode)| mode.is_realtime_writable(policy))
        .and_then(|(index, mode)| u32::try_from(index).ok().map(|index| (index, mode.clone())))
}

fn mode_flag_policy(config: &OpenRgbConfig) -> ModeFlagPolicy {
    ModeFlagPolicy {
        per_led_color_mask: config.mode_per_led_mask,
        persistent_mask: config.mode_persistent_mask,
    }
}

fn active_mode_snapshot(controller: &ControllerData) -> Option<(u32, ControllerMode)> {
    let index = u32::try_from(controller.active_mode).ok()?;
    let mode = controller.modes.get(usize::try_from(index).ok()?)?.clone();
    Some((index, mode))
}

fn previous_mode_snapshot(
    controller: &ControllerData,
    writable_mode: Option<&(u32, ControllerMode)>,
) -> Option<(u32, ControllerMode)> {
    let snapshot = active_mode_snapshot(controller)?;
    if writable_mode.is_some_and(|(index, _)| snapshot.0 == *index) {
        return None;
    }
    Some(snapshot)
}

fn output_disabled_reason(
    ownership: &OpenRgbOwnership,
    confidence: IdentityConfidence,
    detector_class: &str,
    writable_mode: Option<&(u32, ControllerMode)>,
) -> Option<String> {
    if ownership.mode == OpenRgbOwnershipMode::Disabled {
        return Some("OpenRGB fallback ownership is disabled".to_owned());
    }
    if ownership.mode == OpenRgbOwnershipMode::DetectorPartitioned {
        let allowed = normalized_set(&ownership.allowed_detector_classes);
        if !allowed.contains(detector_class) {
            return Some(format!(
                "OpenRGB detector class '{detector_class}' is not in the ownership partition"
            ));
        }
    }
    let native_claimed = normalized_set(&ownership.native_claimed_detector_classes);
    if native_claimed.contains(detector_class) {
        return Some(format!(
            "OpenRGB detector class '{detector_class}' is reserved for native Hypercolor drivers"
        ));
    }
    if confidence == IdentityConfidence::Low {
        if detector_class == "hid" || detector_class == "smbus" {
            return Some(
                "OpenRGB index-only identity is not safe for contention-prone output".to_owned(),
            );
        }
        if !ownership.allow_low_confidence {
            return Some(
                "OpenRGB identity confidence is low; assign ownership explicitly".to_owned(),
            );
        }
    }
    if writable_mode.is_none() {
        return Some("OpenRGB controller has no approved per-LED writable mode".to_owned());
    }
    None
}

fn topology_disabled_reason(controller: &ControllerData) -> Option<String> {
    let Some(reported_zone_led_count) = zone_led_count(controller) else {
        return Some("OpenRGB zone LED count overflowed".to_owned());
    };
    let Some(reported_controller_led_count) = controller_led_count(controller) else {
        return Some("OpenRGB controller LED list count overflowed".to_owned());
    };
    if reported_zone_led_count != reported_controller_led_count {
        return Some(format!(
            "OpenRGB zone LED count {reported_zone_led_count} does not match controller LED list {reported_controller_led_count}"
        ));
    }
    None
}

fn zone_led_count(controller: &ControllerData) -> Option<u32> {
    controller
        .zones
        .iter()
        .try_fold(0_u32, |total, zone| total.checked_add(zone.leds_count))
}

fn controller_led_count(controller: &ControllerData) -> Option<u32> {
    u32::try_from(controller.leds.len()).ok()
}

fn build_device_info(
    id: DeviceId,
    controller: &ControllerData,
    _fingerprint: &DeviceFingerprint,
    _confidence: IdentityConfidence,
    _detector_class: &str,
    target_fps: u32,
    output_enabled: bool,
) -> DeviceInfo {
    let zones = controller.zones.iter().map(zone_info).collect::<Vec<_>>();
    let led_count = controller_led_count(controller).unwrap_or(0);
    let display_name = if controller.vendor.trim().is_empty() {
        controller.name.clone()
    } else {
        format!("{} {}", controller.vendor.trim(), controller.name.trim())
    };

    DeviceInfo {
        id,
        name: display_name.trim().to_owned(),
        vendor: if controller.vendor.trim().is_empty() {
            "OpenRGB".to_owned()
        } else {
            controller.vendor.clone()
        },
        family: DeviceFamily::new(
            device_family_id(&controller.device_type),
            device_family_name(&controller.device_type),
        ),
        model: if controller.description.trim().is_empty() {
            None
        } else {
            Some(controller.description.clone())
        },
        connection_type: ConnectionType::Bridge,
        origin: DeviceOrigin::new(DESCRIPTOR.id, DESCRIPTOR.id, DriverTransportKind::Bridge)
            .with_protocol_id("openrgb-sdk"),
        zones,
        firmware_version: if controller.version.trim().is_empty() {
            None
        } else {
            Some(controller.version.clone())
        },
        capabilities: DeviceCapabilities {
            led_count,
            supports_direct: output_enabled,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: target_fps,
            color_space: DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

fn zone_info(zone: &ControllerZone) -> ZoneInfo {
    let led_count = zone.leds_count;
    let topology = match zone.zone_type {
        hypercolor_openrgb_sdk::ZoneType::Single => DeviceTopologyHint::Point,
        hypercolor_openrgb_sdk::ZoneType::Matrix => {
            if let Some(matrix) = &zone.matrix {
                DeviceTopologyHint::Matrix {
                    rows: matrix.height,
                    cols: matrix.width,
                }
            } else {
                DeviceTopologyHint::Matrix {
                    rows: 1,
                    cols: led_count.max(1),
                }
            }
        }
        hypercolor_openrgb_sdk::ZoneType::Linear | hypercolor_openrgb_sdk::ZoneType::Other(_) => {
            DeviceTopologyHint::Strip
        }
    };
    ZoneInfo {
        name: zone.name.clone(),
        led_count,
        topology,
        color_format: DeviceColorFormat::Rgb,
        layout_hint: None,
    }
}

fn target_fps(config: &OpenRgbConfig, fingerprint: &str, detector_class: &str) -> u32 {
    config
        .controller_fps
        .get(fingerprint)
        .or_else(|| config.controller_fps.get(detector_class))
        .copied()
        .unwrap_or(config.default_target_fps)
        .max(1)
}

fn detector_class(device_type: &DeviceType) -> &'static str {
    match device_type {
        DeviceType::Motherboard | DeviceType::Dram | DeviceType::Gpu => "smbus",
        DeviceType::Virtual | DeviceType::Light => "virtual",
        DeviceType::Unknown | DeviceType::Other(_) => "unknown",
        _ => "hid",
    }
}

fn device_family_id(device_type: &DeviceType) -> &'static str {
    match device_type {
        DeviceType::Motherboard => "openrgb-motherboard",
        DeviceType::Dram => "openrgb-dram",
        DeviceType::Gpu => "openrgb-gpu",
        DeviceType::Cooler => "openrgb-cooler",
        DeviceType::LedStrip => "openrgb-strip",
        DeviceType::Keyboard => "openrgb-keyboard",
        DeviceType::Mouse => "openrgb-mouse",
        DeviceType::Light => "openrgb-light",
        _ => "openrgb-device",
    }
}

fn device_family_name(device_type: &DeviceType) -> &'static str {
    match device_type {
        DeviceType::Motherboard => "OpenRGB Motherboard",
        DeviceType::Dram => "OpenRGB DRAM",
        DeviceType::Gpu => "OpenRGB GPU",
        DeviceType::Cooler => "OpenRGB Cooler",
        DeviceType::LedStrip => "OpenRGB Strip",
        DeviceType::Keyboard => "OpenRGB Keyboard",
        DeviceType::Mouse => "OpenRGB Mouse",
        DeviceType::Light => "OpenRGB Light",
        _ => "OpenRGB Device",
    }
}

fn client_config(config: &OpenRgbConfig) -> OpenRgbClientConfig {
    OpenRgbClientConfig {
        connect_timeout: Duration::from_millis(config.connect_timeout_ms),
        read_timeout: Duration::from_millis(config.read_timeout_ms),
        write_timeout: Duration::from_millis(config.write_timeout_ms),
        ..OpenRgbClientConfig::default()
    }
}

fn validate_openrgb_config(config: &OpenRgbConfig) -> Result<()> {
    if config.endpoints.is_empty() {
        bail!("OpenRGB endpoints must not be empty");
    }
    for endpoint in &config.endpoints {
        if endpoint.port() == 0 {
            bail!("OpenRGB endpoint {endpoint} has invalid port 0");
        }
        if !config.allow_insecure_remote && !endpoint.ip().is_loopback() {
            bail!(
                "OpenRGB endpoint {endpoint} is not loopback; set allow_insecure_remote to opt in"
            );
        }
    }
    for (field, value) in [
        (FIELD_CONNECT_TIMEOUT_MS, config.connect_timeout_ms),
        (FIELD_READ_TIMEOUT_MS, config.read_timeout_ms),
        (FIELD_WRITE_TIMEOUT_MS, config.write_timeout_ms),
    ] {
        if value == 0 || value > MAX_TIMEOUT_MS {
            bail!("OpenRGB {field} must be between 1 and {MAX_TIMEOUT_MS} ms");
        }
    }
    if config.default_target_fps == 0 {
        bail!("OpenRGB default_target_fps must be at least 1");
    }
    if openrgb_detector_partition_needs_confirmation(config) && !config.detector_partition_confirmed
    {
        bail!(
            "OpenRGB detector_partition_confirmed must be true after configuring OpenRGB detectors for the requested ownership partition"
        );
    }
    Ok(())
}

fn openrgb_detector_partition_needs_confirmation(config: &OpenRgbConfig) -> bool {
    if config.ownership.mode == OpenRgbOwnershipMode::Disabled {
        return false;
    }

    config.ownership.mode == OpenRgbOwnershipMode::DetectorPartitioned
        || !normalized_set(&config.ownership.native_claimed_detector_classes).is_empty()
}

fn openrgb_config_settings(config: &OpenRgbConfig) -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        (FIELD_ENDPOINTS.to_owned(), json!(config.endpoints)),
        (
            FIELD_ALLOW_INSECURE_REMOTE.to_owned(),
            json!(config.allow_insecure_remote),
        ),
        (
            FIELD_CONNECT_TIMEOUT_MS.to_owned(),
            json!(config.connect_timeout_ms),
        ),
        (
            FIELD_READ_TIMEOUT_MS.to_owned(),
            json!(config.read_timeout_ms),
        ),
        (
            FIELD_WRITE_TIMEOUT_MS.to_owned(),
            json!(config.write_timeout_ms),
        ),
        (
            FIELD_STARTUP_RESCAN.to_owned(),
            json!(config.startup_rescan),
        ),
        (FIELD_AUTO_CONNECT.to_owned(), json!(config.auto_connect)),
        (FIELD_OWNERSHIP.to_owned(), json!(config.ownership)),
        (
            FIELD_DETECTOR_PARTITION_CONFIRMED.to_owned(),
            json!(config.detector_partition_confirmed),
        ),
        (
            FIELD_DEFAULT_TARGET_FPS.to_owned(),
            json!(config.default_target_fps),
        ),
        (
            FIELD_CONTROLLER_FPS.to_owned(),
            json!(config.controller_fps),
        ),
        (
            FIELD_MODE_PER_LED_MASK.to_owned(),
            json!(config.mode_per_led_mask),
        ),
        (
            FIELD_MODE_PERSISTENT_MASK.to_owned(),
            json!(config.mode_persistent_mask),
        ),
        (
            FIELD_TEARDOWN_POLICY.to_owned(),
            json!(config.teardown_policy),
        ),
    ])
}

fn normalized_set(values: &[String]) -> HashSet<String> {
    values
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

fn default_endpoints() -> Vec<SocketAddr> {
    vec![SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        DEFAULT_OPENRGB_PORT,
    )]
}

const fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

const fn default_target_fps() -> u32 {
    DEFAULT_TARGET_FPS
}

const fn default_auto_connect() -> bool {
    true
}

const fn default_per_led_mask() -> u32 {
    hypercolor_openrgb_sdk::ModeFlag::PerLedColor.mask()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypercolor_openrgb_sdk::{ColorMode, ControllerMode, LedData, ZoneType};

    #[test]
    fn default_config_is_disabled_and_loopback_only() {
        let module = OpenRgbDriverModule;
        let entry = module.default_config();
        assert!(!entry.enabled);
        let config = DriverConfigView {
            driver_id: DESCRIPTOR.id,
            entry: &entry,
        }
        .parse_settings::<OpenRgbConfig>()
        .expect("default config should parse");
        assert_eq!(config.endpoints, default_endpoints());
        assert!(!config.allow_insecure_remote);
        assert!(config.auto_connect);
        assert!(!config.detector_partition_confirmed);
        assert_eq!(
            config.teardown_policy,
            OpenRgbTeardownPolicy::RestorePreviousOrLeave
        );
        validate_openrgb_config(&config).expect("default config should validate");
    }

    #[test]
    fn config_rejects_non_loopback_without_explicit_opt_in() {
        let mut config = OpenRgbConfig {
            endpoints: vec!["192.0.2.10:6742".parse().expect("fixture endpoint")],
            ..OpenRgbConfig::default()
        };

        assert!(validate_openrgb_config(&config).is_err());
        config.allow_insecure_remote = true;
        validate_openrgb_config(&config).expect("explicit opt-in should validate");
    }

    #[test]
    fn config_requires_detector_partition_confirmation_for_partitioned_ownership() {
        let mut config = OpenRgbConfig {
            ownership: OpenRgbOwnership {
                mode: OpenRgbOwnershipMode::DetectorPartitioned,
                allowed_detector_classes: vec!["hid".to_owned()],
                ..OpenRgbOwnership::default()
            },
            ..OpenRgbConfig::default()
        };

        let error =
            validate_openrgb_config(&config).expect_err("unconfirmed partition should fail");
        assert!(error.to_string().contains("detector_partition_confirmed"));

        config.detector_partition_confirmed = true;
        validate_openrgb_config(&config).expect("confirmed partition should validate");
    }

    #[test]
    fn config_requires_detector_partition_confirmation_for_native_claims() {
        let mut config = OpenRgbConfig {
            ownership: OpenRgbOwnership {
                mode: OpenRgbOwnershipMode::OpenRgbOwned,
                native_claimed_detector_classes: vec!["smbus".to_owned()],
                ..OpenRgbOwnership::default()
            },
            ..OpenRgbConfig::default()
        };

        assert!(validate_openrgb_config(&config).is_err());
        config.detector_partition_confirmed = true;
        validate_openrgb_config(&config).expect("confirmed native claims should validate");
    }

    #[test]
    fn openrgb_owned_without_native_claims_does_not_require_partition_confirmation() {
        let config = OpenRgbConfig {
            ownership: OpenRgbOwnership {
                mode: OpenRgbOwnershipMode::OpenRgbOwned,
                ..OpenRgbOwnership::default()
            },
            ..OpenRgbConfig::default()
        };

        assert!(!openrgb_detector_partition_needs_confirmation(&config));
        validate_openrgb_config(&config).expect("owned static partition should validate");
    }

    #[test]
    fn disabled_ownership_never_requires_partition_confirmation() {
        let config = OpenRgbConfig {
            ownership: OpenRgbOwnership {
                mode: OpenRgbOwnershipMode::Disabled,
                native_claimed_detector_classes: vec!["smbus".to_owned()],
                ..OpenRgbOwnership::default()
            },
            detector_partition_confirmed: false,
            ..OpenRgbConfig::default()
        };

        assert!(!openrgb_detector_partition_needs_confirmation(&config));
        validate_openrgb_config(&config).expect("disabled ownership should validate");
    }

    #[test]
    fn identity_confidence_prefers_serial_or_location() {
        let mut controller = sample_controller();
        assert_eq!(identity_confidence(&controller), IdentityConfidence::High);

        controller.serial.clear();
        controller.location.clear();
        assert_eq!(identity_confidence(&controller), IdentityConfidence::Medium);

        controller.vendor.clear();
        assert_eq!(identity_confidence(&controller), IdentityConfidence::Low);
    }

    #[test]
    fn ownership_filter_blocks_low_confidence_by_default() {
        let ownership = OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::OpenRgbOwned,
            ..OpenRgbOwnership::default()
        };

        let reason = output_disabled_reason(
            &ownership,
            IdentityConfidence::Low,
            "virtual",
            Some(&(0, sample_mode())),
        );
        assert!(
            reason
                .expect("low-confidence controller should be disabled")
                .contains("low")
        );
    }

    #[test]
    fn ownership_filter_blocks_index_only_hid_even_with_override() {
        let ownership = OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::OpenRgbOwned,
            allow_low_confidence: true,
            ..OpenRgbOwnership::default()
        };

        let reason = output_disabled_reason(
            &ownership,
            IdentityConfidence::Low,
            "hid",
            Some(&(0, sample_mode())),
        );

        assert!(
            reason
                .expect("index-only HID controller should be disabled")
                .contains("index-only")
        );
        assert!(
            output_disabled_reason(
                &ownership,
                IdentityConfidence::Low,
                "virtual",
                Some(&(0, sample_mode()))
            )
            .is_none()
        );
    }

    #[test]
    fn ownership_partition_allows_configured_detector_class() {
        let ownership = OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::DetectorPartitioned,
            allowed_detector_classes: vec!["hid".to_owned()],
            native_claimed_detector_classes: Vec::new(),
            allow_low_confidence: false,
        };

        assert!(
            output_disabled_reason(
                &ownership,
                IdentityConfidence::High,
                "hid",
                Some(&(0, sample_mode()))
            )
            .is_none()
        );
        assert!(
            output_disabled_reason(
                &ownership,
                IdentityConfidence::High,
                "smbus",
                Some(&(0, sample_mode()))
            )
            .expect("unpartitioned smbus should be disabled")
            .contains("not in the ownership partition")
        );
    }

    #[test]
    fn route_builds_output_disabled_device_for_unwritable_controller() {
        let mut controller = sample_controller();
        controller.modes[0].flags = 0;
        let config = OpenRgbConfig {
            ownership: OpenRgbOwnership {
                mode: OpenRgbOwnershipMode::OpenRgbOwned,
                ..OpenRgbOwnership::default()
            },
            ..OpenRgbConfig::default()
        };

        let route = build_route(default_endpoints()[0], 0, 5, controller, &config);
        assert!(route.disabled_reason.is_some());
        assert!(!route.info.capabilities.supports_direct);
    }

    #[test]
    fn route_disables_output_for_mismatched_led_topology() {
        let mut controller = sample_controller();
        controller.zones[0].leds_count = 5;
        let config = OpenRgbConfig {
            ownership: OpenRgbOwnership {
                mode: OpenRgbOwnershipMode::OpenRgbOwned,
                ..OpenRgbOwnership::default()
            },
            ..OpenRgbConfig::default()
        };

        let route = build_route(default_endpoints()[0], 0, 5, controller, &config);

        assert_eq!(route.info.capabilities.led_count, 4);
        assert!(!route.info.capabilities.supports_direct);
        assert!(
            route
                .disabled_reason
                .expect("mismatched topology should disable output")
                .contains("does not match controller LED list")
        );
    }

    #[test]
    fn auto_connect_false_defers_output_enabled_routes() {
        let config = OpenRgbConfig {
            ownership: OpenRgbOwnership {
                mode: OpenRgbOwnershipMode::OpenRgbOwned,
                ..OpenRgbOwnership::default()
            },
            auto_connect: false,
            ..OpenRgbConfig::default()
        };

        let route = build_route(default_endpoints()[0], 0, 5, sample_controller(), &config);
        assert!(route.disabled_reason.is_none());

        let discovered = DiscoveredDevice::from(route);
        assert_eq!(
            discovered.connect_behavior,
            DiscoveryConnectBehavior::Deferred
        );
    }

    #[test]
    fn duplicate_medium_fingerprints_are_disambiguated_and_disabled() {
        let mut first = sample_controller();
        first.serial.clear();
        first.location.clear();
        let second = first.clone();
        let config = OpenRgbConfig {
            ownership: OpenRgbOwnership {
                mode: OpenRgbOwnershipMode::OpenRgbOwned,
                ..OpenRgbOwnership::default()
            },
            ..OpenRgbConfig::default()
        };
        let endpoint = default_endpoints()[0];
        let mut routes = vec![
            build_route(endpoint, 0, 5, first, &config),
            build_route(endpoint, 1, 5, second, &config),
        ];

        disambiguate_duplicate_fingerprints(&mut routes);

        assert_ne!(routes[0].fingerprint, routes[1].fingerprint);
        assert_ne!(routes[0].info.id, routes[1].info.id);
        for route in routes {
            assert!(
                route
                    .disabled_reason
                    .expect("duplicate fingerprint should be disabled")
                    .contains("collides")
            );
            assert!(!route.info.capabilities.supports_direct);
        }
    }

    #[test]
    fn active_mode_snapshot_uses_nonnegative_mode_index() {
        let mut controller = sample_controller();
        controller.active_mode = 0;
        let snapshot = active_mode_snapshot(&controller).expect("active mode should exist");
        assert_eq!(snapshot.0, 0);
        assert_eq!(snapshot.1.name, "Direct");

        controller.active_mode = -1;
        assert!(active_mode_snapshot(&controller).is_none());

        controller.active_mode = 99;
        assert!(active_mode_snapshot(&controller).is_none());
    }

    #[test]
    fn previous_mode_snapshot_skips_selected_writable_mode() {
        let mut controller = sample_controller();
        let writable_mode = select_writable_mode(
            &controller,
            ModeFlagPolicy {
                per_led_color_mask: default_per_led_mask(),
                persistent_mask: 0,
            },
        )
        .expect("sample controller should have writable mode");

        assert!(previous_mode_snapshot(&controller, Some(&writable_mode)).is_none());

        let mut previous_mode = sample_mode();
        previous_mode.name = "Static".to_owned();
        previous_mode.flags = 0;
        previous_mode.color_mode = ColorMode::ModeSpecific;
        controller.modes.push(previous_mode);
        controller.active_mode = 1;

        let snapshot =
            previous_mode_snapshot(&controller, Some(&writable_mode)).expect("mode should restore");
        assert_eq!(snapshot.0, 1);
        assert_eq!(snapshot.1.name, "Static");
    }

    #[test]
    fn reconnect_backoff_blocks_until_deadline_and_caps_delay() {
        let now = Instant::now();
        let mut backoff = ReconnectBackoff::default();

        assert!(backoff.can_attempt(now));
        backoff.record_failure(now);
        assert!(!backoff.can_attempt(now));
        assert_eq!(backoff.remaining(now), INITIAL_RECONNECT_BACKOFF);
        assert!(backoff.can_attempt(now + INITIAL_RECONNECT_BACKOFF));

        for _ in 0..8 {
            backoff.record_failure(now);
        }
        assert_eq!(backoff.next_delay, MAX_RECONNECT_BACKOFF);

        backoff.reset();
        assert!(backoff.can_attempt(now));
        assert_eq!(backoff.next_delay, INITIAL_RECONNECT_BACKOFF);
    }

    fn sample_controller() -> ControllerData {
        ControllerData {
            device_type: DeviceType::Keyboard,
            name: "Board".to_owned(),
            vendor: "Acme".to_owned(),
            description: "Keyboard".to_owned(),
            version: "1.0".to_owned(),
            serial: "SER123".to_owned(),
            location: "hidraw0".to_owned(),
            active_mode: 0,
            modes: vec![sample_mode()],
            zones: vec![ControllerZone {
                name: "Main".to_owned(),
                zone_type: ZoneType::Linear,
                leds_min: 4,
                leds_max: 4,
                leds_count: 4,
                matrix: None,
                segments: Vec::new(),
                flags: None,
            }],
            leds: vec![
                LedData {
                    name: "0".to_owned(),
                    value: 0,
                },
                LedData {
                    name: "1".to_owned(),
                    value: 1,
                },
                LedData {
                    name: "2".to_owned(),
                    value: 2,
                },
                LedData {
                    name: "3".to_owned(),
                    value: 3,
                },
            ],
            colors: vec![RgbColor::new(1, 2, 3); 4],
            led_alt_names: Vec::new(),
            flags: None,
        }
    }

    fn sample_mode() -> ControllerMode {
        ControllerMode {
            name: "Direct".to_owned(),
            value: 0,
            flags: default_per_led_mask(),
            speed_min: 0,
            speed_max: 100,
            brightness_min: Some(0),
            brightness_max: Some(100),
            colors_min: 0,
            colors_max: 0,
            speed: 0,
            brightness: Some(100),
            direction: 0,
            color_mode: ColorMode::PerLed,
            colors: Vec::new(),
        }
    }
}
