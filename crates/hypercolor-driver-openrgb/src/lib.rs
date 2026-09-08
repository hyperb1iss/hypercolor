//! OpenRGB fallback Bridge driver for Hypercolor.
//!
//! This driver talks to a user-managed OpenRGB SDK server over the clean
//! `hypercolor-openrgb-sdk` crate. It does not supervise or bundle OpenRGB.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex as StdMutex, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Error, Result, bail};
use async_trait::async_trait;
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceBackendFactory, DeviceDeliveryAck, DeviceDeliveryId,
    DeviceDeliveryObserver, DeviceFrameSink, DiscoveredDevice, DiscoveryCapability,
    DiscoveryConnectBehavior, DiscoveryRequest, DriverConfigProvider, DriverConfigView,
    DriverDescriptor, DriverError, DriverHost, DriverModule, DriverPresentationProvider,
    DriverRuntimeActions, OutputBinding,
};
use hypercolor_openrgb_sdk::{
    ControllerData, ControllerMode, ControllerZone, DeviceType, ModeFlagPolicy, OpenRgbClient,
    OpenRgbClientConfig, OpenRgbError, PacketId, RgbColor,
};
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceClassHint, DeviceColorFormat, DeviceColorSpace,
    DeviceError, DeviceFamily, DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo,
    DeviceOrigin, DeviceTopologyHint, DriverCapabilitySet, DriverModuleDescriptor,
    DriverModuleKind, DriverPresentation, DriverTransportDescriptor, DriverTransportKind,
    FingerprintNamespace, SegmentInfo,
};
use hypercolor_types::identity::BackendId;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{Mutex, Notify, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, timeout};
use tracing::{debug, warn};

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
const FIELD_ZONE_SIZES: &str = "zone_sizes";

const METADATA_ENDPOINT: &str = "endpoint";
const METADATA_CONTROLLER_INDEX: &str = "controller_index";
const METADATA_FINGERPRINT: &str = "fingerprint";
const METADATA_IDENTITY_CONFIDENCE: &str = "identity_confidence";
const METADATA_DETECTOR_CLASS: &str = "detector_class";
const METADATA_OUTPUT_ENABLED: &str = "output_enabled";
const METADATA_DISABLED_REASON: &str = "disabled_reason";
const METADATA_PROTOCOL_VERSION: &str = "protocol_version";
/// OpenRGB's location string, verbatim (a hidraw node, an i2c bus, a USB
/// path). Published so the daemon can label the connection.
const METADATA_LOCATION: &str = "location";
/// OpenRGB's serial string, verbatim. The daemon already reads `serial` for
/// connection summaries.
const METADATA_SERIAL: &str = "serial";

const DEFAULT_OPENRGB_PORT: u16 = 6742;
const DEFAULT_TIMEOUT_MS: u64 = 750;
const DEFAULT_TARGET_FPS: u32 = 30;
const ZERO_LEDS_REASON: &str = "OpenRGB controller reports zero LEDs";
/// Frame interval of the native SMBus protocols (`hypercolor-hal` ASUS Aura /
/// ENE at 16 ms). The bridge's `smbus` detector class paces to the same
/// cadence the native backend derives from it, so a DRAM stick behind
/// OpenRGB is never driven faster than the same stick behind the native
/// driver.
const NATIVE_SMBUS_FRAME_INTERVAL_MS: u32 = 16;
/// Detector-class default cadence for `smbus`, matching the native backend's
/// `fps_from_frame_interval(16 ms)`.
pub const SMBUS_DETECTOR_TARGET_FPS: u32 = 1_000 / NATIVE_SMBUS_FRAME_INTERVAL_MS;
const MAX_TIMEOUT_MS: u64 = 10_000;
const MAX_CONTROLLERS_PER_ENDPOINT: u32 = 1024;
const OUTPUT_WORKER_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const DELIVERY_PENDING: u8 = 0;
const DELIVERY_STARTED: u8 = 1;
const DELIVERY_REJECTED: u8 = 2;

