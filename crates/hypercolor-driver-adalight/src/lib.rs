//! Adalight serial LED controller driver for Hypercolor (Spec 91).
//!
//! Drives DIY strips behind Arduino, ESP8266, ESP32, and RP2040 boards that
//! run Adalight-compatible firmware: the Adalight sketches (`"Ada"`
//! framing) and HyperSerial (`"Awa"` framing with a Fletcher trailer).
//!
//! These boards sit behind generic USB-serial chips (CH340, CP210x, FTDI,
//! Arduino and Espressif CDC) that also appear in unrelated hardware, so
//! the driver never claims a device by USB id and never enumerates ports.
//! Every device comes from the driver config; "discovery" materializes
//! those entries without touching a port, and the backend opens only the
//! port a config entry names.

pub mod backend;
pub mod color;
pub mod config;
pub mod controls;
pub mod drain;
pub mod plan;
pub mod protocol;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use hypercolor_driver_api::{
    ControlApplyTarget, DeviceBackend, DeviceBackendFactory, DiscoveredDevice, DiscoveryCapability,
    DiscoveryConnectBehavior, DiscoveryRequest, DriverConfigProvider, DriverConfigSource,
    DriverConfigView, DriverControlProvider, DriverDescriptor, DriverError, DriverHost,
    DriverModule, DriverPresentationProvider, OutputBinding, TrackedDeviceCtx,
    ValidatedControlChanges,
};
use hypercolor_driver_support::control_apply;
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::controls::{
    ApplyControlChangesResponse, ControlChange, ControlSurfaceDocument,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceClassHint, DeviceColorFormat, DeviceColorSpace,
    DeviceFamily, DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin,
    DeviceTopologyHint, DriverPresentation, DriverTransportKind, FingerprintNamespace, SegmentInfo,
};
use hypercolor_types::identity::BackendId;

pub use backend::{
    AdalightBackend, DISCONNECT_BUDGET, KEEPALIVE_INTERVAL, METADATA_DEVICE_CONFIG, SnapshotConfig,
    output_cadence_for, stall_budget,
};
pub use color::ColorOrder;
pub use config::{AdalightConfig, AdalightConfigError, AdalightDeviceConfig, WhiteMode};
pub use controls::{adalight_device_control_surface, adalight_driver_control_surface};
pub use drain::{DrainVerdict, DrainWatch};
pub use plan::{AdalightDevicePlan, MAX_BLACKOUT_FRAMES, resolve_config, resolve_device};
pub use protocol::{AdalightProtocol, FrameEncoder, FrameLayout, Framing, WhiteCalibration};

/// Static driver descriptor.
pub static DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "adalight",
    "Adalight Serial",
    DriverTransportKind::Serial,
    true,
    false,
);

/// Adalight serial driver module.
#[derive(Debug, Clone, Default)]
pub struct AdalightDriverModule;

