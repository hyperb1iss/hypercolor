//! First-run defaults: what a genuinely fresh install starts with, and the
//! durable state that keeps an existing install from ever seeing it.
//!
//! Every test boots a real daemon over private config, data, and state
//! directories, with the bundled effect root pointed at fixture copies of
//! the curated picks. Hardware discovery, audio, capture, host input, and
//! session monitoring stay off.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use hypercolor_core::config::{BootConfig, ConfigManager};
use hypercolor_core::effect::{bundled_html_effect_id, set_bundled_effects_root};
use hypercolor_daemon::first_run::FIRST_RUN_FAVORITES;
use hypercolor_daemon::startup::{DaemonState, default_config};
use hypercolor_types::config::{HypercolorConfig, RenderAccelerationMode, ServoGpuImportMode};
use hypercolor_types::effect::EffectId;
use hypercolor_types::scene::SceneId;
use tokio::sync::Mutex;

static PATH_OVERRIDE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Private daemon directories plus a bundled effect root holding fixture
/// copies of the named picks, released together when dropped.
struct Install {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    root: tempfile::TempDir,
}

impl Install {
    async fn with_bundled(stems: &[&str]) -> Self {
        let fixtures: Vec<_> = stems.iter().map(|stem| (*stem, "ambient")).collect();
        Self::with_fixtures(&fixtures).await
    }

    /// Fixture copies of `(stem, category)` pairs under the bundled root.
    async fn with_fixtures(fixtures: &[(&str, &str)]) -> Self {
        let lock = PATH_OVERRIDE_LOCK.lock().await;
        let root = tempfile::tempdir().expect("tempdir should be created");
        ConfigManager::set_config_dir_override(Some(root.path().join("config")));
        ConfigManager::set_data_dir_override(Some(root.path().join("data")));
        ConfigManager::set_state_dir_override(Some(root.path().join("state")));

        // Ids come from the stem under an `effects/hypercolor` root, which
        // is the layout a dev tree and every release share.
        let bundled = root.path().join("effects").join("hypercolor");
        std::fs::create_dir_all(&bundled).expect("bundled effect root should build");
        for (stem, category) in fixtures {
            std::fs::write(
                bundled.join(format!("{stem}.html")),
                format!(
                    "<head><title>{stem}</title>\
                     <meta description=\"First-run fixture\" />\
                     <meta publisher=\"Hypercolor\" />\
                     <meta category=\"{category}\" /></head><body></body>"
                ),
            )
            .expect("fixture effect should write");
        }
        set_bundled_effects_root(Some(bundled));

        Self { _lock: lock, root }
    }

    fn data_path(&self, file_name: &str) -> PathBuf {
        self.root.path().join("data").join(file_name)
    }

    fn library_path(&self) -> PathBuf {
        self.data_path("library.json")
    }

    fn initialize_with(&self, config: HypercolorConfig) -> DaemonState {
        let config_manager = Arc::new(ConfigManager::from_config_unchecked(
            self.root.path().join("config").join("hypercolor.toml"),
            config.clone(),
        ));
        DaemonState::initialize(BootConfig::from_config_unchecked(config), config_manager)
            .expect("daemon should initialize")
    }

    async fn start(&self) -> DaemonState {
        self.start_with(quiet_config()).await
    }

    async fn start_with(&self, config: HypercolorConfig) -> DaemonState {
        // Boxed so the state is not held inline across the await, which
        // would make every caller's future several kilobytes.
        let mut state = Box::new(self.initialize_with(config));
        state.start().await.expect("daemon should start");
        *state
    }
}

impl Drop for Install {
    fn drop(&mut self) {
        set_bundled_effects_root(None);
        ConfigManager::set_config_dir_override(None);
        ConfigManager::set_data_dir_override(None);
        ConfigManager::set_state_dir_override(None);
    }
}

/// Production defaults, including `start_scene = "last"`, with every
/// subsystem that would reach hardware or the host session switched off.
fn quiet_config() -> HypercolorConfig {
    let mut config = default_config();
    assert_eq!(config.daemon.start_scene, "last");
    config.audio.enabled = false;
    config.capture.enabled = false;
    config.input.enabled = false;
    config.session.enabled = false;
    config.effect_engine.compositor_acceleration_mode = RenderAccelerationMode::Cpu;
    config.rendering.servo_gpu_import.mode = ServoGpuImportMode::Off;
    config.effect_engine.watch_effects = false;
    config.discovery.background_enabled = false;
    config.discovery.mdns_enabled = false;
    config.discovery.blocks_scan = false;
    config.network.mdns_publish = false;
    config
}

