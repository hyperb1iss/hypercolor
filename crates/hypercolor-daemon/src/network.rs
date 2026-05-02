//! Built-in driver module registry and host adapters.

mod host;

use std::collections::BTreeSet;
#[cfg(not(feature = "builtin-drivers"))]
use std::sync::Arc;

use anyhow::{Context, Result};
use hypercolor_core::device::{
    BackendManager, BlocksBackend, BlocksScanner, SmBusBackend, SmBusScanner, TransportScanner,
    UsbBackend, UsbProtocolConfigStore, UsbScanner,
};
#[cfg(not(feature = "builtin-drivers"))]
use hypercolor_driver_api::CredentialStore;
use hypercolor_driver_api::{DriverConfigView, DriverHost};
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::{DriverConfigEntry, HypercolorConfig};
use hypercolor_types::device::{
    BLOCKS_OUTPUT_BACKEND_ID, DeviceClassHint, DeviceInfo, DriverModuleDescriptor,
    DriverModuleKind, DriverPresentation, DriverProtocolDescriptor, DriverTransportKind,
    SMBUS_OUTPUT_BACKEND_ID, USB_OUTPUT_BACKEND_ID,
};

pub use host::DaemonDriverHost;
#[cfg(feature = "builtin-drivers")]
pub use hypercolor_driver_builtin::build_driver_module_registry as build_builtin_driver_module_registry;
#[cfg(feature = "builtin-drivers")]
pub use hypercolor_driver_builtin::normalize_driver_config_entries as normalize_builtin_driver_config_entries;

pub const USB_HOST_TRANSPORT_TARGET_ID: &str = USB_OUTPUT_BACKEND_ID;
pub const SMBUS_HOST_TRANSPORT_TARGET_ID: &str = SMBUS_OUTPUT_BACKEND_ID;
pub const BLOCKS_HOST_TRANSPORT_TARGET_ID: &str = BLOCKS_OUTPUT_BACKEND_ID;
pub const HOST_TRANSPORT_TARGET_IDS: &[&str] = &[
    USB_HOST_TRANSPORT_TARGET_ID,
    SMBUS_HOST_TRANSPORT_TARGET_ID,
    BLOCKS_HOST_TRANSPORT_TARGET_ID,
];
pub const USB_HOST_DRIVER_TRANSPORTS: &[DriverTransportKind] = &[
    DriverTransportKind::Usb,
    DriverTransportKind::Midi,
    DriverTransportKind::Serial,
];
pub const SMBUS_HOST_DRIVER_TRANSPORTS: &[DriverTransportKind] = &[DriverTransportKind::Smbus];

#[cfg(not(feature = "builtin-drivers"))]
pub fn build_builtin_driver_module_registry(
    _config: &HypercolorConfig,
    _credential_store: Arc<CredentialStore>,
) -> Result<DriverModuleRegistry> {
    Ok(DriverModuleRegistry::new())
}

#[cfg(not(feature = "builtin-drivers"))]
pub fn normalize_builtin_driver_config_entries(_config: &mut HypercolorConfig) {}

/// Whether a driver is enabled by the active config.
#[must_use]
pub fn driver_enabled(config: &HypercolorConfig, driver_id: &str) -> bool {
    driver_enabled_with_default(config, driver_id, true)
}

/// Whether a driver is enabled after applying the module default.
#[must_use]
pub fn driver_enabled_with_default(
    config: &HypercolorConfig,
    driver_id: &str,
    default_enabled: bool,
) -> bool {
    config
        .drivers
        .get(driver_id)
        .map_or(default_enabled, |entry| entry.enabled)
}

/// Whether a driver module descriptor is enabled by the active config.
#[must_use]
pub fn module_enabled(config: &HypercolorConfig, descriptor: &DriverModuleDescriptor) -> bool {
    driver_enabled_with_default(config, &descriptor.id, descriptor.default_enabled)
}

/// Whether one registered driver module is enabled by the active config.
#[must_use]
pub fn module_enabled_by_id(
    registry: &DriverModuleRegistry,
    config: &HypercolorConfig,
    driver_id: &str,
) -> bool {
    module_descriptor(registry, driver_id)
        .is_some_and(|descriptor| module_enabled(config, &descriptor))
}

/// Module descriptors for one driver module kind.
#[must_use]
pub fn module_descriptors_for_kind(
    registry: &DriverModuleRegistry,
    module_kind: DriverModuleKind,
) -> Vec<DriverModuleDescriptor> {
    registry
        .module_descriptors()
        .into_iter()
        .filter(|descriptor| descriptor.module_kind == module_kind)
        .collect()
}