/// User-approved zone sizes: controller fingerprint string (as shown in
/// device metadata, matched case-insensitively) to zone name (exactly as
/// OpenRGB reports it) to LED count.
pub type ZoneSizeMap = BTreeMap<String, BTreeMap<String, u32>>;

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
    /// Zone sizes to apply with `RESIZEZONE` on connect. Non-empty enables
    /// the SDK client's zone-resize gate; `SAVEMODE` stays forbidden.
    #[serde(default)]
    pub zone_sizes: ZoneSizeMap,
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
            zone_sizes: ZoneSizeMap::new(),
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

    fn parse(value: &str) -> Option<Self> {
        match value {
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            _ => None,
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
        descriptor.transports = vec![DriverTransportDescriptor::available(
            DriverTransportKind::Bridge,
        )];
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

    fn output(&self) -> OutputBinding<'_> {
        OutputBinding::Owned {
            id: BackendId::new(DESCRIPTOR.id).expect("OpenRGB backend ID must be valid"),
            factory: self,
        }
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

impl DeviceBackendFactory for OpenRgbDriverModule {
    fn build(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> std::result::Result<Arc<dyn DeviceBackend>, DriverError> {
        let config = config.parse_settings::<OpenRgbConfig>()?;
        let mut backend = OpenRgbBackend::new(config)?;
        backend.runtime = host.runtime_handle();
        Ok(Arc::new(backend))
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

    fn validate_config(&self, config: &DriverConfigEntry) -> std::result::Result<(), DriverError> {
        let config = DriverConfigView {
            driver_id: DESCRIPTOR.id,
            entry: config,
        }
        .parse_settings::<OpenRgbConfig>()?;
        validate_openrgb_config(&config).map_err(|error| DriverError::Configuration {
            message: error.to_string(),
        })
    }
}

#[async_trait]
impl DiscoveryCapability for OpenRgbDriverModule {
    async fn discover(
        &self,
        _host: &dyn DriverHost,
        _request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> std::result::Result<Vec<DiscoveredDevice>, DriverError> {
        let config = config.parse_settings::<OpenRgbConfig>()?;
        validate_openrgb_config(&config).map_err(|error| DriverError::Configuration {
            message: error.to_string(),
        })?;
        let routes = discover_routes(&config)
            .await
            .map_err(|error| map_openrgb_driver_error(&error))?;
        Ok(routes.into_iter().map(DiscoveredDevice::from).collect())
    }
}

/// Runtime backend for OpenRGB-proxied output.
///
/// Every controller on one OpenRGB endpoint shares a single SDK connection
/// owned by the process-wide [`EndpointPool`]; the backend tracks adopted
/// routes and the per-controller writer tasks layered on top of it.
pub struct OpenRgbBackend {
    config: OpenRgbConfig,
    runtime: Option<Arc<dyn DriverRuntimeActions>>,
    discovered: StdRwLock<HashMap<DeviceId, ControllerRoute>>,
    connected: StdRwLock<HashMap<DeviceId, Arc<ConnectedOutput>>>,
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
            runtime: None,
            discovered: StdRwLock::new(HashMap::new()),
            connected: StdRwLock::new(HashMap::new()),
        })
    }

    fn discovered_route(&self, id: &DeviceId) -> Option<ControllerRoute> {
        self.discovered
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    fn connected_output(&self, id: &DeviceId) -> Option<Arc<ConnectedOutput>> {
        self.connected
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
    }
}

impl Drop for OpenRgbBackend {
    /// Release every endpoint this backend still holds so a rebound backend
    /// does not leave shared links pinned by writers nobody owns.
    fn drop(&mut self) {
        let outputs = self
            .connected
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
            .collect::<Vec<_>>();
        let handle = tokio::runtime::Handle::try_current().ok();
        for (id, output) in outputs {
            output.endpoint.unregister_controller(&id);
            let endpoint = Arc::clone(&output.endpoint);
            drop(output);
            if let Some(handle) = &handle {
                handle.spawn(async move {
                    endpoint_pool().release_if_idle(&endpoint).await;
                });
            }
        }
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

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        let rejected = || DeviceError::NotAdopted {
            device_id: discovered.info.id,
        };
        let endpoint = discovered
            .metadata
            .get(METADATA_ENDPOINT)
            .and_then(|value| value.parse::<SocketAddr>().ok())
            .ok_or_else(rejected)?;
        let controller_index = discovered
            .metadata
            .get(METADATA_CONTROLLER_INDEX)
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or_else(rejected)?;
        let fingerprint = discovered
            .metadata
            .get(METADATA_FINGERPRINT)
            .map(|value| DeviceFingerprint::from_persisted(value.clone()))
            .ok_or_else(rejected)?;
        let confidence = discovered
            .metadata
            .get(METADATA_IDENTITY_CONFIDENCE)
            .and_then(|value| IdentityConfidence::parse(value))
            .ok_or_else(rejected)?;
        let detector_class = discovered
            .metadata
            .get(METADATA_DETECTOR_CLASS)
            .cloned()
            .ok_or_else(rejected)?;
        let protocol_version = discovered
            .metadata
            .get(METADATA_PROTOCOL_VERSION)
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or_else(rejected)?;
        let route = ControllerRoute {
            endpoint,
            controller_index,
            target_fps: target_fps(&self.config, fingerprint.as_str(), &detector_class),
            auto_connect: matches!(
                discovered.connect_behavior,
                DiscoveryConnectBehavior::AutoConnect
            ),
            disabled_reason: discovered.metadata.get(METADATA_DISABLED_REASON).cloned(),
            fingerprint,
            confidence,
            detector_class,
            writable_mode: None,
            previous_mode: None,
            info: discovered.info.clone(),
            protocol_version,
            serial: discovered
                .metadata
                .get(METADATA_SERIAL)
                .cloned()
                .unwrap_or_default(),
            location: discovered
                .metadata
                .get(METADATA_LOCATION)
                .cloned()
                .unwrap_or_default(),
        };
        self.discovered
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(discovered.info.id, route);
        Ok(())
    }

    /// The live route's `DeviceInfo`, which the daemon republishes after
    /// every connect. Shape changes and index remaps land here.
    async fn connected_device_info(
        &self,
        id: &DeviceId,
    ) -> std::result::Result<Option<DeviceInfo>, DeviceError> {
        let Some(output) = self.connected_output(id) else {
            return Ok(None);
        };
        let controller = output.controller.lock().await;
        Ok(Some(controller.route.info.clone()))
    }

    async fn connect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        let route = self
            .discovered_route(id)
            .ok_or(DeviceError::NotAdopted { device_id: *id })?;
        if let Some(reason) = &route.disabled_reason
            && !can_restore_zero_led_zones(&route, &self.config)
        {
            return Err(DeviceError::connection(id, reason));
        }

        let pool = endpoint_pool();
        let endpoint = pool.acquire(route.endpoint, &self.config);
        let connected = connect_controller(&endpoint, &route, &self.config).await;
        let result = match connected {
            Ok(mut controller) => {
                controller.runtime.clone_from(&self.runtime);
                let target_fps = controller.route.target_fps;
                let controller = Arc::new(Mutex::new(controller));
                endpoint.register_controller(*id, Arc::clone(&controller));
                endpoint.ensure_task();
                self.connected
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(
                        *id,
                        Arc::new(ConnectedOutput::spawn(
                            *id,
                            Arc::clone(&endpoint),
                            controller,
                            target_fps,
                        )),
                    );
                Ok(())
            }
            Err(error) => Err(map_openrgb_device_error(
                *id,
                &error,
                OpenRgbDeviceOperation::Connect,
            )),
        };
        pool.unpin(&endpoint).await;
        result
    }

    async fn disconnect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        let output = self
            .connected
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        let Some(output) = output else {
            return Ok(());
        };
        let controller = output.stop().await;
        let endpoint = Arc::clone(&output.endpoint);
        {
            let mut link = endpoint.link.lock().await;
            let mut controller = controller.lock().await;
            controller.accepting_frames = false;
            if let Some(client) = link.client.as_mut() {
                let teardown = teardown_connected_controller(
                    client,
                    &mut controller,
                    endpoint.config.teardown_policy,
                )
                .await;
                if let Err(error) = teardown {
                    debug!(
                        device_id = %id,
                        error = %error,
                        "OpenRGB teardown failed during disconnect"
                    );
                    if is_transport_error(&error) {
                        endpoint.fail_link(&mut link, &error).await;
                    }
                }
            }
        }
        endpoint.unregister_controller(id);
        drop(output);
        endpoint_pool().release_if_idle(&endpoint).await;
        Ok(())
    }

    async fn write_colors(
        &self,
        id: &DeviceId,
        colors: &[[u8; 3]],
    ) -> std::result::Result<(), DeviceError> {
        let Some(output) = self.connected_output(id) else {
            return Err(DeviceError::Disconnected {
                device: id.to_string(),
            });
        };
        output.enqueue_colors(Arc::new(colors.to_vec()))
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        self.connected
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .and_then(|output| {
                output
                    .controller
                    .try_lock()
                    .ok()
                    .map(|controller| controller.route.target_fps)
            })
            .or_else(|| {
                self.discovered
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(id)
                    .map(|route| route.target_fps)
            })
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.connected
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .map(|output| output.frame_sink() as Arc<dyn DeviceFrameSink>)
    }

    /// Identify on a known, idle bridge device adopts, connects, flashes, and
    /// tears down like any direct-control backend.
    ///
    /// An output-disabled route also answers `true` so the daemon calls
    /// [`Self::connect`], which refuses with the route's `disabled_reason`;
    /// otherwise the daemon would only ever report "not connected" for a
    /// device whose real problem is spelled out in its metadata.
    fn supports_temporary_direct_control(&self, info: &DeviceInfo) -> bool {
        if info.total_led_count() == 0 {
            return false;
        }
        info.capabilities.supports_direct
            || self
                .discovered_route(&info.id)
                .is_some_and(|route| route.disabled_reason.is_some())
    }
}

struct ConnectedOutput {
    device_id: DeviceId,
    endpoint: Arc<EndpointConnection>,
    controller: Arc<Mutex<ConnectedController>>,
    frame_tx: watch::Sender<Option<Arc<OpenRgbFramePayload>>>,
    io_task: StdMutex<Option<JoinHandle<()>>>,
    active: Arc<AtomicBool>,
    lifecycle_gate: Arc<StdMutex<()>>,
    last_async_error: Arc<StdMutex<Option<DeviceError>>>,
}

impl ConnectedOutput {
    fn spawn(
        device_id: DeviceId,
        endpoint: Arc<EndpointConnection>,
        controller: Arc<Mutex<ConnectedController>>,
        target_fps: u32,
    ) -> Self {
        let (frame_tx, frame_rx) = watch::channel(None::<Arc<OpenRgbFramePayload>>);
        let active = Arc::new(AtomicBool::new(true));
        let lifecycle_gate = Arc::new(StdMutex::new(()));
        let last_async_error = Arc::new(StdMutex::new(None::<DeviceError>));
        let io_task = tokio::spawn(run_openrgb_output_worker(
            device_id,
            Arc::clone(&endpoint),
            Arc::clone(&controller),
            frame_rx,
            Arc::clone(&active),
            Arc::clone(&last_async_error),
            frame_interval_for_fps(target_fps),
        ));

        Self {
            device_id,
            endpoint,
            controller,
            frame_tx,
            io_task: StdMutex::new(Some(io_task)),
            active,
            lifecycle_gate,
            last_async_error,
        }
    }

    fn frame_sink(&self) -> Arc<OpenRgbFrameSink> {
        Arc::new(OpenRgbFrameSink {
            device_id: self.device_id,
            frame_tx: self.frame_tx.clone(),
            active: Arc::clone(&self.active),
            lifecycle_gate: Arc::clone(&self.lifecycle_gate),
            last_async_error: Arc::clone(&self.last_async_error),
        })
    }

    fn enqueue_colors(&self, colors: Arc<Vec<[u8; 3]>>) -> std::result::Result<(), DeviceError> {
        enqueue_openrgb_payload(
            self.device_id,
            &self.frame_tx,
            &self.active,
            &self.lifecycle_gate,
            &self.last_async_error,
            Arc::new(OpenRgbFramePayload::untracked(colors)),
        )
    }

    async fn stop(&self) -> Arc<Mutex<ConnectedController>> {
        {
            let _gate = lock_lifecycle_gate(&self.lifecycle_gate);
            self.active.store(false, Ordering::Release);
            if let Some(pending) = self.frame_tx.send_replace(None) {
                pending.reject_pending(DeviceError::Disconnected {
                    device: self.device_id.to_string(),
                });
            }
        }
        {
            let mut controller = self.controller.lock().await;
            controller.accepting_frames = false;
        }
        let controller = Arc::clone(&self.controller);
        let io_task = self
            .io_task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(mut io_task) = io_task {
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
        {
            let _gate = lock_lifecycle_gate(&self.lifecycle_gate);
            self.active.store(false, Ordering::Release);
            if let Some(pending) = self.frame_tx.send_replace(None) {
                pending.reject_pending(DeviceError::Disconnected {
                    device: self.device_id.to_string(),
                });
            }
        }
        if let Some(io_task) = self
            .io_task
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            io_task.abort();
        }
    }
}

struct OpenRgbFramePayload {
    colors: Arc<Vec<[u8; 3]>>,
    delivery_id: Option<DeviceDeliveryId>,
    delivery_observer: Option<Arc<dyn DeviceDeliveryObserver>>,
    delivery_tx: StdMutex<Option<oneshot::Sender<DeviceDeliveryAck>>>,
    delivery_state: AtomicU8,
}

impl OpenRgbFramePayload {
    fn untracked(colors: Arc<Vec<[u8; 3]>>) -> Self {
        Self {
            colors,
            delivery_id: None,
            delivery_observer: None,
            delivery_tx: StdMutex::new(None),
            delivery_state: AtomicU8::new(DELIVERY_PENDING),
        }
    }

    fn tracked(
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> (Self, oneshot::Receiver<DeviceDeliveryAck>) {
        Self::tracked_observed(id, colors, None)
    }

    fn tracked_observed(
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        delivery_observer: Option<Arc<dyn DeviceDeliveryObserver>>,
    ) -> (Self, oneshot::Receiver<DeviceDeliveryAck>) {
        let (delivery_tx, delivery_rx) = oneshot::channel();
        (
            Self {
                colors,
                delivery_id: Some(id),
                delivery_observer,
                delivery_tx: StdMutex::new(Some(delivery_tx)),
                delivery_state: AtomicU8::new(DELIVERY_PENDING),
            },
            delivery_rx,
        )
    }

    fn mark_transport_started(&self) -> bool {
        let Some(id) = self.delivery_id else {
            return true;
        };
        if self
            .delivery_state
            .compare_exchange(
                DELIVERY_PENDING,
                DELIVERY_STARTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return false;
        }
        if let Some(observer) = &self.delivery_observer {
            observer.transport_started(id);
        }
        true
    }

    fn acknowledge(&self, ack: DeviceDeliveryAck) {
        if let Ok(mut delivery_tx) = self.delivery_tx.lock()
            && let Some(delivery_tx) = delivery_tx.take()
        {
            let _ = delivery_tx.send(ack);
        }
    }

    fn reject_pending(&self, error: DeviceError) {
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
    device_id: DeviceId,
    frame_tx: watch::Sender<Option<Arc<OpenRgbFramePayload>>>,
    active: Arc<AtomicBool>,
    lifecycle_gate: Arc<StdMutex<()>>,
    last_async_error: Arc<StdMutex<Option<DeviceError>>>,
}

#[async_trait]
impl DeviceFrameSink for OpenRgbFrameSink {
    async fn write_colors_shared(
        &self,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> std::result::Result<(), DeviceError> {
        enqueue_openrgb_payload(
            self.device_id,
            &self.frame_tx,
            &self.active,
            &self.lifecycle_gate,
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
            self.device_id,
            &self.frame_tx,
            &self.active,
            &self.lifecycle_gate,
            &self.last_async_error,
            Arc::new(payload),
        ) {
            return DeviceDeliveryAck::rejected(id, error);
        }

        delivery_rx.await.unwrap_or_else(|_| {
            DeviceDeliveryAck::rejected(
                id,
                DeviceError::Disconnected {
                    device: self.device_id.to_string(),
                },
            )
        })
    }

    async fn deliver_colors_shared_observed(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        let (payload, delivery_rx) =
            OpenRgbFramePayload::tracked_observed(id, colors, Some(observer));
        if let Err(error) = enqueue_openrgb_payload(
            self.device_id,
            &self.frame_tx,
            &self.active,
            &self.lifecycle_gate,
            &self.last_async_error,
            Arc::new(payload),
        ) {
            return DeviceDeliveryAck::rejected(id, error);
        }

        delivery_rx.await.unwrap_or_else(|_| {
            DeviceDeliveryAck::rejected(
                id,
                DeviceError::Disconnected {
                    device: self.device_id.to_string(),
                },
            )
        })
    }
}

fn enqueue_openrgb_payload(
    device_id: DeviceId,
    frame_tx: &watch::Sender<Option<Arc<OpenRgbFramePayload>>>,
    active: &AtomicBool,
    lifecycle_gate: &StdMutex<()>,
    last_async_error: &StdMutex<Option<DeviceError>>,
    payload: Arc<OpenRgbFramePayload>,
) -> std::result::Result<(), DeviceError> {
    let _gate = lock_lifecycle_gate(lifecycle_gate);
    if !active.load(Ordering::Acquire) {
        return Err(DeviceError::Disconnected {
            device: device_id.to_string(),
        });
    }
    if let Some(error) = last_async_error
        .lock()
        .map_err(|_| DeviceError::write(device_id, "OpenRGB async error state lock poisoned"))?
        .take()
    {
        return Err(error);
    }
    if let Some(previous) = frame_tx.send_replace(Some(payload)) {
        previous.reject_pending(DeviceError::write(
            device_id,
            "OpenRGB frame was superseded before transport started",
        ));
    }
    Ok(())
}

fn lock_lifecycle_gate(gate: &StdMutex<()>) -> std::sync::MutexGuard<'_, ()> {
    match gate.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Interval between writes for a paced controller.
fn frame_interval_for_fps(target_fps: u32) -> Duration {
    Duration::from_secs(1) / target_fps.max(1)
}

/// Per-controller writer: latest-value frames, paced to the controller's
/// `target_fps`.
///
/// Frames that arrive while the writer waits for its next slot replace the
/// pending one (the superseded delivery is rejected by the enqueue path), so
/// a slow controller never accumulates a backlog and never runs its bus
/// faster than its class allows.
async fn run_openrgb_output_worker(
    device_id: DeviceId,
    endpoint: Arc<EndpointConnection>,
    controller: Arc<Mutex<ConnectedController>>,
    mut frame_rx: watch::Receiver<Option<Arc<OpenRgbFramePayload>>>,
    active: Arc<AtomicBool>,
    last_async_error: Arc<StdMutex<Option<DeviceError>>>,
    frame_interval: Duration,
) {
    let mut next_slot: Option<Instant> = None;
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

        if let Some(slot) = next_slot {
            let pace = sleep(slot.saturating_duration_since(Instant::now()));
            tokio::pin!(pace);
            loop {
                tokio::select! {
                    () = &mut pace => break,
                    changed = frame_rx.changed() => {
                        if changed.is_err() || !active.load(Ordering::Acquire) {
                            return;
                        }
                        let Some(latest) = frame_rx.borrow_and_update().clone() else {
                            return;
                        };
                        frame = latest;
                    }
                }
            }
        }

        if !frame.mark_transport_started() {
            continue;
        }
        let transport_started_at = Instant::now();
        next_slot = Some(transport_started_at + frame_interval);
        match write_controller_colors(&endpoint, &controller, frame.colors.as_slice()).await {
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
                let error =
                    map_openrgb_device_error(device_id, &error, OpenRgbDeviceOperation::Write);
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

#[derive(Clone, Copy)]
enum OpenRgbDeviceOperation {
    Connect,
    Write,
}

fn map_openrgb_device_error(
    device_id: DeviceId,
    error: &Error,
    operation: OpenRgbDeviceOperation,
) -> DeviceError {
    match error
        .chain()
        .find_map(|cause| cause.downcast_ref::<OpenRgbError>())
    {
        Some(OpenRgbError::Timeout { after, .. }) => DeviceError::Timeout { after: *after },
        Some(OpenRgbError::ConnectionClosed) => DeviceError::Disconnected {
            device: device_id.to_string(),
        },
        _ => match operation {
            OpenRgbDeviceOperation::Connect => DeviceError::connection(device_id, error),
            OpenRgbDeviceOperation::Write => DeviceError::write(device_id, error),
        },
    }
}

fn map_openrgb_driver_error(error: &Error) -> DriverError {
    match error
        .chain()
        .find_map(|cause| cause.downcast_ref::<OpenRgbError>())
    {
        Some(OpenRgbError::Timeout { after, .. }) => DriverError::Timeout { after: *after },
        _ => DriverError::discovery(error),
    }
}

const fn is_openrgb_transport_failure(error: &OpenRgbError) -> bool {
    matches!(
        error,
        OpenRgbError::Timeout { .. } | OpenRgbError::ConnectionClosed | OpenRgbError::Io(_)
    )
}

/// Write one frame through the shared endpoint link.
///
/// The link lock is taken before the controller lock everywhere (the endpoint
/// task uses the same order), so a writer never blocks re-enumeration. When a
/// `DEVICE_LIST_UPDATED` is pending the writer parks until the endpoint task
/// has remapped every route instead of streaming to a possibly stale index.
async fn write_controller_colors(
    endpoint: &EndpointConnection,
    controller: &Arc<Mutex<ConnectedController>>,
    colors: &[[u8; 3]],
) -> Result<()> {
    let mut enumeration = endpoint.enumeration.subscribe();
    for attempt in 0..2_u8 {
        let generation = *enumeration.borrow_and_update();
        {
            let mut link = endpoint.link.lock().await;
            if link.client.is_none() {
                bail!("OpenRGB endpoint {} is reconnecting", endpoint.endpoint);
            }
            if !link.stale_device_list {
                let Some(client) = link.client.as_mut() else {
                    bail!("OpenRGB endpoint {} is reconnecting", endpoint.endpoint);
                };
                let saw_update = match client.drain_pending_packets() {
                    Ok(packets) => packets
                        .iter()
                        .any(|packet| packet.header.packet_id == PacketId::DeviceListUpdated),
                    Err(error) => {
                        let error = Error::new(error).context("OpenRGB notification drain failed");
                        endpoint.fail_link(&mut link, &error).await;
                        return Err(error);
                    }
                };
                if saw_update {
                    endpoint.mark_stale(&mut link);
                }
            }
            if !link.stale_device_list {
                let Some(client) = link.client.as_mut() else {
                    bail!("OpenRGB endpoint {} is reconnecting", endpoint.endpoint);
                };
                let (controller_index, shaped) = {
                    let mut controller = controller.lock().await;
                    if !controller.accepting_frames {
                        bail!("OpenRGB controller is disconnected");
                    }
                    ensure_route_output_enabled(&controller.route)?;
                    let shaped = shape_frame_colors(&mut controller, colors)?;
                    (controller.route.controller_index, shaped)
                };
                let written = client.update_leds(controller_index, &shaped).await;
                return match written {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        let error = Error::new(error).context("OpenRGB update_leds failed");
                        if is_transport_error(&error) {
                            endpoint.fail_link(&mut link, &error).await;
                        }
                        Err(error)
                    }
                };
            }
        }
        if attempt == 0 {
            match timeout(
                ENUMERATION_WAIT,
                enumeration.wait_for(|current| *current > generation),
            )
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(_)) => bail!(
                    "OpenRGB endpoint task for {} stopped before re-enumerating",
                    endpoint.endpoint
                ),
                Err(_) => bail!(
                    "OpenRGB device list update at {} did not settle within {:?}",
                    endpoint.endpoint,
                    ENUMERATION_WAIT
                ),
            }
        }
    }
    bail!(
        "OpenRGB device list update at {} is still pending",
        endpoint.endpoint
    )
}

/// Fit a frame to the controller's LED count.
///
/// OpenRGB silently discards an `UPDATELEDS` whose color count differs from
/// the controller's LED count, so a mismatched frame is padded with black or
/// truncated rather than sent as-is. The mismatch is logged once per route.
fn shape_frame_colors(
    controller: &mut ConnectedController,
    colors: &[[u8; 3]],
) -> Result<Vec<RgbColor>> {
    let led_count = usize::try_from(controller.route.info.capabilities.led_count)
        .context("OpenRGB LED count does not fit usize")?;
    if led_count == 0 {
        bail!(
            "OpenRGB controller {} reports zero LEDs; output is disabled",
            controller.route.info.id
        );
    }
    if colors.len() != led_count && !controller.shape_warning_logged {
        controller.shape_warning_logged = true;
        warn!(
            device_id = %controller.route.info.id,
            controller_led_count = led_count,
            frame_led_count = colors.len(),
            "OpenRGB frame length does not match controller LED count; \
             padding or truncating so the server does not drop the frame"
        );
    }
    Ok(fit_frame_to_led_count(colors, led_count))
}

/// Pad a frame with black or truncate it to exactly `led_count` colors.
fn fit_frame_to_led_count(colors: &[[u8; 3]], led_count: usize) -> Vec<RgbColor> {
    let mut shaped = colors
        .iter()
        .take(led_count)
        .map(|[red, green, blue]| RgbColor::new(*red, *green, *blue))
        .collect::<Vec<_>>();
    shaped.resize(led_count, RgbColor::new(0, 0, 0));
    shaped
}

/// Describe a controller-side shape change for `disabled_reason`.
fn shape_changed_reason(previous_led_count: u32, current_led_count: u32) -> String {
    format!("zone shape changed (was {previous_led_count}, now {current_led_count}); rescan")
}

/// Whether two routes describe different LED shapes.
fn route_shape_differs(previous: &ControllerRoute, current: &ControllerRoute) -> bool {
    previous.info.capabilities.led_count != current.info.capabilities.led_count
        || previous.info.segments.len() != current.info.segments.len()
        || previous
            .info
            .segments
            .iter()
            .zip(&current.info.segments)
            .any(|(before, after)| {
                before.name != after.name
                    || before.led_count != after.led_count
                    || before.topology != after.topology
            })
}

/// Carry a controller-side shape change into the refreshed route.
///
/// The refreshed route keeps its new shape so the daemon sees the current
/// LED count, but output is disabled until the layout is rescanned.
fn apply_shape_change(previous: &ControllerRoute, current: &mut ControllerRoute) {
    if current.disabled_reason.is_some() || !route_shape_differs(previous, current) {
        return;
    }
    current.disabled_reason = Some(shape_changed_reason(
        previous.info.capabilities.led_count,
        current.info.capabilities.led_count,
    ));
    current.info.capabilities.supports_direct = false;
}

/// Baseline reconnect delay after an endpoint link drops.
const RECONNECT_BACKOFF_BASE: Duration = Duration::from_secs(1);
/// Ceiling for the doubling reconnect delay.
const RECONNECT_BACKOFF_MAX: Duration = Duration::from_mins(1);
/// Jitter band applied to every reconnect delay, as a fraction of the delay.
const RECONNECT_BACKOFF_JITTER: f64 = 0.10;
/// Cadence at which an idle endpoint task drains server notifications.
const ENDPOINT_IDLE_TICK: Duration = Duration::from_secs(1);
/// How long a writer waits for the endpoint task to finish re-enumerating.
const ENUMERATION_WAIT: Duration = Duration::from_secs(2);

/// The process-wide endpoint pool.
///
/// An OpenRGB endpoint is a process-global resource (one server per address),
/// so the pool is too: discovery and every backend instance built from this
/// crate share one SDK connection per endpoint.
fn endpoint_pool() -> &'static EndpointPool {
    static POOL: LazyLock<EndpointPool> = LazyLock::new(EndpointPool::default);
    &POOL
}

#[derive(Default)]
struct EndpointPool {
    endpoints: StdMutex<HashMap<SocketAddr, Arc<EndpointConnection>>>,
}

impl EndpointPool {
    /// Fetch or create the connection for `endpoint`, pinned so a concurrent
    /// release cannot close it before the caller registers a controller.
    fn acquire(&self, endpoint: SocketAddr, config: &OpenRgbConfig) -> Arc<EndpointConnection> {
        let mut endpoints = self
            .endpoints
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let connection = endpoints
            .entry(endpoint)
            .or_insert_with(|| Arc::new(EndpointConnection::new(endpoint, config.clone())));
        connection.pins.fetch_add(1, Ordering::AcqRel);
        Arc::clone(connection)
    }

    /// The connection for `endpoint` if one is pooled, without pinning it.
    fn open(&self, endpoint: SocketAddr) -> Option<Arc<EndpointConnection>> {
        self.endpoints
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&endpoint)
            .cloned()
    }

    async fn unpin(&self, connection: &Arc<EndpointConnection>) {
        connection.pins.fetch_sub(1, Ordering::AcqRel);
        self.release_if_idle(connection).await;
    }

    /// Close and forget a connection nobody pins and no controller uses.
    async fn release_if_idle(&self, connection: &Arc<EndpointConnection>) {
        let idle = {
            let mut endpoints = self
                .endpoints
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let idle = connection.pins.load(Ordering::Acquire) == 0
                && connection
                    .controllers
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_empty();
            if idle
                && endpoints
                    .get(&connection.endpoint)
                    .is_some_and(|pooled| Arc::ptr_eq(pooled, connection))
            {
                endpoints.remove(&connection.endpoint);
            }
            idle
        };
        if idle {
            connection.shutdown().await;
        }
    }
}

/// One shared SDK connection to an OpenRGB endpoint plus the controllers
/// streaming through it.
struct EndpointConnection {
    endpoint: SocketAddr,
    config: OpenRgbConfig,
    link: Mutex<EndpointLink>,
    controllers: StdMutex<HashMap<DeviceId, Arc<Mutex<ConnectedController>>>>,
    pins: AtomicUsize,
    wake: Notify,
    /// Bumped after every completed re-enumeration; writers park on it.
    enumeration: watch::Sender<u64>,
    task: StdMutex<Option<JoinHandle<()>>>,
}

/// Connection state guarded by the endpoint's async mutex.
struct EndpointLink {
    client: Option<OpenRgbClient>,
    /// Consecutive failed reconnect attempts since the link dropped.
    failures: u32,
    next_attempt: Option<Instant>,
    /// A `DEVICE_LIST_UPDATED` arrived and the endpoint task has not
    /// re-enumerated yet.
    stale_device_list: bool,
}

impl EndpointConnection {
    fn new(endpoint: SocketAddr, config: OpenRgbConfig) -> Self {
        Self {
            endpoint,
            config,
            link: Mutex::new(EndpointLink {
                client: None,
                failures: 0,
                next_attempt: None,
                stale_device_list: false,
            }),
            controllers: StdMutex::new(HashMap::new()),
            pins: AtomicUsize::new(0),
            wake: Notify::new(),
            enumeration: watch::Sender::new(0),
            task: StdMutex::new(None),
        }
    }

    fn register_controller(&self, id: DeviceId, controller: Arc<Mutex<ConnectedController>>) {
        self.controllers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, controller);
    }

    fn unregister_controller(&self, id: &DeviceId) {
        self.controllers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
    }

    fn controllers_snapshot(&self) -> Vec<(DeviceId, Arc<Mutex<ConnectedController>>)> {
        self.controllers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(id, controller)| (*id, Arc::clone(controller)))
            .collect()
    }

    /// Start the endpoint task if it is not already running.
    fn ensure_task(self: &Arc<Self>) {
        let mut task = self.task.lock().unwrap_or_else(PoisonError::into_inner);
        if task.as_ref().is_none_or(JoinHandle::is_finished) {
            *task = Some(tokio::spawn(run_endpoint_task(Arc::clone(self))));
        }
    }

    /// Stop the endpoint task and close the link cleanly.
    async fn shutdown(&self) {
        let task = self
            .task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            task.abort();
        }
        let client = self.link.lock().await.client.take();
        if let Some(client) = client
            && let Err(error) = client.close().await
        {
            debug!(endpoint = %self.endpoint, error = %error, "OpenRGB endpoint close failed");
        }
    }

    /// Drop the link after a transport failure and schedule a reconnect.
    async fn fail_link(&self, link: &mut EndpointLink, error: &Error) {
        if let Some(client) = link.client.take()
            && let Err(close_error) = client.close().await
        {
            debug!(
                endpoint = %self.endpoint,
                error = %close_error,
                "OpenRGB failed link did not close cleanly"
            );
        }
        link.failures = 0;
        link.next_attempt = Some(Instant::now() + reconnect_backoff(0));
        link.stale_device_list = false;
        warn!(
            endpoint = %self.endpoint,
            error = %error,
            "OpenRGB endpoint link dropped; reconnecting with backoff"
        );
        self.wake.notify_one();
    }

    /// Record a pending device-list change and wake the endpoint task.
    fn mark_stale(&self, link: &mut EndpointLink) {
        link.stale_device_list = true;
        self.wake.notify_one();
    }
}

