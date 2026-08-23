//! Built-in Hypercolor driver bundle.
//!
//! The daemon loads this crate as one local module bundle, keeping concrete
//! built-in driver implementations out of daemon orchestration code.

#[cfg(feature = "hal")]
mod hal;
#[cfg(feature = "hal")]
mod transport;

use std::sync::Arc;

use anyhow::Result;
#[cfg(feature = "hal")]
use hypercolor_core::device::UsbProtocolConfigStore;
use hypercolor_driver_support::CredentialStore;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::HypercolorConfig;

#[cfg(feature = "govee")]
use hypercolor_driver_govee::GoveeDriverModule;
#[cfg(feature = "hue")]
use hypercolor_driver_hue::HueDriverModule;
#[cfg(feature = "nanoleaf")]
use hypercolor_driver_nanoleaf::NanoleafDriverModule;
#[cfg(feature = "openrgb")]
use hypercolor_driver_openrgb::OpenRgbDriverModule;
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
    #[cfg(feature = "hal")] usb_protocol_configs: UsbProtocolConfigStore,
) -> Result<DriverModuleRegistry> {
    let mut registry = DriverModuleRegistry::new();
    register_driver_modules(
        &mut registry,
        config,
        credential_store,
        #[cfg(feature = "hal")]
        usb_protocol_configs,
    )?;
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
    #[cfg(feature = "hal")] usb_protocol_configs: UsbProtocolConfigStore,
) -> Result<()> {
    #[cfg(not(any(
        feature = "wled",
        feature = "govee",
        feature = "hue",
        feature = "nanoleaf",
        feature = "openrgb",
        feature = "hal"
    )))]
    let _ = registry;
    #[cfg(not(all(feature = "hal", unix)))]
    let _ = config;

    #[cfg(feature = "wled")]
    registry.register(WledDriverModule::new())?;
    #[cfg(not(any(feature = "govee", feature = "hue", feature = "nanoleaf")))]
    let _ = &credential_store;

    #[cfg(feature = "govee")]
    registry.register(GoveeDriverModule::with_credential_store(Arc::clone(
        &credential_store,
    )))?;

    #[cfg(feature = "hue")]
    registry.register(HueDriverModule::new(Arc::clone(&credential_store)))?;

    #[cfg(feature = "nanoleaf")]
    registry.register(NanoleafDriverModule::new(Arc::clone(&credential_store)))?;

    #[cfg(feature = "openrgb")]
    registry.register(OpenRgbDriverModule)?;

    #[cfg(feature = "hal")]
    {
        registry.register(transport::UsbTransportDriverModule::new(
            usb_protocol_configs,
        ))?;
        registry.register(transport::SmBusTransportDriverModule)?;
        #[cfg(unix)]
        {
            let socket_path = config.discovery.blocks_socket_path.as_ref().map_or_else(
                hypercolor_core::device::BlocksBackend::default_socket_path,
                std::path::PathBuf::from,
            );
            registry.register(transport::BlocksTransportDriverModule::new(
                socket_path,
                config.discovery.blocks_scan,
            ))?;
        }
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
        feature = "openrgb",
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

    #[cfg(feature = "openrgb")]
    config
        .drivers
        .entry(hypercolor_driver_openrgb::DESCRIPTOR.id.to_owned())
        .or_insert_with(|| {
            hypercolor_types::config::DriverConfigEntry::disabled(
                std::collections::BTreeMap::default(),
            )
        });

    #[cfg(feature = "hal")]
    for descriptor in hal::hal_module_descriptors() {
        config.drivers.entry(descriptor.id.clone()).or_default();
    }
}