/// Module descriptors for all driver modules known by this daemon.
#[must_use]
pub fn module_descriptors(registry: &DriverModuleRegistry) -> Vec<DriverModuleDescriptor> {
    let mut descriptors = registry
        .module_descriptors()
        .into_iter()
        .collect::<Vec<_>>();
    descriptors.sort_by(|left, right| left.id.cmp(&right.id));
    descriptors
}

/// Module descriptor for one known driver module.
#[must_use]
pub fn module_descriptor(
    registry: &DriverModuleRegistry,
    driver_id: &str,
) -> Option<DriverModuleDescriptor> {
    registry
        .get(driver_id)
        .map(|driver| driver.module_descriptor())
}

/// Presentation metadata derived from a driver module descriptor.
#[must_use]
pub fn descriptor_presentation(descriptor: &DriverModuleDescriptor) -> DriverPresentation {
    DriverPresentation {
        label: descriptor.display_name.clone(),
        short_label: None,
        accent_rgb: None,
        secondary_rgb: None,
        icon: None,
        default_device_class: None,
    }
}

/// Presentation metadata for a known driver module.
#[must_use]
pub fn module_presentation(
    registry: &DriverModuleRegistry,
    driver_id: &str,
) -> Option<DriverPresentation> {
    if let Some(driver) = registry.get(driver_id)
        && let Some(provider) = driver.presentation()
    {
        return Some(provider.presentation());
    }

    module_descriptor(registry, driver_id).map(|descriptor| descriptor_presentation(&descriptor))
}

/// Presentation metadata for a concrete device, with a local fallback.
#[must_use]
pub fn device_presentation(
    registry: &DriverModuleRegistry,
    device: &DeviceInfo,
) -> DriverPresentation {
    module_presentation(registry, device.driver_id()).unwrap_or_else(|| DriverPresentation {
        label: device.family.to_string(),
        short_label: None,
        accent_rgb: None,
        secondary_rgb: None,
        icon: None,
        default_device_class: device
            .capabilities
            .has_display
            .then_some(DeviceClassHint::Display),
    })
}

/// Protocol descriptors for one driver module.
#[must_use]
pub fn protocol_descriptors(
    registry: &DriverModuleRegistry,
    driver_id: &str,
) -> Vec<DriverProtocolDescriptor> {
    if let Some(driver) = registry.get(driver_id)
        && let Some(catalog) = driver.protocol_catalog()
    {
        let mut descriptors = catalog.descriptors().to_vec();
        descriptors.sort_by(|left, right| left.protocol_id.cmp(&right.protocol_id));
        return descriptors;
    }

    Vec::new()
}

/// Enabled driver module IDs for one module kind.
#[must_use]
pub fn enabled_module_ids(
    registry: &DriverModuleRegistry,
    config: &HypercolorConfig,
    module_kind: DriverModuleKind,
) -> BTreeSet<String> {
    module_descriptors_for_kind(registry, module_kind)
        .iter()
        .filter(|descriptor| module_enabled(config, descriptor))
        .map(|descriptor| descriptor.id.clone())
        .collect()
}

/// Enabled driver module IDs for one module kind and transport category.
#[must_use]
pub fn enabled_module_ids_for_transport(
    registry: &DriverModuleRegistry,
    config: &HypercolorConfig,
    module_kind: DriverModuleKind,
    transport: &DriverTransportKind,
) -> BTreeSet<String> {
    enabled_module_ids_for_transports(
        registry,
        config,
        module_kind,
        std::slice::from_ref(transport),
    )
}

/// Enabled driver module IDs for one module kind and any matching transport category.
#[must_use]
pub fn enabled_module_ids_for_transports(
    registry: &DriverModuleRegistry,
    config: &HypercolorConfig,
    module_kind: DriverModuleKind,
    transports: &[DriverTransportKind],
) -> BTreeSet<String> {
    module_descriptors_for_kind(registry, module_kind)
        .iter()
        .filter(|descriptor| {
            descriptor
                .transports
                .iter()
                .any(|item| transports.iter().any(|transport| item == transport))
        })
        .filter(|descriptor| module_enabled(config, descriptor))
        .map(|descriptor| descriptor.id.clone())
        .collect()
}

/// Host-owned discovery target that services one driver transport category.
#[must_use]
pub const fn host_transport_target_for_driver_transport(
    transport: &DriverTransportKind,
) -> Option<&'static str> {
    match transport {
        DriverTransportKind::Usb | DriverTransportKind::Midi | DriverTransportKind::Serial => {
            Some(USB_HOST_TRANSPORT_TARGET_ID)
        }
        DriverTransportKind::Smbus => Some(SMBUS_HOST_TRANSPORT_TARGET_ID),
        DriverTransportKind::Bridge => Some(BLOCKS_HOST_TRANSPORT_TARGET_ID),
        DriverTransportKind::Network
        | DriverTransportKind::Virtual
        | DriverTransportKind::Custom(_) => None,
    }
}