fn ids(stems: &[&str]) -> Vec<EffectId> {
    stems
        .iter()
        .map(|stem| bundled_html_effect_id(stem))
        .collect()
}

async fn favorite_ids(state: &DaemonState) -> Vec<EffectId> {
    state
        .library_store()
        .list_favorites()
        .await
        .into_iter()
        .map(|favorite| favorite.effect_id)
        .collect()
}

/// The effect the default scene's primary zone runs, if any.
async fn primary_effect(state: &DaemonState) -> Option<EffectId> {
    let scenes = state.scene_manager.snapshot().await;
    assert_eq!(scenes.active_scene_id(), Some(&SceneId::DEFAULT));
    scenes.active_scene()?.primary_zone()?.effect_ids().next()
}

#[tokio::test]
async fn an_install_with_none_of_the_picks_writes_nothing() {
    let install = Install::with_bundled(&[]).await;
    let mut state = install.start().await;

    assert_eq!(primary_effect(&state).await, None);
    assert!(favorite_ids(&state).await.is_empty());
    // Nothing to seed leaves the library unwritten.
    assert!(!install.library_path().exists());

    state.shutdown().await.expect("shutdown should succeed");
}

#[cfg(not(feature = "servo"))]
#[tokio::test]
async fn a_build_that_cannot_render_html_seeds_nothing() {
    let install = Install::with_bundled(FIRST_RUN_FAVORITES).await;
    let mut state = install.start().await;

    // The picks are registered, but this build has no renderer for them.
    let catalog = state.effect_registry.read().await;
    for id in ids(FIRST_RUN_FAVORITES) {
        assert!(catalog.get(&id).is_some(), "fixture {id} should register");
    }
    drop(catalog);
    assert_eq!(primary_effect(&state).await, None);
    assert!(favorite_ids(&state).await.is_empty());
    assert!(!install.library_path().exists());

    state.shutdown().await.expect("shutdown should succeed");
}

/// Seeds can only land where the build renders HTML effects.
#[cfg(feature = "servo")]
mod renderable {
    use std::path::Path;

    use hypercolor_daemon::domain::scene_tree::{ClearScene, clear_scene};
    use hypercolor_daemon::first_run::FIRST_RUN_SCENE_EFFECT;
    use hypercolor_daemon::runtime_state::{self, RuntimeSessionSnapshot};
    use hypercolor_types::layer::LayerSource;

    use super::*;

    impl Install {
        fn initialize(&self) -> DaemonState {
            self.initialize_with(quiet_config())
        }

        fn runtime_state_path(&self) -> PathBuf {
            self.root.path().join("state").join("runtime-state.json")
        }
    }

    fn write_json(path: &Path, value: &serde_json::Value) {
        std::fs::create_dir_all(path.parent().expect("fixture path should have a parent"))
            .expect("fixture directory should build");
        std::fs::write(
            path,
            serde_json::to_vec_pretty(value).expect("fixture should serialize"),
        )
        .expect("fixture should write");
    }

    #[tokio::test]
    async fn a_fresh_install_starts_with_hyperbloom_and_the_curated_favorites() {
        let install = Install::with_bundled(FIRST_RUN_FAVORITES).await;
        let mut state = install.start().await;

        let hyperbloom = bundled_html_effect_id(FIRST_RUN_SCENE_EFFECT);
        assert_eq!(
            hyperbloom.to_string(),
            "040ddf5e-bd07-4164-a71c-6415936be19b"
        );
        assert_eq!(primary_effect(&state).await, Some(hyperbloom));
        {
            let scenes = state.scene_manager.snapshot().await;
            let zone = scenes
                .active_scene()
                .and_then(|scene| scene.primary_zone())
                .expect("default scene should have a primary zone");
            let [layer] = zone.layers.as_slice() else {
                panic!(
                    "primary zone should hold one layer, found {:?}",
                    zone.layers
                );
            };
            let LayerSource::Effect { controls, .. } = &layer.source else {
                panic!("primary layer should run an effect");
            };
            // No overrides: the zone renders the effect's own defaults.
            assert!(controls.is_empty());
        }

        assert_eq!(favorite_ids(&state).await, ids(FIRST_RUN_FAVORITES));
        // Both seeds leave their durable store behind, so neither repeats.
        assert!(install.runtime_state_path().exists());
        assert!(install.library_path().exists());

        state.shutdown().await.expect("shutdown should succeed");
    }