impl AdalightDriverModule {
    /// Create the driver module.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl DriverModule for AdalightDriverModule {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &DESCRIPTOR
    }

    fn output(&self) -> OutputBinding<'_> {
        OutputBinding::Owned {
            id: BackendId::new(DESCRIPTOR.id).expect("adalight backend ID must be valid"),
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

impl DeviceBackendFactory for AdalightDriverModule {
    fn build(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> std::result::Result<Arc<dyn DeviceBackend>, DriverError> {
        // Every adoption and open is checked against the live config, so a
        // removed entry can never be reopened from retained metadata. Hosts
        // without a live config store fall back to the entry the backend is
        // built from; the reconciler rebuilds it whenever that entry
        // changes.
        let source = host.config_source().unwrap_or_else(|| {
            Arc::new(SnapshotConfig::new(config.entry.clone())) as Arc<dyn DriverConfigSource>
        });
        Ok(Arc::new(AdalightBackend::new(source)))
    }
}

impl DriverPresentationProvider for AdalightDriverModule {
    fn presentation(&self) -> DriverPresentation {
        DriverPresentation {
            label: "Adalight Serial".to_owned(),
            short_label: Some("Adalight".to_owned()),
            accent_rgb: Some([225, 53, 255]),
            secondary_rgb: Some([128, 255, 234]),
            icon: Some("cable".to_owned()),
            default_device_class: Some(DeviceClassHint::Controller),
        }
    }
}

#[async_trait]
impl DiscoveryCapability for AdalightDriverModule {
    async fn discover(
        &self,
        _host: &dyn DriverHost,
        _request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> std::result::Result<Vec<DiscoveredDevice>, DriverError> {
        let config = config.parse_settings::<AdalightConfig>()?;
        configured_devices(&config).map_err(|error| DriverError::Configuration {
            message: error.to_string(),
        })
    }
}

/// Materialize every configured device as a discovery result.
///
/// Opens no port and sends nothing: serial controllers are configured, not
/// probed.
///
/// # Errors
///
/// Returns the first config validation failure.
pub fn configured_devices(
    config: &AdalightConfig,
) -> std::result::Result<Vec<DiscoveredDevice>, AdalightConfigError> {
    let plans = resolve_config(config)?;
    Ok(config
        .devices
        .iter()
        .zip(plans)
        .map(|(device, plan)| discovered_device(device, &plan))
        .collect())
}

/// Fingerprint of the device a config entry describes.
///
/// Identity comes from the user's config id, never from the port or the
/// USB-serial chip behind it.
#[must_use]
pub fn device_fingerprint(config_id: &str) -> DeviceFingerprint {
    DeviceFingerprint::mint(FingerprintNamespace::Usb, DESCRIPTOR.id, config_id)
}

/// Device id of the device a config entry describes.
#[must_use]
pub fn device_id_for(config_id: &str) -> DeviceId {
    device_fingerprint(config_id).stable_device_id()
}

fn discovered_device(device: &AdalightDeviceConfig, plan: &AdalightDevicePlan) -> DiscoveredDevice {
    let fingerprint = device_fingerprint(&plan.id);
    let color_format = if plan.white.has_white() {
        DeviceColorFormat::Rgbw
    } else {
        DeviceColorFormat::Rgb
    };
    let info = DeviceInfo {
        id: fingerprint.stable_device_id(),
        name: plan.name.clone(),
        vendor: "Adalight".to_owned(),
        family: DeviceFamily::new_static("adalight", "Adalight"),
        model: Some(plan.protocol().label().to_owned()),
        // Nearly every Adalight board is a USB-serial device; the transport
        // category below is what records that it is driven as a serial port.
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::new(DESCRIPTOR.id, DESCRIPTOR.id, DriverTransportKind::Serial),
        segments: vec![SegmentInfo {
            name: "Strip".to_owned(),
            led_count: plan.led_count(),
            topology: DeviceTopologyHint::Strip,
            color_format,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: plan.led_count(),
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: plan.effective_max_fps(),
            color_space: DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };

    let metadata = HashMap::from([
        ("port".to_owned(), plan.port.clone()),
        ("baud_rate".to_owned(), plan.baud_rate.to_string()),
        ("protocol".to_owned(), plan.protocol().as_str().to_owned()),
        ("frame_bytes".to_owned(), plan.frame_bytes().to_string()),
        ("line_max_fps".to_owned(), plan.line_max_fps().to_string()),
        (
            METADATA_DEVICE_CONFIG.to_owned(),
            serde_json::to_string(device).unwrap_or_default(),
        ),
    ]);

    DiscoveredDevice {
        fingerprint,
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        info,
        metadata,
        // Deliberate refusal: nothing here came from the hardware.
        claim: None,
    }
}

impl DriverConfigProvider for AdalightDriverModule {
    fn default_config(&self) -> DriverConfigEntry {
        DriverConfigEntry::enabled(BTreeMap::from([(
            "devices".to_owned(),
            serde_json::json!([]),
        )]))
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
impl DriverControlProvider for AdalightDriverModule {
    async fn driver_surface(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(adalight_driver_control_surface(
            &config.parse_settings::<AdalightConfig>()?,
        )))
    }

    async fn device_surface(
        &self,
        _host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Result<Option<ControlSurfaceDocument>> {
        Ok(Some(adalight_device_control_surface(device)))
    }

    async fn validate_changes(
        &self,
        _host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> Result<ValidatedControlChanges> {
        controls::validate_adalight_changes(target, changes)
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
                    bail!("adalight controls cannot apply to driver '{driver_id}'");
                }
                let values =
                    controls::adalight_config_values(&config.parse_settings::<AdalightConfig>()?);
                control_apply::apply_driver_value_changes(host, DESCRIPTOR.id, values, changes)
                    .await
            }
            ControlApplyTarget::Device { .. } => Err(anyhow!(
                "serial device fields are read-only; edit the device in the driver's devices list"
            )),
        }
    }
}
