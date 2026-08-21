use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;

use crate::{DiscoveredDevice, DriverConfigView, DriverHost};

/// Discovery request normalized by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRequest {
    pub timeout: Duration,
    pub mdns_enabled: bool,
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
    ) -> Result<Vec<DiscoveredDevice>>;
}
