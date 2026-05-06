use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncCursor {
    pub last_seen_seq: i64,
    pub last_sync_at: DateTime<Utc>,
}

impl SyncCursor {
    #[must_use]
    pub const fn new(last_seen_seq: i64, last_sync_at: DateTime<Utc>) -> Self {
        Self {
            last_seen_seq,
            last_sync_at,
        }
    }

    pub fn record_sync_result(&mut self, next_seq: i64, synced_at: DateTime<Utc>) {
        self.last_seen_seq = self.last_seen_seq.max(next_seq);
        self.last_sync_at = synced_at;
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Option<Self>, SyncCursorError> {
        match fs::read_to_string(path.as_ref()) {
            Ok(raw) => Ok(Some(toml::from_str(&raw)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), SyncCursorError> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }

        let tmp_path = tmp_path(path)?;
        fs::write(&tmp_path, toml::to_string_pretty(self)?)?;
        fs::rename(tmp_path, path)?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SyncCursorError {
    #[error("sync cursor path has no file name: {0}")]
    InvalidPath(PathBuf),
    #[error("sync cursor I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("sync cursor TOML decode failed: {0}")]
    Decode(#[from] toml::de::Error),
    #[error("sync cursor TOML encode failed: {0}")]
    Encode(#[from] toml::ser::Error),
}

fn tmp_path(path: &Path) -> Result<PathBuf, SyncCursorError> {
    let file_name = path
        .file_name()
        .ok_or_else(|| SyncCursorError::InvalidPath(path.to_owned()))?;
    let mut tmp_file_name = file_name.to_os_string();
    tmp_file_name.push(".tmp");

    Ok(path.with_file_name(tmp_file_name))
}
