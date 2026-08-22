//! Per-display default face preferences (spec 69 §3.6).
//!
//! Stores the face a display should show whenever the active scene does not
//! target it, keyed by the device's fingerprint-stable [`DeviceId`]. The
//! daemon materializes each preference into a runtime-only default zone on
//! the scene manager; this store is only the persistence layer.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::BlendMode;
use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};

use crate::domain::effect::{EffectIdMigrations, remap_effect_id};
use crate::path_migration::{
    MigratedStore, MigrationOutcome, PathMigrationEntry, VersionedDocument, migrate,
};
use crate::persistence::{
    AdmittedAtomicWrite, AtomicFileWriter, AtomicWriteCommitResult, AtomicWriteOutcome,
    AtomicWriteReservation, serialize_json_pretty,
};

const STORE_SUBJECT: &str = "display preferences";

fn default_opacity() -> f32 {
    1.0
}

/// A display's stored default face.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayPreference {
    pub effect_id: EffectId,
    #[serde(default)]
    pub controls: HashMap<String, ControlValue>,
    #[serde(default)]
    pub blend_mode: BlendMode,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
}

/// JSON-file-backed store of per-display default faces.
#[derive(Debug)]
pub struct DisplayPreferencesStore {
    preferences: HashMap<DeviceId, DisplayPreference>,
    path: PathBuf,
    writer: AtomicFileWriter,
}

#[derive(Debug)]
pub(crate) struct DisplayPreferencesEffectIdMigration {
    source: HashMap<DeviceId, DisplayPreference>,
    candidate: HashMap<DeviceId, DisplayPreference>,
    write: AtomicWriteReservation,
    payload: Vec<u8>,
    migrated: usize,
}

#[derive(Debug)]
pub(crate) struct AdmittedDisplayPreferencesEffectIdMigration {
    source: HashMap<DeviceId, DisplayPreference>,
    candidate: HashMap<DeviceId, DisplayPreference>,
    write: AdmittedAtomicWrite,
    migrated: usize,
}

#[derive(Debug)]
pub(crate) struct PersistedDisplayPreferencesEffectIdMigration {
    source: HashMap<DeviceId, DisplayPreference>,
    candidate: HashMap<DeviceId, DisplayPreference>,
    migrated: usize,
}

pub(crate) struct DisplayPreferencesEffectIdMigrationPublication {
    store: OwnedRwLockWriteGuard<DisplayPreferencesStore>,
    candidate: Option<HashMap<DeviceId, DisplayPreference>>,
    migrated: usize,
}

impl DisplayPreferencesStore {
    /// Create an empty store for the given file path.
    ///
    /// # Errors
    ///
    /// Returns an error when the persistence destination cannot be prepared.
    pub fn new(path: PathBuf) -> anyhow::Result<Self> {
        let writer = AtomicFileWriter::new(&path).with_context(|| {
            format!(
                "failed to prepare display preferences store at {}",
                path.display()
            )
        })?;
        Ok(Self {
            preferences: HashMap::new(),
            path,
            writer,
        })
    }

