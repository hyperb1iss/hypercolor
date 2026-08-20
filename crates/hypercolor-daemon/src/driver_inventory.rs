//! Durable driver-owned discovery inventory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex as AsyncMutex, MutexGuard};
use tracing::{debug, warn};

use hypercolor_driver_api::DriverHost;
use hypercolor_network::DriverModuleRegistry;

use crate::path_migration::{
    MigratedStore, MigrationOutcome, PathMigrationEntry, PathMigrationError, VersionedDocument,
    migrate,
};
use crate::persistence::{AtomicFileWriter, PersistenceError, serialize_json_pretty};

const INVENTORY_SCHEMA_VERSION: u32 = 1;
const STORE_SUBJECT: &str = "driver inventory";

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
    /// A legacy inventory could not be relocated to its canonical state path.
    #[error(transparent)]
    Migration(#[from] PathMigrationError),
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
    /// A driver-owned inventory update failed.
    #[error("failed to update {driver_id} discovery inventory: {source}")]
    Update {
        /// Driver whose semantic update failed.
        driver_id: String,
        /// Driver-owned update error.
        #[source]
        source: anyhow::Error,
    },
}

/// Thread-safe durable store for opaque, driver-owned discovery hints.
#[derive(Debug)]
pub struct DriverInventoryStore {
    path: PathBuf,
    writer: AtomicFileWriter,
    operation_gate: AsyncMutex<()>,
    document: StdMutex<DriverInventoryDocument>,
}

impl DriverInventoryStore {
    /// Open durable inventory at its canonical path.
    ///
    /// # Errors
    ///
    /// Returns an error when storage cannot be initialized, read, or quarantined.
    pub fn open(path: PathBuf) -> Result<Self, DriverInventoryError> {
        let writer =
            AtomicFileWriter::new(&path).map_err(|source| DriverInventoryError::Persist {
                path: path.clone(),
                source,
            })?;
        let document = if path.exists() {
            load_document_or_quarantine(&path)?
        } else {
            DriverInventoryDocument::default()
        };

        Ok(Self {
            path,
            writer,
            operation_gate: AsyncMutex::new(()),
            document: StdMutex::new(document),
        })
    }

    /// Relocate a legacy data-tier inventory and open the state-tier store.
    ///
    /// # Errors
    ///
    /// Returns an error when storage cannot be initialized, read, migrated,
    /// quarantined, or persisted.
    pub fn open_migrated(
        legacy_path: PathBuf,
        canonical_path: PathBuf,
    ) -> Result<(Self, MigrationOutcome), DriverInventoryError> {
        let writer = AtomicFileWriter::new(&canonical_path).map_err(|source| {
            DriverInventoryError::Persist {
                path: canonical_path.clone(),
                source,
            }
        })?;
        let entry = PathMigrationEntry::new(STORE_SUBJECT, legacy_path, canonical_path.clone());
        let migrated = migrate(&DriverInventoryCodec, &entry, &writer)?;
        let outcome = migrated.outcome;
        let store = Self {
            path: canonical_path,
            writer,
            operation_gate: AsyncMutex::new(()),
            document: StdMutex::new(migrated.document.unwrap_or_default()),
        };
        Ok((store, outcome))
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
    /// Returns an error if the replacement cannot be serialized. Once admitted,
    /// replacement failures remain queued for the persistence retry worker.
    pub async fn replace_driver(
        &self,
        driver_id: &str,
        cache: BTreeMap<String, Value>,
    ) -> Result<(), DriverInventoryError> {
        let guard = self.operation_gate.lock().await;
        self.replace_driver_guarded(&guard, driver_id, cache)?;
        Ok(())
    }

    /// Atomically transform one driver's complete cache map.
    ///
    /// # Errors
    ///
    /// Returns an error if the driver transform or serialization fails.
    pub async fn update_driver(
        &self,
        driver_id: &str,
        update: impl FnOnce(&BTreeMap<String, Value>) -> anyhow::Result<BTreeMap<String, Value>>,
    ) -> Result<bool, DriverInventoryError> {
        let guard = self.operation_gate.lock().await;
        self.update_driver_guarded(&guard, driver_id, update)
    }

    pub(crate) async fn operation_guard(&self) -> MutexGuard<'_, ()> {
        self.operation_gate.lock().await
    }

