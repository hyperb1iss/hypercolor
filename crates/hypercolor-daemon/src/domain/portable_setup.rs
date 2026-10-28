//! Portable setup projection and restore coordination for native transports.

mod archive;
mod authority;
mod bindings;
mod domains;
mod export;
mod journal;
pub(crate) use journal::pending_restore_exists;
mod media;
mod plan;
pub(crate) mod prepared;
mod reconcile;
mod restore;
mod runtime_preferences;
mod templates;
mod transaction;

#[cfg(all(test, feature = "persistence-test-hooks"))]
mod tests;

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use hypercolor_core::portable_setup::{SetupBlob, SetupBlobStore};
use hypercolor_types::portable_setup::{PortableBlob, PortableSetupV1, SetupBindings};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::app_state::AppState;
use crate::domain::DomainError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupIdentity {
    pub setup_id: Uuid,
    pub name: String,
}

/// The revision fingerprints authored state, not runtime counters or host paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortableSetupSnapshot {
    pub manifest: PortableSetupV1,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RestoreMode {
    /// Upsert supplied members and leave missing members untouched.
    Merge,
    /// Delete only members explicitly removed from the previously accepted setup.
    Synchronize { previous: Box<PortableSetupV1> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreOptions {
    pub operation_id: Uuid,
    pub expected_revision: String,
    pub mode: RestoreMode,
    pub apply_selection: bool,
}

/// Local choices captured when a manual backup confirmation is prepared.
#[derive(Debug, Clone, Default)]
pub struct BackupRestoreExpectation {
    pub preferences: Option<hypercolor_types::portable_setup::PortablePreferences>,
    pub tui_preferences: Option<hypercolor_types::client_preferences::TuiPreferences>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreReport {
    pub applied: PortableSetupSnapshot,
    pub unresolved_bindings: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreReceipt {
    pub operation_id: Uuid,
    pub setup_id: Uuid,
    pub requested_revision: String,
    pub applied: PortableSetupSnapshot,
    pub unresolved_bindings: Vec<Uuid>,
    pub materialized_bindings: SetupBindings,
}

#[derive(Default)]
struct RestoreRuntime {
    gate: tokio::sync::Mutex<()>,
    closed: std::sync::atomic::AtomicBool,
    closed_notify: tokio::sync::Notify,
    pending: std::sync::atomic::AtomicBool,
    active: std::sync::Mutex<Option<crate::persistence::PersistenceShutdown>>,
}

/// Shared by every adapter of one native domain graph, without owning AppState.
#[derive(Clone, Default)]
pub(crate) struct PortableSetupAuthority {
    association_gate: Arc<tokio::sync::Mutex<()>>,
    restore_runtime: Arc<RestoreRuntime>,
}

/// A service instance belongs to the application state, shared by every adapter.
/// Platform configuration and account credentials are outside this authority.
#[derive(Clone)]
pub struct PortableSetupService {
    state: Arc<AppState>,
    blobs: SetupBlobStore,
    root: PathBuf,
    association_gate: Arc<tokio::sync::Mutex<()>>,
    restore_runtime: Arc<RestoreRuntime>,
}

impl PortableSetupService {
    pub fn new(state: Arc<AppState>) -> Result<Self, DomainError> {
        let root = state.state_dir.join("portable-setups");
        let blobs = SetupBlobStore::open(root.join("blobs")).map_err(internal)?;
        let service = Self {
            association_gate: Arc::clone(&state.domains.portable_setup.association_gate),
            restore_runtime: Arc::clone(&state.domains.portable_setup.restore_runtime),
            state,
            blobs,
            root,
        };
        if service.has_pending_restore()? {
            service
                .restore_runtime
                .pending
                .store(true, std::sync::atomic::Ordering::Release);
        }
        Ok(service)
    }

    /// Read only a verified object from this installation's staging store.
    pub fn export_blob(&self, descriptor: &PortableBlob) -> Result<SetupBlob, DomainError> {
        self.blobs.read(descriptor).map_err(internal)
    }

    /// Stage a streamed transfer before any domain restore begins.
    pub fn stage_blob(
        &self,
        descriptor: &PortableBlob,
        source: impl Read,
    ) -> Result<(), DomainError> {
        self.blobs.stage(descriptor, source).map_err(internal)
    }
}

fn fingerprint(value: &impl Serialize) -> Result<String, DomainError> {
    let bytes =
        serde_json::to_vec(&serde_json::to_value(value).map_err(internal)?).map_err(internal)?;
    Ok(hypercolor_core::asset::AssetLibrary::hash_bytes(&bytes))
}

fn internal(error: impl Into<anyhow::Error>) -> DomainError {
    DomainError::Internal(error.into())
}
