use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use hypercolor_core::scene::SceneManager;
use hypercolor_types::api::scene::ReplaceLayerRequest;
use hypercolor_types::api::scenes::ReplaceSceneRequest;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceOrigin, DeviceTopologyHint, DisplayFrameFormat, SegmentInfo,
};
use hypercolor_types::effect::{EffectCategory, EffectId};
use hypercolor_types::event::HypercolorEvent;
use hypercolor_types::layer::{BlendMode, LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::library::{
    EffectPlaylist, EffectPreset, PlaylistId, PlaylistItem, PlaylistItemId, PlaylistItemTarget,
    PresetId,
};
use hypercolor_types::scene::{DisplayFaceTarget, SceneId, SceneKind, SceneMutationMode, ZoneRole};
use tempfile::TempDir;

use super::remap_zones;
use crate::app_state::{AppState, AppStateBuilder};
use crate::display_preferences::DisplayPreference;
use crate::domain::DomainError;
use crate::library::JsonLibraryStore;
use crate::playlist_runtime::ActivePlaylistRuntime;

struct LateMigrationFixture {
    state: AppState,
    effect_path: PathBuf,
    legacy_id: EffectId,
    canonical_id: EffectId,
    device_id: DeviceId,
    named_scene_id: SceneId,
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

fn installable_effect(title: &str, marker: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><title>{title}</title></head><body><canvas id=\"exCanvas\"></canvas><script>{marker}</script></body></html>"
    )
}

async fn promote_effect_to_display(state: &AppState, effect_id: EffectId) {
    let mut entry = state
        .domains
        .effects
        .registry_handle()
        .read()
        .await
        .get(&effect_id)
        .cloned()
        .expect("fixture effect should exist");
    entry.metadata.category = EffectCategory::Display;
    state.domains.effects.register(entry).await;
}

async fn late_migration_fixture(temp: &TempDir) -> LateMigrationFixture {
    let data_dir = temp.path().join("state");
    std::fs::create_dir_all(&data_dir).expect("state directory should be created");
    let library = Arc::new(
        JsonLibraryStore::open(data_dir.join("library.json")).expect("library should open"),
    );
    let state = AppStateBuilder::new(data_dir.clone())
        .with_library(library)
        .build();
    let effect_path = data_dir.join("effects/bundled/late-arrival.html");
    write_effect(&effect_path, "Late Arrival");
    let source_path = std::fs::canonicalize(&effect_path).expect("effect should canonicalize");
    let legacy_id =
        deterministic_html_effect_id(&format!("hypercolor:html:{}", source_path.display()));
    let canonical_id = deterministic_html_effect_id("hypercolor:html-bundled:late-arrival");
    let mut legacy_entry = hypercolor_core::effect::load_html_effect_file(&effect_path)
        .expect("legacy effect should load")
        .expect("legacy effect should be runnable");
    legacy_entry.metadata.id = legacy_id;
    state.domains.effects.register(legacy_entry).await;

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
    let named_scene_id = SceneId::new();
    named_scene.id = named_scene_id;
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
        .domains
        .display
        .preferences()
        .write()
        .await
        .set(
            device_id,
            DisplayPreference {
                effect_id: legacy_id,
                controls: HashMap::new(),
                blend_mode: BlendMode::Alpha,
                opacity: 1.0,
            },
        )
        .expect("display preference should persist");
    state
        .library_store()
        .upsert_favorite(legacy_id, 10)
        .await
        .expect("favorite should persist");
    let preset_id = PresetId::new();
    state
        .library_store()
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
        .library_store()
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
        named_scene_id,
        preset_id,
        playlist_id,
    }
}

fn effect_layer_request(effect_id: EffectId) -> ReplaceLayerRequest {
    ReplaceLayerRequest {
        source: LayerSource::Effect {
            effect_id,
            controls: HashMap::new(),
            control_bindings: HashMap::new(),
            preset_id: None,
        },
        name: None,
        blend: None,
        opacity: None,
        transform: None,
        adjust: None,
        bindings: None,
        enabled: None,
    }
}

