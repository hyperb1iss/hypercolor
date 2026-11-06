//! Built-in Hypercolor driver bundle.
//!
//! The daemon loads this crate as one local module bundle, keeping concrete
//! built-in driver implementations out of daemon orchestration code.

#[cfg(feature = "hal")]
mod hal;
#[cfg(feature = "hal")]
mod transport;

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
#[cfg(feature = "hal")]
use hypercolor_core::device::{UnclaimedDeviceStore, UsbProtocolConfigStore};
use hypercolor_driver_api::DriverDescriptor;
use hypercolor_driver_support::CredentialStore;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::{DriverConfigEntry, HypercolorConfig};

#[cfg(feature = "govee")]
use hypercolor_driver_govee::GoveeDriverModule;
#[cfg(feature = "hue")]
use hypercolor_driver_hue::HueDriverModule;
#[cfg(feature = "lifx")]
use hypercolor_driver_lifx::LifxDriverModule;
#[cfg(feature = "nanoleaf")]
use hypercolor_driver_nanoleaf::NanoleafDriverModule;
#[cfg(feature = "openrgb")]
use hypercolor_driver_openrgb::OpenRgbDriverModule;
#[cfg(feature = "pixel")]
use hypercolor_driver_pixel::PixelDriverModule;
#[cfg(feature = "yeelight")]
use hypercolor_driver_yeelight::YeelightDriverModule;
#[cfg(feature = "wiz")]
use hypercolor_driver_wiz::WizDriverModule;
#[cfg(feature = "twinkly")]
use hypercolor_driver_twinkly::TwinklyDriverModule;

/// One compiled-in standalone driver.
struct BuiltinDriver {
    descriptor: &'static DriverDescriptor,
    register: fn(&mut DriverModuleRegistry, &Arc<CredentialStore>) -> Result<()>,
    /// Config entry minted the first time this driver is seen.
    default_entry: fn() -> DriverConfigEntry,
}

impl BuiltinDriver {
    const fn new(
        descriptor: &'static DriverDescriptor,
        register: fn(&mut DriverModuleRegistry, &Arc<CredentialStore>) -> Result<()>,
    ) -> Self {
        Self {
            descriptor,
            register,
            default_entry: DriverConfigEntry::default,
        }
    }

    const fn minted_disabled(mut self) -> Self {
        self.default_entry = || DriverConfigEntry::disabled(BTreeMap::default());
        self
    }
}

/// Every compiled-in standalone driver, in registration order.
///
/// This table is the one place a driver joins the bundle: a row here plus its
/// Cargo feature. Registration and config seeding both read it, so they cannot
/// disagree about which drivers exist.
static BUILTIN_DRIVERS: &[BuiltinDriver] = &[
    #[cfg(feature = "wled")]
    BuiltinDriver::new(&hypercolor_driver_wled::DESCRIPTOR, |registry, _| {
        Ok(registry.register(WledDriverModule::new())?)
    }),
    #[cfg(feature = "govee")]
    BuiltinDriver::new(&hypercolor_driver_govee::DESCRIPTOR, |registry, store| {
        Ok(registry.register(GoveeDriverModule::with_credential_store(Arc::clone(store)))?)
    }),
    #[cfg(feature = "hue")]
    BuiltinDriver::new(&hypercolor_driver_hue::DESCRIPTOR, |registry, store| {
        Ok(registry.register(HueDriverModule::new(Arc::clone(store)))?)
    }),
    #[cfg(feature = "lifx")]
    BuiltinDriver::new(&hypercolor_driver_lifx::DESCRIPTOR, |registry, _| {
        Ok(registry.register(LifxDriverModule::new())?)
    }),
    #[cfg(feature = "nanoleaf")]
    BuiltinDriver::new(
        &hypercolor_driver_nanoleaf::DESCRIPTOR,
        |registry, store| Ok(registry.register(NanoleafDriverModule::new(Arc::clone(store)))?),
    ),
    #[cfg(feature = "openrgb")]
    BuiltinDriver::new(&hypercolor_driver_openrgb::DESCRIPTOR, |registry, _| {
        Ok(registry.register(OpenRgbDriverModule)?)
    })
    .minted_disabled(),
    #[cfg(feature = "pixel")]
    BuiltinDriver::new(&hypercolor_driver_pixel::DESCRIPTOR, |registry, _| {
        Ok(registry.register(PixelDriverModule::new())?)
    }),
    #[cfg(feature = "twinkly")]
    BuiltinDriver::new(&hypercolor_driver_twinkly::DESCRIPTOR, |registry, _| {
        Ok(registry.register(TwinklyDriverModule::new())?)
    }),
    #[cfg(feature = "wiz")]
    BuiltinDriver::new(&hypercolor_driver_wiz::DESCRIPTOR, |registry, _| {
        Ok(registry.register(WizDriverModule::new())?)
    }),
    #[cfg(feature = "yeelight")]
    BuiltinDriver::new(&hypercolor_driver_yeelight::DESCRIPTOR, |registry, _| {
        Ok(registry.register(YeelightDriverModule::new())?)
    }),
];

/// Ids of every compiled-in standalone driver.
pub fn builtin_driver_ids() -> impl Iterator<Item = &'static str> {
    BUILTIN_DRIVERS.iter().map(|driver| driver.descriptor.id)
}
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
    #[cfg(feature = "hal")] unclaimed_devices: UnclaimedDeviceStore,
) -> Result<DriverModuleRegistry> {
    let mut registry = DriverModuleRegistry::new();
    register_driver_modules(
        &mut registry,
        config,
        credential_store,
        #[cfg(feature = "hal")]
        usb_protocol_configs,
        #[cfg(feature = "hal")]
        unclaimed_devices,
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
    #[cfg(feature = "hal")] unclaimed_devices: UnclaimedDeviceStore,
) -> Result<()> {
    #[cfg(not(all(feature = "hal", unix)))]
    let _ = config;

    for driver in BUILTIN_DRIVERS {
        (driver.register)(registry, &credential_store)?;
    }

    #[cfg(feature = "hal")]
    {
        registry.register(transport::UsbTransportDriverModule::new(
            usb_protocol_configs,
            unclaimed_devices,
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
    for driver in BUILTIN_DRIVERS {
        config
            .drivers
            .entry(driver.descriptor.id.to_owned())
            .or_insert_with(driver.default_entry);
    }

    #[cfg(feature = "hal")]
    for descriptor in hal::hal_module_descriptors() {
        config.drivers.entry(descriptor.id.clone()).or_default();
    }
}