/// Reconnect delay for the given number of consecutive failures.
fn reconnect_backoff(failures: u32) -> Duration {
    reconnect_backoff_with_jitter(failures, pseudo_random_unit())
}

/// Reconnect delay: 1 s doubling to a 60 s ceiling, with `jitter` in
/// `[-1, 1]` selecting a point in the ±10% band.
fn reconnect_backoff_with_jitter(failures: u32, jitter: f64) -> Duration {
    let base = RECONNECT_BACKOFF_BASE
        .saturating_mul(1_u32 << failures.min(6))
        .min(RECONNECT_BACKOFF_MAX);
    base.mul_f64(1.0 + RECONNECT_BACKOFF_JITTER * jitter.clamp(-1.0, 1.0))
}

/// A cheap jitter source in `[-1, 1]`; backoff spreading needs no CSPRNG.
fn pseudo_random_unit() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    f64::from(nanos % 2_001) / 1_000.0 - 1.0
}

/// Per-endpoint task: drains notifications, re-enumerates once per
/// `DEVICE_LIST_UPDATED`, and reconnects a dropped link with bounded backoff.
async fn run_endpoint_task(endpoint: Arc<EndpointConnection>) {
    loop {
        let wait = {
            let link = endpoint.link.lock().await;
            match (&link.client, link.next_attempt) {
                (Some(_), _) if link.stale_device_list => Duration::ZERO,
                (Some(_), _) => ENDPOINT_IDLE_TICK,
                (None, Some(next)) => next.saturating_duration_since(Instant::now()),
                (None, None) => Duration::ZERO,
            }
        };
        if !wait.is_zero() {
            tokio::select! {
                () = endpoint.wake.notified() => {}
                () = sleep(wait) => {}
            }
        }

        let mut link = endpoint.link.lock().await;
        if link.client.is_none() {
            if link.next_attempt.is_some_and(|next| Instant::now() < next) {
                continue;
            }
            match connect_openrgb_client(&endpoint.config, endpoint.endpoint).await {
                Ok(client) => {
                    debug!(
                        endpoint = %endpoint.endpoint,
                        attempts = link.failures,
                        "OpenRGB endpoint link established"
                    );
                    link.client = Some(client);
                    link.failures = 0;
                    link.next_attempt = None;
                    reenumerate_controllers(&endpoint, &mut link).await;
                }
                Err(error) => {
                    link.failures = link.failures.saturating_add(1);
                    let delay = reconnect_backoff(link.failures);
                    link.next_attempt = Some(Instant::now() + delay);
                    debug!(
                        endpoint = %endpoint.endpoint,
                        attempt = link.failures,
                        retry_in_ms = delay.as_millis(),
                        error = %error,
                        "OpenRGB endpoint reconnect failed"
                    );
                }
            }
            continue;
        }

        let stale = if link.stale_device_list {
            true
        } else {
            let Some(client) = link.client.as_mut() else {
                continue;
            };
            match client.drain_pending_packets() {
                Ok(packets) => packets
                    .iter()
                    .any(|packet| packet.header.packet_id == PacketId::DeviceListUpdated),
                Err(error) => {
                    let error = Error::new(error).context("OpenRGB notification drain failed");
                    endpoint.fail_link(&mut link, &error).await;
                    continue;
                }
            }
        };
        if stale {
            reenumerate_controllers(&endpoint, &mut link).await;
        }
    }
}