async fn register_display_device(state: &AppState, device_id: DeviceId) {
    let info = DeviceInfo {
        id: device_id,
        name: "Migration Display".to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::new_static("test-display", "Test Display"),
        model: Some("LCD".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("test-display", "usb", ConnectionType::Usb),
        segments: vec![SegmentInfo {
            name: "LCD".to_owned(),
            led_count: 320 * 320,
            topology: DeviceTopologyHint::Display {
                width: 320,
                height: 320,
                circular: false,
                format: DisplayFrameFormat::Jpeg,
            },
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: 320 * 320,
            supports_direct: true,
            supports_brightness: true,
            has_display: true,
            display_resolution: Some((320, 320)),
            max_fps: 30,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    state.device_registry.add(info).await;
}

async fn assert_migration_is_blocked(
    state: Arc<AppState>,
) -> tokio::task::JoinHandle<Result<hypercolor_core::effect::RescanReport, DomainError>> {
    let migration = tokio::spawn(async move { state.domains.effects.rescan_registry().await });
    tokio::task::yield_now().await;
    assert!(
        !migration.is_finished(),
        "effect migration must wait for the admitted scene write"
    );
    migration
}

async fn assert_scene_effects_are_canonical(state: &AppState, canonical_id: EffectId) {
    let manager = state.scene_manager.snapshot().await;
    let effect_ids = manager
        .list()
        .into_iter()
        .flat_map(|scene| &scene.zones)
        .flat_map(hypercolor_types::scene::Zone::effect_ids)
        .collect::<Vec<_>>();
    assert!(!effect_ids.is_empty());
    assert!(
        effect_ids
            .into_iter()
            .all(|effect_id| effect_id == canonical_id)
    );
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
async fn same_stem_installs_and_watcher_reload_publish_whole_file_versions() {
    let temp = TempDir::new().expect("tempdir");
    let state = Arc::new(AppState::new_with_data_dir(temp.path().join("state")));
    let effect_path = temp.path().join("state/effects/user/shared.html");
    let first_html = installable_effect("First Version", "const version = 'first';");
    let second_html = installable_effect("Second Version", "const version = 'second';");
    let barrier = state.domains.effects.pause_next_install_write_for_test();

    let first_install = {
        let state = Arc::clone(&state);
        let effect_path = effect_path.clone();
        tokio::spawn(async move {
            state
                .domains
                .effects
                .install_registry_file(&effect_path, &first_html)
                .await
        })
    };
    barrier.wait_until_entered().await;

    let watcher_reload = {
        let state = Arc::clone(&state);
        let effect_path = effect_path.clone();
        tokio::spawn(async move {
            state
                .domains
                .effects
                .reload_registry_file(&effect_path)
                .await
        })
    };
    tokio::task::yield_now().await;
    let second_install = {
        let state = Arc::clone(&state);
        let effect_path = effect_path.clone();
        tokio::spawn(async move {
            state
                .domains
                .effects
                .install_registry_file(&effect_path, &second_html)
                .await
        })
    };
    tokio::task::yield_now().await;

    assert!(!watcher_reload.is_finished());
    assert!(!second_install.is_finished());
    barrier.release();

    let first = first_install
        .await
        .expect("first install task should finish")
        .expect("first install should publish");
    assert_eq!(first.metadata.name, "First Version");
    watcher_reload
        .await
        .expect("watcher task should finish")
        .expect("watcher should reload the first publication");
    let second = second_install
        .await
        .expect("second install task should finish")
        .expect("second install should publish");
    assert_eq!(second.metadata.name, "Second Version");
    assert_eq!(second.metadata.id, first.metadata.id);

    let written = std::fs::read_to_string(&effect_path).expect("installed file should read");
    assert!(written.contains("const version = 'second';"));
    let registry = state.domains.effects.registry_handle();
    let registry = registry.read().await;
    assert_eq!(
        registry
            .get(&second.metadata.id)
            .map(|entry| entry.metadata.name.as_str()),
        Some("Second Version")
    );
}

#[tokio::test]
async fn rejected_install_restores_the_previous_file_and_publication() {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::new_with_data_dir(temp.path().join("state"));
    let effect_path = temp.path().join("state/effects/user/shared.html");
    let original_html = installable_effect("Original", "const version = 'original';");
    let original = state
        .domains
        .effects
        .install_registry_file(&effect_path, &original_html)
        .await
        .expect("original install should publish");
    let invalid_html = r#"<!DOCTYPE html>
<html><head><title>Broken</title>
<meta preset="One" preset-id="duplicate" preset-controls='{}' />
<meta preset="Two" preset-id="duplicate" preset-controls='{}' />
</head><body><canvas id="exCanvas"></canvas><script>1</script></body></html>"#;

    let error = state
        .domains
        .effects
        .install_registry_file(&effect_path, invalid_html)
        .await
        .expect_err("duplicate preset ids should reject the install");

    assert!(error.to_string().contains("duplicate bundled preset id"));
    assert_eq!(
        std::fs::read_to_string(&effect_path).expect("restored file should read"),
        original_html
    );
    let registry = state.domains.effects.registry_handle();
    let registry = registry.read().await;
    assert_eq!(
        registry
            .get(&original.metadata.id)
            .map(|entry| entry.metadata.name.as_str()),
        Some("Original")
    );
}

#[tokio::test]
async fn late_rescan_migrates_every_live_and_durable_reference_before_publication() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let stale_mutation = fixture.state.scene_manager.begin_mutation().await;
    let revision_before = fixture.state.scene_manager.revision();
    let mut events = fixture.state.event_bus.subscribe_all();

    let report = fixture
        .state
        .domains
        .effects
        .rescan_registry()
        .await
        .expect("late rescan should migrate");
    let registry_updates = std::iter::from_fn(|| events.try_recv().ok())
        .filter_map(|event| match event.event {
            HypercolorEvent::EffectRegistryUpdated {
                added,
                removed,
                updated,
            } => Some((added, removed, updated)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        registry_updates,
        vec![(report.added, report.removed, report.updated)]
    );

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
            .default_display_zones()
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
            .domains
            .display
            .preferences()
            .read()
            .await
            .get(fixture.device_id)
            .map(|preference| preference.effect_id),
        Some(fixture.canonical_id)
    );
    assert_eq!(
        fixture.state.library_store().list_favorites().await[0].effect_id,
        fixture.canonical_id
    );
    assert_eq!(
        fixture
            .state
            .library_store()
            .get_preset(fixture.preset_id)
            .await
            .map(|preset| preset.effect_id),
        Some(fixture.canonical_id)
    );
    let stored_playlist = fixture
        .state
        .library_store()
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
            .default_scene_zones
            .iter()
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .all(|effect_id| effect_id == fixture.canonical_id)
    );
    let scenes = crate::scene_store::load(&fixture.state.data_dir.join("scenes.json"))
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
        fixture.state.state_dir.join("display-preferences.json"),
        fixture.state.data_dir.join("library.json"),
    ];
    let before_restart = durable_paths
        .iter()
        .map(|path| std::fs::read(path).expect("migrated store should read"))
        .collect::<Vec<_>>();
    fixture
        .state
        .domains
        .effects
        .rescan_registry()
        .await
        .expect("repeated discovery should remain idempotent");
    let after_restart = durable_paths
        .iter()
        .map(|path| std::fs::read(path).expect("migrated store should read again"))
        .collect::<Vec<_>>();
    assert_eq!(after_restart, before_restart);
}

#[tokio::test]
async fn identity_publication_blocks_every_observer_until_registry_assignment() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let legacy_id = fixture.legacy_id;
    let canonical_id = fixture.canonical_id;
    let device_id = fixture.device_id;
    let state = Arc::new(fixture.state);
    let barrier = state
        .domains
        .effects
        .pause_next_identity_inter_component_for_test();
    let rescan_state = Arc::clone(&state);
    let rescan = tokio::spawn(async move { rescan_state.domains.effects.rescan_registry().await });

    barrier.wait_until_entered().await;

    let library_state = Arc::clone(&state);
    let mut library_observer =
        tokio::spawn(
            async move { library_state.library_store().list_favorites().await[0].effect_id },
        );
    let display_state = Arc::clone(&state);
    let mut display_observer = tokio::spawn(async move {
        display_state
            .domains
            .display
            .preferences()
            .read()
            .await
            .get(device_id)
            .map(|preference| preference.effect_id)
    });
    let scene_state = Arc::clone(&state);
    let mut scene_observer = tokio::spawn(async move {
        scene_state
            .scene_manager
            .snapshot()
            .await
            .list()
            .into_iter()
            .flat_map(|scene| &scene.zones)
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .collect::<Vec<_>>()
    });
    let registry = state.domains.effects.registry_handle();
    let mut registry_observer = tokio::spawn(async move {
        let registry = registry.read().await;
        (
            registry.get(&legacy_id).is_some(),
            registry.get(&canonical_id).is_some(),
        )
    });
    let playlist_state = Arc::clone(&state);
    let mut playlist_observer = tokio::spawn(async move {
        let playlist = playlist_state
            .playlist_runtime
            .lock()
            .await
            .active
            .as_ref()
            .map(|active| Arc::clone(&active.playlist))
            .expect("playlist should remain active");
        playlist.read().await.items[0].target.clone()
    });

    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut library_observer)
            .await
            .is_err(),
        "library observer escaped the retained publication transaction"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut display_observer)
            .await
            .is_err(),
        "display observer escaped the retained publication transaction"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut registry_observer)
            .await
            .is_err(),
        "registry observer escaped the retained publication transaction"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut scene_observer)
            .await
            .is_err(),
        "scene observer escaped the retained publication transaction"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut playlist_observer)
            .await
            .is_err(),
        "playlist observer escaped the retained publication transaction"
    );

    barrier.release();
    rescan
        .await
        .expect("rescan should not panic")
        .expect("rescan should publish atomically");

    assert_eq!(
        library_observer
            .await
            .expect("library observer should finish"),
        canonical_id
    );
    assert_eq!(
        display_observer
            .await
            .expect("display observer should finish"),
        Some(canonical_id)
    );
    assert!(
        scene_observer
            .await
            .expect("scene observer should finish")
            .into_iter()
            .all(|effect_id| effect_id == canonical_id)
    );
    assert_eq!(
        registry_observer
            .await
            .expect("registry observer should finish"),
        (false, true)
    );
    assert_eq!(
        playlist_observer
            .await
            .expect("playlist observer should finish"),
        PlaylistItemTarget::Effect {
            effect_id: canonical_id
        }
    );
}