    #[tokio::test]
    async fn seeds_apply_once_and_never_return_after_the_user_clears_them() {
        let install = Install::with_bundled(FIRST_RUN_FAVORITES).await;
        let hyperbloom = bundled_html_effect_id(FIRST_RUN_SCENE_EFFECT);

        let mut first = install.start().await;
        let seeded = first.library_store().list_favorites().await;
        assert_eq!(seeded.len(), FIRST_RUN_FAVORITES.len());
        first
            .shutdown()
            .await
            .expect("first shutdown should succeed");
        drop(first);

        // A restart restores the seeded session and leaves the library as
        // it was: same entries, same timestamps, nothing added.
        let mut second = install.start().await;
        assert_eq!(primary_effect(&second).await, Some(hyperbloom));
        assert_eq!(second.library_store().list_favorites().await, seeded);

        for favorite in &seeded {
            assert!(
                second
                    .library_store()
                    .remove_favorite(favorite.effect_id)
                    .await
                    .expect("favorite removal should persist")
            );
        }
        clear_scene(
            &second.domains.scene_tree,
            ClearScene {
                zone: None,
                fence: None,
            },
        )
        .await
        .expect("scene clear should commit");
        assert_eq!(primary_effect(&second).await, None);
        second
            .shutdown()
            .await
            .expect("second shutdown should succeed");
        drop(second);

        let mut third = install.start().await;
        assert_eq!(primary_effect(&third).await, None);
        assert!(favorite_ids(&third).await.is_empty());
        third
            .shutdown()
            .await
            .expect("third shutdown should succeed");
    }

    /// No seeds at all: the scene stays empty and the favorites list stays
    /// empty.
    async fn assert_unseeded(state: &DaemonState) {
        assert_eq!(primary_effect(state).await, None);
        assert!(favorite_ids(state).await.is_empty());
    }

    #[tokio::test]
    async fn an_existing_empty_session_gets_no_seeds() {
        let install = Install::with_bundled(FIRST_RUN_FAVORITES).await;
        runtime_state::save(
            &install.runtime_state_path(),
            &RuntimeSessionSnapshot {
                active_scene_id: Some(SceneId::DEFAULT.to_string()),
                default_scene_zones: Vec::new(),
                active_layout_id: None,
                manual_paused: false,
            },
        )
        .expect("runtime session should save");

        let mut state = install.start().await;

        // The user left this scene black, and an upgrade that never wrote
        // its library is still an existing install.
        assert_unseeded(&state).await;

        state.shutdown().await.expect("shutdown should succeed");
    }

    #[tokio::test]
    async fn a_legacy_session_counts_as_an_existing_install() {
        let install = Install::with_bundled(FIRST_RUN_FAVORITES).await;
        write_json(
            &install.data_path("runtime-state.json"),
            &serde_json::json!({
                "active_scene_id": SceneId::DEFAULT.to_string(),
                "default_scene_zones": [],
                "active_layout_id": null,
                "manual_paused": false,
            }),
        );

        let mut state = install.start().await;

        assert_unseeded(&state).await;

        state.shutdown().await.expect("shutdown should succeed");
    }

    #[tokio::test]
    async fn an_unreadable_session_is_never_replaced() {
        let install = Install::with_bundled(FIRST_RUN_FAVORITES).await;
        write_json(
            &install.runtime_state_path(),
            &serde_json::json!({ "retired_field": true }),
        );

        let mut state = install.start().await;

        assert_unseeded(&state).await;

        state.shutdown().await.expect("shutdown should succeed");
    }

