//! Portable setup projection and restore coordination for native transports.

mod bindings;
mod export;

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use hypercolor_core::portable_setup::{SetupBlob, SetupBlobStore};
use hypercolor_types::portable_setup::{PortableBlob, PortableSetupV1};
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

#[derive(Debug, Clone)]
pub enum RestoreMode {
    /// Upsert supplied members and leave missing members untouched.
    Merge,
    /// Delete only members explicitly removed from the previously accepted setup.
    Synchronize { previous: Box<PortableSetupV1> },
}

#[derive(Debug, Clone)]
pub struct RestoreOptions {
    pub expected_revision: String,
    pub mode: RestoreMode,
    pub apply_selection: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreReport {
    pub applied: PortableSetupSnapshot,
    pub unresolved_bindings: Vec<Uuid>,
}

/// A service instance belongs to the application state, shared by every adapter.
/// Platform configuration and account credentials are outside this authority.
pub struct PortableSetupService {
    state: Arc<AppState>,
    blobs: SetupBlobStore,
    root: PathBuf,
    association_gate: tokio::sync::Mutex<()>,
}

impl PortableSetupService {
    pub fn new(state: Arc<AppState>) -> Result<Self, DomainError> {
        let root = state.state_dir.join("portable-setups");
        let blobs = SetupBlobStore::open(root.join("blobs")).map_err(internal)?;
        Ok(Self {
            state,
            blobs,
            root,
            association_gate: tokio::sync::Mutex::new(()),
        })
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

    fn projection_path(&self, setup_id: Uuid) -> PathBuf {
        self.root.join(format!("{setup_id}.json"))
    }

    fn prior_projection(&self, setup_id: Uuid) -> Result<Option<PortableSetupV1>, DomainError> {
        match std::fs::read(self.projection_path(setup_id)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(internal),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(internal(error)),
        }
    }
}

fn internal(error: impl Into<anyhow::Error>) -> DomainError {
    DomainError::Internal(error.into())
}