#[tokio::test]
async fn watcher_reload_reapplies_the_ephemeral_map_idempotently() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    fixture
        .state
        .domains
        .effects
        .rescan_registry()
        .await
        .expect("initial rescan should migrate");
    fixture
        .state
        .library_store()
        .upsert_favorite(fixture.legacy_id, 20)
        .await
        .expect("late legacy favorite should persist");
    write_effect(&fixture.effect_path, "Late Arrival Reloaded");

    let report = fixture
        .state
        .domains
        .effects
        .reload_registry_file(&fixture.effect_path)
        .await
        .expect("watcher reload should migrate");

    assert_eq!(
        report.legacy_effect_ids.get(&fixture.legacy_id),
        Some(&fixture.canonical_id)
    );
    let favorites = fixture.state.library_store().list_favorites().await;
    assert_eq!(favorites.len(), 1);
    assert_eq!(favorites[0].effect_id, fixture.canonical_id);
    assert_eq!(favorites[0].added_at_ms, 20);
}

#[cfg(not(feature = "servo"))]
#[tokio::test]
async fn skipped_screen_cast_port_never_migrates_scene_persistence() {
    let temp = TempDir::new().expect("tempdir");
    let data_dir = temp.path().join("state");
    let state = AppState::new_with_data_dir(data_dir.clone());
    let effect_path = data_dir.join("effects/bundled/screen-cast.html");
    std::fs::create_dir_all(
        effect_path
            .parent()
            .expect("effect path should have a parent"),
    )
    .expect("effect directory should build");
    std::fs::write(
        &effect_path,
        r#"<head><title>Screen Cast</title><meta builtin-id="screen_cast" /></head>"#,
    )
    .expect("screen cast port should write");
    let source_path = std::fs::canonicalize(&effect_path).expect("effect should canonicalize");
    let legacy_id =
        deterministic_html_effect_id(&format!("hypercolor:html:{}", source_path.display()));
    let unregistered_target = deterministic_html_effect_id("hypercolor:html-bundled:screen-cast");

    let mut mutation = state.scene_manager.begin_mutation().await;
    let mut named_scene = mutation
        .scenes()
        .get(&SceneId::DEFAULT)
        .cloned()
        .expect("default scene should exist");
    named_scene.id = SceneId::new();
    named_scene.name = "Legacy screen scene".to_owned();
    named_scene.kind = SceneKind::Named;
    named_scene.mutation_mode = SceneMutationMode::Live;
    named_scene.zones[0].layers = vec![SceneLayer::from_effect(
        SceneLayerId::new(),
        legacy_id,
        HashMap::new(),
        HashMap::new(),
        None,
    )];
    mutation
        .create_scene(named_scene)
        .expect("named scene should enter the candidate");
    state
        .scene_manager
        .commit_mutation(mutation)
        .await
        .expect("named scene should commit");

    for report in [
        state
            .domains
            .effects
            .rescan_registry()
            .await
            .expect("rescan should skip the unavailable port"),
        state
            .domains
            .effects
            .reload_registry_file(&effect_path)
            .await
            .expect("watcher reload should skip the unavailable port"),
    ] {
        assert!(
            report.legacy_effect_ids.is_empty(),
            "skipped port exposed migrations: {:?}",
            report.legacy_effect_ids
        );
    }
    let registry = state.domains.effects.registry_handle();
    assert!(registry.read().await.get(&unregistered_target).is_none());
    let manager = state.scene_manager.snapshot().await;
    assert!(
        manager
            .list()
            .into_iter()
            .filter(|scene| scene.kind == SceneKind::Named)
            .flat_map(|scene| &scene.zones)
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .all(|effect_id| effect_id == legacy_id)
    );
    let durable = crate::scene_store::load(&data_dir.join("scenes.json"))
        .expect("scene store should remain readable");
    assert!(
        durable
            .list()
            .flat_map(|scene| &scene.zones)
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .all(|effect_id| effect_id == legacy_id)
    );
}