    /// Load the store, returning an empty one when the file does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read or parsed.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Self::new(path.to_path_buf());
        }

        let raw = fs::read_to_string(path).with_context(|| {
            format!(
                "failed to read display preferences store at {}",
                path.display()
            )
        })?;
        let preferences: HashMap<DeviceId, DisplayPreference> = serde_json::from_str(&raw)
            .with_context(|| {
                format!(
                    "failed to parse display preferences store at {}",
                    path.display()
                )
            })?;

        Ok(Self {
            preferences,
            path: path.to_path_buf(),
            writer: AtomicFileWriter::new(path).with_context(|| {
                format!(
                    "failed to prepare display preferences store at {}",
                    path.display()
                )
            })?,
        })
    }

    /// Relocate a legacy data-tier file and open the state-tier store.
    ///
    /// # Errors
    ///
    /// Returns an error when either document is unreadable or invalid, the
    /// canonical destination cannot be prepared, or retirement fails after a
    /// durable import.
    pub fn load_migrated(
        legacy_path: &Path,
        canonical_path: &Path,
    ) -> anyhow::Result<(Self, MigrationOutcome)> {
        let writer = AtomicFileWriter::new(canonical_path).with_context(|| {
            format!(
                "failed to prepare display preferences store at {}",
                canonical_path.display()
            )
        })?;
        let entry = PathMigrationEntry::new(
            STORE_SUBJECT,
            legacy_path.to_path_buf(),
            canonical_path.to_path_buf(),
        );
        let migrated = migrate(&DisplayPreferencesCodec, &entry, &writer)?;
        let preferences = migrated.document.unwrap_or_default();
        Ok((
            Self {
                preferences,
                path: canonical_path.to_path_buf(),
                writer,
            },
            migrated.outcome,
        ))
    }

    /// Persist the store to its file path.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be created or the file
    /// cannot be written.
    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create display preferences directory {}",
                    parent.display()
                )
            })?;
        }
        let raw = serialize_json_pretty(&self.preferences)
            .context("failed to serialize display preferences")?;
        self.writer.write(&raw).with_context(|| {
            format!(
                "failed to persist display preferences at {}",
                self.path.display()
            )
        })?;
        Ok(())
    }

    #[must_use]
    pub fn get(&self, device_id: DeviceId) -> Option<&DisplayPreference> {
        self.preferences.get(&device_id)
    }

    /// Serialize and admit an assignment before changing the live store.
    ///
    /// # Errors
    ///
    /// Returns an error when the candidate snapshot cannot be serialized.
    pub fn set(
        &mut self,
        device_id: DeviceId,
        preference: DisplayPreference,
    ) -> anyhow::Result<()> {
        let mut candidate = self.preferences.clone();
        candidate.insert(device_id, preference);
        self.install_candidate(candidate)
    }

    /// Merge controls only while the stored assignment still matches the
    /// assignment whose effect schema admitted them.
    pub(crate) fn merge_controls_if_unchanged(
        &mut self,
        device_id: DeviceId,
        expected: &DisplayPreference,
        controls: &HashMap<String, ControlValue>,
    ) -> anyhow::Result<bool> {
        if self.preferences.get(&device_id) != Some(expected) {
            return Ok(false);
        }
        let mut updated = expected.clone();
        updated.controls.extend(controls.clone());
        self.set(device_id, updated)?;
        Ok(true)
    }

    /// Serialize and admit a removal before changing the live store.
    ///
    /// # Errors
    ///
    /// Returns an error when the candidate snapshot cannot be serialized.
    pub fn remove(&mut self, device_id: DeviceId) -> anyhow::Result<Option<DisplayPreference>> {
        let mut candidate = self.preferences.clone();
        let removed = candidate.remove(&device_id);
        if removed.is_some() {
            self.install_candidate(candidate)?;
        }
        Ok(removed)
    }

    pub fn iter(&self) -> impl Iterator<Item = (DeviceId, &DisplayPreference)> {
        self.preferences
            .iter()
            .map(|(device_id, preference)| (*device_id, preference))
    }

    /// Rewrite path-derived effect IDs before publishing stored preferences.
    pub fn migrate_effect_ids(&mut self, migrations: &EffectIdMigrations) -> anyhow::Result<usize> {
        let mut candidate = self.preferences.clone();
        let migrated = candidate
            .values_mut()
            .map(|preference| usize::from(remap_effect_id(&mut preference.effect_id, migrations)))
            .sum();
        if migrated == 0 {
            return Ok(0);
        }

        let payload = serialize_json_pretty(&candidate)
            .context("failed to serialize migrated display preferences")?;
        match self
            .writer
            .write(&payload)
            .context("failed to persist migrated display preferences")?
        {
            AtomicWriteOutcome::Written => {
                self.preferences = candidate;
                Ok(migrated)
            }
            AtomicWriteOutcome::Superseded => {
                anyhow::bail!("effect ID migration was superseded by newer display preferences")
            }
        }
    }

    pub(crate) fn prepare_effect_id_migration(
        &self,
        migrations: &EffectIdMigrations,
    ) -> anyhow::Result<Option<DisplayPreferencesEffectIdMigration>> {
        let mut candidate = self.preferences.clone();
        let migrated = candidate
            .values_mut()
            .map(|preference| usize::from(remap_effect_id(&mut preference.effect_id, migrations)))
            .sum();
        if migrated == 0 {
            return Ok(None);
        }
        let payload = serialize_json_pretty(&candidate)
            .context("failed to serialize migrated display preferences")?;
        Ok(Some(DisplayPreferencesEffectIdMigration {
            source: self.preferences.clone(),
            candidate,
            write: self.writer.reserve(),
            payload,
            migrated,
        }))
    }

    fn effect_id_migration_is_current(
        &self,
        migration: &PersistedDisplayPreferencesEffectIdMigration,
    ) -> bool {
        self.preferences == migration.source
    }

    fn install_candidate(
        &mut self,
        candidate: HashMap<DeviceId, DisplayPreference>,
    ) -> anyhow::Result<()> {
        let payload =
            serialize_json_pretty(&candidate).context("failed to serialize display preferences")?;
        let pending = self.writer.reserve().admit(payload);
        self.preferences = candidate;
        if let Err(error) = pending.commit() {
            tracing::warn!(
                path = %self.path.display(),
                %error,
                "Failed to persist display preferences; retry remains active"
            );
        }
        Ok(())
    }
}

