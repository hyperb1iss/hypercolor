//! File transfer staging around the shared restore transaction.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Arc;

use hypercolor_core::portable_setup::archive::{read_archive, write_archive};
use hypercolor_types::api::backups::{BackupApplyResponse, BackupPreview};
use hypercolor_types::portable_setup::{PortableBackupV1, PortablePreferences};
use serde::Serialize;
use uuid::Uuid;

use super::export::ProjectionIdentity;
use super::{PortableSetupService, RestoreMode, RestoreOptions, SetupIdentity, internal};
use crate::domain::DomainError;
use crate::portable_setup_state::StagedBackup;

impl PortableSetupService {
    /// Spool transfers beside setup data, avoiding memory-backed system temp directories.
    pub fn archive_upload_file(&self) -> Result<File, DomainError> {
        tempfile::tempfile_in(&self.root).map_err(internal)
    }

    /// Create a disk-backed download without retaining archive bytes in memory.
    pub async fn export_archive(&self) -> Result<File, DomainError> {
        let identity = self.archive_identity().await?;
        self.reconcile_projection(ProjectionIdentity::CurrentNative(identity.clone()))
            .await?;
        let bindings = self.assign_bindings(identity.setup_id, None).await?;
        let preferences = self.backup_preferences();
        let tui_preferences = Some(self.backup_tui_preferences().await?);
        let favorites = self.state.library_store().list_favorites().await;
        let snapshot = self
            .export_projection(ProjectionIdentity::CurrentNative(identity), &bindings)
            .await?;
        if preferences != self.backup_preferences()
            || tui_preferences != Some(self.backup_tui_preferences().await?)
            || favorites != self.state.library_store().list_favorites().await
        {
            return Err(DomainError::conflict(
                "lighting preferences changed during backup",
            ));
        }
        let backup = PortableBackupV1 {
            schema_version: 1,
            setup: snapshot.manifest,
            preferences,
            tui_preferences,
            favorites,
        };
        let blobs = self.blobs.clone();
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            let mut file = tempfile::tempfile_in(root).map_err(internal)?;
            write_archive(&mut file, &backup, &blobs).map_err(internal)?;
            file.seek(SeekFrom::Start(0)).map_err(internal)?;
            Ok(file)
        })
        .await
        .map_err(internal)?
    }

    /// Validate an uploaded archive and pin the destination revision shown in
    /// the confirmation. No authored destination records change at this stage.
    pub async fn preview_archive(&self, source: File) -> Result<BackupPreview, DomainError> {
        let blobs = self.blobs.clone();
        let mut backup = tokio::task::spawn_blocking(move || read_archive(source, &blobs))
            .await
            .map_err(internal)?
            .map_err(|error| DomainError::validation(error.to_string()))?;
        let identity = self.archive_identity().await?;
        self.reconcile_projection(ProjectionIdentity::CurrentNative(identity.clone()))
            .await?;
        backup.setup.setup_id = identity.setup_id;
        let bindings = self
            .assign_bindings(backup.setup.setup_id, Some(&backup.setup))
            .await?;
        let current = self
            .export(
                SetupIdentity {
                    setup_id: backup.setup.setup_id,
                    name: backup.setup.name.clone(),
                },
                &bindings,
            )
            .await?;
        let operation_id = Uuid::now_v7();
        let preview = BackupPreview {
            operation_id,
            name: backup.setup.name.clone(),
            scenes: backup.setup.scenes.len() as u64,
            layouts: backup.setup.layouts.len() as u64,
            effects: backup.setup.effects.len() as u64,
            media: backup.setup.assets.len() as u64,
            favorites: backup.favorites.len() as u64,
            includes_preferences: backup.preferences.is_some() || backup.tui_preferences.is_some(),
            unresolved_devices: backup
                .setup
                .devices
                .iter()
                .filter(|(id, _)| bindings.local_device(**id).is_none())
                .map(|(_, device)| device.name.clone())
                .collect(),
        };
        let path = self.root.join(format!("backup-{operation_id}.json"));
        let expected_preferences = backup
            .preferences
            .as_ref()
            .and_then(|_| self.backup_preferences());
        let expected_tui_preferences = if backup.tui_preferences.is_some() {
            Some(self.backup_tui_preferences().await?)
        } else {
            None
        };
        let staged = StagedBackup {
            backup,
            expected_preferences,
            expected_tui_preferences,
            bindings,
            expected_revision: current.revision,
        };
        tokio::task::spawn_blocking(move || write_staging(&path, &staged))
            .await
            .map_err(internal)??;
        Ok(preview)
    }

    /// Replays the same staged operation after a lost response. The shared
    /// coordinator owns its durable receipt and all publication fences.
    pub async fn apply_archive(
        &self,
        operation_id: Uuid,
    ) -> Result<BackupApplyResponse, DomainError> {
        let path = self.root.join(format!("backup-{operation_id}.json"));
        let staged: StagedBackup = tokio::task::spawn_blocking(move || {
            let file = hypercolor_platform_fs::open_no_follow(&path).map_err(internal)?;
            serde_json::from_reader(file).map_err(internal)
        })
        .await
        .map_err(internal)??;
        let report = self
            .restore_backup_checked(
                &staged.backup,
                &staged.bindings,
                RestoreOptions {
                    operation_id,
                    expected_revision: staged.expected_revision,
                    mode: RestoreMode::Merge,
                    apply_selection: true,
                },
                super::BackupRestoreExpectation {
                    preferences: staged.expected_preferences,
                    tui_preferences: staged.expected_tui_preferences,
                },
            )
            .await?;
        Ok(BackupApplyResponse {
            operation_id,
            unresolved_devices: report
                .unresolved_bindings
                .iter()
                .filter_map(|id| {
                    staged
                        .backup
                        .setup
                        .devices
                        .get(id)
                        .map(|device| device.name.clone())
                })
                .collect(),
        })
    }

    pub(super) fn tui_preference_store(
        &self,
    ) -> hypercolor_core::client_preferences::TuiPreferenceStore {
        self.state.tui_preferences.clone()
    }

    async fn backup_tui_preferences(
        &self,
    ) -> Result<hypercolor_types::client_preferences::TuiPreferences, DomainError> {
        let store = self.tui_preference_store();
        let runtime = Arc::clone(&self.restore_runtime);
        tokio::task::spawn_blocking(move || {
            store
                .load_while(|| !runtime.closed.load(std::sync::atomic::Ordering::Acquire))
                .map_err(internal)
        })
        .await
        .map_err(internal)?
    }

    fn backup_preferences(&self) -> Option<PortablePreferences> {
        self.state
            .config_manager
            .as_ref()
            .map(|manager| PortablePreferences::from_config(&manager.get()))
    }

    async fn archive_identity(&self) -> Result<SetupIdentity, DomainError> {
        let _gate = self.association_gate.lock().await;
        let path = self.root.join("manual-backup-identity.json");
        match hypercolor_platform_fs::open_no_follow(&path) {
            Ok(mut file) => {
                let mut bytes = String::new();
                file.read_to_string(&mut bytes).map_err(internal)?;
                serde_json::from_str(&bytes).map_err(internal)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let identity = SetupIdentity {
                    setup_id: Uuid::now_v7(),
                    name: "My lighting".into(),
                };
                write_staging(&path, &identity)?;
                Ok(identity)
            }
            Err(error) => Err(internal(error)),
        }
    }
}

fn write_staging(path: &std::path::Path, value: &impl Serialize) -> Result<(), DomainError> {
    let parent = path
        .parent()
        .ok_or_else(|| DomainError::validation("backup staging directory is missing"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(internal)?;
    serde_json::to_writer(&mut temporary, value).map_err(internal)?;
    temporary.flush().map_err(internal)?;
    temporary.as_file().sync_all().map_err(internal)?;
    hypercolor_platform_fs::durable_replace(temporary.path(), path).map_err(internal)
}