#[tokio::test]
async fn publication_conflict_reprepares_inside_the_same_rescan() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let state = Arc::new(fixture.state);
    let barrier = state
        .domains
        .effects
        .pause_next_identity_publication_for_test();
    let rescan_state = Arc::clone(&state);
    let rescan = tokio::spawn(async move { rescan_state.domains.effects.rescan_registry().await });

    barrier.wait_until_entered().await;
    state
        .domains
        .display
        .preferences()
        .write()
        .await
        .set(
            fixture.device_id,
            DisplayPreference {
                effect_id: fixture.legacy_id,
                controls: HashMap::new(),
                blend_mode: BlendMode::Replace,
                opacity: 1.0,
            },
        )
        .expect("concurrent preference should publish");
    barrier.release();

    rescan
        .await
        .expect("rescan should not panic")
        .expect("the original rescan should reprepare and converge");
    assert_eq!(
        state
            .domains
            .display
            .preferences()
            .read()
            .await
            .get(fixture.device_id)
            .map(|preference| preference.effect_id),
        Some(fixture.canonical_id)
    );
    let registry = state.domains.effects.registry_handle();
    assert!(registry.read().await.get(&fixture.canonical_id).is_some());
}

#[tokio::test]
async fn rest_apply_resolved_before_migration_is_rejected_after_publication() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let legacy_id = fixture.legacy_id;
    let canonical_id = fixture.canonical_id;
    let state = Arc::new(fixture.state);
    let barrier = state.domains.effects.pause_next_resolution_for_test();
    let apply_state = Arc::clone(&state);
    let apply = tokio::spawn(async move {
        crate::api::effects::apply_effect(
            axum::extract::State(apply_state),
            axum::extract::Path(legacy_id.to_string()),
            axum::http::HeaderMap::new(),
            None,
        )
        .await
    });

    barrier.wait_until_entered().await;
    state
        .domains
        .effects
        .rescan_registry()
        .await
        .expect("migration should publish while REST resolution is paused");
    barrier.release();

    let response = apply.await.expect("REST apply should not panic");
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    let manager = state.scene_manager.snapshot().await;
    assert!(
        manager
            .list()
            .into_iter()
            .flat_map(|scene| &scene.zones)
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .all(|effect_id| effect_id == canonical_id)
    );
}

