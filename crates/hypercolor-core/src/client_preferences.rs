//! Terminal preference persistence shared by the terminal and backup restore.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use hypercolor_types::client_preferences::TuiPreferences;

use crate::persistence::{AtomicWriteWaitOutcome, PersistenceShutdown};

#[derive(Debug, Clone)]
pub struct TuiPreferenceStore {
    path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
#[error("terminal preferences changed after backup confirmation")]
pub struct TuiPreferenceConflict;

impl TuiPreferenceStore {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The terminal and backup service resolve the same local presentation file.
    #[must_use]
    pub fn canonical() -> Self {
        Self::new(tui_config_path())
    }

    pub fn load(&self) -> Result<TuiPreferences> {
        self.load_while(|| true)
    }

    pub fn load_while(&self, keep_waiting: impl Fn() -> bool) -> Result<TuiPreferences> {
        let _lock = self.lock_while(true, keep_waiting)?;
        Ok(read_document(&self.path)?.1)
    }

    /// Read and modify under the same cross-process lock used by restore.
    /// Changes to one field retain other fields and unknown local TOML keys.
    pub fn modify(&self, mutate: impl FnOnce(&mut TuiPreferences)) -> Result<()> {
        let lock = self.lock_while(false, || true)?;
        let (document, mut candidate) = read_document(&self.path)?;
        mutate(&mut candidate);
        let mut prepared = self.prepare_locked(lock, document, &candidate)?;
        match prepared.persist(&PersistenceShutdown::default())? {
            AtomicWriteWaitOutcome::Written => Ok(()),
            outcome => bail!("terminal preferences were not persisted: {outcome:?}"),
        }
    }

    /// Hold the ordinary writer lock through the host transaction. Recovery
    /// accepts only the journal's before or candidate values.
    pub fn prepare(
        &self,
        expected: &TuiPreferences,
        candidate: &TuiPreferences,
        recovering: bool,
        shutdown: &PersistenceShutdown,
    ) -> Result<PreparedTuiPreferences> {
        let lock = self.lock_while(false, || !shutdown.is_shutdown())?;
        let (document, current) = read_document(&self.path)?;
        if current != *expected && !(recovering && current == *candidate) {
            return Err(TuiPreferenceConflict.into());
        }
        self.prepare_locked(lock, document, candidate)
    }

    fn lock_while(&self, shared: bool, keep_waiting: impl Fn() -> bool) -> Result<File> {
        let lock = self.open_lock()?;
        loop {
            if !keep_waiting() {
                bail!("terminal preference admission stopped");
            }
            let result = if shared {
                lock.try_lock_shared()
            } else {
                lock.try_lock()
            };
            match result {
                Ok(()) => return Ok(lock),
                Err(TryLockError::WouldBlock) => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    }

    fn open_lock(&self) -> Result<File> {
        let parent = self
            .path
            .parent()
            .context("terminal preference directory missing")?;
        std::fs::create_dir_all(parent)?;
        let mut name = self.path.as_os_str().to_os_string();
        name.push(".lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(PathBuf::from(name))?;
        Ok(lock)
    }

    fn prepare_locked(
        &self,
        lock: File,
        mut document: toml::Table,
        candidate: &TuiPreferences,
    ) -> Result<PreparedTuiPreferences> {
        match &candidate.theme {
            Some(theme) => {
                document.insert("theme".into(), toml::Value::String(theme.clone()));
            }
            None => {
                document.remove("theme");
            }
        }
        document.insert(
            "show_donate".into(),
            toml::Value::Boolean(candidate.show_donate),
        );
        let payload = toml::to_string_pretty(&document)?.into_bytes();

        Ok(PreparedTuiPreferences {
            _lock: lock,
            path: self.path.clone(),
            payload,
            durable: false,
        })
    }
}

/// The lockfile is stable across atomic data-file replacements and closes on
/// drop, including process termination. No file writes precede `persist`.
pub struct PreparedTuiPreferences {
    _lock: File,
    path: PathBuf,
    payload: Vec<u8>,
    durable: bool,
}

impl PreparedTuiPreferences {
    pub fn persist(&mut self, shutdown: &PersistenceShutdown) -> Result<AtomicWriteWaitOutcome> {
        if self.durable {
            return Ok(AtomicWriteWaitOutcome::Written);
        }
        if shutdown.is_shutdown() {
            return Ok(AtomicWriteWaitOutcome::ShutdownPending);
        }
        let parent = self
            .path
            .parent()
            .context("terminal preference directory missing")?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&self.payload)?;
        temporary.as_file().sync_all()?;
        hypercolor_platform_fs::durable_replace(temporary.path(), &self.path)?;
        self.durable = true;
        Ok(AtomicWriteWaitOutcome::Written)
    }
}

fn read_document(path: &Path) -> Result<(toml::Table, TuiPreferences)> {
    let document: toml::Table = match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).context("parse terminal preferences")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(error) => return Err(error.into()),
    };
    let theme = document
        .get("theme")
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .context("terminal theme must be a string")
        })
        .transpose()?;
    let show_donate = document
        .get("show_donate")
        .map(|value| {
            value
                .as_bool()
                .context("terminal sponsor visibility must be a boolean")
        })
        .transpose()?
        .unwrap_or(true);
    Ok((document, TuiPreferences { theme, show_donate }))
}

