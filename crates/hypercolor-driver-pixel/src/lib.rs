//! Generic pixel controller driver for Hypercolor.
//!
//! Streams to manually configured receivers that speak the open pixel
//! protocols: E1.31 (sACN) and Art-Net 4 ArtDmx for DMX512 universes, and
//! DDP for offset-addressed byte streams. That covers Falcon controllers,
//! Falcon Player (FPP), ESPixelStick, xLights-style props, and DMX
//! fixtures behind Art-Net or sACN nodes.
//!
//! Every device comes from the driver config; "discovery" materializes
//! those entries and never sends a packet. Output is unicast only.

pub mod backend;
pub mod color;
pub mod config;
pub mod controls;
pub mod plan;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use hypercolor_driver_api::{
    ControlApplyTarget, DeviceBackend, DeviceBackendFactory, DiscoveredDevice, DiscoveryCapability,
    DiscoveryConnectBehavior, DiscoveryRequest, DriverConfigProvider, DriverConfigView,
    DriverControlProvider, DriverDescriptor, DriverError, DriverHost, DriverModule,
    DriverPresentationProvider, OutputBinding, TrackedDeviceCtx, ValidatedControlChanges,
};
use hypercolor_driver_support::control_apply;
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::controls::{
    ApplyControlChangesResponse, ControlChange, ControlSurfaceDocument,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceClassHint, DeviceColorFormat, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceInfo, DeviceOrigin, DeviceTopologyHint,
    DriverPresentation, DriverTransportKind, FingerprintNamespace, SegmentInfo,
};
use hypercolor_types::identity::BackendId;

pub use backend::{KEEPALIVE_INTERVAL, METADATA_DEVICE_CONFIG, PixelBackend};
pub use color::{ColorOrder, encode_pixels};
pub use config::{
    PixelConfig, PixelConfigError, PixelDeviceConfig, PixelProtocol, PixelSegmentConfig,
};
pub use controls::{pixel_device_control_surface, pixel_driver_control_surface};
pub use plan::{PixelDevicePlan, derived_cid, resolve_config, resolve_device};

/// Static driver descriptor.
pub static DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "pixel",
    "Pixel Controllers",
    DriverTransportKind::Network,
    true,
    false,
);

/// Device capability frame rate reported when a device follows the render loop.
const RENDER_LOOP_MAX_FPS: u32 = 60;

/// Pixel controller driver module.
#[derive(Debug, Clone, Default)]
pub struct PixelDriverModule;

impl PixelDriverModule {
    /// Create the driver module.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl DriverModule for PixelDriverModule {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn output(&self) -> OutputBinding<'_> {
        OutputBinding::Owned {
            id: BackendId::new(DESCRIPTOR.id).expect("pixel backend ID must be valid"),
            factory: self,
        }
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }

    fn config(&self) -> Option<&dyn DriverConfigProvider> {
        Some(self)
    }

    fn controls(&self) -> Option<&dyn DriverControlProvider> {
        Some(self)
    }

    fn presentation(&self) -> Option<&dyn DriverPresentationProvider> {
        Some(self)
    }
}

impl DeviceBackendFactory for PixelDriverModule {
    fn build(
        &self,
        _host: &dyn DriverHost,
        _config: DriverConfigView<'_>,
    ) -> std::result::Result<Arc<dyn DeviceBackend>, DriverError> {
        // The backend is config-free: every device carries its effective
        // config through discovery metadata, so a config edit never needs
        // a backend rebuild to reach new connections.
        Ok(Arc::new(PixelBackend::new()))
    }
}

impl DriverPresentationProvider for PixelDriverModule {
    fn presentation(&self) -> DriverPresentation {
        DriverPresentation {
            label: "Pixel Controllers".to_owned(),
            short_label: Some("Pixel".to_owned()),
            accent_rgb: Some([128, 255, 234]),
            secondary_rgb: Some([255, 106, 193]),
            icon: Some("lightbulb".to_owned()),
            default_device_class: Some(DeviceClassHint::Controller),
        }
    }
}

