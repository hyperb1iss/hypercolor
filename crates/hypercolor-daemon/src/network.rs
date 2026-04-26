//! Built-in network driver registry and host adapters.

mod host;

use std::sync::Arc;

use anyhow::Result;
use hypercolor_core::device::BackendManager;
use hypercolor_core::device::net::CredentialStore;
use hypercolor_driver_api::{DriverConfigView, DriverHost};
#[cfg(feature = "hue")]
use hypercolor_driver_hue::HueDriverFactory;
#[cfg(feature = "nanoleaf")]
use hypercolor_driver_nanoleaf::NanoleafDriverFactory;
use hypercolor_driver_wled::WledDriverFactory;
use hypercolor_network::DriverRegistry;
use hypercolor_types::config::HypercolorConfig;

pub use host::DaemonDriverHost;
#[cfg(feature = "hue")]
pub use hypercolor_driver_hue::resolve_hue_probe_bridges_from_sources;
#[cfg(feature = "nanoleaf")]
pub use hypercolor_driver_nanoleaf::resolve_nanoleaf_probe_devices_from_sources;
pub use hypercolor_driver_wled::{
    resolve_wled_probe_ips_from_sources, resolve_wled_probe_targets_from_sources,
};

/// Build the daemon's compiled-in network driver registry.
///
/// # Errors
///
/// Returns an error if a built-in driver registration collides.
pub fn build_builtin_driver_registry(
    config: &HypercolorConfig,
    credential_store: Arc<CredentialStore>,
) -> Result<DriverRegistry> {
    let mut registry = DriverRegistry::new();
    registry.register(WledDriverFactory::new(config.discovery.mdns_enabled))?;
    #[cfg(not(any(feature = "hue", feature = "nanoleaf")))]
    let _ = &credential_store;

    #[cfg(feature = "hue")]
    registry.register(HueDriverFactory::new(
        Arc::clone(&credential_store),
        config.discovery.mdns_enabled,
    ))?;

    #[cfg(feature = "nanoleaf")]
    registry.register(NanoleafDriverFactory::new(
        credential_store,
        config.discovery.mdns_enabled,
    ))?;

    Ok(registry)
}

/// Whether a built-in network driver is enabled by the active config.
#[must_use]
pub fn driver_enabled(config: &HypercolorConfig, driver_id: &str) -> bool {
    config
        .drivers
        .get(driver_id)
        .map_or(true, |entry| entry.enabled)
}

/// Config key responsible for enabling a built-in network driver.
#[must_use]
pub fn driver_config_flag(driver_id: &str) -> String {
    format!("drivers.{driver_id}.enabled")
}

/// Resolve the active config entry for a driver.
#[must_use]
pub fn driver_config_view<'a>(
    config: &'a HypercolorConfig,
    driver_id: &'a str,
) -> Option<DriverConfigView<'a>> {
    config
        .drivers
        .get(driver_id)
        .map(|entry| DriverConfigView { driver_id, entry })
}

/// Register all enabled built-in network backends with the backend manager.
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
        if !driver_enabled(config, &driver_id) {
            continue;
        }

        let Some(driver) = registry.get(&driver_id) else {
            continue;
        };
        let Some(config_view) = driver_config_view(config, &driver_id) else {
            continue;
        };
        let Some(backend) = driver.build_backend(host, config_view)? else {
            continue;
        };
        backend_manager.register_backend(backend);
    }

    Ok(())
}
