//! The persisted scene revision reservation, `scene-revisions.json` in the
//! state tier.
//!
//! The file holds one ceiling: every scene revision any daemon run has
//! handed out is strictly below it. [`SceneRun`](crate::domain::scene_run::SceneRun)
//! owns the policy; this module owns the bytes and guarantees the ceiling
//! on disk only ever moves up, however many writers race.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::persistence::{serialize_json_pretty, write_atomic};

/// The state-tier file that holds the revision reservation.
pub const SCENE_REVISIONS_FILE: &str = "scene-revisions.json";

/// On-disk shape of [`SCENE_REVISIONS_FILE`].
#[derive(Debug, Serialize, Deserialize)]
struct ReservationFile {
    /// Every scene revision any run has handed out is below this.
    reserved_until: u64,
}

/// Read the persisted ceiling. A missing file is `Ok(None)`; an unreadable
/// or unparseable one is an error.
pub(crate) fn read_reservation(path: &Path) -> anyhow::Result<Option<u64>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let file: ReservationFile = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(file.reserved_until))
}

/// The one writer of a run's reservation file.
///
/// Writes are serialized and never lower the ceiling, so a background
/// pre-extension that lands after a synchronous one cannot roll the file
/// back.
#[derive(Debug)]
pub(crate) struct ReservationStore {
    path: PathBuf,
    /// The highest ceiling a write has made durable, readable without the
    /// write lock.
    persisted: AtomicU64,
    write: Mutex<()>,
}

impl ReservationStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            persisted: AtomicU64::new(0),
            write: Mutex::new(()),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The highest ceiling known to be on disk.
    pub(crate) fn persisted(&self) -> u64 {
        self.persisted.load(Ordering::Acquire)
    }

    /// Make the ceiling on disk at least `ceiling`.
    pub(crate) fn persist_at_least(&self, ceiling: u64) -> anyhow::Result<()> {
        let _write = self.write.lock().unwrap_or_else(PoisonError::into_inner);
        if self.persisted() >= ceiling {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let payload = serialize_json_pretty(&ReservationFile {
            reserved_until: ceiling,
        })?;
        write_atomic(&self.path, &payload)
            .with_context(|| format!("failed to write {}", self.path.display()))?;
        self.persisted.store(ceiling, Ordering::Release);
        Ok(())
    }
}