/// Re-enumerate the endpoint and remap every connected controller.
///
/// Index changes are applied silently. A shape change or a vanished
/// fingerprint disables the route and requests a lifecycle reconnect even
/// when no frames are being submitted. The callback runs outside endpoint
/// and controller locks because disconnect itself needs both locks.
async fn reenumerate_controllers(endpoint: &EndpointConnection, link: &mut EndpointLink) {
    let Some(client) = link.client.as_mut() else {
        return;
    };
    let routes = match enumerate_routes(client, endpoint.endpoint, &endpoint.config).await {
        Ok(routes) => routes,
        Err(error) => {
            if is_transport_error(&error) {
                endpoint.fail_link(link, &error).await;
            } else {
                debug!(
                    endpoint = %endpoint.endpoint,
                    error = %error,
                    "OpenRGB re-enumeration failed; will retry on the next notification"
                );
            }
            return;
        }
    };

    for (id, controller) in endpoint.controllers_snapshot() {
        let Some(client) = link.client.as_mut() else {
            break;
        };
        let mut controller = controller.lock().await;
        let Some(route) = routes
            .iter()
            .find(|route| route.fingerprint == controller.route.fingerprint)
        else {
            warn!(
                device_id = %id,
                endpoint = %endpoint.endpoint,
                "OpenRGB controller disappeared after a device list update"
            );
            controller.route.disabled_reason = Some(
                "OpenRGB controller disappeared after a device list update; rescan".to_owned(),
            );
            controller.route.info.capabilities.supports_direct = false;
            request_route_reconnect(id, &mut controller);
            continue;
        };
        let mut route = route.clone();
        let shape_changed = route_shape_differs(&controller.route, &route);
        apply_shape_change(&controller.route, &mut route);
        if route.controller_index != controller.route.controller_index {
            debug!(
                device_id = %id,
                from = controller.route.controller_index,
                to = route.controller_index,
                "OpenRGB controller index remapped"
            );
        }
        controller.route = route;
        if shape_changed {
            request_route_reconnect(id, &mut controller);
        }
        if let Some(reason) = &controller.route.disabled_reason {
            warn!(device_id = %id, reason = %reason, "OpenRGB controller output disabled");
            continue;
        }
        if let Err(error) =
            configure_controller_output(client, &controller.route, &endpoint.config).await
        {
            if is_transport_error(&error) {
                drop(controller);
                endpoint.fail_link(link, &error).await;
                break;
            }
            controller.route.disabled_reason = Some(format!(
                "OpenRGB output setup failed after device list update: {error}"
            ));
            controller.route.info.capabilities.supports_direct = false;
        }
    }
    link.stale_device_list = false;
    endpoint
        .enumeration
        .send_modify(|generation| *generation += 1);
}