impl DisplayPreferencesEffectIdMigration {
    pub(crate) fn admit(self) -> AdmittedDisplayPreferencesEffectIdMigration {
        AdmittedDisplayPreferencesEffectIdMigration {
            source: self.source,
            candidate: self.candidate,
            write: self.write.admit(self.payload),
            migrated: self.migrated,
        }
    }
}

impl AdmittedDisplayPreferencesEffectIdMigration {
    pub(crate) fn persist(
        self,
    ) -> (
        PersistedDisplayPreferencesEffectIdMigration,
        crate::domain::effect::IdentityMigrationPersistence,
    ) {
        let persistence = match self.write.commit_stage_aware() {
            AtomicWriteCommitResult::DurableWritten => {
                crate::domain::effect::IdentityMigrationPersistence::Written
            }
            AtomicWriteCommitResult::Superseded => {
                crate::domain::effect::IdentityMigrationPersistence::Superseded
            }
            AtomicWriteCommitResult::FailedBeforeReplacement(error)
            | AtomicWriteCommitResult::ReplacementVisibleButNotDurable(error) => {
                crate::domain::effect::IdentityMigrationPersistence::Retrying(error.to_string())
            }
        };
        (
            PersistedDisplayPreferencesEffectIdMigration {
                source: self.source,
                candidate: self.candidate,
                migrated: self.migrated,
            },
            persistence,
        )
    }
}

impl DisplayPreferencesEffectIdMigrationPublication {
    pub(crate) async fn prepare(
        store: std::sync::Arc<RwLock<DisplayPreferencesStore>>,
        migration: PersistedDisplayPreferencesEffectIdMigration,
    ) -> anyhow::Result<Self> {
        let store = store.write_owned().await;
        if !store.effect_id_migration_is_current(&migration) {
            anyhow::bail!("effect ID migration was superseded by newer display preferences");
        }
        Ok(Self {
            store,
            candidate: Some(migration.candidate),
            migrated: migration.migrated,
        })
    }

    pub(crate) fn publish(&mut self) -> usize {
        self.store.preferences = self
            .candidate
            .take()
            .expect("display migration publication must publish exactly once");
        self.migrated
    }
}

struct DisplayPreferencesCodec;

impl DisplayPreferencesCodec {
    fn read(
        path: &Path,
    ) -> anyhow::Result<VersionedDocument<HashMap<DeviceId, DisplayPreference>>> {
        let raw = fs::read_to_string(path).with_context(|| {
            format!(
                "failed to read display preferences store at {}",
                path.display()
            )
        })?;
        let preferences = serde_json::from_str(&raw).with_context(|| {
            format!(
                "failed to parse display preferences store at {}",
                path.display()
            )
        })?;
        Ok(VersionedDocument::unversioned(preferences))
    }
}

