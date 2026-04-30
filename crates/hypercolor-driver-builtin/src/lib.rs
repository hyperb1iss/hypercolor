//! Built-in Hypercolor driver bundle.
//!
//! The daemon loads this crate as one local module bundle, keeping concrete
//! built-in driver implementations out of daemon orchestration code.

#[cfg(feature = "hal")]
mod hal;

use std::sync::Arc;

use anyhow::Result;
use hypercolor_driver_api::CredentialStore;
use hypercolor_network::DriverModuleRegistry;
#[cfg(feature = "govee")]
use hypercolor_types::config::GoveeConfig;
use hypercolor_types::config::HypercolorConfig;

#[cfg(feature = "govee")]
use hypercolor_driver_govee::GoveeDriverModule;
#[cfg(feature = "hue")]
use hypercolor_driver_hue::HueDriverModule;
#[cfg(feature = "nanoleaf")]
use hypercolor_driver_nanoleaf::NanoleafDriverModule;
#[cfg(feature = "wled")]
use hypercolor_driver_wled::WledDriverModule;

/// Build the compiled-in driver module registry for this process.
///
/// # Errors
///
/// Returns an error if two built-in drivers collide or advertise an unsupported
/// driver API schema version.
pub fn build_driver_module_registry(
    config: &HypercolorConfig,
    credential_store: Arc<CredentialStore>,
) -> Result<DriverModuleRegistry> {
    let mut registry = DriverModuleRegistry::new();
    register_driver_modules(&mut registry, config, credential_store)?;
    Ok(registry)
}

/// Register all compiled-in driver modules into an existing registry.
///
/// # Errors
///
/// Returns an error if a built-in driver registration fails.
pub fn register_driver_modules(
    registry: &mut DriverModuleRegistry,
    config: &HypercolorConfig,
    credential_store: Arc<CredentialStore>,
) -> Result<()> {
    #[cfg(not(any(
        feature = "wled",
        feature = "govee",
        feature = "hue",
        feature = "nanoleaf",
        feature = "hal"
    )))]
    let _ = registry;
    #[cfg(not(any(feature = "wled", feature = "hue", feature = "nanoleaf")))]
    let _ = config;

    #[cfg(feature = "wled")]
    registry.register(WledDriverModule::new(config.discovery.mdns_enabled))?;
    #[cfg(not(any(feature = "govee", feature = "hue", feature = "nanoleaf")))]
    let _ = &credential_store;

    #[cfg(feature = "govee")]
    registry.register(GoveeDriverModule::with_credential_store(
        GoveeConfig::default(),
        Arc::clone(&credential_store),
    ))?;

    #[cfg(feature = "hue")]
    registry.register(HueDriverModule::new(
        Arc::clone(&credential_store),
        config.discovery.mdns_enabled,
    ))?;

    #[cfg(feature = "nanoleaf")]
    registry.register(NanoleafDriverModule::new(
        credential_store,
        config.discovery.mdns_enabled,
    ))?;

    #[cfg(feature = "hal")]
    {
        for driver in hal::hal_catalog_driver_modules() {
            registry.register(driver)?;
        }
    }

    Ok(())
}

/// Ensure config entries exist for compiled-in driver modules with dynamic catalogs.
pub fn normalize_driver_config_entries(config: &mut HypercolorConfig) {
    #[cfg(not(any(
        feature = "wled",
        feature = "govee",
        feature = "hue",
        feature = "nanoleaf",
        feature = "hal"
    )))]
    let _ = config;

    #[cfg(feature = "wled")]
    config
        .drivers
        .entry(hypercolor_driver_wled::DESCRIPTOR.id.to_owned())
        .or_default();

    #[cfg(feature = "govee")]
    config
        .drivers
        .entry(hypercolor_driver_govee::DESCRIPTOR.id.to_owned())
        .or_default();

    #[cfg(feature = "hue")]
    config
        .drivers
        .entry(hypercolor_driver_hue::DESCRIPTOR.id.to_owned())
        .or_default();

    #[cfg(feature = "nanoleaf")]
    config
        .drivers
        .entry(hypercolor_driver_nanoleaf::DESCRIPTOR.id.to_owned())
        .or_default();

    #[cfg(feature = "hal")]
    for descriptor in hal::hal_module_descriptors() {
        config.drivers.entry(descriptor.id.clone()).or_default();
    }
}
