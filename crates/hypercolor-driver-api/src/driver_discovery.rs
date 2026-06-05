use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_types::device::{DeviceFingerprint, DeviceInfo};

use crate::{DiscoveredDevice, DiscoveryConnectBehavior, DriverConfigView, DriverHost};

/// Discovery request normalized by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRequest {
    pub timeout: Duration,
    pub mdns_enabled: bool,
}

/// Driver-produced discovery payload for one tracked device.
#[derive(Debug, Clone)]
pub struct DriverDiscoveredDevice {
    pub info: DeviceInfo,
    pub fingerprint: DeviceFingerprint,
    pub metadata: HashMap<String, String>,
    pub connect_behavior: DiscoveryConnectBehavior,
}

impl From<DiscoveredDevice> for DriverDiscoveredDevice {
    fn from(device: DiscoveredDevice) -> Self {
        Self {
            info: device.info,
            fingerprint: device.fingerprint,
            metadata: device.metadata,
            connect_behavior: device.connect_behavior,
        }
    }
}

/// Discovery result for one driver execution.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryResult {
    pub devices: Vec<DriverDiscoveredDevice>,
}

/// Driver capability for device discovery.
#[async_trait]
pub trait DiscoveryCapability: Send + Sync {
    /// Discover reachable devices for this driver.
    ///
    /// # Errors
    ///
    /// Returns an error if discovery fails.
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> Result<DiscoveryResult>;
}
