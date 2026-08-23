use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use hypercolor_core::effect::RescanReport;
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::LayerSource;
use hypercolor_types::library::{EffectPlaylist, PlaylistItemTarget};
use hypercolor_types::scene::Zone;
use tokio::sync::{OwnedMutexGuard, OwnedRwLockWriteGuard, RwLock};

use super::{EffectContext, EffectRegistryPublication, EffectRegistryUpdate, InstalledEffect};
use crate::display_preferences::{
    AdmittedDisplayPreferencesEffectIdMigration, DisplayPreferencesEffectIdMigrationPublication,
    DisplayPreferencesStore, PersistedDisplayPreferencesEffectIdMigration,
};
use crate::domain::DomainError;
use crate::domain::context::PersistedRuntimeSessionEffectIdMigration;
use crate::domain::effect::IdentityMigrationPersistence;
use crate::domain::scene::SceneEffectIdMigrationPublication;
use crate::library::{
    AdmittedLibraryEffectIdMigration, LibraryEffectIdMigrationPublication, LibraryIdentityMigration,
};
use crate::playlist_runtime::PlaylistRuntimeState;

pub(crate) type EffectIdMigrations = HashMap<EffectId, EffectId>;

#[derive(Clone)]
pub(crate) struct EffectIdentityResources {
    display_preferences: Arc<RwLock<DisplayPreferencesStore>>,
    library_identity: Arc<dyn LibraryIdentityMigration>,
    playlist_runtime: Arc<tokio::sync::Mutex<PlaylistRuntimeState>>,
}

impl EffectIdentityResources {
    pub(crate) fn new(
        display_preferences: Arc<RwLock<DisplayPreferencesStore>>,
        library_identity: Arc<dyn LibraryIdentityMigration>,
        playlist_runtime: Arc<tokio::sync::Mutex<PlaylistRuntimeState>>,
    ) -> Self {
        Self {
            display_preferences,
            library_identity,
            playlist_runtime,
        }
    }
}

struct ActivePlaylistEffectIdMigration {
    generation: Option<u64>,
    playlist: Option<Arc<tokio::sync::RwLock<EffectPlaylist>>>,
    source: Option<EffectPlaylist>,
    candidate: Option<EffectPlaylist>,
    migrated: usize,
}

struct ActivePlaylistEffectIdMigrationPublication {
    _runtime: OwnedMutexGuard<PlaylistRuntimeState>,
    playlist: Option<OwnedRwLockWriteGuard<EffectPlaylist>>,
    candidate: Option<EffectPlaylist>,
    migrated: usize,
}

struct EffectIdMigrationPublication {
    registry: EffectRegistryPublication,
    scene: SceneEffectIdMigrationPublication,
    display: Option<DisplayPreferencesEffectIdMigrationPublication>,
    library: Option<Box<dyn LibraryEffectIdMigrationPublication>>,
    active_playlist: ActivePlaylistEffectIdMigrationPublication,
    _runtime: PersistedRuntimeSessionEffectIdMigration,
}

struct EffectIdMigrationPublicationParts {
    scene: SceneEffectIdMigrationPublication,
    display: Option<DisplayPreferencesEffectIdMigrationPublication>,
    library: Option<Box<dyn LibraryEffectIdMigrationPublication>>,
    active_playlist: ActivePlaylistEffectIdMigrationPublication,
    runtime: PersistedRuntimeSessionEffectIdMigration,
}

pub(crate) fn remap_effect_id(effect_id: &mut EffectId, migrations: &EffectIdMigrations) -> bool {
    let Some(canonical_id) = migrations.get(effect_id).copied() else {
        return false;
    };
    *effect_id = canonical_id;
    true
}

pub(crate) fn remap_zones(zones: &mut [Zone], migrations: &EffectIdMigrations) -> usize {
    zones
        .iter_mut()
        .flat_map(|zone| &mut zone.layers)
        .map(|layer| match &mut layer.source {
            LayerSource::Effect { effect_id, .. } => {
                usize::from(remap_effect_id(effect_id, migrations))
            }
            LayerSource::Media { .. }
            | LayerSource::ScreenRegion { .. }
            | LayerSource::WebViewport { .. }
            | LayerSource::ColorFill { .. } => 0,
        })
        .sum()
}

pub(crate) fn remap_playlist(
    playlist: &mut EffectPlaylist,
    migrations: &EffectIdMigrations,
) -> usize {
    playlist
        .items
        .iter_mut()
        .map(|item| match &mut item.target {
            PlaylistItemTarget::Effect { effect_id } => {
                usize::from(remap_effect_id(effect_id, migrations))
            }
            PlaylistItemTarget::Preset { .. } => 0,
        })
        .sum()
}