/// Resolve, validate, and activate one controller over the endpoint link.
async fn connect_controller(
    endpoint: &EndpointConnection,
    discovered: &ControllerRoute,
    config: &OpenRgbConfig,
) -> Result<ConnectedController> {
    let mut link = endpoint.link.lock().await;
    if link.client.is_none() {
        let client = connect_openrgb_client(config, endpoint.endpoint).await?;
        link.client = Some(client);
        link.failures = 0;
        link.next_attempt = None;
    }
    let Some(client) = link.client.as_mut() else {
        bail!("OpenRGB endpoint {} is reconnecting", endpoint.endpoint);
    };
    let route = match find_current_route(client, endpoint.endpoint, &discovered.fingerprint, config)
        .await
    {
        Ok(route) => route,
        Err(error) => {
            if is_transport_error(&error) {
                endpoint.fail_link(&mut link, &error).await;
            }
            return Err(error);
        }
    };
    if !can_restore_zero_led_zones(&route, config) {
        ensure_route_output_enabled(&route)?;
    }
    let Some(client) = link.client.as_mut() else {
        bail!("OpenRGB endpoint {} is reconnecting", endpoint.endpoint);
    };
    let route = match apply_configured_zone_sizes(client, endpoint.endpoint, route, config).await {
        Ok(route) => route,
        Err(error) => {
            if is_transport_error(&error) {
                endpoint.fail_link(&mut link, &error).await;
            }
            return Err(error);
        }
    };
    let Some(client) = link.client.as_mut() else {
        bail!("OpenRGB endpoint {} is reconnecting", endpoint.endpoint);
    };
    if let Err(error) = configure_controller_output(client, &route, config).await {
        if is_transport_error(&error) {
            endpoint.fail_link(&mut link, &error).await;
        }
        return Err(error);
    }
    Ok(ConnectedController {
        previous_mode: route.previous_mode.clone(),
        runtime: None,
        reconnect_requested: false,
        route,
        accepting_frames: true,
        shape_warning_logged: false,
    })
}

/// How long to wait for OpenRGB to re-announce the device list after a
/// `RESIZEZONE`.
const ZONE_RESIZE_WAIT: Duration = Duration::from_secs(2);

/// The user-approved zone sizes for a route, if any.
///
/// Sizes are keyed by fingerprint, so they are only honored for controllers
/// whose fingerprint survives a resize: a shape-based (medium confidence)
/// fingerprint changes with the LED count and would orphan the entry.
fn configured_zone_sizes<'a>(
    config: &'a OpenRgbConfig,
    route: &ControllerRoute,
) -> Result<Option<&'a BTreeMap<String, u32>>> {
    let fingerprint = route.fingerprint.as_str();
    let sizes = config.zone_sizes.get(fingerprint).or_else(|| {
        config
            .zone_sizes
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(fingerprint))
            .map(|(_, sizes)| sizes)
    });
    let Some(sizes) = sizes else {
        return Ok(None);
    };
    if route.confidence != IdentityConfidence::High {
        bail!(
            "OpenRGB zone_sizes for '{fingerprint}' need a serial or location identity; \
             a shape-based fingerprint changes when a zone is resized"
        );
    }
    Ok(Some(sizes))
}

/// Permit setup for an owned, writable controller whose only output blocker
/// is an empty LED array and whose named zones have approved nonzero sizes.
/// Frame output remains disabled until the resize has been verified.
fn can_restore_zero_led_zones(route: &ControllerRoute, config: &OpenRgbConfig) -> bool {
    route.disabled_reason.as_deref() == Some(ZERO_LEDS_REASON)
        && configured_zone_sizes(config, route)
            .ok()
            .flatten()
            .is_some_and(|sizes| {
                route
                    .info
                    .segments
                    .iter()
                    .any(|segment| sizes.get(&segment.name).is_some_and(|size| *size > 0))
            })
}

/// Resize every configured zone whose reported LED count differs, wait for
/// the server's re-announce, and return the re-enumerated route.
async fn apply_configured_zone_sizes(
    client: &mut OpenRgbClient,
    endpoint: SocketAddr,
    route: ControllerRoute,
    config: &OpenRgbConfig,
) -> Result<ControllerRoute> {
    let Some(sizes) = configured_zone_sizes(config, &route)? else {
        return Ok(route);
    };
    let mut resized = false;
    for (zone_index, segment) in route.info.segments.iter().enumerate() {
        let Some(target) = sizes.get(&segment.name) else {
            continue;
        };
        if segment.led_count == *target {
            continue;
        }
        let zone_index = u32::try_from(zone_index).context("OpenRGB zone index overflow")?;
        let requested = client
            .resize_zone(route.controller_index, zone_index, *target)
            .await
            .with_context(|| {
                format!(
                    "OpenRGB zone '{}' resize to {target} LEDs failed on controller {}",
                    segment.name, route.info.id
                )
            })?;
        if requested != *target {
            warn!(
                device_id = %route.info.id,
                zone = %segment.name,
                configured = target,
                requested,
                "OpenRGB clamped the configured zone size to the zone's advertised range"
            );
        }
        debug!(
            device_id = %route.info.id,
            zone = %segment.name,
            from = segment.led_count,
            to = requested,
            "OpenRGB zone resized"
        );
        resized = true;
        if !client.wait_for_device_list_update(ZONE_RESIZE_WAIT).await? {
            warn!(
                device_id = %route.info.id,
                zone = %segment.name,
                "OpenRGB did not announce a device list update after the zone resize"
            );
        }
    }
    if !resized {
        return Ok(route);
    }
    let refreshed = find_current_route(client, endpoint, &route.fingerprint, config)
        .await
        .context("OpenRGB re-enumeration after zone resize failed")?;
    ensure_route_output_enabled(&refreshed)?;
    for (segment, target) in refreshed
        .info
        .segments
        .iter()
        .filter_map(|segment| sizes.get(&segment.name).map(|target| (segment, *target)))
    {
        if segment.led_count != target {
            warn!(
                device_id = %refreshed.info.id,
                zone = %segment.name,
                configured = target,
                reported = segment.led_count,
                "OpenRGB zone size differs from the configured size after resize"
            );
        }
    }
    Ok(refreshed)
}

