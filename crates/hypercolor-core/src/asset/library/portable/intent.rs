//! Operation-bound media recovery authority, written after the global journal.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

use hypercolor_persistence::{AtomicFileWriter, AtomicWriteWaitOutcome, PersistenceShutdown};
use hypercolor_types::portable_setup::PortableAsset;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const FILE_NAME: &str = "portable-restore.json";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MediaIntent {
    schema_version: u32,
    pub operation_id: Uuid,
    pub before: BTreeMap<Uuid, PortableAsset>,
    pub desired: BTreeMap<Uuid, PortableAsset>,
    pub completed: bool,
}

impl MediaIntent {
    pub fn new(
        root: &Path,
        operation_id: Uuid,
        before: &BTreeMap<Uuid, PortableAsset>,
        desired: &BTreeMap<Uuid, PortableAsset>,
    ) -> anyhow::Result<Self> {
        if let Some(previous) = read(root)? {
            anyhow::ensure!(
                previous.completed || previous.matches(operation_id, before, desired),
                "another media restore is pending"
            );
        }
        let intent = Self {
            schema_version: 1,
            operation_id,
            before: before.clone(),
            desired: desired.clone(),
            completed: false,
        };
        intent.validate()?;
        Ok(intent)
    }

    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.schema_version == 1 && !self.operation_id.is_nil(),
            "invalid media restore intent identity"
        );
        for (id, asset) in self.before.iter().chain(&self.desired) {
            anyhow::ensure!(
                !id.is_nil() && !asset.mime_type.is_empty(),
                "invalid media restore asset identity"
            );
            asset.content.validate()?;
        }
        Ok(())
    }

    pub fn matches(
        &self,
        operation_id: Uuid,
        before: &BTreeMap<Uuid, PortableAsset>,
        desired: &BTreeMap<Uuid, PortableAsset>,
    ) -> bool {
        self.operation_id == operation_id && self.before == *before && self.desired == *desired
    }

    pub fn persist(
        &self,
        root: &Path,
        shutdown: &PersistenceShutdown,
    ) -> anyhow::Result<AtomicWriteWaitOutcome> {
        if shutdown.is_shutdown() {
            return Ok(AtomicWriteWaitOutcome::ShutdownPending);
        }
        let writer = AtomicFileWriter::new(&root.join(FILE_NAME))?;
        Ok(writer
            .reserve()
            .admit(serde_json::to_vec_pretty(self)?)
            .commit_and_wait_durable_or_shutdown(shutdown))
    }
}

pub(super) fn read(root: &Path) -> anyhow::Result<Option<MediaIntent>> {
    let file = match File::open(root.join(FILE_NAME)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let intent: MediaIntent = serde_json::from_reader(file)?;
    intent.validate()?;
    Ok(Some(intent))
}

pub(super) fn pending(root: &Path) -> anyhow::Result<bool> {
    Ok(read(root)?.is_some_and(|intent| !intent.completed))
}

pub(super) fn finalize(
    root: &Path,
    operation_id: Uuid,
    desired: &BTreeMap<Uuid, PortableAsset>,
    shutdown: &PersistenceShutdown,
) -> anyhow::Result<AtomicWriteWaitOutcome> {
    let mut intent =
        read(root)?.ok_or_else(|| anyhow::anyhow!("media restore intent is missing"))?;
    anyhow::ensure!(
        intent.operation_id == operation_id && intent.desired == *desired,
        "media restore completion differs from committed receipt"
    );
    if intent.completed {
        return Ok(AtomicWriteWaitOutcome::Written);
    }
    intent.completed = true;
    intent.persist(root, shutdown)
}