#[tokio::test]
async fn display_assignment_resolved_before_migration_is_rejected() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let resolved = fixture
        .state
        .domains
        .effects
        .metadata_for_mutation(fixture.legacy_id)
        .await
        .expect("legacy effect should resolve before migration");
    fixture
        .state
        .domains
        .effects
        .rescan_registry()
        .await
        .expect("migration should publish");
    let revision = fixture.state.scene_manager.revision();
    let result = crate::domain::display::set_display_face(
        &fixture.state.domains.effects,
        crate::domain::display::SetDisplayFace {
            device_id: fixture.device_id,
            device_name: "Race Display".to_owned(),
            effect: resolved,
            controls: HashMap::new(),
            layout: crate::domain::display::display_face_layout(
                fixture.device_id,
                "Race Display",
                crate::domain::display::DisplaySurfaceInfo {
                    width: 320,
                    height: 320,
                    circular: false,
                },
            ),
            target: DisplayFaceTarget::new(fixture.device_id),
        },
    )
    .await;

    assert!(result.is_err());
    assert_eq!(fixture.state.scene_manager.revision(), revision);
}

#[tokio::test]
async fn cloned_playlist_item_resolved_before_migration_cannot_commit() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let canonical_id = fixture.canonical_id;
    let state = Arc::new(fixture.state);
    let item = {
        let runtime = state.playlist_runtime.lock().await;
        let playlist = Arc::clone(
            &runtime
                .active
                .as_ref()
                .expect("playlist should be active")
                .playlist,
        );
        drop(runtime);
        playlist.read().await.items[0].clone()
    };
    let barrier = state.domains.effects.pause_next_resolution_for_test();
    let worker_state = Arc::clone(&state);
    let worker = tokio::spawn(async move {
        crate::api::library::activate_playlist_item(&worker_state, &item).await
    });

    barrier.wait_until_entered().await;
    state
        .domains
        .effects
        .rescan_registry()
        .await
        .expect("migration should publish while playlist resolution is paused");
    barrier.release();

    let result = worker.await.expect("playlist worker should not panic");
    assert!(result.is_err());
    let manager = state.scene_manager.snapshot().await;
    assert!(
        manager
            .list()
            .into_iter()
            .flat_map(|scene| &scene.zones)
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .all(|effect_id| effect_id == canonical_id)
    );
}

#[tokio::test]
async fn layer_create_holds_effect_admission_through_scene_commit() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let legacy_id = fixture.legacy_id;
    let canonical_id = fixture.canonical_id;
    let state = Arc::new(fixture.state);
    let zone_id = state
        .scene_manager
        .snapshot()
        .await
        .get(&SceneId::DEFAULT)
        .and_then(|scene| scene.zones.first())
        .map(|zone| zone.id)
        .expect("default scene should have a zone");
    let barrier = state.domains.effects.pause_next_resolution_for_test();
    let create_state = Arc::clone(&state);
    let create = tokio::spawn(async move {
        crate::api::scene::create_layer(
            State(create_state),
            AxumPath(zone_id.to_string()),
            HeaderMap::new(),
            Json(effect_layer_request(legacy_id).into()),
        )
        .await
    });

    barrier.wait_until_entered().await;
    let migration = assert_migration_is_blocked(Arc::clone(&state)).await;
    barrier.release();

    assert_eq!(
        create
            .await
            .expect("layer create should not panic")
            .status(),
        StatusCode::CREATED
    );
    migration
        .await
        .expect("migration should not panic")
        .expect("migration should publish after layer create");
    assert_scene_effects_are_canonical(state.as_ref(), canonical_id).await;
}

#[tokio::test]
async fn layer_replace_holds_effect_admission_through_scene_commit() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let legacy_id = fixture.legacy_id;
    let canonical_id = fixture.canonical_id;
    let state = Arc::new(fixture.state);
    let (zone_id, layer_id) = state
        .scene_manager
        .snapshot()
        .await
        .get(&SceneId::DEFAULT)
        .and_then(|scene| scene.zones.first())
        .and_then(|zone| zone.layers.first().map(|layer| (zone.id, layer.id)))
        .expect("default scene should have the legacy layer");
    let barrier = state.domains.effects.pause_next_resolution_for_test();
    let replace_state = Arc::clone(&state);
    let replace = tokio::spawn(async move {
        crate::api::scene::replace_layer(
            State(replace_state),
            AxumPath((zone_id.to_string(), layer_id.to_string())),
            HeaderMap::new(),
            Json(effect_layer_request(legacy_id)),
        )
        .await
    });

    barrier.wait_until_entered().await;
    let migration = assert_migration_is_blocked(Arc::clone(&state)).await;
    barrier.release();

    assert_eq!(
        replace
            .await
            .expect("layer replacement should not panic")
            .status(),
        StatusCode::OK
    );
    migration
        .await
        .expect("migration should not panic")
        .expect("migration should publish after layer replacement");
    assert_scene_effects_are_canonical(state.as_ref(), canonical_id).await;
}

