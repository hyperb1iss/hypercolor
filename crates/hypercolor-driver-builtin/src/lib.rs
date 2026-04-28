//! Built-in Hypercolor driver bundle.
//!
//! The daemon loads this crate as one local module bundle, keeping concrete
//! built-in driver implementations out of daemon orchestration code.

use std::sync::Arc;

use anyhow::Result;
use hypercolor_driver_api::CredentialStore;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::{GoveeConfig, HypercolorConfig};

#[cfg(feature = "govee")]
use hypercolor_driver_govee::GoveeDriverModule;
#[cfg(feature = "hue")]
use hypercolor_driver_hue::HueDriverModule;
#[cfg(feature = "nanoleaf")]
use hypercolor_driver_nanoleaf::NanoleafDriverModule;
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

    Ok(())
}
