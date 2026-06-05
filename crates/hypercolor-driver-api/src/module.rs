use std::collections::BTreeMap;

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_types::device::{
    DriverModuleDescriptor, DriverPresentation, DriverProtocolDescriptor,
};

use crate::{
    DeviceBackend, DiscoveryCapability, DriverConfigProvider, DriverConfigView,
    DriverControlProvider, DriverDescriptor, DriverHost, PairingCapability,
};

/// Driver capability for persisting discovery/runtime hints between daemon runs.
#[async_trait]
pub trait DriverRuntimeCacheProvider: Send + Sync {
    /// Build a driver-scoped cache snapshot from host state.
    ///
    /// # Errors
    ///
    /// Returns an error if cache serialization fails.
    async fn snapshot(&self, host: &dyn DriverHost) -> Result<BTreeMap<String, serde_json::Value>>;
}

/// Driver capability for exposing protocol descriptors to the host.
pub trait DriverProtocolCatalog: Send + Sync {
    /// Return value-shaped protocol descriptors owned by this driver.
    fn descriptors(&self) -> &[DriverProtocolDescriptor];
}

/// Driver capability for exposing presentation metadata to the host.
pub trait DriverPresentationProvider: Send + Sync {
    /// Return API and UI presentation metadata for this driver.
    fn presentation(&self) -> DriverPresentation;
}

/// Capability root for one modular driver.
pub trait DriverModule: Send + Sync {
    /// Static metadata about the driver.
    fn descriptor(&self) -> &'static DriverDescriptor;

    /// Host-wide module descriptor for this driver module.
    fn module_descriptor(&self) -> DriverModuleDescriptor {
        let mut descriptor = self.descriptor().module_descriptor();
        descriptor.capabilities.config = self.config().is_some();
        descriptor.capabilities.discovery = self.discovery().is_some();
        descriptor.capabilities.pairing = self.pairing().is_some();
        descriptor.capabilities.runtime_cache = self.runtime_cache().is_some();
        descriptor.capabilities.credentials = descriptor.capabilities.pairing;
        descriptor.capabilities.output_backend = self.has_output_backend();
        descriptor.capabilities.protocol_catalog = self.protocol_catalog().is_some();
        descriptor.capabilities.presentation = self.presentation().is_some();
        descriptor.capabilities.controls = self.controls().is_some();
        descriptor
    }

    /// Config capability, if the driver exposes host-readable defaults or validation.
    fn config(&self) -> Option<&dyn DriverConfigProvider> {
        None
    }

    /// Whether this driver contributes a runtime backend for color output.
    fn has_output_backend(&self) -> bool {
        false
    }

    /// Build the optional runtime backend used for color output.
    ///
    /// Returning `Ok(None)` allows capability-only drivers.
    ///
    /// # Errors
    ///
    /// Returns an error if backend construction fails.
    fn build_output_backend(
        &self,
        _host: &dyn DriverHost,
        _config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        Ok(None)
    }

    /// Discovery capability, if supported.
    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        None
    }

    /// Pairing capability, if supported.
    fn pairing(&self) -> Option<&dyn PairingCapability> {
        None
    }

    /// Control-surface capability, if supported.
    fn controls(&self) -> Option<&dyn DriverControlProvider> {
        None
    }

    /// Protocol catalog capability, if this driver exposes host-readable protocols.
    fn protocol_catalog(&self) -> Option<&dyn DriverProtocolCatalog> {
        None
    }

    /// Presentation metadata capability, if this driver customizes host-facing metadata.
    fn presentation(&self) -> Option<&dyn DriverPresentationProvider> {
        None
    }

    /// Runtime cache capability, if supported.
    fn runtime_cache(&self) -> Option<&dyn DriverRuntimeCacheProvider> {
        None
    }
}
