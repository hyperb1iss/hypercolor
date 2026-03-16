//! Built-in network driver registry and host adapters.

mod host;

use std::sync::Arc;

use anyhow::Result;
use hypercolor_core::device::BackendManager;
use hypercolor_core::device::net::CredentialStore;
use hypercolor_driver_api::DriverHost;
#[cfg(feature = "hue")]
use hypercolor_driver_hue::HueDriverFactory;
#[cfg(feature = "nanoleaf")]
use hypercolor_driver_nanoleaf::NanoleafDriverFactory;
use hypercolor_driver_wled::WledDriverFactory;
use hypercolor_network::DriverRegistry;
use hypercolor_types::config::HypercolorConfig;

pub use host::DaemonDriverHost;
#[cfg(feature = "hue")]
pub use hypercolor_driver_hue::pair_hue_bridge_at_ip;
#[cfg(feature = "hue")]
pub use hypercolor_driver_hue::resolve_hue_probe_bridges_from_sources;
#[cfg(feature = "nanoleaf")]
pub use hypercolor_driver_nanoleaf::pair_nanoleaf_device_at_ip;
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
    registry.register(WledDriverFactory::new(config.clone()))?;
    #[cfg(not(any(feature = "hue", feature = "nanoleaf")))]
    let _ = &credential_store;

    #[cfg(feature = "hue")]
    registry.register(HueDriverFactory::new(
        Arc::clone(&credential_store),
        config.hue.clone(),
        config.discovery.mdns_enabled,
    ))?;

    #[cfg(feature = "nanoleaf")]
    registry.register(NanoleafDriverFactory::new(
        credential_store,
        config.nanoleaf.clone(),
        config.discovery.mdns_enabled,
    ))?;

    Ok(registry)
}

/// Whether a built-in network driver is enabled by the active config.
#[must_use]
pub fn driver_enabled(config: &HypercolorConfig, driver_id: &str) -> bool {
    match driver_id {
        "wled" => config.discovery.wled_scan,
        "hue" => config.discovery.hue_scan,
        "nanoleaf" => config.discovery.nanoleaf_scan,
        _ => true,
    }
}

/// Config key responsible for enabling a built-in network driver.
#[must_use]
pub fn driver_config_flag(driver_id: &str) -> Option<&'static str> {
    match driver_id {
        "wled" => Some("discovery.wled_scan"),
        "hue" => Some("discovery.hue_scan"),
        "nanoleaf" => Some("discovery.nanoleaf_scan"),
        _ => None,
    }
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
        let Some(backend) = driver.build_backend(host)? else {
            continue;
        };
        backend_manager.register_backend(backend);
    }

    Ok(())
}
