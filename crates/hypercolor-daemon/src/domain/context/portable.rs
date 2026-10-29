//! Runtime projection reservation shared with portable setup transactions.

use super::{OwnedRwLockWriteGuard, RuntimeSessionService};
use crate::domain::DomainError;
use crate::persistence::{AtomicWriteWaitOutcome, PersistenceShutdown};
use crate::runtime_state::{self, RuntimeSessionSnapshot};

pub(crate) struct PortableRuntimeAdmission {
    path: std::path::PathBuf,
    guard: OwnedRwLockWriteGuard<()>,
}

pub(crate) struct PreparedPortableRuntime {
    pending: Option<runtime_state::PreparedRuntimeSnapshotSave>,
    _guard: OwnedRwLockWriteGuard<()>,
}

impl RuntimeSessionService {
    pub(crate) async fn begin_portable_restore(&self) -> PortableRuntimeAdmission {
        let guard = self
            .projection
            .identity_publication_gate
            .clone()
            .write_owned()
            .await;
        PortableRuntimeAdmission {
            path: self.path.clone(),
            guard,
        }
    }
}

impl PortableRuntimeAdmission {
    pub(crate) fn prepare(
        self,
        snapshot: &RuntimeSessionSnapshot,
    ) -> Result<PreparedPortableRuntime, DomainError> {
        let pending = runtime_state::reserve_save(&self.path)
            .map_err(|error| DomainError::Internal(error.into()))?;
        let pending = runtime_state::prepare_reserved(pending, snapshot)
            .map_err(|error| DomainError::Internal(error.into()))?;
        Ok(PreparedPortableRuntime {
            pending: Some(pending),
            _guard: self.guard,
        })
    }
}

impl PreparedPortableRuntime {
    pub(crate) fn persist(&mut self, shutdown: &PersistenceShutdown) -> AtomicWriteWaitOutcome {
        if shutdown.is_shutdown() {
            return AtomicWriteWaitOutcome::ShutdownPending;
        }
        self.pending
            .take()
            .expect("runtime candidate persisted once")
            .admit()
            .commit_and_wait_durable_or_shutdown(shutdown)
    }
}