impl MigratedStore for DisplayPreferencesCodec {
    type Document = HashMap<DeviceId, DisplayPreference>;
    type Error = anyhow::Error;

    fn decode_current(
        &self,
        path: &Path,
    ) -> Result<VersionedDocument<Self::Document>, Self::Error> {
        Self::read(path)
    }

    fn decode_legacy(
        &self,
        path: &Path,
    ) -> Result<Option<VersionedDocument<Self::Document>>, Self::Error> {
        Self::read(path).map(Some)
    }

    fn encode(&self, document: &Self::Document) -> Result<Vec<u8>, Self::Error> {
        serialize_json_pretty(document).context("failed to serialize display preferences")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use hypercolor_types::control::ControlValue;
    use hypercolor_types::device::DeviceId;
    use hypercolor_types::effect::EffectId;
    use hypercolor_types::layer::BlendMode;
    use tempfile::TempDir;

    use super::{DisplayPreference, DisplayPreferencesStore};

    fn preference(effect_id: EffectId) -> DisplayPreference {
        DisplayPreference {
            effect_id,
            controls: HashMap::new(),
            blend_mode: BlendMode::Alpha,
            opacity: 1.0,
        }
    }

    #[test]
    fn control_merge_refuses_to_replace_a_changed_assignment() {
        let temp = TempDir::new().expect("tempdir");
        let mut store = DisplayPreferencesStore::new(temp.path().join("display-preferences.json"))
            .expect("store should open");
        let device_id = DeviceId::new();
        let stale = preference(EffectId::new(uuid::Uuid::now_v7()));
        let current = preference(EffectId::new(uuid::Uuid::now_v7()));
        store
            .set(device_id, current.clone())
            .expect("current preference should persist");

        assert!(
            !store
                .merge_controls_if_unchanged(
                    device_id,
                    &stale,
                    &HashMap::from([("speed".into(), ControlValue::Float(0.5))]),
                )
                .expect("stale merge should be rejected")
        );
        assert_eq!(store.get(device_id), Some(&current));

        assert!(
            store
                .merge_controls_if_unchanged(
                    device_id,
                    &current,
                    &HashMap::from([("speed".into(), ControlValue::Float(0.5))]),
                )
                .expect("current merge should persist")
        );
        assert_eq!(
            store
                .get(device_id)
                .and_then(|preference| preference.controls.get("speed")),
            Some(&ControlValue::Float(0.5))
        );
    }

    #[test]
    fn effect_id_migration_is_durable_before_preferences_are_published() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("display-preferences.json");
        let device_id = DeviceId::new();
        let legacy_id = EffectId::new(uuid::Uuid::now_v7());
        let canonical_id = EffectId::new(uuid::Uuid::now_v7());
        let mut store = DisplayPreferencesStore::new(path.clone()).expect("store should open");
        store
            .set(
                device_id,
                DisplayPreference {
                    effect_id: legacy_id,
                    controls: HashMap::new(),
                    blend_mode: BlendMode::Alpha,
                    opacity: 1.0,
                },
            )
            .expect("preference should persist");

        assert_eq!(
            store
                .migrate_effect_ids(&HashMap::from([(legacy_id, canonical_id)]))
                .expect("migration should persist"),
            1
        );
        assert_eq!(
            store.get(device_id).map(|preference| preference.effect_id),
            Some(canonical_id)
        );
        drop(store);

        let reopened = DisplayPreferencesStore::load(&path).expect("store should reopen");
        assert_eq!(
            reopened
                .get(device_id)
                .map(|preference| preference.effect_id),
            Some(canonical_id)
        );
        assert!(
            !std::fs::read_to_string(path)
                .expect("preference file should read")
                .contains(&legacy_id.to_string())
        );
    }
}
