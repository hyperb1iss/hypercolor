//! Durable encoding for in-progress cross-host device binding migrations.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::Context as _;
use hypercolor_types::device::DeviceId;
use serde::{Deserialize, Serialize};

use crate::domain::device_binding::DeviceBindingRemaps;
use crate::persistence::{AtomicFileWriter, AtomicWriteCommitResult, serialize_json_pretty};

const DEVICE_BINDING_JOURNAL_SCHEMA_VERSION: u32 = 1;

#[derive(Clone)]
pub(crate) struct DeviceBindingMigrationJournal {
    pub(crate) path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceBindingMigrationJournalDocument {
    schema_version: u32,
    remaps: Option<PersistedDeviceBindingRemaps>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedDeviceBindingRemaps {
    layout_device_ids: HashMap<String, String>,
    physical_device_ids: HashMap<DeviceId, DeviceId>,
    persisted_setting_keys: HashMap<String, String>,
}

impl From<&DeviceBindingRemaps> for PersistedDeviceBindingRemaps {
    fn from(remaps: &DeviceBindingRemaps) -> Self {
        Self {
            layout_device_ids: remaps.layout_device_ids.clone(),
            physical_device_ids: remaps.physical_device_ids.clone(),
            persisted_setting_keys: remaps.persisted_setting_keys.clone(),
        }
    }
}

impl From<PersistedDeviceBindingRemaps> for DeviceBindingRemaps {
    fn from(remaps: PersistedDeviceBindingRemaps) -> Self {
        Self {
            layout_device_ids: remaps.layout_device_ids,
            physical_device_ids: remaps.physical_device_ids,
            persisted_setting_keys: remaps.persisted_setting_keys,
        }
    }
}

impl DeviceBindingMigrationJournal {
    pub(crate) const fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn load(&self) -> anyhow::Result<Option<DeviceBindingRemaps>> {
        let payload = match std::fs::read(&self.path) {
            Ok(payload) => payload,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to read device binding migration journal at {}",
                        self.path.display()
                    )
                });
            }
        };
        let document: DeviceBindingMigrationJournalDocument = serde_json::from_slice(&payload)
            .with_context(|| {
                format!(
                    "failed to parse device binding migration journal at {}",
                    self.path.display()
                )
            })?;
        anyhow::ensure!(
            document.schema_version == DEVICE_BINDING_JOURNAL_SCHEMA_VERSION,
            "device binding migration journal at {} uses unsupported schema version {}; expected {}",
            self.path.display(),
            document.schema_version,
            DEVICE_BINDING_JOURNAL_SCHEMA_VERSION
        );
        Ok(document.remaps.map(Into::into))
    }

    pub(crate) fn persist_active(&self, remaps: &DeviceBindingRemaps) -> anyhow::Result<()> {
        self.persist(Some(remaps.into()))
    }

    pub(crate) fn clear(&self) -> anyhow::Result<()> {
        self.persist(None)
    }

    fn persist(&self, remaps: Option<PersistedDeviceBindingRemaps>) -> anyhow::Result<()> {
        let payload = serialize_json_pretty(&DeviceBindingMigrationJournalDocument {
            schema_version: DEVICE_BINDING_JOURNAL_SCHEMA_VERSION,
            remaps,
        })
        .context("failed to serialize device binding migration journal")?;
        let outcome = AtomicFileWriter::new(&self.path)?
            .reserve()
            .admit(payload)
            .commit_stage_aware();
        match outcome {
            AtomicWriteCommitResult::DurableWritten => Ok(()),
            AtomicWriteCommitResult::Superseded => {
                anyhow::bail!("device binding migration journal write was superseded")
            }
            AtomicWriteCommitResult::FailedBeforeReplacement(error)
            | AtomicWriteCommitResult::ReplacementVisibleButNotDurable(error) => Err(error)
                .with_context(|| {
                    format!(
                        "failed to persist device binding migration journal at {}",
                        self.path.display()
                    )
                }),
        }
    }
}