/// Resolve the terminal's own local preferences, independent of the engine's
/// optional custom configuration file.
#[must_use]
pub fn tui_config_path() -> PathBuf {
    std::env::var_os("HYPERCOLOR_TUI_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::paths::config_dir().join("tui.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn field_mutation_preserves_latest_preferences_and_unknown_local_fields() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("tui.toml");
        std::fs::write(&path, "show_donate = false\nfuture_setting = 9\n").expect("fixture");
        let store = TuiPreferenceStore::new(path.clone());
        store
            .modify(|value| value.theme = Some("silkcircuit".into()))
            .expect("save theme");
        assert_eq!(
            store.load().expect("load"),
            TuiPreferences {
                theme: Some("silkcircuit".into()),
                show_donate: false
            }
        );
        let disk: toml::Table =
            toml::from_str(&std::fs::read_to_string(path).expect("read")).expect("TOML");
        assert_eq!(disk["future_setting"].as_integer(), Some(9));
    }

    #[test]
    fn prepared_restore_fences_other_writers_and_abort_changes_nothing() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("tui.toml");
        let store = TuiPreferenceStore::new(path.clone());
        let before = store.load().expect("load");
        let desired = TuiPreferences {
            theme: Some("violet".into()),
            show_donate: false,
        };
        let prepared = store
            .prepare(&before, &desired, false, &PersistenceShutdown::default())
            .expect("prepare");
        let (send, receive) = mpsc::channel();
        let other = store.clone();
        let worker = std::thread::spawn(move || {
            other
                .modify(|value| value.theme = Some("later".into()))
                .expect("ordinary save");
            send.send(()).expect("completion");
        });
        assert!(receive.recv_timeout(Duration::from_millis(30)).is_err());
        assert!(!path.exists());
        drop(prepared);
        receive
            .recv_timeout(Duration::from_secs(2))
            .expect("writer released");
        worker.join().expect("join");
        assert_eq!(
            store.load().expect("load"),
            TuiPreferences {
                theme: Some("later".into()),
                show_donate: true
            }
        );
    }

    #[test]
    fn recovery_accepts_only_before_or_candidate_values() {
        let dir = tempfile::tempdir().expect("directory");
        let store = TuiPreferenceStore::new(dir.path().join("tui.toml"));
        let before = store.load().expect("load");
        let desired = TuiPreferences {
            theme: Some("restored".into()),
            show_donate: false,
        };
        let mut prepared = store
            .prepare(&before, &desired, false, &PersistenceShutdown::default())
            .expect("prepare");
        assert_eq!(
            prepared
                .persist(&PersistenceShutdown::default())
                .expect("persist"),
            AtomicWriteWaitOutcome::Written
        );
        drop(prepared);
        drop(
            store
                .prepare(&before, &desired, true, &PersistenceShutdown::default())
                .expect("partial commit recovery"),
        );
        assert!(
            store
                .prepare(&before, &desired, false, &PersistenceShutdown::default())
                .is_err()
        );
        store
            .modify(|value| value.theme = Some("new edit".into()))
            .expect("local edit");
        assert!(
            store
                .prepare(&before, &desired, true, &PersistenceShutdown::default())
                .is_err()
        );
    }

    #[test]
    fn shutdown_releases_waiting_readers_and_restore_admission() {
        let dir = tempfile::tempdir().expect("directory");
        let store = TuiPreferenceStore::new(dir.path().join("tui.toml"));
        let before = store.load().expect("load");
        let held = store
            .prepare(&before, &before, false, &PersistenceShutdown::default())
            .expect("hold writer");
        let shutdown = PersistenceShutdown::default();
        let mut workers = Vec::new();
        let (send, receive) = mpsc::channel();
        for read in [true, false] {
            let store = store.clone();
            let shutdown = shutdown.clone();
            let before = before.clone();
            let send = send.clone();
            workers.push(std::thread::spawn(move || {
                let stopped = if read {
                    store.load_while(|| !shutdown.is_shutdown()).is_err()
                } else {
                    store.prepare(&before, &before, false, &shutdown).is_err()
                };
                send.send(stopped).expect("completion");
            }));
        }
        assert!(receive.recv_timeout(Duration::from_millis(30)).is_err());
        shutdown.writers_stopped();
        for _ in 0..2 {
            assert!(
                receive
                    .recv_timeout(Duration::from_secs(2))
                    .expect("waiter exits")
            );
        }
        for worker in workers {
            worker.join().expect("join");
        }
        drop(held);
    }
}