    pub(crate) fn update_driver_guarded(
        &self,
        guard: &MutexGuard<'_, ()>,
        driver_id: &str,
        update: impl FnOnce(&BTreeMap<String, Value>) -> anyhow::Result<BTreeMap<String, Value>>,
    ) -> Result<bool, DriverInventoryError> {
        let current = self.driver_cache(driver_id);
        let updated = update(&current).map_err(|source| DriverInventoryError::Update {
            driver_id: driver_id.to_owned(),
            source,
        })?;
        if updated == current {
            return Ok(false);
        }
        self.replace_driver_guarded(guard, driver_id, updated)?;
        Ok(true)
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
            let guard = self.operation_gate.lock().await;

            match provider.snapshot(host).await {
                Ok(cache) if cache.is_empty() => {
                    preserved += 1;
                    debug!(
                        driver_id,
                        "Preserving driver inventory after empty snapshot"
                    );
                }
                Ok(cache) => match self.merge_driver_guarded(&guard, &driver_id, cache) {
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

    fn replace_driver_guarded(
        &self,
        _guard: &MutexGuard<'_, ()>,
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
        let bytes = serialize_json_pretty(&candidate).map_err(DriverInventoryError::Serialize)?;
        let pending = self.writer.reserve().admit(bytes);
        *document = candidate;
        if let Err(error) = pending.commit() {
            warn!(
                path = %self.path.display(),
                %error,
                "Failed to persist driver inventory; retry remains active"
            );
        }
        Ok(())
    }

    fn merge_driver_guarded(
        &self,
        guard: &MutexGuard<'_, ()>,
        driver_id: &str,
        cache: BTreeMap<String, Value>,
    ) -> Result<(), DriverInventoryError> {
        let mut merged = self.driver_cache(driver_id);
        merged.extend(cache);
        self.replace_driver_guarded(guard, driver_id, merged)
    }
}

fn load_document_or_quarantine(
    path: &Path,
) -> Result<DriverInventoryDocument, DriverInventoryError> {
    Ok(read_document_or_quarantine(path)?.unwrap_or_default())
}

fn read_document_or_quarantine(
    path: &Path,
) -> Result<Option<DriverInventoryDocument>, DriverInventoryError> {
    let raw = std::fs::read_to_string(path).map_err(|source| DriverInventoryError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    match serde_json::from_str(&raw) {
        Ok(document) => Ok(Some(document)),
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
            Ok(None)
        }
    }
}

struct DriverInventoryCodec;

impl MigratedStore for DriverInventoryCodec {
    type Document = DriverInventoryDocument;
    type Error = DriverInventoryError;

    fn decode_current(
        &self,
        path: &Path,
    ) -> Result<VersionedDocument<Self::Document>, Self::Error> {
        Ok(match read_document_or_quarantine(path)? {
            Some(document) => VersionedDocument::new(document.schema_version, document),
            None => VersionedDocument::unversioned(DriverInventoryDocument::default()),
        })
    }

    fn decode_legacy(
        &self,
        path: &Path,
    ) -> Result<Option<VersionedDocument<Self::Document>>, Self::Error> {
        Ok(read_document_or_quarantine(path)?
            .map(|document| VersionedDocument::new(document.schema_version, document)))
    }

    fn encode(&self, document: &Self::Document) -> Result<Vec<u8>, Self::Error> {
        serialize_json_pretty(document).map_err(DriverInventoryError::Serialize)
    }
}

fn quarantine_path(path: &Path) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let file_name = path.file_name().map_or_else(
        || "driver-inventory.json".into(),
        std::borrow::ToOwned::to_owned,
    );
    let mut quarantine_name = file_name;
    quarantine_name.push(format!(".corrupt-{timestamp}"));
    path.with_file_name(quarantine_name)
}