impl EffectContext {
    pub(crate) async fn rescan_registry(&self) -> Result<RescanReport, DomainError> {
        let update = self.prepare_rescan().await;
        self.apply_registry_update(update).await
    }

    pub(crate) async fn reload_registry_file(
        &self,
        path: &Path,
    ) -> Result<RescanReport, DomainError> {
        let update = self.prepare_reload(path).await;
        self.apply_registry_update(update).await
    }

    pub(crate) async fn install_registry_file(
        &self,
        path: &Path,
        raw_html: &str,
    ) -> Result<InstalledEffect, DomainError> {
        let (update, metadata, replaced_existing) = self.prepare_install(path, raw_html).await?;
        let source_path = match &metadata.source {
            hypercolor_types::effect::EffectSource::Html { path } => path.clone(),
            hypercolor_types::effect::EffectSource::Native { .. }
            | hypercolor_types::effect::EffectSource::Shader { .. } => path.to_path_buf(),
        };
        self.apply_registry_update(update).await?;
        Ok(InstalledEffect {
            metadata,
            source_path,
            replaced_existing,
        })
    }

    async fn apply_registry_update(
        &self,
        update: EffectRegistryUpdate,
    ) -> Result<RescanReport, DomainError> {
        let migrations = update.report().legacy_effect_ids.clone();
        if migrations.is_empty() {
            let mut publication = update.prepare_publication().await?;
            let report = publication.publish();
            if report.added > 0 || report.removed > 0 || report.updated > 0 {
                super::invalidate_active_zones(self).await?;
            }
            self.publish_registry_update(&report);
            return Ok(report);
        }

        let (parts, scene_migrated) = loop {
            let runtime = self.scene.begin_runtime_effect_id_migration().await;
            let scene = self.scene.prepare_effect_id_migration(&migrations).await?;
            let runtime = runtime.prepare(scene.candidate())?;
            let display = self
                .identity
                .display_preferences
                .read()
                .await
                .prepare_effect_id_migration(&migrations)
                .map_err(DomainError::Internal)?;
            let library = self
                .identity
                .library_identity
                .prepare_effect_id_migration(&migrations)
                .await
                .map_err(|error| DomainError::Internal(error.into()))?;
            let active_playlist = ActivePlaylistEffectIdMigration::prepare(
                &self.identity.playlist_runtime,
                &migrations,
            )
            .await;
            let scene_migrated = scene.migrated();

            let scene = scene.admit();
            let runtime = runtime.admit();
            let display =
                display.map(crate::display_preferences::DisplayPreferencesEffectIdMigration::admit);
            let mut library = library.map(crate::library::LibraryEffectIdMigration::admit);

            let (persisted_scene, scene_persistence) = scene.persist();
            let (persisted_runtime, runtime_persistence) = runtime.persist();
            let (persisted_display, display_persistence) = persist_display_migration(display);
            let library_persistence = library
                .as_mut()
                .map_or(IdentityMigrationPersistence::Written, |migration| {
                    migration.persist()
                });

            if [
                &scene_persistence,
                &runtime_persistence,
                &display_persistence,
                &library_persistence,
            ]
            .into_iter()
            .any(|outcome| matches!(outcome, IdentityMigrationPersistence::Superseded))
            {
                tokio::task::yield_now().await;
                continue;
            }

            #[cfg(test)]
            self.pause_before_identity_publication_for_test().await;
            let Ok(parts) = EffectIdMigrationPublicationParts::prepare(
                self,
                persisted_scene,
                persisted_runtime,
                persisted_display,
                library,
                active_playlist,
            )
            .await
            else {
                tokio::task::yield_now().await;
                continue;
            };
            break (parts, scene_migrated);
        };

        let registry = update.prepare_publication().await?;
        let publication = parts.with_registry(registry);
        let (report, scene_commit, library_migrated, active_playlist_migrated) =
            publication.publish(self).await;
        tracing::info!(
            mappings = migrations.len(),
            scene_references = scene_migrated,
            library_references = library_migrated,
            active_playlist_references = active_playlist_migrated,
            scene_revision = scene_commit.revision(),
            "Migrated late path-derived effect identities"
        );
        self.publish_registry_update(&report);
        Ok(report)
    }
}

fn persist_display_migration(
    migration: Option<AdmittedDisplayPreferencesEffectIdMigration>,
) -> (
    Option<PersistedDisplayPreferencesEffectIdMigration>,
    IdentityMigrationPersistence,
) {
    migration.map_or((None, IdentityMigrationPersistence::Written), |migration| {
        let (persisted, outcome) = migration.persist();
        (Some(persisted), outcome)
    })
}

