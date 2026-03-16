//! Built-in network driver registry and host adapters.

mod host;
#[cfg(feature = "hue")]
mod hue;
#[cfg(feature = "nanoleaf")]
mod nanoleaf;
#[cfg(any(feature = "hue", feature = "nanoleaf"))]
mod pairing;
mod wled;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use hypercolor_network::DriverRegistry;
use hypercolor_types::config::HypercolorConfig;

pub use host::DaemonDriverHost;
#[cfg(feature = "hue")]
pub use hue::pair_hue_bridge_at_ip;
#[cfg(feature = "nanoleaf")]
pub use nanoleaf::pair_nanoleaf_device_at_ip;
pub use wled::build_wled_backend;

/// Build the daemon's compiled-in network driver registry.
///
/// # Errors
///
/// Returns an error if a built-in driver registration collides.
pub fn build_builtin_driver_registry(
    config: &HypercolorConfig,
    host: Arc<DaemonDriverHost>,
    runtime_state_path: PathBuf,
) -> Result<DriverRegistry> {
    let mut registry = DriverRegistry::new();
    registry.register(wled::WledDriverFactory::new(
        config.clone(),
        runtime_state_path,
    ))?;
    #[cfg(not(any(feature = "hue", feature = "nanoleaf")))]
    let _ = &host;

    #[cfg(feature = "hue")]
    registry.register(hue::HueDriverFactory::new(
        Arc::clone(&host),
        config.hue.clone(),
        config.discovery.mdns_enabled,
    ))?;

    #[cfg(feature = "nanoleaf")]
    registry.register(nanoleaf::NanoleafDriverFactory::new(
        host,
        config.nanoleaf.clone(),
        config.discovery.mdns_enabled,
    ))?;

    Ok(registry)
}
