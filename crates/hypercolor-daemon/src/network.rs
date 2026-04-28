//! Built-in driver module registry and host adapters.

mod host;

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use hypercolor_core::device::{
    BackendManager, BlocksBackend, SmBusBackend, UsbBackend, UsbProtocolConfigStore,
};
use hypercolor_driver_api::{DriverConfigView, DriverHost};
use hypercolor_hal::ProtocolDatabase;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::{DriverConfigEntry, HypercolorConfig};
use hypercolor_types::device::{
    DeviceClassHint, DeviceInfo, DriverModuleDescriptor, DriverPresentation,
    DriverProtocolDescriptor, DriverTransportKind,
};

pub use host::DaemonDriverHost;
pub use hypercolor_driver_builtin::build_driver_module_registry as build_builtin_driver_module_registry;

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

/// Whether a HAL driver module is enabled by the active config.
#[must_use]
pub fn hal_driver_enabled(config: &HypercolorConfig, driver_id: &str) -> bool {
    hal_module_descriptors()
        .iter()
        .find(|descriptor| descriptor.id == driver_id)
        .is_some_and(|descriptor| module_enabled(config, descriptor))
}

/// Module descriptors for HAL-backed driver modules.
#[must_use]
pub fn hal_module_descriptors() -> &'static [DriverModuleDescriptor] {
    ProtocolDatabase::module_descriptors()
}

/// Module descriptors for all driver modules known by this daemon.
#[must_use]
pub fn module_descriptors(registry: &DriverModuleRegistry) -> Vec<DriverModuleDescriptor> {
    let mut descriptors = registry
        .module_descriptors()
        .into_iter()
        .collect::<Vec<_>>();
    descriptors.extend(hal_module_descriptors().iter().cloned());
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
        .or_else(|| {
            hal_module_descriptors()
                .iter()
                .find(|descriptor| descriptor.id == driver_id)
                .cloned()
        })
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
pub fn protocol_descriptors(driver_id: &str) -> Vec<DriverProtocolDescriptor> {
    ProtocolDatabase::protocol_descriptors_for_driver(driver_id)
}

/// Ensure config entries exist for HAL-backed driver modules.
pub fn normalize_hal_driver_config_entries(config: &mut HypercolorConfig) {
    for descriptor in hal_module_descriptors() {
        config.drivers.entry(descriptor.id.clone()).or_default();
    }
}

/// Enabled HAL driver module IDs from the shared protocol catalog.
#[must_use]
pub fn enabled_hal_driver_ids(config: &HypercolorConfig) -> BTreeSet<String> {
    hal_module_descriptors()
        .iter()
        .filter(|descriptor| module_enabled(config, descriptor))
        .map(|descriptor| descriptor.id.clone())
        .collect()
}

/// Enabled HAL driver module IDs that advertise one transport category.
#[must_use]
pub fn enabled_hal_driver_ids_for_transport(
    config: &HypercolorConfig,
    transport: &DriverTransportKind,
) -> BTreeSet<String> {
    hal_module_descriptors()
        .iter()
        .filter(|descriptor| descriptor.transports.iter().any(|item| item == transport))
        .filter(|descriptor| module_enabled(config, descriptor))
        .map(|descriptor| descriptor.id.clone())
        .collect()
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

    if !enabled_hal_driver_ids_for_transport(config, &DriverTransportKind::Smbus).is_empty() {
        backend_manager.register_backend(Box::new(SmBusBackend::new()));
    }

    backend_manager.register_backend(Box::new(
        UsbBackend::with_protocol_config_store_and_enabled_driver_ids(
            usb_protocol_configs,
            enabled_hal_driver_ids(config),
        ),
    ));

    Ok(())
}
