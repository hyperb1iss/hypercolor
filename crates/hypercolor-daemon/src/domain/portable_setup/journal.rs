//! Durable restore authority, separate from any transport acknowledgment.

use hypercolor_types::portable_setup::SetupBindings;
use uuid::Uuid;

use super::{PortableSetupService, internal};
use crate::domain::DomainError;
use crate::persistence::{
    AtomicFileWriter, AtomicWriteWaitOutcome, PersistenceShutdown, serialize_json_pretty,
};
pub(super) use crate::portable_setup_state::{
    BackupAncillary, RestoreJournal, RuntimeRestoreProof,
};

pub(super) struct RestoreRecovery {
    pub runtime: RuntimeRestoreProof,
    pub bindings: SetupBindings,
}

/// Detect recovery authority before startup loaders or background writers run.
pub(crate) fn pending_restore_exists(state_dir: &std::path::Path) -> Result<bool, DomainError> {
    let entries = match std::fs::read_dir(state_dir.join("portable-setups")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(internal(error)),
    };
    let mut pending = false;
    for entry in entries {
        let entry = entry.map_err(internal)?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".transaction.json"))
        else {
            continue;
        };
        let id: Uuid = id.parse().map_err(internal)?;
        let journal: RestoreJournal =
            serde_json::from_slice(&std::fs::read(entry.path()).map_err(internal)?)
                .map_err(internal)?;
        let (version, actual) = match journal {
            RestoreJournal::Pending {
                schema_version,
                before,
                candidate,
                ..
            } => {
                if before.setup_id != candidate.setup_id {
                    return Err(DomainError::validation(
                        "restore journal identities disagree",
                    ));
                }
                pending = true;
                (schema_version, candidate.setup_id)
            }
            RestoreJournal::Committed {
                schema_version,
                receipt,
                finalized,
                ..
            } => {
                if receipt.setup_id != receipt.applied.manifest.setup_id {
                    return Err(DomainError::validation(
                        "restore receipt identities disagree",
                    ));
                }
                pending |= !finalized;
                (schema_version, receipt.setup_id)
            }
        };
        if version != 1 || actual != id {
            return Err(DomainError::validation(
                "unsupported restore journal identity or version",
            ));
        }
    }
    Ok(pending)
}

impl PortableSetupService {
    pub(super) fn read_journal(
        &self,
        setup_id: Uuid,
    ) -> Result<Option<RestoreJournal>, DomainError> {
        let path = self.root.join(format!("{setup_id}.transaction.json"));
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(internal(error)),
        };
        let journal: RestoreJournal = serde_json::from_slice(&bytes).map_err(internal)?;
        let (version, identity) = match &journal {
            RestoreJournal::Pending {
                schema_version,
                before,
                candidate,
                ..
            } => {
                if before.setup_id != candidate.setup_id {
                    return Err(DomainError::validation(
                        "restore journal setup identities disagree",
                    ));
                }
                (*schema_version, candidate.setup_id)
            }
            RestoreJournal::Committed {
                schema_version,
                receipt,
                ..
            } => {
                if receipt.setup_id != receipt.applied.manifest.setup_id {
                    return Err(DomainError::validation(
                        "restore receipt setup identities disagree",
                    ));
                }
                (*schema_version, receipt.setup_id)
            }
        };
        if version != 1 || identity != setup_id {
            return Err(DomainError::validation(
                "unsupported restore journal identity or version",
            ));
        }
        Ok(Some(journal))
    }

    /// Called on the owned blocking persistence worker while mutation guards
    /// remain retained. A failed write never outlives those guards.
    pub(super) fn write_journal(
        &self,
        setup_id: Uuid,
        journal: &RestoreJournal,
        shutdown: &PersistenceShutdown,
    ) -> Result<AtomicWriteWaitOutcome, DomainError> {
        let path = self.root.join(format!("{setup_id}.transaction.json"));
        let payload = serialize_json_pretty(journal).map_err(internal)?;
        let writer = AtomicFileWriter::new(&path).map_err(internal)?;
        Ok(writer
            .reserve()
            .admit(payload)
            .commit_and_wait_durable_or_shutdown(shutdown))
    }
}
