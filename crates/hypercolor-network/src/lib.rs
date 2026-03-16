//! Network driver registry and orchestration primitives.
//!
//! This crate owns the host-side registry for modular network drivers. It does
//! not contain driver implementations; it provides the lookup and capability
//! filtering layer that the daemon will use to dispatch discovery and pairing
//! without backend-specific branching.

use std::collections::BTreeMap;
use std::sync::Arc;

use hypercolor_driver_api::{DriverDescriptor, NetworkDriverFactory};
use thiserror::Error;

/// Registry of all compiled-in network drivers.
#[derive(Default)]
pub struct DriverRegistry {
    drivers: BTreeMap<String, Arc<dyn NetworkDriverFactory>>,
}

impl DriverRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a concrete driver factory.
    ///
    /// # Errors
    ///
    /// Returns an error if another driver is already registered with the same
    /// descriptor ID.
    pub fn register<D>(&mut self, driver: D) -> Result<(), DriverRegistryError>
    where
        D: NetworkDriverFactory + 'static,
    {
        self.register_shared(Arc::new(driver))
    }

    /// Register a shared driver factory.
    ///
    /// # Errors
    ///
    /// Returns an error if another driver is already registered with the same
    /// descriptor ID.
    pub fn register_shared(
        &mut self,
        driver: Arc<dyn NetworkDriverFactory>,
    ) -> Result<(), DriverRegistryError> {
        let descriptor = driver.descriptor();
        let id = descriptor.id.to_owned();

        if self.drivers.contains_key(&id) {
            return Err(DriverRegistryError::DuplicateDriverId { id });
        }

        self.drivers.insert(id, driver);
        Ok(())
    }

    /// Retrieve one driver by its stable ID.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<Arc<dyn NetworkDriverFactory>> {
        self.drivers.get(id).map(Arc::clone)
    }

    /// Return all driver IDs in deterministic order.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.drivers.keys().cloned().collect()
    }

    /// Return all registered descriptors in deterministic order.
    #[must_use]
    pub fn descriptors(&self) -> Vec<&'static DriverDescriptor> {
        self.drivers
            .values()
            .map(|driver| driver.descriptor())
            .collect()
    }

    /// Return all drivers that advertise discovery capability.
    #[must_use]
    pub fn discovery_drivers(&self) -> Vec<Arc<dyn NetworkDriverFactory>> {
        self.drivers
            .values()
            .filter(|driver| driver.discovery().is_some())
            .map(Arc::clone)
            .collect()
    }

    /// Return all drivers that advertise pairing capability.
    #[must_use]
    pub fn pairing_drivers(&self) -> Vec<Arc<dyn NetworkDriverFactory>> {
        self.drivers
            .values()
            .filter(|driver| driver.pairing().is_some())
            .map(Arc::clone)
            .collect()
    }
}

/// Errors produced by the driver registry.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DriverRegistryError {
    /// A second driver tried to register the same ID.
    #[error("duplicate driver id: {id}")]
    DuplicateDriverId { id: String },
}
