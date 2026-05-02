//! Driver module registry and orchestration primitives.
//!
//! This crate owns host-side lookup and capability filtering for compiled-in
//! driver modules. Concrete drivers live in separate crates so the daemon can
//! dispatch discovery, pairing, protocol catalogs, and backend construction without
//! backend-specific branching.

use std::collections::BTreeMap;
use std::sync::Arc;

use hypercolor_driver_api::{DRIVER_API_SCHEMA_VERSION, DriverDescriptor, DriverModule};
use hypercolor_types::device::DriverModuleDescriptor;
use thiserror::Error;

/// Registry of all compiled-in driver modules.
#[derive(Default)]
pub struct DriverModuleRegistry {
    drivers: BTreeMap<String, Arc<dyn DriverModule>>,
}

impl DriverModuleRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a concrete driver module.
    ///
    /// # Errors
    ///
    /// Returns an error if another driver is already registered with the same
    /// descriptor ID.
    pub fn register<D>(&mut self, driver: D) -> Result<(), DriverModuleRegistryError>
    where
        D: DriverModule + 'static,
    {
        self.register_shared(Arc::new(driver))
    }

    /// Register a shared driver module.
    ///
    /// # Errors
    ///
    /// Returns an error if another driver is already registered with the same
    /// descriptor ID, or if the driver reports a schema version that does not
    /// match [`DRIVER_API_SCHEMA_VERSION`].
    pub fn register_shared(
        &mut self,
        driver: Arc<dyn DriverModule>,
    ) -> Result<(), DriverModuleRegistryError> {
        let descriptor = driver.descriptor();
        let id = descriptor.id.to_owned();

        if descriptor.schema_version != DRIVER_API_SCHEMA_VERSION {
            return Err(DriverModuleRegistryError::SchemaVersionMismatch {
                id,
                expected: DRIVER_API_SCHEMA_VERSION,
                found: descriptor.schema_version,
            });
        }

        let module_descriptor = driver.module_descriptor();
        if module_descriptor.api_schema_version != DRIVER_API_SCHEMA_VERSION {
            return Err(DriverModuleRegistryError::SchemaVersionMismatch {
                id,
                expected: DRIVER_API_SCHEMA_VERSION,
                found: module_descriptor.api_schema_version,
            });
        }

        if self.drivers.contains_key(&id) {
            return Err(DriverModuleRegistryError::DuplicateDriverId { id });
        }

        self.drivers.insert(id, driver);
        Ok(())
    }

    /// Retrieve one driver by its stable ID.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<Arc<dyn DriverModule>> {
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

    /// Return all registered module descriptors in deterministic order.
    #[must_use]
    pub fn module_descriptors(&self) -> Vec<DriverModuleDescriptor> {
        self.drivers
            .values()
            .map(|driver| driver.module_descriptor())
            .collect()
    }

    /// Return all drivers that advertise discovery capability.
    #[must_use]
    pub fn discovery_drivers(&self) -> Vec<Arc<dyn DriverModule>> {
        self.drivers
            .values()
            .filter(|driver| driver.discovery().is_some())
            .map(Arc::clone)
            .collect()
    }

    /// Return all drivers that advertise pairing capability.
    #[must_use]
    pub fn pairing_drivers(&self) -> Vec<Arc<dyn DriverModule>> {
        self.drivers
            .values()
            .filter(|driver| driver.pairing().is_some())
            .map(Arc::clone)
            .collect()
    }

    /// Return all drivers that advertise control-surface capability.
    #[must_use]
    pub fn control_drivers(&self) -> Vec<Arc<dyn DriverModule>> {
        self.drivers
            .values()
            .filter(|driver| driver.controls().is_some())
            .map(Arc::clone)
            .collect()
    }

    /// Return all drivers that advertise protocol catalog capability.
    #[must_use]
    pub fn protocol_catalog_drivers(&self) -> Vec<Arc<dyn DriverModule>> {
        self.drivers
            .values()
            .filter(|driver| driver.protocol_catalog().is_some())
            .map(Arc::clone)
            .collect()
    }

    /// Return all drivers that advertise presentation metadata capability.
    #[must_use]
    pub fn presentation_drivers(&self) -> Vec<Arc<dyn DriverModule>> {
        self.drivers
            .values()
            .filter(|driver| driver.presentation().is_some())
            .map(Arc::clone)
            .collect()
    }
}

/// Errors produced by the driver module registry.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DriverModuleRegistryError {
    /// A second driver tried to register the same ID.
    #[error("duplicate driver id: {id}")]
    DuplicateDriverId { id: String },
    /// The driver advertised a schema version the host does not understand.
    #[error("driver '{id}' schema version {found} does not match host version {expected}")]
    SchemaVersionMismatch {
        id: String,
        expected: u32,
        found: u32,
    },
}