#[tokio::test]
async fn whole_scene_put_holds_effect_admission_through_scene_commit() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let scene_id = fixture.named_scene_id;
    let canonical_id = fixture.canonical_id;
    let state = Arc::new(fixture.state);
    let document = {
        let manager = state.scene_manager.snapshot().await;
        crate::domain::scene_tree::scene_document(
            manager.get(&scene_id).expect("named scene should exist"),
            state.scene_manager.revision(),
        )
    };
    let barrier = state.domains.effects.pause_next_resolution_for_test();
    let replace_state = Arc::clone(&state);
    let replace = tokio::spawn(async move {
        crate::api::scenes::update_scene(
            State(replace_state),
            AxumPath(scene_id.to_string()),
            HeaderMap::new(),
            Json(ReplaceSceneRequest::from(&document)),
        )
        .await
    });

    barrier.wait_until_entered().await;
    let migration = assert_migration_is_blocked(Arc::clone(&state)).await;
    barrier.release();

    assert_eq!(
        replace
            .await
            .expect("whole-scene PUT should not panic")
            .status(),
        StatusCode::OK
    );
    migration
        .await
        .expect("migration should not panic")
        .expect("migration should publish after whole-scene PUT");
    assert_scene_effects_are_canonical(state.as_ref(), canonical_id).await;
}

#[tokio::test]
async fn default_overlay_reconciliation_holds_admission_through_scene_commit() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let device_id = fixture.device_id;
    let canonical_id = fixture.canonical_id;
    register_display_device(&fixture.state, device_id).await;
    promote_effect_to_display(&fixture.state, fixture.legacy_id).await;
    let state = Arc::new(fixture.state);
    let barrier = state.domains.effects.pause_next_resolution_for_test();
    let overlay_state = Arc::clone(&state);
    let overlay = tokio::spawn(async move {
        overlay_state
            .domains
            .display
            .apply_preference_overlay(device_id)
            .await
    });

    barrier.wait_until_entered().await;
    let migration = assert_migration_is_blocked(Arc::clone(&state)).await;
    barrier.release();

    assert!(
        overlay
            .await
            .expect("overlay reconciliation should not panic")
            .is_some()
    );
    migration
        .await
        .expect("migration should not panic")
        .expect("migration should publish after overlay reconciliation");
    let manager = state.scene_manager.snapshot().await;
    assert!(
        manager
            .default_display_zones()
            .iter()
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .all(|effect_id| effect_id == canonical_id)
    );
}

#[tokio::test]
async fn rejected_default_overlay_holds_admission_through_retraction() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let device_id = fixture.device_id;
    register_display_device(&fixture.state, device_id).await;
    let state = Arc::new(fixture.state);
    let barrier = state.domains.effects.pause_next_resolution_for_test();
    let overlay_state = Arc::clone(&state);
    let overlay = tokio::spawn(async move {
        overlay_state
            .domains
            .display
            .apply_preference_overlay(device_id)
            .await
    });

    barrier.wait_until_entered().await;
    let migration = assert_migration_is_blocked(Arc::clone(&state)).await;
    barrier.release();

    assert!(
        overlay
            .await
            .expect("overlay reconciliation should not panic")
            .is_none()
    );
    migration
        .await
        .expect("migration should not panic")
        .expect("migration should publish after overlay retraction");
    assert!(
        state
            .scene_manager
            .snapshot()
            .await
            .default_display_zone_for(device_id)
            .is_none()
    );
}

#[tokio::test]
async fn default_overlay_reconciliation_retries_a_replaced_preference() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let device_id = fixture.device_id;
    register_display_device(&fixture.state, device_id).await;
    promote_effect_to_display(&fixture.state, fixture.legacy_id).await;
    let state = Arc::new(fixture.state);
    let barrier = state.domains.effects.pause_next_resolution_for_test();
    let overlay_state = Arc::clone(&state);
    let overlay = tokio::spawn(async move {
        overlay_state
            .domains
            .display
            .apply_preference_overlay(device_id)
            .await
    });

    barrier.wait_until_entered().await;
    {
        let mut store = state.domains.display.preferences().write().await;
        let mut replacement = store
            .get(device_id)
            .cloned()
            .expect("fixture should have a display preference");
        replacement.opacity = 0.25;
        store
            .set(device_id, replacement)
            .expect("replacement preference should persist");
    }
    barrier.release();

    let zone = overlay
        .await
        .expect("overlay reconciliation should not panic");
    let manager = state.scene_manager.snapshot().await;
    assert_eq!(
        (
            zone.as_ref()
                .and_then(|zone| zone.display_target.as_ref())
                .map(|target| target.opacity),
            manager
                .default_display_zone_for(device_id)
                .and_then(|zone| zone.display_target.as_ref())
                .map(|target| target.opacity),
        ),
        (Some(0.25), Some(0.25))
    );
}