impl ActivePlaylistEffectIdMigration {
    async fn prepare(
        playlist_runtime: &Arc<tokio::sync::Mutex<PlaylistRuntimeState>>,
        migrations: &EffectIdMigrations,
    ) -> Self {
        let active = {
            let runtime = playlist_runtime.lock().await;
            runtime
                .active
                .as_ref()
                .map(|active| (active.generation, Arc::clone(&active.playlist)))
        };
        let Some((generation, playlist)) = active else {
            return Self {
                generation: None,
                playlist: None,
                source: None,
                candidate: None,
                migrated: 0,
            };
        };
        let source = playlist.read().await.clone();
        let mut candidate = source.clone();
        let migrated = remap_playlist(&mut candidate, migrations);
        Self {
            generation: Some(generation),
            playlist: Some(playlist),
            source: Some(source),
            candidate: Some(candidate),
            migrated,
        }
    }

    async fn prepare_publication(
        self,
        playlist_runtime: &Arc<tokio::sync::Mutex<PlaylistRuntimeState>>,
    ) -> Result<ActivePlaylistEffectIdMigrationPublication, DomainError> {
        let runtime = Arc::clone(playlist_runtime).lock_owned().await;
        let current = runtime.active.as_ref();
        let identity_matches = match (self.generation, self.playlist.as_ref(), current) {
            (None, None, None) => true,
            (Some(generation), Some(playlist), Some(active)) => {
                active.generation == generation && Arc::ptr_eq(&active.playlist, playlist)
            }
            _ => false,
        };
        if !identity_matches {
            return Err(DomainError::conflict(
                "effect ID migration was superseded by newer playlist runtime state",
            ));
        }

        let playlist = match self.playlist {
            Some(playlist) => {
                let playlist = playlist.write_owned().await;
                if Some(&*playlist) != self.source.as_ref() {
                    return Err(DomainError::conflict(
                        "effect ID migration was superseded by a newer active playlist",
                    ));
                }
                Some(playlist)
            }
            None => None,
        };
        Ok(ActivePlaylistEffectIdMigrationPublication {
            _runtime: runtime,
            playlist,
            candidate: self.candidate,
            migrated: self.migrated,
        })
    }
}

impl ActivePlaylistEffectIdMigrationPublication {
    fn publish(&mut self) -> usize {
        if let (Some(playlist), Some(candidate)) = (self.playlist.as_mut(), self.candidate.take()) {
            **playlist = candidate;
        }
        self.migrated
    }
}

impl EffectIdMigrationPublicationParts {
    async fn prepare(
        effects: &EffectContext,
        scene: crate::domain::scene::PersistedSceneEffectIdMigration,
        runtime: PersistedRuntimeSessionEffectIdMigration,
        display: Option<PersistedDisplayPreferencesEffectIdMigration>,
        library: Option<Box<dyn AdmittedLibraryEffectIdMigration>>,
        active_playlist: ActivePlaylistEffectIdMigration,
    ) -> Result<Self, DomainError> {
        let scene = effects
            .scene
            .prepare_effect_id_migration_publication(scene)
            .await?;
        let display = match display {
            Some(display) => Some(
                DisplayPreferencesEffectIdMigrationPublication::prepare(
                    Arc::clone(&effects.identity.display_preferences),
                    display,
                )
                .await
                .map_err(DomainError::Internal)?,
            ),
            None => None,
        };
        let library = match library {
            Some(library) => Some(
                library
                    .prepare_publication()
                    .await
                    .map_err(|error| DomainError::Internal(error.into()))?,
            ),
            None => None,
        };
        let active_playlist = active_playlist
            .prepare_publication(&effects.identity.playlist_runtime)
            .await?;
        Ok(Self {
            scene,
            display,
            library,
            active_playlist,
            runtime,
        })
    }

    fn with_registry(self, registry: EffectRegistryPublication) -> EffectIdMigrationPublication {
        EffectIdMigrationPublication {
            registry,
            scene: self.scene,
            display: self.display,
            library: self.library,
            active_playlist: self.active_playlist,
            _runtime: self.runtime,
        }
    }
}

impl EffectIdMigrationPublication {
    async fn publish(
        mut self,
        effects: &EffectContext,
    ) -> (
        RescanReport,
        crate::domain::commit::SceneCommit,
        usize,
        usize,
    ) {
        let library_migrated = match self.library.as_mut() {
            Some(library) => library.publish().await,
            None => 0,
        };
        #[cfg(test)]
        effects.pause_between_identity_components_for_test().await;
        if let Some(display) = self.display.as_mut() {
            display.publish();
        }
        let active_playlist_migrated = self.active_playlist.publish();
        let scene_commit = effects.scene.publish_effect_id_migration(&mut self.scene);
        let report = self.registry.publish();
        (
            report,
            scene_commit,
            library_migrated,
            active_playlist_migrated,
        )
    }
}

#[cfg(test)]
#[path = "identity/tests.rs"]
mod tests;
