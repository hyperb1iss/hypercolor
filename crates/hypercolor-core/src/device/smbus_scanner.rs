//! `SMBus` transport scanner for HAL-managed controllers.

use std::path::PathBuf;

use anyhow::Result;
use hypercolor_driver_api::{DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_hal::{probe_smbus_devices_in_root, probe_smbus_devices_system};

/// `SMBus` transport scanner.
pub struct SmBusScanner {
    /// Custom device-node root for root-scoped scans; `None` scans the
    /// platform's real buses (Linux `/dev`, Windows PawnIO).
    dev_root: Option<PathBuf>,
}

impl SmBusScanner {
    /// Create an `SMBus` scanner over the platform's system buses.
    #[must_use]
    pub fn new() -> Self {
        Self { dev_root: None }
    }

    /// Create an `SMBus` scanner scoped to a custom device-node root.
    ///
    /// Only device nodes under the root are scanned, which keeps tests
    /// hermetic: platforms whose buses are not filesystem nodes (Windows
    /// PawnIO) discover nothing under a custom root.
    #[must_use]
    pub fn with_dev_root<P: Into<PathBuf>>(dev_root: P) -> Self {
        Self {
            dev_root: Some(dev_root.into()),
        }
    }
}

impl Default for SmBusScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl SmBusScanner {
    /// Discover SMBus devices supported by HAL probes.
    pub async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
        let probes = match &self.dev_root {
            Some(dev_root) => probe_smbus_devices_in_root(dev_root).await?,
            None => probe_smbus_devices_system().await?,
        };
        Ok(probes
            .into_iter()
            .map(|probe| DiscoveredDevice {
                fingerprint: probe.fingerprint,
                connect_behavior: DiscoveryConnectBehavior::AutoConnect,
                info: probe.info,
                metadata: probe.metadata,
                // Deliberate refusal: a bus path plus address locates a
                // device on this host and identifies nothing across hosts.
                claim: None,
            })
            .collect())
    }
}
