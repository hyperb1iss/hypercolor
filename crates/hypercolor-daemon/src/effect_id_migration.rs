use std::collections::HashMap;
use std::path::Path;

use hypercolor_core::effect::RescanReport;
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::LayerSource;
use hypercolor_types::library::{EffectPlaylist, PlaylistItemTarget};
use hypercolor_types::scene::Zone;

use crate::app_state::AppState;
use crate::domain::DomainError;
use crate::domain::effect::EffectRegistryUpdate;

pub(crate) type EffectIdMigrations = HashMap<EffectId, EffectId>;

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

pub(crate) async fn rescan_registry(state: &AppState) -> Result<RescanReport, DomainError> {
    let update = state.domains.effects.prepare_rescan().await;
    apply_registry_update(state, update).await
}

pub(crate) async fn reload_registry_file(
    state: &AppState,
    path: &Path,
) -> Result<RescanReport, DomainError> {
    let update = state.domains.effects.prepare_reload(path).await;
    apply_registry_update(state, update).await
}

async fn apply_registry_update(
    state: &AppState,
    update: EffectRegistryUpdate<'_>,
) -> Result<RescanReport, DomainError> {
    let migrations = update.report().legacy_effect_ids.clone();
    if migrations.is_empty() {
        let report = update.prepare_publication().await?.publish();
        if report.added > 0 || report.removed > 0 || report.updated > 0 {
            crate::domain::effect::invalidate_active_zones(&state.domains.effects).await?;
        }
        return Ok(report);
    }

    let scene = state
        .scene_manager
        .prepare_effect_id_migration(&migrations)
        .await?;
    let runtime = state
        .domains
        .runtime_session
        .prepare_effect_id_migration(scene.candidate())?;
    let display = state
        .display_preferences
        .read()
        .await
        .prepare_effect_id_migration(&migrations)
        .map_err(DomainError::Internal)?;
    let mut library = state
        .library_store
        .prepare_effect_id_migration(&migrations)
        .await
        .map_err(|error| DomainError::Internal(error.into()))?;

    let scene_migrated = scene.migrated();
    let persisted_scene = scene.persist().map_err(DomainError::Internal)?;
    runtime.persist().map_err(DomainError::Internal)?;
    let persisted_display = display
        .map(crate::display_preferences::DisplayPreferencesEffectIdMigration::persist)
        .transpose()
        .map_err(DomainError::Internal)?;
    if let Some(migration) = library.as_mut() {
        migration
            .persist()
            .map_err(|error| DomainError::Internal(error.into()))?;
    }

    let publication = update.prepare_publication().await?;
    let scene_commit = state
        .scene_manager
        .install_effect_id_migration(persisted_scene)
        .await?;
    if let Some(migration) = persisted_display {
        state
            .display_preferences
            .write()
            .await
            .install_effect_id_migration(migration)
            .map_err(DomainError::Internal)?;
    }
    let library_migrated = match library {
        Some(migration) => migration
            .install()
            .await
            .map_err(|error| DomainError::Internal(error.into()))?,
        None => 0,
    };

    let active_playlist = {
        let runtime = state.playlist_runtime.lock().await;
        runtime
            .active
            .as_ref()
            .map(|active| std::sync::Arc::clone(&active.playlist))
    };
    let active_playlist_migrated = if let Some(playlist) = active_playlist {
        let mut playlist = playlist.write().await;
        remap_playlist(&mut playlist, &migrations)
    } else {
        0
    };

    let report = publication.publish();
    tracing::info!(
        mappings = migrations.len(),
        scene_references = scene_migrated,
        library_references = library_migrated,
        active_playlist_references = active_playlist_migrated,
        scene_revision = scene_commit.revision(),
        "Migrated late path-derived effect identities"
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use hypercolor_core::scene::SceneManager;
    use hypercolor_types::device::DeviceId;
    use hypercolor_types::effect::EffectId;
    use hypercolor_types::layer::{SceneLayer, SceneLayerId};
    use hypercolor_types::library::{
        EffectPlaylist, EffectPreset, PlaylistId, PlaylistItem, PlaylistItemId, PlaylistItemTarget,
        PresetId,
    };
    use hypercolor_types::scene::{
        DisplayFaceBlendMode, DisplayFaceTarget, SceneId, SceneKind, SceneMutationMode, ZoneRole,
    };
    use tempfile::TempDir;

    use super::{reload_registry_file, remap_zones, rescan_registry};
    use crate::app_state::AppState;
    use crate::display_preferences::DisplayPreference;
    use crate::library::JsonLibraryStore;
    use crate::playlist_runtime::ActivePlaylistRuntime;

    struct LateMigrationFixture {
        state: AppState,
        effect_path: PathBuf,
        legacy_id: EffectId,
        canonical_id: EffectId,
        device_id: DeviceId,
        preset_id: PresetId,
        playlist_id: PlaylistId,
    }

    fn deterministic_html_effect_id(key: &str) -> EffectId {
        let mut hash: u128 = 0x6c62_69f0_7bb0_14d9_8d4f_1283_7ec6_3b8b;
        for byte in key.bytes() {
            hash ^= u128::from(byte);
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
        let mut bytes = hash.to_be_bytes();
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        EffectId::new(uuid::Uuid::from_bytes(bytes))
    }

    fn write_effect(path: &Path, title: &str) {
        std::fs::create_dir_all(path.parent().expect("effect should have a parent"))
            .expect("effect directory should be created");
        std::fs::write(
            path,
            format!(
                "<head><title>{title}</title><meta description=\"late\" /><meta publisher=\"Hypercolor\" /></head>"
            ),
        )
        .expect("effect should be written");
    }

    async fn late_migration_fixture(temp: &TempDir) -> LateMigrationFixture {
        let data_dir = temp.path().join("state");
        let mut state = AppState::new_with_data_dir(data_dir.clone());
        state.library_store = Arc::new(
            JsonLibraryStore::open(data_dir.join("library.json")).expect("library should open"),
        );
        let effect_path = data_dir.join("effects/bundled/late-arrival.html");
        write_effect(&effect_path, "Late Arrival");
        let source_path = std::fs::canonicalize(&effect_path).expect("effect should canonicalize");
        let legacy_id =
            deterministic_html_effect_id(&format!("hypercolor:html:{}", source_path.display()));
        let canonical_id = deterministic_html_effect_id("hypercolor:html-bundled:late-arrival");

        let device_id = DeviceId::new();
        let mut mutation = state.scene_manager.begin_mutation().await;
        let default_zone_id = mutation
            .scenes()
            .get(&SceneId::DEFAULT)
            .and_then(|scene| scene.zones.first())
            .map(|zone| zone.id)
            .expect("default scene should have a zone");
        mutation
            .insert_layer(
                SceneId::DEFAULT,
                default_zone_id,
                SceneLayer::from_effect(
                    SceneLayerId::new(),
                    legacy_id,
                    HashMap::new(),
                    HashMap::new(),
                    None,
                ),
                None,
                None,
            )
            .expect("legacy layer should be inserted");
        let mut named_scene = mutation
            .scenes()
            .get(&SceneId::DEFAULT)
            .cloned()
            .expect("default scene should exist");
        named_scene.id = SceneId::new();
        named_scene.name = "Imported Legacy".to_owned();
        named_scene.kind = SceneKind::Named;
        named_scene.mutation_mode = SceneMutationMode::Live;
        mutation
            .create_scene(named_scene)
            .expect("named scene should be created");
        let mut overlay = mutation
            .scenes()
            .get(&SceneId::DEFAULT)
            .and_then(|scene| scene.zones.first())
            .cloned()
            .expect("default zone should exist");
        overlay.id = hypercolor_types::scene::ZoneId::new();
        overlay.name = "Legacy Face".to_owned();
        overlay.role = ZoneRole::Display;
        overlay.display_target = Some(DisplayFaceTarget::new(device_id));
        assert!(mutation.set_default_display_zone(overlay));
        state
            .scene_manager
            .commit_mutation(mutation)
            .await
            .expect("legacy scene should commit");

        state
            .display_preferences
            .write()
            .await
            .set(
                device_id,
                DisplayPreference {
                    effect_id: legacy_id,
                    controls: HashMap::new(),
                    blend_mode: DisplayFaceBlendMode::Alpha,
                    opacity: 1.0,
                },
            )
            .expect("display preference should persist");
        state
            .library_store
            .upsert_favorite(legacy_id, 10)
            .await
            .expect("favorite should persist");
        let preset_id = PresetId::new();
        state
            .library_store
            .insert_preset(EffectPreset {
                id: preset_id,
                name: "Legacy preset".to_owned(),
                description: None,
                effect_id: legacy_id,
                controls: HashMap::new(),
                tags: Vec::new(),
                created_at_ms: 10,
                updated_at_ms: 10,
            })
            .await
            .expect("preset should persist");
        let playlist_id = PlaylistId::new();
        let playlist = EffectPlaylist {
            id: playlist_id,
            name: "Legacy playlist".to_owned(),
            description: None,
            items: vec![PlaylistItem {
                id: PlaylistItemId::new(),
                target: PlaylistItemTarget::Effect {
                    effect_id: legacy_id,
                },
                duration_ms: Some(60_000),
                transition_ms: None,
            }],
            loop_enabled: true,
            created_at_ms: 10,
            updated_at_ms: 10,
        };
        state
            .library_store
            .insert_playlist(playlist.clone())
            .await
            .expect("playlist should persist");
        let (stop_tx, _stop_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(std::future::pending());
        state.playlist_runtime.lock().await.active = Some(ActivePlaylistRuntime {
            generation: 1,
            playlist_id,
            playlist_name: playlist.name.clone(),
            loop_enabled: playlist.loop_enabled,
            item_count: playlist.items.len(),
            started_at_ms: 10,
            stop_tx,
            playlist: Arc::new(tokio::sync::RwLock::new(playlist)),
            task,
        });

        LateMigrationFixture {
            state,
            effect_path,
            legacy_id,
            canonical_id,
            device_id,
            preset_id,
            playlist_id,
        }
    }

    #[test]
    fn remaps_effect_layers_without_touching_other_layer_state() {
        let legacy_id = EffectId::new(uuid::Uuid::now_v7());
        let canonical_id = EffectId::new(uuid::Uuid::now_v7());
        let mut zone = SceneManager::with_default()
            .get(&SceneId::DEFAULT)
            .and_then(|scene| scene.zones.first())
            .cloned()
            .expect("default scene should expose a primary zone");
        let layer_id = SceneLayerId::new();
        zone.layers = vec![SceneLayer::from_effect(
            layer_id,
            legacy_id,
            HashMap::new(),
            HashMap::new(),
            None,
        )];

        let migrated = remap_zones(
            std::slice::from_mut(&mut zone),
            &HashMap::from([(legacy_id, canonical_id)]),
        );

        assert_eq!(migrated, 1);
        assert_eq!(zone.layers[0].id, layer_id);
        assert_eq!(zone.effect_ids().collect::<Vec<_>>(), vec![canonical_id]);
    }

    #[tokio::test]
    async fn late_rescan_migrates_every_live_and_durable_reference_before_publication() {
        let temp = TempDir::new().expect("tempdir");
        let fixture = late_migration_fixture(&temp).await;
        let stale_mutation = fixture.state.scene_manager.begin_mutation().await;
        let revision_before = fixture.state.scene_manager.revision();

        let report = rescan_registry(&fixture.state)
            .await
            .expect("late rescan should migrate");

        assert_eq!(
            report.legacy_effect_ids.get(&fixture.legacy_id),
            Some(&fixture.canonical_id)
        );
        assert!(fixture.state.scene_manager.revision() > revision_before);
        let registry = fixture.state.domains.effects.registry_handle();
        let registry = registry.read().await;
        assert!(registry.get(&fixture.legacy_id).is_none());
        assert!(registry.get(&fixture.canonical_id).is_some());
        drop(registry);

        let manager = fixture.state.scene_manager.snapshot().await;
        assert!(
            manager
                .list()
                .into_iter()
                .flat_map(|scene| &scene.zones)
                .flat_map(hypercolor_types::scene::Zone::effect_ids)
                .all(|effect_id| effect_id == fixture.canonical_id)
        );
        assert!(
            manager
                .default_display_groups()
                .iter()
                .flat_map(hypercolor_types::scene::Zone::effect_ids)
                .all(|effect_id| effect_id == fixture.canonical_id)
        );
        assert!(
            fixture
                .state
                .scene_manager
                .commit_mutation(stale_mutation)
                .await
                .is_err()
        );

        assert_eq!(
            fixture
                .state
                .display_preferences
                .read()
                .await
                .get(fixture.device_id)
                .map(|preference| preference.effect_id),
            Some(fixture.canonical_id)
        );
        assert_eq!(
            fixture.state.library_store.list_favorites().await[0].effect_id,
            fixture.canonical_id
        );
        assert_eq!(
            fixture
                .state
                .library_store
                .get_preset(fixture.preset_id)
                .await
                .map(|preset| preset.effect_id),
            Some(fixture.canonical_id)
        );
        let stored_playlist = fixture
            .state
            .library_store
            .get_playlist(fixture.playlist_id)
            .await
            .expect("playlist should remain stored");
        assert_eq!(
            stored_playlist.items[0].target,
            PlaylistItemTarget::Effect {
                effect_id: fixture.canonical_id
            }
        );
        let active_playlist = fixture
            .state
            .playlist_runtime
            .lock()
            .await
            .active
            .as_ref()
            .map(|active| Arc::clone(&active.playlist))
            .expect("playlist should remain active");
        assert_eq!(
            active_playlist.read().await.items[0].target,
            PlaylistItemTarget::Effect {
                effect_id: fixture.canonical_id
            }
        );

        let runtime = crate::runtime_state::load(&fixture.state.runtime_state_path)
            .expect("runtime state should load")
            .expect("runtime state should exist");
        assert!(
            runtime
                .default_scene_groups
                .iter()
                .flat_map(hypercolor_types::scene::Zone::effect_ids)
                .all(|effect_id| effect_id == fixture.canonical_id)
        );
        let scenes =
            crate::scene_store::SceneStore::load(&fixture.state.data_dir.join("scenes.json"))
                .expect("scene store should reload");
        assert!(
            scenes
                .list()
                .flat_map(|scene| &scene.zones)
                .flat_map(hypercolor_types::scene::Zone::effect_ids)
                .all(|effect_id| effect_id == fixture.canonical_id)
        );

        let durable_paths = [
            fixture.state.data_dir.join("scenes.json"),
            fixture.state.runtime_state_path.clone(),
            fixture.state.data_dir.join("display-preferences.json"),
            fixture.state.data_dir.join("library.json"),
        ];
        let before_restart = durable_paths
            .iter()
            .map(|path| std::fs::read(path).expect("migrated store should read"))
            .collect::<Vec<_>>();
        rescan_registry(&fixture.state)
            .await
            .expect("repeated discovery should remain idempotent");
        let after_restart = durable_paths
            .iter()
            .map(|path| std::fs::read(path).expect("migrated store should read again"))
            .collect::<Vec<_>>();
        assert_eq!(after_restart, before_restart);
    }

    #[tokio::test]
    async fn watcher_reload_reapplies_the_ephemeral_map_idempotently() {
        let temp = TempDir::new().expect("tempdir");
        let fixture = late_migration_fixture(&temp).await;
        rescan_registry(&fixture.state)
            .await
            .expect("initial rescan should migrate");
        fixture
            .state
            .library_store
            .upsert_favorite(fixture.legacy_id, 20)
            .await
            .expect("late legacy favorite should persist");
        write_effect(&fixture.effect_path, "Late Arrival Reloaded");

        let report = reload_registry_file(&fixture.state, &fixture.effect_path)
            .await
            .expect("watcher reload should migrate");

        assert_eq!(
            report.legacy_effect_ids.get(&fixture.legacy_id),
            Some(&fixture.canonical_id)
        );
        let favorites = fixture.state.library_store.list_favorites().await;
        assert_eq!(favorites.len(), 1);
        assert_eq!(favorites[0].effect_id, fixture.canonical_id);
        assert_eq!(favorites[0].added_at_ms, 20);
    }

    #[cfg(feature = "persistence-test-hooks")]
    #[tokio::test]
    async fn failed_late_migration_does_not_publish_the_canonical_registry() {
        let temp = TempDir::new().expect("tempdir");
        let fixture = late_migration_fixture(&temp).await;
        hypercolor_core::persistence::AtomicFileWriter::new(
            &fixture.state.data_dir.join("scenes.json"),
        )
        .expect("scene writer should resolve")
        .set_injected_replace_failures(1);

        assert!(rescan_registry(&fixture.state).await.is_err());

        let registry = fixture.state.domains.effects.registry_handle();
        assert!(registry.read().await.get(&fixture.canonical_id).is_none());
        assert!(
            fixture
                .state
                .scene_manager
                .snapshot()
                .await
                .list()
                .into_iter()
                .flat_map(|scene| &scene.zones)
                .flat_map(hypercolor_types::scene::Zone::effect_ids)
                .any(|effect_id| effect_id == fixture.legacy_id)
        );

        rescan_registry(&fixture.state)
            .await
            .expect("retry should migrate after persistence recovers");
        let registry = fixture.state.domains.effects.registry_handle();
        assert!(registry.read().await.get(&fixture.canonical_id).is_some());
        assert!(
            fixture
                .state
                .scene_manager
                .snapshot()
                .await
                .list()
                .into_iter()
                .flat_map(|scene| &scene.zones)
                .flat_map(hypercolor_types::scene::Zone::effect_ids)
                .all(|effect_id| effect_id == fixture.canonical_id)
        );
    }
}
