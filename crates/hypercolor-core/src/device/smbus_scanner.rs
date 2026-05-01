//! `SMBus` transport scanner for HAL-managed controllers.

use std::path::PathBuf;

use anyhow::Result;
use hypercolor_hal::probe_smbus_devices_in_root;

use super::{DiscoveredDevice, DiscoveryConnectBehavior, TransportScanner};

/// `SMBus` transport scanner.
pub struct SmBusScanner {
    dev_root: PathBuf,
}

impl SmBusScanner {
    /// Create an `SMBus` scanner.
    #[must_use]
    pub fn new() -> Self {
        Self::with_dev_root("/dev")
    }

    /// Create an `SMBus` scanner with a custom device-node root.
    #[must_use]
    pub fn with_dev_root<P: Into<PathBuf>>(dev_root: P) -> Self {
        Self {
            dev_root: dev_root.into(),
        }
    }
}

impl Default for SmBusScanner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl TransportScanner for SmBusScanner {
    fn name(&self) -> &'static str {
        "SMBus HAL"
    }

    async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
        let probes = probe_smbus_devices_in_root(&self.dev_root).await?;
        Ok(probes
            .into_iter()
            .map(|probe| DiscoveredDevice {
                fingerprint: probe.fingerprint,
                connect_behavior: DiscoveryConnectBehavior::AutoConnect,
                info: probe.info,
                metadata: probe.metadata,
            })
            .collect())
    }
}