fn is_transport_error(error: &Error) -> bool {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<OpenRgbError>())
        .is_some_and(is_openrgb_transport_failure)
}

struct ConnectedController {
    runtime: Option<Arc<dyn DriverRuntimeActions>>,
    reconnect_requested: bool,
    previous_mode: Option<(u32, ControllerMode)>,
    route: ControllerRoute,
    accepting_frames: bool,
    shape_warning_logged: bool,
}

fn request_route_reconnect(id: DeviceId, controller: &mut ConnectedController) {
    if controller.reconnect_requested {
        return;
    }
    let Some(runtime) = controller.runtime.clone() else {
        return;
    };
    controller.reconnect_requested = true;
    let updated = DiscoveredDevice::from(controller.route.clone());
    tokio::spawn(async move {
        if let Err(error) = runtime
            .request_reconnect(id, DESCRIPTOR.id, Some(updated))
            .await
        {
            warn!(device_id = %id, error = %error, "OpenRGB lifecycle reconnect request failed");
        }
    });
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
    serial: String,
    location: String,
}

impl From<ControllerRoute> for DiscoveredDevice {
    fn from(route: ControllerRoute) -> Self {
        let mut metadata = HashMap::from([
            (METADATA_ENDPOINT.to_owned(), route.endpoint.to_string()),
            (
                METADATA_CONTROLLER_INDEX.to_owned(),
                route.controller_index.to_string(),
            ),
            (
                METADATA_FINGERPRINT.to_owned(),
                route.fingerprint.as_str().to_owned(),
            ),
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
        if !route.serial.trim().is_empty() {
            metadata.insert(METADATA_SERIAL.to_owned(), route.serial);
        }
        if !route.location.trim().is_empty() {
            metadata.insert(METADATA_LOCATION.to_owned(), route.location);
        }
        Self {
            fingerprint: route.fingerprint,
            connect_behavior: if route.auto_connect {
                DiscoveryConnectBehavior::AutoConnect
            } else {
                DiscoveryConnectBehavior::Deferred
            },
            // Deliberate refusal: OpenRGB device ids are bridge-local and
            // their stability across hosts has not been reviewed.
            claim: None,
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

/// Enumerate one endpoint, reusing the shared link when a controller on it
/// is connected and probing with a short-lived connection otherwise.
async fn discover_endpoint(
    endpoint: SocketAddr,
    config: &OpenRgbConfig,
) -> Result<Vec<ControllerRoute>> {
    if let Some(connection) = endpoint_pool().open(endpoint) {
        let mut link = connection.link.lock().await;
        if let Some(client) = link.client.as_mut() {
            match enumerate_routes(client, endpoint, config).await {
                Ok(routes) => return Ok(routes),
                Err(error) if is_transport_error(&error) => {
                    // The shared link is dead; let the endpoint task reconnect
                    // it and report the live server state from a fresh probe.
                    connection.fail_link(&mut link, &error).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    let mut client = connect_openrgb_client(config, endpoint).await?;
    let routes = enumerate_routes(&mut client, endpoint, config).await;
    if let Err(error) = client.close().await {
        debug!(endpoint = %endpoint, error = %error, "OpenRGB discovery probe close failed");
    }
    routes
}

/// Enumerate every controller the server reports and build routes for them.
///
/// A controller whose data block fails to parse is skipped; a transport
/// failure aborts the enumeration.
async fn enumerate_routes(
    client: &mut OpenRgbClient,
    endpoint: SocketAddr,
    config: &OpenRgbConfig,
) -> Result<Vec<ControllerRoute>> {
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
            Err(error) if is_openrgb_transport_failure(&error) => {
                return Err(Error::new(error).context(format!(
                    "OpenRGB controller {controller_index} enumeration failed at {endpoint}"
                )));
            }
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
    let routes = enumerate_routes(client, endpoint, config).await?;
    routes
        .into_iter()
        .find(|route| route.fingerprint == *fingerprint)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "OpenRGB controller fingerprint '{}' disappeared",
                fingerprint.as_str()
            )
        })
}

async fn configure_controller_output(
    client: &mut OpenRgbClient,
    route: &ControllerRoute,
    config: &OpenRgbConfig,
) -> Result<()> {
    client.set_custom_mode(route.controller_index).await?;
    if let Some((mode_index, mode)) = &route.writable_mode {
        let mode = output_mode_for_write(mode);
        client
            .update_mode(route.controller_index, *mode_index, &mode)
            .await?;
    }
    verify_controller_output_mode(client, route, mode_flag_policy(config)).await?;
    Ok(())
}

/// The mode block Hypercolor writes when activating realtime output.
///
/// A mode that advertises a brightness range is written at its maximum
/// instead of echoing whatever the server last reported; a correct mode
/// streaming correct frames into a controller left at low brightness is the
/// failure this prevents.
fn output_mode_for_write(mode: &ControllerMode) -> ControllerMode {
    let mut mode = mode.clone();
    if let Some(max) = mode_brightness_target(&mode) {
        mode.brightness = Some(max);
    }
    mode
}

/// The brightness a mode with an advertised range must report once active.
fn mode_brightness_target(mode: &ControllerMode) -> Option<u32> {
    match (mode.brightness_min, mode.brightness_max) {
        (Some(min), Some(max)) if min <= max => Some(max),
        _ => None,
    }
}

async fn verify_controller_output_mode(
    client: &mut OpenRgbClient,
    route: &ControllerRoute,
    policy: ModeFlagPolicy,
) -> Result<()> {
    let controller = client
        .controller_data(route.controller_index)
        .await
        .context("OpenRGB active mode readback failed after output setup")?;
    let Some((mode_index, mode)) = active_mode_snapshot(&controller) else {
        bail!(
            "OpenRGB controller {} has no active mode after output setup",
            route.info.id
        );
    };
    if !mode.is_realtime_writable(policy) {
        bail!(
            "OpenRGB controller {} active mode {mode_index} is not approved for realtime output",
            route.info.id
        );
    }
    if let Some(expected) = mode_brightness_target(&mode)
        && mode.brightness != Some(expected)
    {
        bail!(
            "OpenRGB controller {} active mode {mode_index} reports brightness {} after \
             output setup; expected the advertised maximum {expected}",
            route.info.id,
            mode.brightness
                .map_or_else(|| "none".to_owned(), |value| value.to_string()),
        );
    }
    Ok(())
}

async fn teardown_connected_controller(
    client: &mut OpenRgbClient,
    controller: &mut ConnectedController,
    policy: OpenRgbTeardownPolicy,
) -> Result<()> {
    match policy {
        OpenRgbTeardownPolicy::RestorePreviousOrLeave => {
            if let Err(error) = try_restore_previous_mode(client, controller).await {
                debug!(
                    controller_index = controller.route.controller_index,
                    error = %error,
                    "OpenRGB previous mode restore failed; leaving last frame"
                );
            }
        }
        OpenRgbTeardownPolicy::RestorePreviousOrBlackout => {
            match try_restore_previous_mode(client, controller).await {
                Ok(true) => {}
                Ok(false) => {
                    blackout_controller(client, controller).await?;
                }
                Err(error) => {
                    debug!(
                        controller_index = controller.route.controller_index,
                        error = %error,
                        "OpenRGB previous mode restore failed; blacking out"
                    );
                    blackout_controller(client, controller).await?;
                }
            }
        }
        OpenRgbTeardownPolicy::Blackout => {
            blackout_controller(client, controller).await?;
        }
        OpenRgbTeardownPolicy::LeaveLastFrame => {}
    }
    Ok(())
}

async fn try_restore_previous_mode(
    client: &mut OpenRgbClient,
    controller: &ConnectedController,
) -> Result<bool> {
    let Some((mode_index, mode)) = controller.previous_mode.clone() else {
        return Ok(false);
    };
    client
        .update_mode(controller.route.controller_index, mode_index, &mode)
        .await
        .context("OpenRGB previous mode restore failed")?;
    Ok(true)
}

async fn blackout_controller(
    client: &mut OpenRgbClient,
    controller: &ConnectedController,
) -> Result<()> {
    let led_count = usize::try_from(controller.route.info.capabilities.led_count)
        .context("OpenRGB LED count does not fit usize")?;
    let colors = vec![RgbColor::new(0, 0, 0); led_count];
    client
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
        let key = format!("duplicate:{}:{ordinal}", route.info.id);
        route.fingerprint = DeviceFingerprint::mint(FingerprintNamespace::Bridge, "openrgb", &key);
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
    let target_fps = target_fps(config, fingerprint.as_str(), &detector_class);
    let info = build_device_info(
        device_id,
        &controller,
        &fingerprint,
        confidence,
        &detector_class,
        target_fps,
        disabled_reason.is_none(),
    );

    let mut route = ControllerRoute {
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
        serial: controller.serial,
        location: controller.location,
    };
    route.auto_connect &=
        route.disabled_reason.is_none() || can_restore_zero_led_zones(&route, config);
    route
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
    DeviceFingerprint::mint(
        FingerprintNamespace::Bridge,
        "openrgb",
        &format!("{endpoint}:{identity}"),
    )
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
    if reported_controller_led_count == 0 {
        return Some(ZERO_LEDS_REASON.to_owned());
    }
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
    let segments = controller
        .zones
        .iter()
        .map(segment_info)
        .collect::<Vec<_>>();
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
        segments,
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

fn segment_info(zone: &ControllerZone) -> SegmentInfo {
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
    SegmentInfo {
        name: zone.name.clone(),
        led_count,
        topology,
        color_format: DeviceColorFormat::Rgb,
        layout_hint: None,
    }
}

/// Resolve a controller's cadence.
///
/// Precedence: a `controller_fps` entry for the fingerprint, then one for
/// the detector class, then the detector-class default table, then
/// `default_target_fps`.
fn target_fps(config: &OpenRgbConfig, fingerprint: &str, detector_class: &str) -> u32 {
    lookup_case_insensitive(&config.controller_fps, fingerprint)
        .or_else(|| lookup_case_insensitive(&config.controller_fps, detector_class))
        .or_else(|| detector_class_default_fps(detector_class))
        .unwrap_or(config.default_target_fps)
        .max(1)
}

/// Detector-class cadence defaults. `smbus` matches the native SMBus backend;
/// `hid` and every other class fall through to `default_target_fps`.
fn detector_class_default_fps(detector_class: &str) -> Option<u32> {
    match detector_class {
        "smbus" => Some(SMBUS_DETECTOR_TARGET_FPS),
        _ => None,
    }
}

fn lookup_case_insensitive<V: Copy>(map: &BTreeMap<String, V>, key: &str) -> Option<V> {
    map.get(key).copied().or_else(|| {
        map.iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
            .map(|(_, value)| *value)
    })
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
        allow_zone_resize: !config.zone_sizes.is_empty(),
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
    for (fingerprint, zones) in &config.zone_sizes {
        if fingerprint.trim().is_empty() {
            bail!("OpenRGB zone_sizes keys must be controller fingerprints, not empty");
        }
        if zones.keys().any(|zone| zone.trim().is_empty()) {
            bail!("OpenRGB zone_sizes for '{fingerprint}' contains an empty zone name");
        }
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
        (FIELD_ZONE_SIZES.to_owned(), json!(config.zone_sizes)),
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disconnect_gate_prevents_post_cleanup_tracked_publication() {
        let (frame_tx, _frame_rx) = watch::channel(None::<Arc<OpenRgbFramePayload>>);
        let active = Arc::new(AtomicBool::new(true));
        let lifecycle_gate = Arc::new(StdMutex::new(()));
        let device_id = DeviceId::new();
        let sink = OpenRgbFrameSink {
            device_id,
            frame_tx: frame_tx.clone(),
            active: Arc::clone(&active),
            lifecycle_gate: Arc::clone(&lifecycle_gate),
            last_async_error: Arc::new(StdMutex::new(None)),
        };
        let delivery_id = DeviceDeliveryId {
            queue_generation: 37,
            sequence: 6,
        };

        let gate = lock_lifecycle_gate(&lifecycle_gate);
        let delivery = tokio::spawn(async move {
            sink.deliver_colors_shared(delivery_id, Arc::new(vec![[1, 2, 3]]))
                .await
        });
        active.store(false, Ordering::Release);
        if let Some(pending) = frame_tx.send_replace(None) {
            pending.reject_pending(DeviceError::Disconnected {
                device: device_id.to_string(),
            });
        }
        drop(gate);

        let ack = tokio::time::timeout(Duration::from_secs(1), delivery)
            .await
            .expect("publication blocked across cleanup should not hang")
            .expect("delivery task should join");
        assert_eq!(ack.id, delivery_id);
        assert_eq!(
            ack.status,
            hypercolor_driver_api::DeviceDeliveryStatus::Failed
        );
        assert!(!ack.transport_started);
        assert!(frame_tx.borrow().is_none());
    }

    #[test]
    fn sdk_timeout_maps_to_typed_boundary_timeouts() {
        let device_id = DeviceId::new();
        let after = Duration::from_millis(325);
        let error = Error::new(OpenRgbError::Timeout {
            operation: "write",
            after,
        })
        .context("OpenRGB update_leds failed");

        assert_eq!(
            map_openrgb_device_error(device_id, &error, OpenRgbDeviceOperation::Write),
            DeviceError::Timeout { after }
        );
        assert_eq!(
            map_openrgb_device_error(device_id, &error, OpenRgbDeviceOperation::Connect),
            DeviceError::Timeout { after }
        );
        assert!(matches!(
            map_openrgb_driver_error(&error),
            DriverError::Timeout { after: mapped } if mapped == after
        ));
    }

    #[test]
    fn sdk_connection_close_maps_to_typed_disconnect() {
        let device_id = DeviceId::new();
        let error = Error::new(OpenRgbError::ConnectionClosed)
            .context("OpenRGB protocol negotiation failed");
        let expected = DeviceError::Disconnected {
            device: device_id.to_string(),
        };

        assert_eq!(
            map_openrgb_device_error(device_id, &error, OpenRgbDeviceOperation::Connect),
            expected
        );
        assert_eq!(
            map_openrgb_device_error(device_id, &error, OpenRgbDeviceOperation::Write),
            expected
        );
    }

    #[test]
    fn discovery_propagates_transport_failures() {
        assert!(is_openrgb_transport_failure(&OpenRgbError::Timeout {
            operation: "read",
            after: Duration::from_millis(25),
        }));
        assert!(is_openrgb_transport_failure(
            &OpenRgbError::ConnectionClosed
        ));
        assert!(is_openrgb_transport_failure(&OpenRgbError::Io(
            "connection reset".to_owned()
        )));
        assert!(!is_openrgb_transport_failure(&OpenRgbError::InvalidUtf8));
    }

    #[tokio::test]
    async fn queued_async_timeout_reaches_delivery_ack_typed() {
        let (frame_tx, _frame_rx) = watch::channel(None::<Arc<OpenRgbFramePayload>>);
        let device_id = DeviceId::new();
        let after = Duration::from_millis(325);
        let sink = OpenRgbFrameSink {
            device_id,
            frame_tx,
            active: Arc::new(AtomicBool::new(true)),
            lifecycle_gate: Arc::new(StdMutex::new(())),
            last_async_error: Arc::new(StdMutex::new(Some(DeviceError::Timeout { after }))),
        };
        let delivery_id = DeviceDeliveryId {
            queue_generation: 8,
            sequence: 13,
        };

        let ack = sink
            .deliver_colors_shared(delivery_id, Arc::new(vec![[1, 2, 3]]))
            .await;

        assert_eq!(ack.id, delivery_id);
        assert_eq!(
            ack.status,
            hypercolor_driver_api::DeviceDeliveryStatus::Failed
        );
        assert!(!ack.transport_started);
        assert_eq!(ack.error, Some(DeviceError::Timeout { after }));
    }

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

    #[tokio::test]
    async fn temporary_identify_is_offered_and_disabled_routes_answer_their_reason() {
        let config = OpenRgbConfig {
            ownership: OpenRgbOwnership {
                mode: OpenRgbOwnershipMode::OpenRgbOwned,
                ..OpenRgbOwnership::default()
            },
            ..OpenRgbConfig::default()
        };
        let backend = OpenRgbBackend::new(config).expect("config should validate");
        let endpoint = default_endpoints()[0];

        let enabled = build_route(endpoint, 0, 5, sample_controller(), &backend.config);
        assert!(enabled.disabled_reason.is_none());
        let enabled_info = enabled.info.clone();
        backend
            .adopt_device(&DiscoveredDevice::from(enabled))
            .expect("enabled route should adopt");
        assert!(backend.supports_temporary_direct_control(&enabled_info));

        let mut unwritable = sample_controller();
        unwritable.serial = "OTHER".to_owned();
        unwritable.modes[0].flags = 0;
        let disabled = build_route(endpoint, 1, 5, unwritable, &backend.config);
        let reason = disabled
            .disabled_reason
            .clone()
            .expect("unwritable controller should be disabled");
        let disabled_info = disabled.info.clone();
        assert!(!disabled_info.capabilities.supports_direct);
        backend
            .adopt_device(&DiscoveredDevice::from(disabled))
            .expect("disabled route should adopt");
        assert!(
            backend.supports_temporary_direct_control(&disabled_info),
            "disabled routes opt in so connect() can surface the reason"
        );
        let error = backend
            .connect(&disabled_info.id)
            .await
            .expect_err("output-disabled route must refuse to connect");
        assert_eq!(
            error,
            DeviceError::connection(disabled_info.id, &reason),
            "the refusal carries the disabled_reason verbatim"
        );

        let mut empty = sample_controller();
        empty.serial = "EMPTY".to_owned();
        empty.leds.clear();
        empty.colors.clear();
        empty.zones[0].leds_count = 0;
        let zero = build_route(endpoint, 2, 5, empty, &backend.config);
        let zero_info = zero.info.clone();
        backend
            .adopt_device(&DiscoveredDevice::from(zero))
            .expect("zero-LED route should adopt");
        assert!(
            !backend.supports_temporary_direct_control(&zero_info),
            "nothing to flash on a zero-LED controller"
        );
        let unknown = DeviceInfo {
            id: DeviceId::new(),
            ..enabled_info
        };
        assert!(
            !backend.supports_temporary_direct_control(&DeviceInfo {
                capabilities: DeviceCapabilities {
                    supports_direct: false,
                    ..unknown.capabilities
                },
                ..unknown
            }),
            "an unadopted, non-direct device is not offered"
        );
    }

    #[test]
    fn zone_sizes_parse_validate_and_gate_the_resize_opcode() {
        let module = OpenRgbDriverModule;
        let entry = module.default_config();
        assert_eq!(entry.settings["zone_sizes"], json!({}));

        let mut config = OpenRgbConfig::default();
        assert!(!client_config(&config).allow_zone_resize);
        config.zone_sizes.insert(
            "bridge:openrgb:127.0.0.1:6742:serial:SER123".to_owned(),
            BTreeMap::from([("Channel ATX 1".to_owned(), 24)]),
        );
        assert!(client_config(&config).allow_zone_resize);
        validate_openrgb_config(&config).expect("zone sizes should validate");

        let settings = openrgb_config_settings(&config);
        let entry = DriverConfigEntry::enabled(settings);
        let parsed = DriverConfigView {
            driver_id: DESCRIPTOR.id,
            entry: &entry,
        }
        .parse_settings::<OpenRgbConfig>()
        .expect("zone sizes should round-trip through settings");
        assert_eq!(parsed.zone_sizes, config.zone_sizes);

        config.zone_sizes.insert(
            "bridge:openrgb:127.0.0.1:6742:serial:OTHER".to_owned(),
            BTreeMap::from([(String::new(), 4)]),
        );
        assert!(validate_openrgb_config(&config).is_err());
    }

    #[test]
    fn configured_zone_sizes_match_fingerprints_case_insensitively_for_stable_identities() {
        let ownership = OpenRgbOwnership {
            mode: OpenRgbOwnershipMode::OpenRgbOwned,
            ..OpenRgbOwnership::default()
        };
        let mut config = OpenRgbConfig {
            ownership,
            ..OpenRgbConfig::default()
        };
        let endpoint = default_endpoints()[0];
        let route = build_route(endpoint, 0, 5, sample_controller(), &config);
        assert_eq!(route.confidence, IdentityConfidence::High);
        assert!(
            configured_zone_sizes(&config, &route)
                .expect("no entry is fine")
                .is_none()
        );

        config.zone_sizes.insert(
            route.fingerprint.as_str().to_ascii_uppercase(),
            BTreeMap::from([("Main".to_owned(), 8)]),
        );
        let sizes = configured_zone_sizes(&config, &route)
            .expect("stable identity should be honored")
            .expect("upper-cased key should still match");
        assert_eq!(sizes.get("Main"), Some(&8));

        let mut shape_only = sample_controller();
        shape_only.serial.clear();
        shape_only.location.clear();
        let medium = build_route(endpoint, 1, 5, shape_only, &config);
        assert_eq!(medium.confidence, IdentityConfidence::Medium);
        config.zone_sizes.insert(
            medium.fingerprint.as_str().to_owned(),
            BTreeMap::from([("Main".to_owned(), 8)]),
        );
        let error = configured_zone_sizes(&config, &medium)
            .expect_err("shape-based fingerprints must be refused");
        assert!(error.to_string().contains("serial or location"));
    }

    #[test]
    fn discovery_metadata_publishes_identity_strings_verbatim() {
        let config = OpenRgbConfig {
            ownership: OpenRgbOwnership {
                mode: OpenRgbOwnershipMode::OpenRgbOwned,
                ..OpenRgbOwnership::default()
            },
            ..OpenRgbConfig::default()
        };
        let mut controller = sample_controller();
        controller.location = "/dev/hidraw7 ".to_owned();
        let route = build_route(default_endpoints()[0], 0, 5, controller, &config);
        let discovered = DiscoveredDevice::from(route);
        assert_eq!(discovered.metadata["serial"], "SER123");
        assert_eq!(discovered.metadata["location"], "/dev/hidraw7 ");
        assert_eq!(
            discovered.metadata["endpoint"],
            default_endpoints()[0].to_string()
        );

        let mut anonymous = sample_controller();
        anonymous.serial.clear();
        anonymous.location = "   ".to_owned();
        let route = build_route(default_endpoints()[0], 0, 5, anonymous, &config);
        let discovered = DiscoveredDevice::from(route);
        assert!(!discovered.metadata.contains_key("serial"));
        assert!(!discovered.metadata.contains_key("location"));

        let backend = OpenRgbBackend::new(config).expect("config should validate");
        let mut controller = sample_controller();
        controller.location = "hidraw0".to_owned();
        let route = build_route(default_endpoints()[0], 0, 5, controller, &backend.config);
        let id = route.info.id;
        backend
            .adopt_device(&DiscoveredDevice::from(route))
            .expect("backend should adopt its own discovery result");
        let adopted = backend
            .discovered_route(&id)
            .expect("adopted route should be tracked");
        assert_eq!(adopted.serial, "SER123");
        assert_eq!(adopted.location, "hidraw0");
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
    fn reconnect_backoff_doubles_to_a_capped_ceiling_with_bounded_jitter() {
        assert_eq!(
            reconnect_backoff_with_jitter(0, 0.0),
            Duration::from_secs(1)
        );
        assert_eq!(
            reconnect_backoff_with_jitter(3, 0.0),
            Duration::from_secs(8)
        );
        assert_eq!(
            reconnect_backoff_with_jitter(6, 0.0),
            Duration::from_mins(1),
            "64 s is clamped to the 60 s ceiling"
        );
        assert_eq!(
            reconnect_backoff_with_jitter(40, 0.0),
            Duration::from_mins(1),
            "large failure counts must not overflow the shift"
        );
        assert_eq!(
            reconnect_backoff_with_jitter(0, 1.0),
            Duration::from_millis(1_100)
        );
        assert_eq!(
            reconnect_backoff_with_jitter(0, -1.0),
            Duration::from_millis(900)
        );
        assert_eq!(
            reconnect_backoff_with_jitter(1, 7.0),
            Duration::from_millis(2_200),
            "jitter is clamped to the +-10% band"
        );
        for _ in 0..64 {
            let sampled = reconnect_backoff(0);
            assert!(
                sampled >= Duration::from_millis(900) && sampled <= Duration::from_millis(1_100),
                "sampled backoff {sampled:?} left the jitter band"
            );
        }
    }

    #[test]
    fn target_fps_prefers_overrides_then_detector_class_defaults() {
        let mut config = OpenRgbConfig::default();
        assert_eq!(SMBUS_DETECTOR_TARGET_FPS, 62);
        assert_eq!(
            target_fps(&config, "fp", "smbus"),
            SMBUS_DETECTOR_TARGET_FPS
        );
        assert_eq!(target_fps(&config, "fp", "hid"), DEFAULT_TARGET_FPS);
        assert_eq!(target_fps(&config, "fp", "virtual"), DEFAULT_TARGET_FPS);

        config.default_target_fps = 20;
        assert_eq!(target_fps(&config, "fp", "hid"), 20);
        assert_eq!(
            target_fps(&config, "fp", "smbus"),
            SMBUS_DETECTOR_TARGET_FPS,
            "class table outranks the global default"
        );

        config.controller_fps.insert("smbus".to_owned(), 15);
        assert_eq!(target_fps(&config, "fp", "smbus"), 15);
        config
            .controller_fps
            .insert("BRIDGE:OPENRGB:FP".to_owned(), 45);
        assert_eq!(
            target_fps(&config, "bridge:openrgb:fp", "smbus"),
            45,
            "fingerprint override wins and matches case-insensitively"
        );
        assert_eq!(frame_interval_for_fps(0), Duration::from_secs(1));
        assert_eq!(frame_interval_for_fps(20), Duration::from_millis(50));
    }

    #[test]
    fn output_mode_is_written_at_advertised_brightness_maximum() {
        let mut mode = sample_mode();
        mode.brightness = Some(40);
        let written = output_mode_for_write(&mode);
        assert_eq!(written.brightness, Some(100));
        assert_eq!(written.flags, mode.flags);

        let mut without_range = sample_mode();
        without_range.brightness_min = None;
        without_range.brightness_max = None;
        without_range.brightness = None;
        assert_eq!(output_mode_for_write(&without_range).brightness, None);
        assert_eq!(mode_brightness_target(&without_range), None);

        let mut inverted = sample_mode();
        inverted.brightness_min = Some(100);
        inverted.brightness_max = Some(0);
        inverted.brightness = Some(7);
        assert_eq!(output_mode_for_write(&inverted).brightness, Some(7));
    }

    #[test]
    fn frame_is_padded_or_truncated_to_controller_led_count() {
        let frame = [[1, 2, 3], [4, 5, 6], [7, 8, 9]];

        let truncated = fit_frame_to_led_count(&frame, 2);
        assert_eq!(
            truncated,
            vec![RgbColor::new(1, 2, 3), RgbColor::new(4, 5, 6)]
        );

        let padded = fit_frame_to_led_count(&frame[..1], 3);
        assert_eq!(
            padded,
            vec![
                RgbColor::new(1, 2, 3),
                RgbColor::new(0, 0, 0),
                RgbColor::new(0, 0, 0)
            ]
        );

        assert_eq!(fit_frame_to_led_count(&frame, 3).len(), 3);
    }

    #[test]
    fn zero_led_controller_is_output_disabled() {
        let mut controller = sample_controller();
        controller.leds.clear();
        controller.colors.clear();
        controller.zones[0].leds_count = 0;
        let config = OpenRgbConfig {
            ownership: OpenRgbOwnership {
                mode: OpenRgbOwnershipMode::OpenRgbOwned,
                ..OpenRgbOwnership::default()
            },
            ..OpenRgbConfig::default()
        };

        let route = build_route(default_endpoints()[0], 0, 5, controller, &config);

        assert_eq!(route.info.capabilities.led_count, 0);
        assert!(!route.info.capabilities.supports_direct);
        assert!(
            route
                .disabled_reason
                .expect("zero-LED controller should be disabled")
                .contains("zero LEDs")
        );
    }

    #[test]
    fn controller_side_shape_change_disables_output_with_rescan_reason() {
        let config = OpenRgbConfig {
            ownership: OpenRgbOwnership {
                mode: OpenRgbOwnershipMode::OpenRgbOwned,
                ..OpenRgbOwnership::default()
            },
            ..OpenRgbConfig::default()
        };
        let endpoint = default_endpoints()[0];
        let previous = build_route(endpoint, 0, 5, sample_controller(), &config);

        let mut same = build_route(endpoint, 1, 5, sample_controller(), &config);
        apply_shape_change(&previous, &mut same);
        assert!(
            same.disabled_reason.is_none(),
            "index remap alone is not a shape change"
        );

        let mut grown = sample_controller();
        grown.zones[0].leds_count = 6;
        grown.leds.extend((4..6).map(|index| LedData {
            name: index.to_string(),
            value: index,
        }));
        grown.colors.resize(6, RgbColor::new(0, 0, 0));
        let mut current = build_route(endpoint, 0, 5, grown, &config);
        apply_shape_change(&previous, &mut current);

        assert_eq!(current.info.capabilities.led_count, 6);
        assert!(!current.info.capabilities.supports_direct);
        assert_eq!(
            current.disabled_reason.as_deref(),
            Some("zone shape changed (was 4, now 6); rescan")
        );
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
