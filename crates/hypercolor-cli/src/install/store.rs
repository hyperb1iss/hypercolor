use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use hypercolor_platform_fs::ExclusiveDirectory;

use super::model::{InstallJournalV1, UnitId, active_target};

const INSTALL_LOCK_FILE: &str = "install.lock";
const INSTALL_JOURNAL_FILE: &str = "install-journal.json";
const UNITS_DIRECTORY: &str = "units";
const MAX_JOURNAL_STAGE_ATTEMPTS: usize = 128;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct InstallStore {
    root: PathBuf,
    max_journal_bytes: usize,
}

impl InstallStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, max_journal_bytes: usize) -> Self {
        Self {
            root: root.into(),
            max_journal_bytes,
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn journal_path(&self) -> PathBuf {
        self.root.join(INSTALL_JOURNAL_FILE)
    }

    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.root.join(INSTALL_LOCK_FILE)
    }

    #[must_use]
    pub fn active_path(&self) -> PathBuf {
        self.root.join("active")
    }

    pub fn acquire_lock(&self) -> Result<InstallLock, InstallStoreError> {
        fs::create_dir_all(&self.root).map_err(InstallStoreError::CreateRoot)?;
        let directory = ExclusiveDirectory::try_acquire(&self.root, Path::new(INSTALL_LOCK_FILE))
            .map_err(InstallStoreError::AcquireLock)?
            .ok_or(InstallStoreError::LockContended)?;
        Ok(InstallLock {
            root: self.root.clone(),
            directory,
        })
    }

    pub fn active_unit(&self, lock: &InstallLock) -> Result<Option<UnitId>, InstallStoreError> {
        let directory = self.authority(lock)?;
        let Some(target) = directory
            .read_symlink(Path::new("active"))
            .map_err(InstallStoreError::InspectActive)?
        else {
            return Ok(None);
        };
        let mut components = target.components();
        let units = components
            .next()
            .and_then(|component| component.as_os_str().to_str());
        let unit = components
            .next()
            .and_then(|component| component.as_os_str().to_str());
        if units != Some(UNITS_DIRECTORY) || components.next().is_some() {
            return Err(InstallStoreError::InvalidActiveLink(format!(
                "unexpected active target {}",
                target.display()
            )));
        }
        let id = UnitId::new(unit.unwrap_or_default()).map_err(|error| {
            InstallStoreError::InvalidActiveLink(format!("invalid active unit: {error}"))
        })?;
        Ok(Some(id))
    }

    pub fn set_active(
        &self,
        unit: Option<&UnitId>,
        lock: &InstallLock,
    ) -> Result<(), InstallStoreError> {
        let directory = self.authority(lock)?;
        match unit {
            Some(unit) => {
                directory
                    .durable_replace_symlink(&active_target(unit), Path::new("active"))
                    .map_err(InstallStoreError::SwitchActive)?;
            }
            None => {
                directory
                    .durable_remove_file(Path::new("active"))
                    .map_err(InstallStoreError::SwitchActive)?;
            }
        }
        Ok(())
    }

