//! Driver module registry and orchestration primitives.
//!
//! This crate owns host-side lookup and capability filtering for compiled-in
//! driver modules. Concrete drivers live in separate crates so the daemon can
//! dispatch discovery, pairing, protocol catalogs, and backend construction without
//! backend-specific branching.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use hypercolor_driver_api::{
    DRIVER_API_SCHEMA_VERSION, DeviceBackend, DriverConfigView, DriverDescriptor, DriverError,
    DriverHost, DriverModule, OutputBinding,
};
use hypercolor_types::device::DriverModuleDescriptor;
use hypercolor_types::identity::BackendId;
use thiserror::Error;

/// Registry of all compiled-in driver modules.
#[derive(Default)]
pub struct DriverModuleRegistry {
    drivers: BTreeMap<String, Arc<dyn DriverModule>>,
}

/// One output backend provider selected by registry finalization.
#[derive(Clone)]
pub struct FinalizedOutputProvider {
    backend_id: BackendId,
    driver_id: String,
    driver: Arc<dyn DriverModule>,
}

impl FinalizedOutputProvider {
    /// Declared backend ID owned by this provider.
    #[must_use]
    pub const fn backend_id(&self) -> &BackendId {
        &self.backend_id
    }

    /// Driver module ID that owns the backend factory.
    #[must_use]
    pub fn driver_id(&self) -> &str {
        &self.driver_id
    }

    /// Build and verify the provider's live backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the provider changes its declaration, construction
    /// fails, or the constructed backend reports a different ID.
    pub fn build(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Arc<dyn DeviceBackend>, DriverError> {
        let OutputBinding::Owned { id, factory } = self.driver.output() else {
            return Err(DriverError::Contract {
                message: format!(
                    "finalized provider '{}' no longer owns an output backend",
                    self.driver_id
                ),
            });
        };
        if id != self.backend_id {
            return Err(DriverError::Contract {
                message: format!(
                    "finalized provider '{}' changed backend ID from '{}' to '{}'",
                    self.driver_id, self.backend_id, id
                ),
            });
        }

        let backend = factory.build(host, config)?;
        let reported_id = backend.info().id;
        if reported_id != self.backend_id.as_str() {
            return Err(DriverError::Contract {
                message: format!(
                    "provider '{}' declared backend '{}' but built '{}'",
                    self.driver_id, self.backend_id, reported_id
                ),
            });
        }
        Ok(backend)
    }
}

/// Validated output providers required by the enabled module set.
pub struct FinalizedOutputBindings {
    providers: Vec<FinalizedOutputProvider>,
}

impl FinalizedOutputBindings {
    /// Providers to build, ordered by backend ID.
    #[must_use]
    pub fn providers(&self) -> &[FinalizedOutputProvider] {
        &self.providers
    }

    /// Find the finalized provider for one backend ID.
    #[must_use]
    pub fn provider(&self, backend_id: &BackendId) -> Option<&FinalizedOutputProvider> {
        self.providers
            .iter()
            .find(|provider| provider.backend_id() == backend_id)
    }
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
    /// descriptor ID, if it duplicates an owned output backend, or if the
    /// driver reports a schema version that does not match
    /// [`DRIVER_API_SCHEMA_VERSION`].
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

        if let OutputBinding::Owned { id: backend_id, .. } = driver.output()
            && let Some((first_driver_id, _)) = self.drivers.iter().find(|(_, registered)| {
                matches!(
                    registered.output(),
                    OutputBinding::Owned { id, .. } if id == backend_id
                )
            })
        {
            return Err(DriverModuleRegistryError::DuplicateOutputProvider {
                backend_id,
                first_driver_id: first_driver_id.clone(),
                second_driver_id: id,
            });
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

    /// Validate output ownership and select providers needed by enabled modules.
    ///
    /// # Errors
    ///
    /// Returns an error for duplicate providers, unresolved shared bindings,
    /// or an enabled module ID that is not registered.
    pub fn finalize_output_bindings(
        &self,
        enabled_driver_ids: &BTreeSet<String>,
    ) -> Result<FinalizedOutputBindings, DriverModuleRegistryError> {
        let mut providers = BTreeMap::<BackendId, (String, Arc<dyn DriverModule>)>::new();
        let mut bindings = BTreeMap::<String, Option<BackendId>>::new();
        let mut shared = Vec::<(String, BackendId)>::new();

        for (driver_id, driver) in &self.drivers {
            match driver.output() {
                OutputBinding::Owned { id, .. } => {
                    if let Some((existing_driver_id, _)) =
                        providers.insert(id.clone(), (driver_id.clone(), Arc::clone(driver)))
                    {
                        return Err(DriverModuleRegistryError::DuplicateOutputProvider {
                            backend_id: id,
                            first_driver_id: existing_driver_id,
                            second_driver_id: driver_id.clone(),
                        });
                    }
                    bindings.insert(driver_id.clone(), Some(id));
                }
                OutputBinding::Shared(id) => {
                    shared.push((driver_id.clone(), id.clone()));
                    bindings.insert(driver_id.clone(), Some(id));
                }
                OutputBinding::None => {
                    bindings.insert(driver_id.clone(), None);
                }
            }
        }

        for (consumer_driver_id, backend_id) in shared {
            if !providers.contains_key(&backend_id) {
                return Err(DriverModuleRegistryError::UnresolvedSharedOutput {
                    backend_id,
                    consumer_driver_id,
                });
            }
        }

        let mut requested_backend_ids = BTreeSet::new();
        for driver_id in enabled_driver_ids {
            let binding = bindings.get(driver_id).ok_or_else(|| {
                DriverModuleRegistryError::UnknownEnabledDriverId {
                    id: driver_id.clone(),
                }
            })?;
            if let Some(backend_id) = binding {
                requested_backend_ids.insert(backend_id.clone());
            }
        }

        let providers = requested_backend_ids
            .into_iter()
            .filter_map(|backend_id| {
                let (driver_id, driver) = providers.get(&backend_id)?;
                Some(FinalizedOutputProvider {
                    backend_id,
                    driver_id: driver_id.clone(),
                    driver: Arc::clone(driver),
                })
            })
            .collect();

        Ok(FinalizedOutputBindings { providers })
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
    /// Two modules declared ownership of the same output backend ID.
    #[error(
        "drivers '{first_driver_id}' and '{second_driver_id}' both provide output backend '{backend_id}'"
    )]
    DuplicateOutputProvider {
        /// Conflicting backend ID.
        backend_id: BackendId,
        /// First registered provider.
        first_driver_id: String,
        /// Second registered provider.
        second_driver_id: String,
    },
    /// A shared output binding named a backend with no provider.
    #[error(
        "driver '{consumer_driver_id}' shares output backend '{backend_id}', but no provider is registered"
    )]
    UnresolvedSharedOutput {
        /// Missing backend ID.
        backend_id: BackendId,
        /// Module that declared the shared binding.
        consumer_driver_id: String,
    },
    /// The enabled module set named an unregistered driver.
    #[error("enabled driver is not registered: {id}")]
    UnknownEnabledDriverId {
        /// Unknown driver ID.
        id: String,
    },
}
