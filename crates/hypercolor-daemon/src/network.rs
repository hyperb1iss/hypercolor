//! Built-in driver module registry and host adapters.

mod host;

use std::collections::BTreeSet;
#[cfg(not(feature = "builtin-drivers"))]
use std::sync::Arc;

use anyhow::{Context, Result};
use hypercolor_core::device::BackendManager;
#[cfg(not(feature = "builtin-drivers"))]
use hypercolor_core::device::UsbProtocolConfigStore;
use hypercolor_driver_api::{DriverConfigView, DriverHost};
#[cfg(not(feature = "builtin-drivers"))]
use hypercolor_driver_support::CredentialStore;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::{DriverConfigEntry, HypercolorConfig};
use hypercolor_types::device::{
    DeviceClassHint, DeviceInfo, DriverModuleDescriptor, DriverModuleKind, DriverPresentation,
    DriverProtocolDescriptor, DriverTransportKind,
};

pub use host::DaemonDriverHost;
#[cfg(feature = "builtin-drivers")]
pub use hypercolor_driver_builtin::build_driver_module_registry as build_builtin_driver_module_registry;
#[cfg(feature = "builtin-drivers")]
pub use hypercolor_driver_builtin::normalize_driver_config_entries as normalize_builtin_driver_config_entries;

#[cfg(not(feature = "builtin-drivers"))]
pub fn build_builtin_driver_module_registry(
    _config: &HypercolorConfig,
    _credential_store: Arc<CredentialStore>,
    _usb_protocol_configs: UsbProtocolConfigStore,
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

/// Whether a driver module exposes at least one transport runnable on this host.
#[must_use]
pub fn module_has_available_transport(descriptor: &DriverModuleDescriptor) -> bool {
    descriptor
        .transports
        .iter()
        .any(hypercolor_types::device::DriverTransportDescriptor::is_available)
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
                .any(|item| item.is_available() && transports.contains(&item.kind))
        })
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

pub fn enabled_driver_module_ids(
    registry: &DriverModuleRegistry,
    config: &HypercolorConfig,
) -> BTreeSet<String> {
    registry
        .ids()
        .into_iter()
        .filter(|driver_id| {
            module_descriptor(registry, driver_id).is_some_and(|descriptor| {
                module_enabled(config, &descriptor) && module_has_available_transport(&descriptor)
            })
        })
        .collect()
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
    let enabled_driver_ids = enabled_driver_module_ids(registry, config);
    let finalized = registry
        .finalize_output_bindings(&enabled_driver_ids)
        .context("failed to finalize driver output bindings")?;

    for provider in finalized.providers() {
        let driver_id = provider.driver_id();
        let config_entry = driver_config_entry(config, driver_id);
        let config_view = DriverConfigView {
            driver_id,
            entry: &config_entry,
        };
        let backend = provider.build(host, config_view)?;
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
) -> Result<()> {
    register_enabled_driver_output_backends(backend_manager, registry, host, config)
        .context("failed to register driver module output backends")
}