#[tokio::test]
async fn scene_effect_writes_reject_unknown_and_retired_ids() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let legacy_id = fixture.legacy_id;
    let named_scene_id = fixture.named_scene_id;
    fixture
        .state
        .domains
        .effects
        .rescan_registry()
        .await
        .expect("migration should retire the legacy ID");
    let state = Arc::new(fixture.state);
    let (zone_id, layer_id) = state
        .scene_manager
        .snapshot()
        .await
        .get(&SceneId::DEFAULT)
        .and_then(|scene| scene.zones.first())
        .and_then(|zone| zone.layers.first().map(|layer| (zone.id, layer.id)))
        .expect("default scene should retain its migrated layer");
    let revision = state.scene_manager.revision();

    for effect_id in [legacy_id, EffectId::new(uuid::Uuid::now_v7())] {
        let response = crate::api::scene::create_layer(
            State(Arc::clone(&state)),
            AxumPath(zone_id.to_string()),
            HeaderMap::new(),
            Json(effect_layer_request(effect_id).into()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    let response = crate::api::scene::replace_layer(
        State(Arc::clone(&state)),
        AxumPath((zone_id.to_string(), layer_id.to_string())),
        HeaderMap::new(),
        Json(effect_layer_request(legacy_id)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let mut document = {
        let manager = state.scene_manager.snapshot().await;
        let document = crate::domain::scene_tree::scene_document(
            manager
                .get(&named_scene_id)
                .expect("named scene should remain stored"),
            revision,
        );
        ReplaceSceneRequest::from(&document)
    };
    document.zones[0].layers[0].source = effect_layer_request(legacy_id).source;
    let response = crate::api::scenes::update_scene(
        State(Arc::clone(&state)),
        AxumPath(named_scene_id.to_string()),
        HeaderMap::new(),
        Json(document),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(state.scene_manager.revision(), revision);
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn revision_neutral_snapshot_save_cannot_overtake_identity_publication() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let state = Arc::new(fixture.state);
    let publication_barrier = state
        .domains
        .effects
        .pause_next_identity_publication_for_test();
    let snapshot_barrier = state.scene_manager.pause_next_persistence_for_test();
    let revision = state.scene_manager.revision();

    let migration = {
        let state = Arc::clone(&state);
        tokio::spawn(async move { state.domains.effects.rescan_registry().await })
    };
    publication_barrier.wait_until_entered().await;
    assert_eq!(state.scene_manager.revision(), revision);

    let snapshot_save = {
        let state = Arc::clone(&state);
        tokio::spawn(async move { state.scene_manager.save_snapshot().await })
    };
    snapshot_barrier.wait_until_entered().await;
    snapshot_barrier.release();
    tokio::task::yield_now().await;
    assert!(
        !snapshot_save.is_finished(),
        "a revision-neutral snapshot must wait for identity publication"
    );
    assert_eq!(state.scene_manager.revision(), revision);

    publication_barrier.release();
    migration
        .await
        .expect("migration task should not panic")
        .expect("migration should publish");
    snapshot_save
        .await
        .expect("snapshot task should not panic")
        .expect("snapshot should persist after publication");

    let durable = crate::scene_store::load(&state.data_dir.join("scenes.json"))
        .expect("scene store should load after the race");
    assert!(
        durable
            .list()
            .flat_map(|scene| &scene.zones)
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .all(|effect_id| effect_id == fixture.canonical_id)
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn shutdown_snapshot_cannot_overtake_identity_publication() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let state = Arc::new(fixture.state);
    let publication_barrier = state
        .domains
        .effects
        .pause_next_identity_publication_for_test();
    let snapshot_barrier = state.scene_manager.pause_next_persistence_for_test();

    let migration = {
        let state = Arc::clone(&state);
        tokio::spawn(async move { state.domains.effects.rescan_registry().await })
    };
    publication_barrier.wait_until_entered().await;

    let shutdown_snapshot = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            crate::startup::persist_scene_store_snapshot(&state.scene_manager).await
        })
    };
    snapshot_barrier.wait_until_entered().await;
    snapshot_barrier.release();
    tokio::task::yield_now().await;
    assert!(
        !shutdown_snapshot.is_finished(),
        "shutdown persistence must wait for identity publication"
    );

    publication_barrier.release();
    migration
        .await
        .expect("migration task should not panic")
        .expect("migration should publish");
    assert!(
        shutdown_snapshot
            .await
            .expect("shutdown snapshot task should not panic")
            .expect("shutdown snapshot should persist after publication")
            .is_some()
    );

    let durable = crate::scene_store::load(&state.data_dir.join("scenes.json"))
        .expect("scene store should load after the shutdown race");
    assert!(
        durable
            .list()
            .flat_map(|scene| &scene.zones)
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .all(|effect_id| effect_id == fixture.canonical_id)
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn runtime_save_cannot_overtake_identity_publication() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let state = Arc::new(fixture.state);
    let publication_barrier = state
        .domains
        .effects
        .pause_next_identity_publication_for_test();
    let save_barrier = state
        .domains
        .runtime_session
        .pause_next_save_before_admission_for_test();
    let revision = state.scene_manager.revision();

    let migration = {
        let state = Arc::clone(&state);
        tokio::spawn(async move { state.domains.effects.rescan_registry().await })
    };
    publication_barrier.wait_until_entered().await;
    assert_eq!(state.scene_manager.revision(), revision);

    let runtime_save = {
        let state = Arc::clone(&state);
        tokio::spawn(async move { state.domains.runtime_session.save().await })
    };
    save_barrier.wait_until_entered().await;
    save_barrier.release();
    tokio::task::yield_now().await;
    assert!(
        !runtime_save.is_finished(),
        "a runtime save must wait for identity publication admission"
    );
    assert_eq!(state.scene_manager.revision(), revision);

    publication_barrier.release();
    migration
        .await
        .expect("migration task should not panic")
        .expect("migration should publish");
    runtime_save
        .await
        .expect("runtime save task should not panic");

    let durable = crate::scene_store::load(&state.data_dir.join("scenes.json"))
        .expect("scene store should load after the race");
    assert!(
        durable
            .list()
            .flat_map(|scene| &scene.zones)
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .all(|effect_id| effect_id == fixture.canonical_id)
    );
    let runtime = crate::runtime_state::load(&state.runtime_state_path)
        .expect("runtime state should load after the race")
        .expect("runtime state should exist after the race");
    assert!(
        runtime
            .default_scene_zones
            .iter()
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .all(|effect_id| effect_id == fixture.canonical_id)
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn migration_generation_preserves_an_admitted_newer_named_scene() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let state = Arc::new(fixture.state);
    let barrier = state.scene_manager.pause_next_persistence_for_test();
    let mut mutation = state.scene_manager.begin_mutation().await;
    let mut named_scene = mutation
        .scenes()
        .get(&SceneId::DEFAULT)
        .cloned()
        .expect("default scene should exist");
    let admitted_scene_id = SceneId::new();
    named_scene.id = admitted_scene_id;
    named_scene.name = "Admitted during identity migration".to_owned();
    named_scene.kind = SceneKind::Named;
    named_scene.mutation_mode = SceneMutationMode::Live;
    mutation
        .create_scene(named_scene)
        .expect("named scene should enter the candidate");
    let commit_state = Arc::clone(&state);
    let commit =
        tokio::spawn(async move { commit_state.scene_manager.commit_mutation(mutation).await });

    barrier.wait_until_entered().await;
    let before = crate::scene_store::load(&state.data_dir.join("scenes.json"))
        .expect("pre-migration scene store should load");
    assert!(before.list().all(|scene| scene.id != admitted_scene_id));

    state
        .domains
        .effects
        .rescan_registry()
        .await
        .expect("the same rescan should migrate the admitted manager candidate");
    barrier.release();
    let commit = commit
        .await
        .expect("scene commit should not panic")
        .expect("admitted scene commit should return a durability receipt");
    assert_eq!(
        commit.durability(),
        crate::domain::commit::CommitDurability::Superseded
    );

    let registry = state.domains.effects.registry_handle();
    assert!(registry.read().await.get(&fixture.canonical_id).is_some());
    assert!(
        state
            .scene_manager
            .snapshot()
            .await
            .get(&admitted_scene_id)
            .is_some_and(|scene| scene
                .zones
                .iter()
                .flat_map(hypercolor_types::scene::Zone::effect_ids)
                .all(|effect_id| effect_id == fixture.canonical_id))
    );
    let durable = crate::scene_store::load(&state.data_dir.join("scenes.json"))
        .expect("migrated scene store should load");
    let scene = durable
        .list()
        .find(|scene| scene.id == admitted_scene_id)
        .expect("the N+1 snapshot must retain the admitted N scene");
    assert!(
        scene
            .zones
            .iter()
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .all(|effect_id| effect_id == fixture.canonical_id)
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn transient_store_failure_converges_without_another_rescan() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = late_migration_fixture(&temp).await;
    let scenes_path = fixture.state.data_dir.join("scenes.json");
    let writer = hypercolor_core::persistence::AtomicFileWriter::new(&scenes_path)
        .expect("scene writer should resolve");
    writer.set_injected_replace_failures(1);

    fixture
        .state
        .domains
        .effects
        .rescan_registry()
        .await
        .expect("an admitted migration remains authoritative while persistence retries");

    let registry = fixture.state.domains.effects.registry_handle();
    assert!(registry.read().await.get(&fixture.canonical_id).is_some());
    writer
        .flush(std::time::Duration::from_secs(2))
        .expect("the admitted migration should converge after the transient failure");
    let durable =
        crate::scene_store::load(&scenes_path).expect("converged scene store should load");
    assert!(
        durable
            .list()
            .flat_map(|scene| &scene.zones)
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .all(|effect_id| effect_id == fixture.canonical_id)
    );
}
