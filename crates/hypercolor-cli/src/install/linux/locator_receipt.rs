use std::path::PathBuf;

use hypercolor_platform_fs::ExactEntry;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::super::InstallJournalV1;
use super::locator::LinuxLocatorError;

pub(super) const MAX_PREPARATION_BYTES: u64 = (super::super::MAX_INSTALL_JOURNAL_BYTES * 2) as u64;

pub(super) const RECEIPT_NAME: &str = "adoption-preparation.json";

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AdoptionPreparation {
    schema_version: u32,
    installation_id: Uuid,
    journal_sha256: [u8; 32],
    pub(super) initial_journal: InstallJournalV1,
    legacy_journal: RecordedEntry,
    legacy_active: RecordedEntry,
}

impl AdoptionPreparation {
    pub(super) fn capture(
        installation_id: Uuid,
        journal: &InstallJournalV1,
        legacy_journal: &ExactEntry,
        legacy_active: &ExactEntry,
    ) -> Result<Self, LinuxLocatorError> {
        if !matches!(
            legacy_journal,
            ExactEntry::Absent | ExactEntry::RegularFile { .. }
        ) || !matches!(
            legacy_active,
            ExactEntry::Absent | ExactEntry::Symlink { .. }
        ) {
            return Err(LinuxLocatorError::InvalidLocator);
        }
        Ok(Self {
            schema_version: 2,
            installation_id,
            journal_sha256: Sha256::digest(serde_json::to_vec(journal)?).into(),
            initial_journal: journal.clone(),
            legacy_journal: RecordedEntry::from(legacy_journal),
            legacy_active: RecordedEntry::from(legacy_active),
        })
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
enum RecordedEntry {
    Absent,
    File {
        mode: u32,
        size: u64,
        sha256: [u8; 32],
        device: u64,
        inode: u64,
    },
    Symlink {
        target: PathBuf,
        device: u64,
        inode: u64,
    },
}

impl From<&ExactEntry> for RecordedEntry {
    fn from(value: &ExactEntry) -> Self {
        match value {
            ExactEntry::Absent => Self::Absent,
            ExactEntry::RegularFile {
                mode,
                size,
                sha256,
                device,
                inode,
            } => Self::File {
                mode: *mode,
                size: *size,
                sha256: *sha256,
                device: *device,
                inode: *inode,
            },
            ExactEntry::Symlink {
                target,
                device,
                inode,
            } => Self::Symlink {
                target: target.clone(),
                device: *device,
                inode: *inode,
            },
        }
    }
}