/// Config key responsible for enabling a driver module.
#[must_use]
pub fn driver_config_flag(driver_id: &str) -> String {
    format!("drivers.{driver_id}.enabled")
}

/// Resolve one driver's config entry, falling back to an empty default entry.
#[must_use]
pub fn driver_config_entry(config: &HypercolorConfig, driver_id: &str) -> DriverConfigEntry {
    config.drivers.get(driver_id).cloned().unwrap_or_default()
}

/// Register all enabled driver output backends with the backend manager.
///
/// # Errors
///
/// Returns an error if backend construction fails.
pub fn register_enabled_driver_output_backends(
    backend_manager: &mut BackendManager,
    registry: &DriverModuleRegistry,
    host: &dyn DriverHost,
    config: &HypercolorConfig,
) -> Result<()> {
    for driver_id in registry.ids() {
        let Some(driver) = registry.get(&driver_id) else {
            continue;
        };

        let descriptor = driver.module_descriptor();
        if !module_enabled(config, &descriptor) {
            continue;
        }
        if !descriptor.capabilities.output_backend {
            continue;
        }

        let config_entry = driver_config_entry(config, &driver_id);
        let config_view = DriverConfigView {
            driver_id: &driver_id,
            entry: &config_entry,
        };
        let Some(backend) = driver.build_output_backend(host, config_view)? else {
            continue;
        };
        backend_manager.register_backend(backend);
    }

    Ok(())
}

/// Register every enabled physical/output backend behind the driver boundary.
///
/// # Errors
///
/// Returns an error if a driver module backend cannot be constructed.
pub fn register_enabled_device_backends(
    backend_manager: &mut BackendManager,
    registry: &DriverModuleRegistry,
    host: &dyn DriverHost,
    config: &HypercolorConfig,
    usb_protocol_configs: UsbProtocolConfigStore,
) -> Result<()> {
    register_enabled_driver_output_backends(backend_manager, registry, host, config)
        .context("failed to register driver module output backends")?;

    if config.discovery.blocks_scan {
        let socket_path = config
            .discovery
            .blocks_socket_path
            .as_ref()
            .map_or_else(BlocksBackend::default_socket_path, std::path::PathBuf::from);
        backend_manager.register_backend(Box::new(BlocksBackend::new(socket_path)));
    }

    if !enabled_module_ids_for_transports(
        registry,
        config,
        DriverModuleKind::Hal,
        SMBUS_HOST_DRIVER_TRANSPORTS,
    )
    .is_empty()
    {
        backend_manager.register_backend(Box::new(SmBusBackend::new()));
    }

    let usb_driver_ids = enabled_module_ids_for_transports(
        registry,
        config,
        DriverModuleKind::Hal,
        USB_HOST_DRIVER_TRANSPORTS,
    );
    if !usb_driver_ids.is_empty() {
        backend_manager.register_backend(Box::new(
            UsbBackend::with_protocol_config_store_and_enabled_driver_ids(
                usb_protocol_configs,
                usb_driver_ids,
            ),
        ));
    }

    Ok(())
}

/// Build one host-owned transport scanner by public discovery target id.
#[must_use]
pub fn host_transport_scanner(
    target_id: &str,
    registry: &DriverModuleRegistry,
    config: &HypercolorConfig,
) -> Option<Box<dyn TransportScanner>> {
    match target_id {
        USB_HOST_TRANSPORT_TARGET_ID => Some(Box::new(UsbScanner::with_enabled_driver_ids(
            enabled_module_ids_for_transports(
                registry,
                config,
                DriverModuleKind::Hal,
                USB_HOST_DRIVER_TRANSPORTS,
            ),
        ))),
        SMBUS_HOST_TRANSPORT_TARGET_ID => Some(Box::new(SmBusScanner::new())),
        BLOCKS_HOST_TRANSPORT_TARGET_ID => {
            let socket_path = config
                .discovery
                .blocks_socket_path
                .as_ref()
                .map_or_else(BlocksBackend::default_socket_path, std::path::PathBuf::from);
            Some(Box::new(BlocksScanner::new(socket_path)))
        }
        _ => None,
    }
}

#[must_use]
pub fn is_host_transport_target(target_id: &str) -> bool {
    HOST_TRANSPORT_TARGET_IDS.contains(&target_id)
}
