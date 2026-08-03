//! Durable driver-owned discovery inventory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};

use hypercolor_driver_api::DriverHost;
use hypercolor_network::DriverModuleRegistry;

use crate::persistence::{AtomicFileWriter, PersistenceError, serialize_json_pretty};
use crate::runtime_state;

const INVENTORY_SCHEMA_VERSION: u32 = 1;

/// Durable driver inventory filename inside the daemon data directory.
pub const DRIVER_INVENTORY_FILENAME: &str = "driver-inventory.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct DriverInventoryDocument {
    schema_version: u32,
    drivers: BTreeMap<String, Value>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

impl Default for DriverInventoryDocument {
    fn default() -> Self {
        Self {
            schema_version: INVENTORY_SCHEMA_VERSION,
            drivers: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }
}

/// Errors produced while loading or persisting driver inventory.
#[derive(Debug, thiserror::Error)]
pub enum DriverInventoryError {
    /// The inventory file could not be read.
    #[error("failed to read driver inventory at {path}: {source}")]
    Read {
        /// Inventory path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// A corrupt inventory file could not be moved aside safely.
    #[error("failed to quarantine corrupt driver inventory from {path} to {quarantine}: {source}")]
    Quarantine {
        /// Corrupt inventory path.
        path: PathBuf,
        /// Recovery path reserved for the corrupt payload.
        quarantine: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// Inventory serialization failed.
    #[error("failed to serialize driver inventory: {0}")]
    Serialize(#[source] serde_json::Error),
    /// Atomic persistence failed.
    #[error("failed to persist driver inventory at {path}: {source}")]
    Persist {
        /// Inventory path.
        path: PathBuf,
        /// Atomic persistence error.
        #[source]
        source: PersistenceError,
    },
}

/// Thread-safe durable store for opaque, driver-owned discovery hints.
#[derive(Debug)]
pub struct DriverInventoryStore {
    path: PathBuf,
    writer: AtomicFileWriter,
    document: Mutex<DriverInventoryDocument>,
}

impl DriverInventoryStore {
    /// Open durable inventory and migrate the legacy runtime cache once when needed.
    ///
    /// # Errors
    ///
    /// Returns an error when storage cannot be initialized, read, quarantined, or persisted.
    pub fn open(
        path: PathBuf,
        legacy_runtime_state_path: &Path,
    ) -> Result<Self, DriverInventoryError> {
        let writer =
            AtomicFileWriter::new(&path).map_err(|source| DriverInventoryError::Persist {
                path: path.clone(),
                source,
            })?;
        let (document, migrated) = if path.exists() {
            (load_document_or_quarantine(&path)?, false)
        } else {
            load_legacy_document(legacy_runtime_state_path)
        };
        let store = Self {
            path,
            writer,
            document: Mutex::new(document),
        };

        if migrated {
            store.persist_current()?;
            info!(
                path = %store.path.display(),
                legacy_path = %legacy_runtime_state_path.display(),
                "Migrated driver inventory from runtime session state"
            );
        }

        Ok(store)
    }

    /// Return the backing inventory path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load one driver-scoped cached JSON payload.
    #[must_use]
    pub fn load_cached_json(&self, driver_id: &str, key: &str) -> Option<Value> {
        let document = self
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        document
            .drivers
            .get(driver_id)
            .and_then(Value::as_object)
            .and_then(|cache| cache.get(key))
            .cloned()
    }

    /// Clone one driver's complete opaque cache map.
    #[must_use]
    pub fn driver_cache(&self, driver_id: &str) -> BTreeMap<String, Value> {
        let document = self
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        document
            .drivers
            .get(driver_id)
            .and_then(Value::as_object)
            .map(|cache| {
                cache
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Atomically replace one driver's cache while preserving all other payloads.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or atomic replacement fails.
    pub fn replace_driver(
        &self,
        driver_id: &str,
        cache: BTreeMap<String, Value>,
    ) -> Result<(), DriverInventoryError> {
        let mut document = self
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut candidate = document.clone();
        if cache.is_empty() {
            candidate.drivers.remove(driver_id);
        } else {
            candidate.drivers.insert(
                driver_id.to_owned(),
                Value::Object(cache.into_iter().collect()),
            );
        }
        self.persist_document(&candidate)?;
        *document = candidate;
        Ok(())
    }

    /// Refresh every driver-owned inventory snapshot without clearing prior data on failure.
    pub async fn refresh(&self, registry: &DriverModuleRegistry, host: &dyn DriverHost) {
        let mut updated = 0_usize;
        let mut preserved = 0_usize;
        let mut failed = 0_usize;

        for driver_id in registry.ids() {
            let Some(driver) = registry.get(&driver_id) else {
                continue;
            };
            let Some(provider) = driver.runtime_cache() else {
                continue;
            };

            match provider.snapshot(host).await {
                Ok(cache) if cache.is_empty() => {
                    preserved += 1;
                    debug!(
                        driver_id,
                        "Preserving driver inventory after empty snapshot"
                    );
                }
                Ok(cache) => match self.replace_driver(&driver_id, cache) {
                    Ok(()) => updated += 1,
                    Err(error) => {
                        failed += 1;
                        warn!(
                            driver_id,
                            %error,
                            "Failed to persist refreshed driver inventory"
                        );
                    }
                },
                Err(error) => {
                    failed += 1;
                    warn!(
                        driver_id,
                        %error,
                        "Preserving driver inventory after snapshot failure"
                    );
                }
            }
        }

        debug!(
            path = %self.path.display(),
            updated,
            preserved,
            failed,
            "Driver inventory refresh complete"
        );
    }

    fn persist_current(&self) -> Result<(), DriverInventoryError> {
        let document = self
            .document
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.persist_document(&document)
    }

    fn persist_document(
        &self,
        document: &DriverInventoryDocument,
    ) -> Result<(), DriverInventoryError> {
        let bytes = serialize_json_pretty(document).map_err(DriverInventoryError::Serialize)?;
        self.writer
            .write(&bytes)
            .map(|_| ())
            .map_err(|source| DriverInventoryError::Persist {
                path: self.path.clone(),
                source,
            })
    }
}

fn load_document_or_quarantine(
    path: &Path,
) -> Result<DriverInventoryDocument, DriverInventoryError> {
    let raw = std::fs::read_to_string(path).map_err(|source| DriverInventoryError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    match serde_json::from_str(&raw) {
        Ok(document) => Ok(document),
        Err(error) => {
            let quarantine = quarantine_path(path);
            std::fs::rename(path, &quarantine).map_err(|source| {
                DriverInventoryError::Quarantine {
                    path: path.to_path_buf(),
                    quarantine: quarantine.clone(),
                    source,
                }
            })?;
            warn!(
                path = %path.display(),
                quarantine = %quarantine.display(),
                %error,
                "Quarantined corrupt driver inventory"
            );
            Ok(DriverInventoryDocument::default())
        }
    }
}

fn load_legacy_document(path: &Path) -> (DriverInventoryDocument, bool) {
    let snapshot = match runtime_state::load(path) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            warn!(
                path = %path.display(),
                %error,
                "Legacy runtime cache could not seed driver inventory"
            );
            None
        }
    };
    let Some(snapshot) = snapshot else {
        return (DriverInventoryDocument::default(), false);
    };
    if snapshot.driver_runtime_cache.is_empty() {
        return (DriverInventoryDocument::default(), false);
    }

    let drivers = snapshot
        .driver_runtime_cache
        .into_iter()
        .map(|(driver_id, cache)| (driver_id, Value::Object(cache.into_iter().collect())))
        .collect();
    (
        DriverInventoryDocument {
            drivers,
            ..DriverInventoryDocument::default()
        },
        true,
    )
}

fn quarantine_path(path: &Path) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let file_name = path
        .file_name()
        .map_or_else(|| "driver-inventory.json".into(), |name| name.to_owned());
    let mut quarantine_name = file_name;
    quarantine_name.push(format!(".corrupt-{timestamp}"));
    path.with_file_name(quarantine_name)
}