    #[tokio::test]
    async fn a_session_written_after_initialization_is_never_replaced() {
        let install = Install::with_bundled(FIRST_RUN_FAVORITES).await;
        let mut state = install.initialize();
        // Initialization found no session, but one exists by the time the
        // daemon restores; it fails to parse and still blocks the scene
        // seed, so nothing replaces it.
        write_json(
            &install.runtime_state_path(),
            &serde_json::json!({ "retired_field": true }),
        );
        state.start().await.expect("daemon should start");

        assert_eq!(primary_effect(&state).await, None);

        state.shutdown().await.expect("shutdown should succeed");
    }

    #[tokio::test]
    async fn a_named_scene_store_blocks_the_seeds() {
        let install = Install::with_bundled(FIRST_RUN_FAVORITES).await;
        write_json(
            &install.data_path("scenes.json"),
            &serde_json::json!({ "schema_version": 2, "scenes": {} }),
        );

        let mut state = install.start().await;

        assert_unseeded(&state).await;

        state.shutdown().await.expect("shutdown should succeed");
    }

    #[tokio::test]
    async fn an_existing_empty_library_gets_no_seeds() {
        let install = Install::with_bundled(FIRST_RUN_FAVORITES).await;
        write_json(
            &install.library_path(),
            &serde_json::json!({
                "version": 2,
                "favorites": [],
                "favorite_revisions": [],
                "presets": [],
                "playlists": [],
            }),
        );

        let mut state = install.start().await;

        assert_unseeded(&state).await;

        state.shutdown().await.expect("shutdown should succeed");
    }

    #[tokio::test]
    async fn a_library_whose_favorites_were_all_removed_stays_empty() {
        let install = Install::with_bundled(FIRST_RUN_FAVORITES).await;
        write_json(
            &install.library_path(),
            &serde_json::json!({
                "version": 2,
                "favorites": [],
                "favorite_revisions": [{
                    "effect_id": bundled_html_effect_id("borealis"),
                    "revision": 2,
                }],
                "presets": [],
                "playlists": [],
            }),
        );

        let mut state = install.start().await;

        assert_unseeded(&state).await;

        state.shutdown().await.expect("shutdown should succeed");
    }

    #[tokio::test]
    async fn missing_picks_are_skipped_and_the_rest_keep_their_order() {
        let install = Install::with_bundled(&["spectral-fire", "borealis"]).await;
        let mut state = install.start().await;

        assert_eq!(
            favorite_ids(&state).await,
            ids(&["borealis", "spectral-fire"])
        );
        // Without Hyperbloom the scene starts exactly as it did before.
        assert_eq!(primary_effect(&state).await, None);

        state.shutdown().await.expect("shutdown should succeed");
    }

    #[tokio::test]
    async fn the_scene_seed_needs_only_its_own_pick() {
        let install = Install::with_bundled(&[FIRST_RUN_SCENE_EFFECT]).await;
        let mut state = install.start().await;

        assert_eq!(
            primary_effect(&state).await,
            Some(bundled_html_effect_id(FIRST_RUN_SCENE_EFFECT))
        );
        assert_eq!(favorite_ids(&state).await, ids(&[FIRST_RUN_SCENE_EFFECT]));

        state.shutdown().await.expect("shutdown should succeed");
    }

    #[tokio::test]
    async fn a_display_face_is_never_a_pick() {
        let install =
            Install::with_fixtures(&[(FIRST_RUN_SCENE_EFFECT, "display"), ("borealis", "ambient")])
                .await;
        let mut state = install.start().await;

        assert_eq!(primary_effect(&state).await, None);
        assert_eq!(favorite_ids(&state).await, ids(&["borealis"]));

        state.shutdown().await.expect("shutdown should succeed");
    }

    #[tokio::test]
    async fn a_configured_start_scene_skips_only_the_scene_seed() {
        let install = Install::with_bundled(FIRST_RUN_FAVORITES).await;
        let mut config = quiet_config();
        config.daemon.start_scene = "default".to_owned();
        let mut state = install.start_with(config).await;

        // Only "last" restores a session, so only it starts the curated one.
        assert_eq!(primary_effect(&state).await, None);
        assert_eq!(favorite_ids(&state).await, ids(FIRST_RUN_FAVORITES));

        state.shutdown().await.expect("shutdown should succeed");
    }
}
