//! OpenRGB fallback Bridge driver for Hypercolor.
//!
//! This driver talks to a user-managed OpenRGB SDK server over the clean
//! `hypercolor-openrgb-sdk` crate. It does not supervise or bundle OpenRGB.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceFrameSink, DiscoveredDevice, DiscoveryCapability,
    DiscoveryConnectBehavior, DiscoveryRequest, DiscoveryResult, DriverConfigProvider,
    DriverConfigView, DriverDescriptor, DriverDiscoveredDevice, DriverHost, DriverModule,
    DriverPresentationProvider,
};
use hypercolor_openrgb_sdk::{
    ControllerData, ControllerMode, ControllerZone, DeviceType, ModeFlagPolicy, OpenRgbClient,
    OpenRgbClientConfig, RgbColor,
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
use tokio::sync::Mutex;
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
const FIELD_OWNERSHIP: &str = "ownership";
const FIELD_DEFAULT_TARGET_FPS: &str = "default_target_fps";
const FIELD_CONTROLLER_FPS: &str = "controller_fps";
const FIELD_MODE_PER_LED_MASK: &str = "mode_per_led_mask";
const FIELD_MODE_PERSISTENT_MASK: &str = "mode_persistent_mask";

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
    #[serde(default)]
    pub ownership: OpenRgbOwnership,
    #[serde(default = "default_target_fps")]
    pub default_target_fps: u32,
    #[serde(default)]
    pub controller_fps: BTreeMap<String, u32>,
    #[serde(default = "default_per_led_mask")]
    pub mode_per_led_mask: u32,
    #[serde(default)]
    pub mode_persistent_mask: u32,
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
            ownership: OpenRgbOwnership::default(),
            default_target_fps: default_target_fps(),
            controller_fps: BTreeMap::new(),
            mode_per_led_mask: default_per_led_mask(),
            mode_persistent_mask: 0,
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
    connected: HashMap<DeviceId, Arc<Mutex<ConnectedController>>>,
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
        let routes = discover_routes(&self.config).await?;
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

        let mut client =
            OpenRgbClient::connect(route.endpoint, client_config(&self.config)).await?;
        if self.config.startup_rescan {
            client.request_rescan().await?;
        }
        client.set_custom_mode(route.controller_index).await?;
        if let Some((mode_index, mode)) = route.writable_mode.clone() {
            client
                .update_mode(route.controller_index, mode_index, &mode)
                .await?;
        }

        self.connected.insert(
            *id,
            Arc::new(Mutex::new(ConnectedController {
                route,
                client,
                consecutive_failures: 0,
            })),
        );
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        self.connected.remove(id);
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let Some(controller) = self.connected.get(id) else {
            bail!("OpenRGB controller {id} is not connected");
        };
        write_controller_colors(controller, colors).await
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        self.connected
            .get(id)
            .and_then(|controller| {
                controller
                    .try_lock()
                    .ok()
                    .map(|route| route.route.target_fps)
            })
            .or_else(|| self.discovered.get(id).map(|route| route.target_fps))
    }

    async fn health_check(&self, id: &DeviceId) -> Result<hypercolor_driver_api::HealthStatus> {
        if self.connected.contains_key(id) {
            return Ok(hypercolor_driver_api::HealthStatus::Healthy);
        }
        if self.discovered.contains_key(id) {
            return Ok(hypercolor_driver_api::HealthStatus::Degraded);
        }
        Ok(hypercolor_driver_api::HealthStatus::Unreachable)
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.connected.get(id).map(|controller| {
            Arc::new(OpenRgbFrameSink {
                controller: Arc::clone(controller),
            }) as Arc<dyn DeviceFrameSink>
        })
    }
}

struct OpenRgbFrameSink {
    controller: Arc<Mutex<ConnectedController>>,
}

#[async_trait]
impl DeviceFrameSink for OpenRgbFrameSink {
    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<()> {
        write_controller_colors(&self.controller, colors.as_slice()).await
    }
}

async fn write_controller_colors(
    controller: &Arc<Mutex<ConnectedController>>,
    colors: &[[u8; 3]],
) -> Result<()> {
    let mut controller = controller.lock().await;
    let colors = colors
        .iter()
        .map(|[red, green, blue]| RgbColor::new(*red, *green, *blue))
        .collect::<Vec<_>>();
    let controller_index = controller.route.controller_index;
    match controller
        .client
        .update_leds(controller_index, &colors)
        .await
    {
        Ok(()) => {
            controller.consecutive_failures = 0;
            Ok(())
        }
        Err(error) => {
            controller.consecutive_failures = controller.consecutive_failures.saturating_add(1);
            Err(error).context("OpenRGB update_leds failed")
        }
    }
}

struct ConnectedController {
    route: ControllerRoute,
    client: OpenRgbClient,
    consecutive_failures: u32,
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
    target_fps: u32,
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
            connect_behavior: if route.disabled_reason.is_none() {
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
    let mut client = OpenRgbClient::connect(endpoint, client_config(config)).await?;
    if config.startup_rescan {
        client.request_rescan().await?;
    }

    let protocol_version = client.protocol_version();
    let count = client.controller_count().await?;
    let mut routes = Vec::new();
    for controller_index in 0..count {
        let controller = client.controller_data(controller_index).await?;
        routes.push(build_route(
            endpoint,
            controller_index,
            protocol_version,
            controller,
            config,
        ));
    }
    Ok(routes)
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
    let writable_mode = select_writable_mode(
        &controller,
        ModeFlagPolicy {
            per_led_color_mask: config.mode_per_led_mask,
            persistent_mask: config.mode_persistent_mask,
        },
    );
    let disabled_reason = output_disabled_reason(
        &config.ownership,
        confidence,
        &detector_class,
        writable_mode.as_ref(),
    );
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
        target_fps,
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
    if confidence == IdentityConfidence::Low && !ownership.allow_low_confidence {
        return Some("OpenRGB identity confidence is low; assign ownership explicitly".to_owned());
    }
    if writable_mode.is_none() {
        return Some("OpenRGB controller has no approved per-LED writable mode".to_owned());
    }
    None
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
    let led_count = zones.iter().map(|zone| zone.led_count).sum();
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
    Ok(())
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
        (FIELD_OWNERSHIP.to_owned(), json!(config.ownership)),
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
            "hid",
            Some(&(0, sample_mode())),
        );
        assert!(
            reason
                .expect("low-confidence controller should be disabled")
                .contains("low")
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
