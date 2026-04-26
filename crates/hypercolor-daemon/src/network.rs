//! Built-in network driver registry and host adapters.

mod host;

use anyhow::Result;
use hypercolor_core::device::BackendManager;
use hypercolor_driver_api::{DriverConfigView, DriverHost};
use hypercolor_network::DriverRegistry;
use hypercolor_types::config::{DriverConfigEntry, HypercolorConfig};
use hypercolor_types::device::DriverModuleDescriptor;

pub use host::DaemonDriverHost;
pub use hypercolor_driver_builtin::build_driver_registry as build_builtin_driver_registry;

/// Whether a network driver is enabled by the active config.
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

/// Config key responsible for enabling a built-in network driver.
#[must_use]
pub fn driver_config_flag(driver_id: &str) -> String {
    format!("drivers.{driver_id}.enabled")
}

/// Resolve one driver's config entry, falling back to an empty default entry.
#[must_use]
pub fn driver_config_entry(config: &HypercolorConfig, driver_id: &str) -> DriverConfigEntry {
    config.drivers.get(driver_id).cloned().unwrap_or_default()
}

/// Register all enabled network backends with the backend manager.
///
/// # Errors
///
/// Returns an error if backend construction fails.
pub fn register_enabled_backends(
    backend_manager: &mut BackendManager,
    registry: &DriverRegistry,
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
        let Some(backend) = driver.build_backend(host, config_view)? else {
            continue;
        };
        backend_manager.register_backend(backend);
    }

    Ok(())
}
