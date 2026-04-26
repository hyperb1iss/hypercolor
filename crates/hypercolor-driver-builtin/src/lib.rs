//! Built-in Hypercolor driver bundle.
//!
//! The daemon loads this crate as one local module bundle, keeping concrete
//! built-in driver implementations out of daemon orchestration code.

use std::sync::Arc;

use anyhow::Result;
use hypercolor_core::device::net::CredentialStore;
use hypercolor_network::DriverRegistry;
use hypercolor_types::config::HypercolorConfig;

#[cfg(feature = "hue")]
use hypercolor_driver_hue::HueDriverFactory;
#[cfg(feature = "nanoleaf")]
use hypercolor_driver_nanoleaf::NanoleafDriverFactory;
use hypercolor_driver_wled::WledDriverFactory;

/// Build the compiled-in driver registry for this process.
///
/// # Errors
///
/// Returns an error if two built-in drivers collide or advertise an unsupported
/// driver API schema version.
pub fn build_driver_registry(
    config: &HypercolorConfig,
    credential_store: Arc<CredentialStore>,
) -> Result<DriverRegistry> {
    let mut registry = DriverRegistry::new();
    register_drivers(&mut registry, config, credential_store)?;
    Ok(registry)
}

/// Register all compiled-in driver modules into an existing registry.
///
/// # Errors
///
/// Returns an error if a built-in driver registration fails.
pub fn register_drivers(
    registry: &mut DriverRegistry,
    config: &HypercolorConfig,
    credential_store: Arc<CredentialStore>,
) -> Result<()> {
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

    Ok(())
}