#[async_trait]
impl DiscoveryCapability for PixelDriverModule {
    async fn discover(
        &self,
        _host: &dyn DriverHost,
        _request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> std::result::Result<Vec<DiscoveredDevice>, DriverError> {
        let config = config.parse_settings::<PixelConfig>()?;
        configured_devices(&config).map_err(|error| DriverError::Configuration {
            message: error.to_string(),
        })
    }
}

/// Materialize every configured device as a discovery result.
///
/// Sends no packets: pixel controllers are configured, not probed.
///
/// # Errors
///
/// Returns the first config validation failure.
pub fn configured_devices(
    config: &PixelConfig,
) -> std::result::Result<Vec<DiscoveredDevice>, PixelConfigError> {
    let plans = resolve_config(config)?;
    Ok(config
        .devices
        .iter()
        .zip(plans)
        .map(|(device, plan)| discovered_device(&config.effective_device(device), &plan))
        .collect())
}

fn discovered_device(effective: &PixelDeviceConfig, plan: &PixelDevicePlan) -> DiscoveredDevice {
    let fingerprint = DeviceFingerprint::mint(FingerprintNamespace::Net, DESCRIPTOR.id, &plan.id);
    let color_format = if plan.color_order.has_white() {
        DeviceColorFormat::Rgbw
    } else {
        DeviceColorFormat::Rgb
    };
    let total_pixels = plan.total_pixels();
    let info = DeviceInfo {
        id: fingerprint.stable_device_id(),
        name: plan.name.clone(),
        vendor: "Pixel Controller".to_owned(),
        family: DeviceFamily::new_static("pixel", "Pixel Controller"),
        model: Some(plan.protocol.label().to_owned()),
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native(DESCRIPTOR.id, DESCRIPTOR.id, ConnectionType::Network),
        segments: plan
            .segments
            .iter()
            .map(|segment| SegmentInfo {
                name: segment.name.clone(),
                led_count: segment.pixel_count,
                topology: DeviceTopologyHint::Strip,
                color_format,
                layout_hint: None,
            })
            .collect(),
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: total_pixels,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: if plan.max_fps > 0 {
                plan.max_fps
            } else {
                RENDER_LOOP_MAX_FPS
            },
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };

    let mut metadata = HashMap::from([
        ("ip".to_owned(), plan.target.ip().to_string()),
        ("port".to_owned(), plan.target.port().to_string()),
        ("protocol".to_owned(), plan.protocol.as_str().to_owned()),
        (
            METADATA_DEVICE_CONFIG.to_owned(),
            serde_json::to_string(effective).unwrap_or_default(),
        ),
    ]);
    if let Some(universes) = plan.universe_summary() {
        metadata.insert("universes".to_owned(), universes);
    }

    DiscoveredDevice {
        fingerprint,
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        info,
        metadata,
        // Deliberate refusal: identity comes from a user-chosen config id,
        // not from anything the hardware reported.
        claim: None,
    }
}

impl DriverConfigProvider for PixelDriverModule {
    fn default_config(&self) -> DriverConfigEntry {
        let defaults = PixelConfig::default();
        DriverConfigEntry::enabled(BTreeMap::from([
            ("devices".to_owned(), serde_json::json!([])),
            (
                "source_name".to_owned(),
                serde_json::json!(defaults.source_name),
            ),
            ("priority".to_owned(), serde_json::json!(defaults.priority)),
        ]))
    }

    fn validate_config(&self, config: &DriverConfigEntry) -> std::result::Result<(), DriverError> {
        controls::parse_validated_config(config)
            .map(|_| ())
            .map_err(|error| DriverError::Configuration {
                message: format!("{error:#}"),
            })
    }
}

#[async_trait]
impl DriverControlProvider for PixelDriverModule {
    async fn driver_surface(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(pixel_driver_control_surface(
            &config.parse_settings::<PixelConfig>()?,
        )))
    }

    async fn device_surface(
        &self,
        _host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(pixel_device_control_surface(device)))
    }

    async fn validate_changes(
        &self,
        _host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> Result<ValidatedControlChanges> {
        controls::validate_pixel_changes(target, changes)
    }

    async fn apply_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: ValidatedControlChanges,
    ) -> Result<ApplyControlChangesResponse> {
        match target {
            ControlApplyTarget::Driver { driver_id, config } => {
                if *driver_id != DESCRIPTOR.id {
                    bail!("pixel controls cannot apply to driver '{driver_id}'");
                }
                let values =
                    controls::pixel_config_values(&config.parse_settings::<PixelConfig>()?);
                control_apply::apply_driver_value_changes(host, DESCRIPTOR.id, values, changes)
                    .await
            }
            ControlApplyTarget::Device { .. } => Err(anyhow!(
                "pixel device fields are read-only; edit the device in the driver's devices list"
            )),
        }
    }
}