    pub fn load_journal(
        &self,
        lock: &InstallLock,
    ) -> Result<Option<InstallJournalV1>, InstallStoreError> {
        let directory = self.authority(lock)?;
        let mut file = match directory.open_file(Path::new(INSTALL_JOURNAL_FILE)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(InstallStoreError::OpenJournal(error)),
        };
        let metadata = file.metadata().map_err(InstallStoreError::ReadJournal)?;
        if !metadata.is_file() || metadata.len() > self.max_journal_bytes as u64 {
            return Err(InstallStoreError::JournalTooLarge {
                limit: self.max_journal_bytes,
            });
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take((self.max_journal_bytes + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(InstallStoreError::ReadJournal)?;
        if bytes.len() > self.max_journal_bytes {
            return Err(InstallStoreError::JournalTooLarge {
                limit: self.max_journal_bytes,
            });
        }
        let journal: InstallJournalV1 =
            serde_json::from_slice(&bytes).map_err(InstallStoreError::DecodeJournal)?;
        journal
            .validate()
            .map_err(InstallStoreError::InvalidJournal)?;
        Ok(Some(journal))
    }

    pub fn write_journal(
        &self,
        journal: &InstallJournalV1,
        lock: &InstallLock,
    ) -> Result<(), InstallStoreError> {
        let directory = self.authority(lock)?;
        journal
            .validate()
            .map_err(InstallStoreError::InvalidJournal)?;
        let bytes = serde_json::to_vec(journal).map_err(InstallStoreError::EncodeJournal)?;
        if bytes.len() > self.max_journal_bytes {
            return Err(InstallStoreError::JournalTooLarge {
                limit: self.max_journal_bytes,
            });
        }
        let temporary = stage_journal(directory, &bytes, &TEMPORARY_SEQUENCE)?;
        if let Err(error) =
            directory.durable_replace_file(&temporary, Path::new(INSTALL_JOURNAL_FILE))
        {
            let _ = directory.durable_remove_file(&temporary);
            return Err(InstallStoreError::ReplaceJournal(error));
        }
        Ok(())
    }

    fn authority<'a>(
        &self,
        lock: &'a InstallLock,
    ) -> Result<&'a ExclusiveDirectory, InstallStoreError> {
        if lock.root != self.root {
            return Err(InstallStoreError::WrongLock);
        }
        Ok(&lock.directory)
    }
}

fn stage_journal(
    directory: &ExclusiveDirectory,
    bytes: &[u8],
    sequence: &AtomicU64,
) -> Result<PathBuf, InstallStoreError> {
    for _ in 0..MAX_JOURNAL_STAGE_ATTEMPTS {
        let sequence = sequence.fetch_add(1, Ordering::Relaxed);
        let temporary = PathBuf::from(format!(
            ".{INSTALL_JOURNAL_FILE}.{}.{}",
            std::process::id(),
            sequence
        ));
        match directory.write_secret(&temporary, bytes) {
            Ok(()) => return Ok(temporary),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(InstallStoreError::WriteJournal(error)),
        }
    }
    Err(InstallStoreError::JournalStageCollisions {
        attempts: MAX_JOURNAL_STAGE_ATTEMPTS,
    })
}

#[derive(Debug)]
pub struct InstallLock {
    root: PathBuf,
    directory: ExclusiveDirectory,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallStoreError {
    #[error("failed to create install state directory: {0}")]
    CreateRoot(io::Error),
    #[error("failed to acquire install transaction authority: {0}")]
    AcquireLock(io::Error),
    #[error("install transaction lock is already held")]
    LockContended,
    #[error("install transaction authority belongs to another prefix")]
    WrongLock,
    #[error("failed to inspect active unit: {0}")]
    InspectActive(io::Error),
    #[error("invalid active unit link: {0}")]
    InvalidActiveLink(String),
    #[error("failed to switch active unit: {0}")]
    SwitchActive(io::Error),
    #[error("failed to open install journal without following links: {0}")]
    OpenJournal(io::Error),
    #[error("failed to read install journal: {0}")]
    ReadJournal(io::Error),
    #[error("install journal exceeds {limit} bytes")]
    JournalTooLarge { limit: usize },
    #[error("failed to decode install journal: {0}")]
    DecodeJournal(serde_json::Error),
    #[error("invalid install journal: {0}")]
    InvalidJournal(super::model::InstallModelError),
    #[error("failed to encode install journal: {0}")]
    EncodeJournal(serde_json::Error),
    #[error("failed to write staged install journal: {0}")]
    WriteJournal(io::Error),
    #[error("could not allocate a unique staged install journal after {attempts} attempts")]
    JournalStageCollisions { attempts: usize },
    #[error("failed to publish install journal: {0}")]
    ReplaceJournal(io::Error),
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;

    use super::{INSTALL_JOURNAL_FILE, InstallStore, stage_journal};

    #[test]
    fn stale_stage_from_reused_pid_does_not_block_journal_staging() {
        let root = tempfile::tempdir().expect("temporary install root");
        let store = InstallStore::new(root.path(), 1024);
        let lock = store.acquire_lock().expect("install authority");
        let sequence = AtomicU64::new(0);
        let stale = format!(".{INSTALL_JOURNAL_FILE}.{}.0", std::process::id());
        lock.directory
            .write_secret(stale.as_ref(), b"crash residue")
            .expect("seed stale journal stage");

        let staged = stage_journal(&lock.directory, b"current journal", &sequence)
            .expect("collision retry must stage the current journal");

        assert_eq!(
            staged,
            PathBuf::from(format!(".{INSTALL_JOURNAL_FILE}.{}.1", std::process::id()))
        );
    }
}
