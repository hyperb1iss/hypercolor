//! Tests for the configuration manager and path resolution.

use std::fs;
use std::sync::{Arc, Mutex, MutexGuard};

use hypercolor_core::config::ConfigManager;
use hypercolor_types::config::{
    CURRENT_SCHEMA_VERSION, DEFAULT_BEAT_SENSITIVITY, DEFAULT_TARGET_FPS,
    EffectErrorFallbackPolicy, HypercolorConfig, InteractionRoutePolicy,
};

// ─── TOML Parsing ───────────────────────────────────────────────────────────

#[test]
fn load_minimal_toml() {
    let toml = r"
        schema_version = 7
    ";

    let tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    fs::write(tmp.path(), toml).expect("failed to write temp file");

    let config = ConfigManager::load(tmp.path()).expect("minimal TOML should parse without error");

    assert_eq!(config.schema_version, 7);
    // Sections should fall back to their serde defaults
    assert_eq!(config.daemon.port, 9420);
    assert_eq!(config.daemon.target_fps, 60);
    assert!(config.web.enabled);
    assert_eq!(config.input.daemon_route, InteractionRoutePolicy::Host);
    assert_eq!(config.input.preview_route, InteractionRoutePolicy::Browser);
}

#[test]
fn schema_v4_renames_start_profile_before_deserialization() {
    let config =
        ConfigManager::parse_toml("schema_version = 4\n[daemon]\nstart_profile = \"Evening\"\n")
            .expect("schema v4 should migrate");

    assert_eq!(config.schema_version, 7);
    assert_eq!(config.daemon.start_scene, "Evening");
}

#[test]
fn schema_v4_renames_clear_groups_fallback_before_deserialization() {
    let config = ConfigManager::parse_toml(
        "schema_version = 4\n[effect_engine]\neffect_error_fallback = \"clear_groups\"\n",
    )
    .expect("schema v4 should migrate");

    assert_eq!(
        config.effect_engine.effect_error_fallback,
        EffectErrorFallbackPolicy::ClearZones
    );
}

#[test]
fn schema_v5_refuses_the_retired_clear_groups_fallback() {
    let error = ConfigManager::parse_toml(
        "schema_version = 5\n[effect_engine]\neffect_error_fallback = \"clear_groups\"\n",
    )
    .expect_err("schema v5 must reject the retired fallback value");

    assert!(
        format!("{error:#}").contains("unknown variant `clear_groups`"),
        "{error:#}"
    );
}

#[test]
fn schema_v5_moves_the_retired_beat_default_to_the_current_one() {
    for version in [4, 5] {
        let config = ConfigManager::parse_toml(&format!(
            "schema_version = {version}\n[audio]\nbeat_sensitivity = 0.6\n"
        ))
        .expect("older schemas should migrate");

        assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(
            (config.audio.beat_sensitivity - DEFAULT_BEAT_SENSITIVITY).abs() < f32::EPSILON,
            "schema {version} kept {}",
            config.audio.beat_sensitivity
        );
    }
}

/// A v5 save serialized the f32 default, which TOML may spell as
/// 0.6000000238418579 rather than 0.6. The migration has to match both.
#[test]
fn schema_v5_moves_the_beat_default_exactly_as_a_save_wrote_it() {
    let mut saved = HypercolorConfig {
        schema_version: 5,
        ..HypercolorConfig::default()
    };
    saved.audio.beat_sensitivity = 0.6;
    let written = toml::to_string_pretty(&saved).expect("serialize a v5 save");

    let config = ConfigManager::parse_toml(&written).expect("a v5 save should migrate");

    assert!((config.audio.beat_sensitivity - DEFAULT_BEAT_SENSITIVITY).abs() < f32::EPSILON);
}

#[test]
fn schema_v5_keeps_a_beat_threshold_someone_chose() {
    let config = ConfigManager::parse_toml("schema_version = 5\n[audio]\nbeat_sensitivity = 0.9\n")
        .expect("schema v5 should migrate");

    assert!((config.audio.beat_sensitivity - 0.9).abs() < f32::EPSILON);
}

#[test]
fn schema_v6_and_later_keep_a_beat_threshold_of_point_six() {
    for version in [6, CURRENT_SCHEMA_VERSION] {
        let config = ConfigManager::parse_toml(&format!(
            "schema_version = {version}\n[audio]\nbeat_sensitivity = 0.6\n"
        ))
        .expect("schema v6 and later should parse");

        assert!(
            (config.audio.beat_sensitivity - 0.6).abs() < f32::EPSILON,
            "schema {version} moved {}",
            config.audio.beat_sensitivity
        );
    }
}

#[test]
fn schema_v6_moves_the_retired_render_target_to_the_current_one() {
    for version in [4, 5, 6] {
        let config = ConfigManager::parse_toml(&format!(
            "schema_version = {version}\n[daemon]\ntarget_fps = 30\n"
        ))
        .expect("older schemas should migrate");

        assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(
            config.daemon.target_fps, DEFAULT_TARGET_FPS,
            "schema {version} kept the retired render target"
        );
    }
}

/// A v6 save wrote the whole document, so the retired default sits on disk
/// as an explicit `target_fps = 30` that nobody chose.
#[test]
fn schema_v6_moves_the_render_target_exactly_as_a_save_wrote_it() {
    let mut saved = HypercolorConfig {
        schema_version: 6,
        ..HypercolorConfig::default()
    };
    saved.daemon.target_fps = 30;
    let written = toml::to_string_pretty(&saved).expect("serialize a v6 save");
    assert!(written.contains("target_fps = 30"), "{written}");

    let config = ConfigManager::parse_toml(&written).expect("a v6 save should migrate");

    assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(config.daemon.target_fps, DEFAULT_TARGET_FPS);
}

#[test]
fn schema_v6_keeps_a_render_target_someone_chose() {
    for chosen in [10, 20, 45, 60, 120] {
        let config = ConfigManager::parse_toml(&format!(
            "schema_version = 6\n[daemon]\ntarget_fps = {chosen}\n"
        ))
        .expect("schema v6 should migrate");

        assert_eq!(config.daemon.target_fps, chosen);
    }
}

#[test]
fn current_schema_keeps_a_render_target_of_thirty() {
    let config = ConfigManager::parse_toml(&format!(
        "schema_version = {CURRENT_SCHEMA_VERSION}\n[daemon]\ntarget_fps = 30\n"
    ))
    .expect("current schema should parse");

    assert_eq!(config.daemon.target_fps, 30);
}

/// Loading never rewrites the file; the migrated value reaches disk with
/// the next save, stamped with the current schema so it is never migrated
/// again.
#[test]
fn a_migrated_render_target_reaches_disk_on_the_next_save() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("hypercolor.toml");
    fs::write(&path, "schema_version = 6\n\n[daemon]\ntarget_fps = 30\n")
        .expect("failed to write v6 config");

    let manager = ConfigManager::new(path.clone()).expect("a v6 file should load");
    assert_eq!(manager.get().daemon.target_fps, DEFAULT_TARGET_FPS);
    assert!(manager.save().expect("save should succeed").is_durable());

    let written = fs::read_to_string(&path).expect("saved config should read");
    let on_disk: toml::Value = toml::from_str(&written).expect("saved config should parse");
    assert_eq!(
        on_disk
            .get("schema_version")
            .and_then(toml::Value::as_integer),
        Some(i64::from(CURRENT_SCHEMA_VERSION))
    );
    assert_eq!(
        on_disk
            .get("daemon")
            .and_then(|daemon| daemon.get("target_fps"))
            .and_then(toml::Value::as_integer),
        Some(i64::from(DEFAULT_TARGET_FPS))
    );
}

/// A v4 file runs every migration in order: the start-key rename, the
/// beat threshold, and the render target.
#[test]
fn schema_v4_chains_through_every_later_migration() {
    let config = ConfigManager::parse_toml(concat!(
        "schema_version = 4\n\n",
        "[daemon]\n",
        "start_profile = \"Evening\"\n",
        "target_fps = 30\n\n",
        "[audio]\n",
        "beat_sensitivity = 0.6\n",
    ))
    .expect("schema v4 should migrate");

    assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(config.daemon.start_scene, "Evening");
    assert_eq!(config.daemon.target_fps, DEFAULT_TARGET_FPS);
    assert!((config.audio.beat_sensitivity - DEFAULT_BEAT_SENSITIVITY).abs() < f32::EPSILON);
}

#[test]
fn schema_v4_refuses_conflicting_start_keys() {
    let error = ConfigManager::parse_toml(
        "schema_version = 4\n[daemon]\nstart_profile = \"A\"\nstart_scene = \"B\"\n",
    )
    .expect_err("ambiguous startup targets must be refused");

    assert!(format!("{error:#}").contains("both daemon.start_profile and daemon.start_scene"));
}

#[test]
fn outdated_schema_is_refused_and_names_the_file_and_the_fix() {
    let tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    fs::write(
        tmp.path(),
        "schema_version = 3\n\n[input]\nenabled = true\n",
    )
    .expect("failed to write outdated config");

    let error = ConfigManager::load(tmp.path())
        .expect_err("an outdated schema must be refused, never migrated");
    let rendered = format!("{error:#}");

    assert!(
        rendered.contains(&tmp.path().display().to_string()),
        "{rendered}"
    );
    assert!(rendered.contains("schema_version 3"), "{rendered}");
    // Every edit the hand-migration needs, verbatim. Bumping the version
    // without the routes silently adopts the new daemon_route default.
    assert!(rendered.contains("schema_version = 4"), "{rendered}");
    assert!(rendered.contains(r#"daemon_route = "merge""#), "{rendered}");
    assert!(
        rendered.contains(r#"preview_route = "browser""#),
        "{rendered}"
    );
}

/// The hand-migration lands an old file on the oldest schema the chain
/// still upgrades, so later migrations run on it. Jumping straight to the
/// current schema would skip them and keep a retired beat threshold and
/// render target.
#[test]
fn following_the_hand_migration_runs_every_later_migration() {
    let old =
        "schema_version = 3\n\n[daemon]\ntarget_fps = 30\n\n[audio]\nbeat_sensitivity = 0.6\n";
    let error = ConfigManager::parse_toml(old).expect_err("schema 3 must be refused");
    let rendered = format!("{error:#}");
    assert!(rendered.contains("schema_version = 4"), "{rendered}");

    let edited = old.replace("schema_version = 3", "schema_version = 4")
        + "\n[input]\ndaemon_route = \"merge\"\npreview_route = \"browser\"\n";
    let config = ConfigManager::parse_toml(&edited).expect("the hand-migrated file loads");

    assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
    assert!((config.audio.beat_sensitivity - DEFAULT_BEAT_SENSITIVITY).abs() < f32::EPSILON);
    assert_eq!(config.daemon.target_fps, DEFAULT_TARGET_FPS);
    assert_eq!(config.input.daemon_route, InteractionRoutePolicy::Merge);
    assert_eq!(config.input.preview_route, InteractionRoutePolicy::Browser);
}

#[test]
fn newer_schema_is_refused_as_written_by_a_newer_hypercolor() {
    let tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    fs::write(tmp.path(), "schema_version = 8\n").expect("failed to write future config");

    let error = ConfigManager::load(tmp.path())
        .expect_err("a future schema must be refused, never guessed at");
    let rendered = format!("{error:#}");

    assert!(
        rendered.contains(&tmp.path().display().to_string()),
        "{rendered}"
    );
    assert!(rendered.contains("schema_version 8"), "{rendered}");
    assert!(rendered.contains("newer hypercolor"), "{rendered}");
    // A future file is not an old file: no hand-migration is offered.
    assert!(
        !rendered.contains(r#"daemon_route = "merge""#),
        "{rendered}"
    );
}

#[test]
fn current_schema_keeps_explicit_route_fields() {
    let tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    fs::write(
        tmp.path(),
        concat!(
            "schema_version = 7\n\n",
            "[input]\n",
            "daemon_route = \"host\"\n",
            "preview_route = \"merge\"\n",
        ),
    )
    .expect("failed to write config");

    let config = ConfigManager::load(tmp.path()).expect("current-schema config should load");

    assert_eq!(config.schema_version, 7);
    assert_eq!(config.input.daemon_route, InteractionRoutePolicy::Host);
    assert_eq!(config.input.preview_route, InteractionRoutePolicy::Merge);
}

#[test]
fn load_full_toml_with_overrides() {
    let toml = r#"
        schema_version = 5
        include = ["local.toml"]

        [daemon]
        listen_address = "0.0.0.0"
        port = 8080
        target_fps = 120
        canvas_width = 640
        canvas_height = 400

        [web]
        enabled = false
        websocket_fps = 15

        [audio]
        device = "pulse-monitor"
        fft_size = 2048

        [features]
        wasm_plugins = true
        midi_input = true

        [drivers.openrgb]
        enabled = false
        endpoints = ["127.0.0.1:6742"]
    "#;

    let tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    fs::write(tmp.path(), toml).expect("failed to write temp file");

    let config = ConfigManager::load(tmp.path()).expect("full TOML should parse without error");

    assert_eq!(config.daemon.listen_address, "0.0.0.0");
    assert_eq!(config.daemon.port, 8080);
    assert_eq!(config.daemon.target_fps, 120);
    assert_eq!(config.daemon.canvas_width, 640);
    assert_eq!(config.daemon.canvas_height, 400);
    assert!(!config.web.enabled);
    assert_eq!(config.web.websocket_fps, 15);
    assert_eq!(config.audio.device, "pulse-monitor");
    assert_eq!(config.audio.fft_size, 2048);
    assert!(!config.drivers["openrgb"].enabled);
    assert_eq!(
        config.drivers["openrgb"].settings["endpoints"],
        serde_json::json!(["127.0.0.1:6742"])
    );
    assert!(
        config.extensions.contains_key("include"),
        "a retired top-level key is preserved verbatim"
    );
}

#[test]
fn load_canonicalizes_legacy_audio_device_ids() {
    let toml = r#"
        schema_version = 5

        [audio]
        device = "mic"
    "#;

    let tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    fs::write(tmp.path(), toml).expect("failed to write temp file");

    let config = ConfigManager::load(tmp.path()).expect("legacy TOML should parse without error");

    assert_eq!(config.audio.device, "microphone");
}

#[test]
fn load_refuses_unknown_keys_and_names_the_file_and_each_key_path() {
    let tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    fs::write(
        tmp.path(),
        "schema_version = 5\n\n[daemon]\nprot = 9420\n\n\
         [rendering.servo_gpu_import]\nmdoe = \"auto\"\n",
    )
    .expect("failed to write misspelled config");

    let error = ConfigManager::load(tmp.path())
        .expect_err("a key the schema does not model must be refused, never dropped");
    let rendered = format!("{error:#}");

    assert!(
        rendered.contains(&tmp.path().display().to_string()),
        "{rendered}"
    );
    assert!(
        rendered.contains("unknown config keys `daemon.prot`, `rendering.servo_gpu_import.mdoe`"),
        "{rendered}"
    );
}

#[test]
fn load_invalid_toml_returns_error() {
    let tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    fs::write(tmp.path(), "not valid { toml [[[").expect("failed to write temp file");

    let result = ConfigManager::load(tmp.path());
    assert!(result.is_err());
}

#[test]
fn load_nonexistent_file_returns_error() {
    let result = ConfigManager::load(std::path::Path::new("/tmp/hypercolor_does_not_exist.toml"));
    assert!(result.is_err());
}

// ─── ConfigManager Lifecycle ────────────────────────────────────────────────

#[test]
fn new_with_missing_file_uses_defaults() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("nonexistent.toml");

    let manager =
        ConfigManager::new(path).expect("ConfigManager should fall back to defaults gracefully");
    let config = manager.get();

    assert_eq!(config.schema_version, 7);
    assert_eq!(config.daemon.port, 9420);
    assert_eq!(config.daemon.target_fps, 60);
    assert!(config.web.enabled);
}

#[test]
fn new_with_valid_file_loads_it() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("hypercolor.toml");
    fs::write(
        &path,
        r"
        schema_version = 5

        [daemon]
        port = 7777
    ",
    )
    .expect("failed to write config file");

    let manager = ConfigManager::new(path).expect("ConfigManager should load the file");
    let config = manager.get();

    assert_eq!(config.daemon.port, 7777);
}

#[test]
fn update_canonicalizes_legacy_audio_device_ids() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("hypercolor.toml");

    let manager = ConfigManager::new(path).expect("ConfigManager should use defaults");
    let mut config = manager.get().as_ref().clone();
    config.audio.device = "auto".to_owned();

    manager.update(config);

    assert_eq!(manager.get().audio.device, "default");
}

#[test]
fn config_snapshot_identity_fences_stale_preparation() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("hypercolor.toml");
    let manager = ConfigManager::new(path).expect("ConfigManager should use defaults");
    let first = Arc::clone(&manager.get());

    assert!(manager.is_current(&first));
    manager.modify(|config| config.audio.enabled = !config.audio.enabled);

    assert!(!manager.is_current(&first));
    let current = Arc::clone(&manager.get());
    assert!(manager.is_current(&current));
}

#[test]
fn conditional_config_mutation_rejects_stale_identity_without_side_effects() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("hypercolor.toml");
    let manager = ConfigManager::new(path).expect("ConfigManager should use defaults");
    let stale = Arc::clone(&manager.get());
    manager.modify(|config| config.daemon.port = 7777);

    let applied = manager.modify_if_current(&stale, |config| config.daemon.port = 8888);

    assert!(!applied);
    assert_eq!(manager.get().daemon.port, 7777);
    let current = Arc::clone(&manager.get());
    assert!(manager.modify_if_current(&current, |config| config.daemon.port = 9999));
    assert_eq!(manager.get().daemon.port, 9999);
}

#[test]
fn conditional_save_returns_the_exact_installed_snapshot() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("hypercolor.toml");
    let manager = ConfigManager::new(path).expect("ConfigManager should use defaults");
    let before = Arc::clone(&manager.get());

    let installed = manager
        .modify_and_save_if_current_snapshot(&before, |config| config.daemon.port = 7777)
        .expect("conditional save should persist")
        .expect("current snapshot should be replaced");

    assert!(manager.is_current(&installed));
    assert_eq!(installed.daemon.port, 7777);
    manager.modify(|config| config.daemon.port = 8888);
    assert!(!manager.is_current(&installed));
    assert_eq!(installed.daemon.port, 7777);
    assert_eq!(manager.get().daemon.port, 8888);
}

#[test]
fn new_with_invalid_file_returns_error() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("broken.toml");
    fs::write(&path, "{{{{broken").expect("failed to write config file");

    let result = ConfigManager::new(path);
    assert!(result.is_err());
}

// ─── Reload ─────────────────────────────────────────────────────────────────

#[test]
fn reload_picks_up_file_changes() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("hypercolor.toml");

    // Write initial config
    fs::write(
        &path,
        r"
        schema_version = 5

        [daemon]
        port = 9420
    ",
    )
    .expect("failed to write initial config");

    let manager = ConfigManager::new(path.clone()).expect("initial load should succeed");
    assert_eq!(manager.get().daemon.port, 9420);

    // Overwrite with new port
    fs::write(
        &path,
        r"
        schema_version = 5

        [daemon]
        port = 1234
    ",
    )
    .expect("failed to write updated config");

    manager.reload().expect("reload should succeed");
    assert_eq!(manager.get().daemon.port, 1234);
}

#[test]
fn reload_preserves_old_config_on_parse_error() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("hypercolor.toml");

    fs::write(
        &path,
        r"
        schema_version = 5

        [daemon]
        port = 5555
    ",
    )
    .expect("failed to write initial config");

    let manager = ConfigManager::new(path.clone()).expect("initial load should succeed");
    assert_eq!(manager.get().daemon.port, 5555);

    // Corrupt the file
    fs::write(&path, "{{not valid toml").expect("failed to corrupt config");

    let result = manager.reload();
    assert!(result.is_err());

    // Old config should still be live
    assert_eq!(manager.get().daemon.port, 5555);
}

#[test]
fn reload_serializes_with_an_inflight_config_writer() {
    use std::sync::mpsc;
    use std::time::Duration;

    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("hypercolor.toml");
    fs::write(
        &path,
        r"
        schema_version = 5

        [daemon]
        port = 7000
    ",
    )
    .expect("failed to write reload config");
    let manager = Arc::new(ConfigManager::new(path).expect("initial load should succeed"));
    let (writer_entered_tx, writer_entered_rx) = mpsc::channel();
    let (release_writer_tx, release_writer_rx) = mpsc::channel();
    let writer_manager = Arc::clone(&manager);
    let writer = std::thread::spawn(move || {
        writer_manager.modify(|config| {
            writer_entered_tx
                .send(())
                .expect("writer entry is observed");
            release_writer_rx.recv().expect("writer is released");
            config.daemon.port = 8000;
        });
    });
    writer_entered_rx
        .recv()
        .expect("writer acquired serialization lock");

    let (reload_started_tx, reload_started_rx) = mpsc::channel();
    let (reload_finished_tx, reload_finished_rx) = mpsc::channel();
    let reload_manager = Arc::clone(&manager);
    let reload = std::thread::spawn(move || {
        reload_started_tx
            .send(())
            .expect("reload attempt is observed");
        let result = reload_manager.reload();
        reload_finished_tx
            .send(result)
            .expect("reload completion is observed");
    });
    reload_started_rx
        .recv()
        .expect("reload thread reached the writer");
    assert!(
        reload_finished_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "reload must wait for the active config writer"
    );

    release_writer_tx.send(()).expect("writer release succeeds");
    writer.join().expect("writer thread completes");
    reload_finished_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("reload completes after writer release")
        .expect("reload succeeds");
    reload.join().expect("reload thread completes");
    assert_eq!(manager.get().daemon.port, 7000);
}

// ─── Path Resolution ────────────────────────────────────────────────────────

static PATH_OVERRIDE_LOCK: Mutex<()> = Mutex::new(());

struct PathOverrideTestGuard {
    _lock: MutexGuard<'static, ()>,
}

impl PathOverrideTestGuard {
    fn acquire() -> Self {
        let lock = PATH_OVERRIDE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ConfigManager::set_data_dir_override(None);
        ConfigManager::set_state_dir_override(None);
        Self { _lock: lock }
    }
}

impl Drop for PathOverrideTestGuard {
    fn drop(&mut self) {
        ConfigManager::set_state_dir_override(None);
        ConfigManager::set_data_dir_override(None);
    }
}

#[test]
fn config_dir_ends_with_hypercolor() {
    let dir = ConfigManager::config_dir();
    assert_eq!(
        dir.file_name().and_then(|n| n.to_str()),
        Some("hypercolor"),
        "config dir should end with 'hypercolor', got: {dir:?}"
    );
}

#[test]
fn data_dir_ends_with_hypercolor() {
    let _guard = PathOverrideTestGuard::acquire();
    let dir = ConfigManager::data_dir();
    // On Windows the last component is "hypercolor" (under LocalAppData).
    // On Linux it's also "hypercolor" (under ~/.local/share).
    assert!(
        dir.to_string_lossy().contains("hypercolor"),
        "data dir should contain 'hypercolor', got: {dir:?}"
    );
}

#[test]
fn data_dir_override_replaces_default_resolution() {
    let _guard = PathOverrideTestGuard::acquire();
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let override_path = dir.path().join("override-data");

    ConfigManager::set_data_dir_override(Some(override_path.clone()));
    assert_eq!(ConfigManager::data_dir(), override_path);
}

#[test]
fn state_dir_uses_an_independent_override() {
    let _guard = PathOverrideTestGuard::acquire();
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let data_path = dir.path().join("data");
    let state_path = dir.path().join("state");

    ConfigManager::set_data_dir_override(Some(data_path.clone()));
    ConfigManager::set_state_dir_override(Some(state_path.clone()));
    assert_eq!(ConfigManager::data_dir(), data_path);
    assert_eq!(ConfigManager::state_dir(), state_path);
}

#[test]
fn state_dir_follows_a_relocated_data_dir_without_its_own_override() {
    let _guard = PathOverrideTestGuard::acquire();
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let data_path = dir.path().join("data");

    ConfigManager::set_data_dir_override(Some(data_path.clone()));
    assert_eq!(ConfigManager::state_dir(), data_path.join("state"));
}

#[test]
fn cache_dir_contains_hypercolor() {
    let dir = ConfigManager::cache_dir();
    assert!(
        dir.to_string_lossy().contains("hypercolor"),
        "cache dir should contain 'hypercolor', got: {dir:?}"
    );
}

#[test]
fn all_dirs_are_absolute() {
    let _guard = PathOverrideTestGuard::acquire();
    assert!(ConfigManager::config_dir().is_absolute());
    assert!(ConfigManager::data_dir().is_absolute());
    assert!(ConfigManager::state_dir().is_absolute());
    assert!(ConfigManager::cache_dir().is_absolute());
}

#[test]
fn servo_runtime_cache_dir_is_rooted_under_hypercolor() {
    let dir = hypercolor_core::config::paths::servo_runtime_cache_dir();
    assert!(dir.is_absolute());
    assert!(dir.ends_with(std::path::Path::new("hypercolor").join("servo-runtime")));
}

#[test]
fn user_output_dir_is_desktop_or_home() {
    let Some(dir) = hypercolor_core::config::paths::user_output_dir() else {
        return;
    };
    let home = hypercolor_core::config::paths::home_dir().expect("home resolves");
    assert!(dir.is_absolute());
    assert!(dir == home || dir.is_dir());
}

#[test]
fn macos_user_paths_live_under_the_home_library() {
    use hypercolor_core::config::paths;

    let home = paths::home_dir().expect("home resolves");
    assert_eq!(
        paths::macos_launch_agents_dir().expect("launch agents resolve"),
        home.join("Library").join("LaunchAgents")
    );
    assert_eq!(
        paths::macos_user_log_dir().expect("log dir resolves"),
        home.join("Library").join("Logs").join("hypercolor")
    );
    assert_eq!(
        paths::macos_user_applications_dir().expect("applications resolve"),
        home.join("Applications")
    );
}

// ─── Change Stream ──────────────────────────────────────────────────────────

mod change_stream {
    use std::fs;
    use std::sync::Arc;

    use hypercolor_core::bus::{HypercolorBus, TimestampedEvent};
    use hypercolor_core::config::{CapturePersistenceSource, ConfigManager};
    use hypercolor_types::event::HypercolorEvent;
    use serde_json::{Value, json};
    use tokio::sync::broadcast::Receiver;

    type ConfigChange = (String, Option<Value>, Value);

    /// A manager over a fresh temp path with the change stream attached
    /// and a subscriber opened before any write.
    fn attached_manager() -> (ConfigManager, Receiver<TimestampedEvent>, tempfile::TempDir) {
        let tempdir = tempfile::tempdir().expect("tempdir should build");
        let manager = ConfigManager::new(tempdir.path().join("hypercolor.toml"))
            .expect("config manager should build");
        let bus = Arc::new(HypercolorBus::new());
        let events = bus.subscribe_all();
        manager.attach_change_stream(bus);
        (manager, events, tempdir)
    }

    fn drain_config_changes(events: &mut Receiver<TimestampedEvent>) -> Vec<ConfigChange> {
        let mut changes = Vec::new();
        while let Ok(timestamped) = events.try_recv() {
            if let HypercolorEvent::ConfigChanged {
                key,
                old_value,
                new_value,
            } = timestamped.event
            {
                changes.push((key, old_value, new_value));
            }
        }
        changes
    }

    #[test]
    fn save_publishes_exactly_one_config_changed_for_one_key() {
        let (manager, mut events, _tempdir) = attached_manager();

        manager.modify(|config| config.daemon.target_fps = 45);
        assert!(manager.save().expect("save should succeed").is_durable());

        assert_eq!(
            drain_config_changes(&mut events),
            vec![("daemon.target_fps".to_owned(), Some(json!(60)), json!(45))]
        );
    }

    #[test]
    fn save_of_several_keys_publishes_their_shared_prefix_with_no_payload() {
        let (manager, mut events, _tempdir) = attached_manager();

        manager.modify(|config| {
            config.daemon.target_fps = 45;
            config.daemon.port = 9421;
        });
        assert!(manager.save().expect("save should succeed").is_durable());
        assert_eq!(
            drain_config_changes(&mut events),
            vec![("daemon".to_owned(), None, Value::Null)]
        );

        manager.modify(|config| {
            config.daemon.target_fps = 60;
            config.audio.device = "microphone".to_owned();
        });
        assert!(manager.save().expect("save should succeed").is_durable());
        assert_eq!(
            drain_config_changes(&mut events),
            vec![(String::new(), None, Value::Null)],
            "keys in different sections share only the empty prefix"
        );
    }

    #[test]
    fn unchanged_saves_publish_nothing() {
        let (manager, mut events, _tempdir) = attached_manager();

        assert!(
            manager
                .save()
                .expect("first save should succeed")
                .is_durable()
        );
        manager.modify(|config| config.daemon.target_fps = 45);
        assert!(
            manager
                .save()
                .expect("second save should succeed")
                .is_durable()
        );
        assert!(
            manager
                .save()
                .expect("third save should succeed")
                .is_durable()
        );

        assert_eq!(
            drain_config_changes(&mut events).len(),
            1,
            "only the save that changed the document fans out"
        );
    }

    #[test]
    fn secret_keys_publish_masked_on_both_sides() {
        let (manager, mut events, _tempdir) = attached_manager();

        manager.modify(|config| {
            config
                .extensions
                .insert("cloud".to_owned(), json!({ "token": "sk-live-first" }));
        });
        assert!(manager.save().expect("save should succeed").is_durable());
        assert_eq!(
            drain_config_changes(&mut events),
            vec![("cloud".to_owned(), None, json!({ "redacted": true }))],
            "a new secret section arrives as one masked leaf"
        );

        manager.modify(|config| {
            config
                .extensions
                .insert("cloud".to_owned(), json!({ "token": "sk-live-second" }));
        });
        assert!(manager.save().expect("save should succeed").is_durable());
        assert_eq!(
            drain_config_changes(&mut events),
            vec![(
                "cloud.token".to_owned(),
                Some(json!({ "redacted": true })),
                json!({ "redacted": true })
            )],
            "a rotated credential never fans out in either direction"
        );
    }

    #[test]
    fn modify_and_save_if_current_publishes_the_installed_document() {
        let (manager, mut events, _tempdir) = attached_manager();
        let expected = Arc::clone(&manager.get());

        let installed = manager
            .modify_and_save_if_current_snapshot(&expected, |config| {
                config.daemon.target_fps = 50;
            })
            .expect("save should succeed")
            .expect("snapshot should still be current");

        assert_eq!(installed.daemon.target_fps, 50);
        assert_eq!(manager.get().daemon.target_fps, 50);
        assert_eq!(
            drain_config_changes(&mut events),
            vec![("daemon.target_fps".to_owned(), Some(json!(60)), json!(50))]
        );
    }

    #[test]
    fn every_capture_persistence_path_publishes_exactly_once() {
        let (manager, mut events, _tempdir) = attached_manager();
        let base_fps = manager.get().capture.capture_fps;
        let capture_change = |step: u32| {
            (
                "capture.capture_fps".to_owned(),
                Some(json!(base_fps + step - 1)),
                json!(base_fps + step),
            )
        };

        let expected = Arc::clone(&manager.get());
        let epoch = manager
            .reserve_capture_persistence(&expected)
            .expect("epoch should reserve");
        let mut capture = expected.capture.clone();
        capture.capture_fps = base_fps + 1;
        manager
            .save_capture_and_activate_if_current(&expected, epoch, None, capture)
            .expect("save should succeed")
            .expect("snapshot should still be current");
        assert_eq!(drain_config_changes(&mut events), vec![capture_change(1)]);

        manager
            .modify_capture_if_epoch_current(epoch, |capture| capture.capture_fps = base_fps + 2)
            .expect("save should succeed")
            .expect("epoch should still be current");
        assert_eq!(drain_config_changes(&mut events), vec![capture_change(2)]);

        let source = CapturePersistenceSource::new(Arc::from("screen:0"), 1, 1);
        manager
            .modify_capture_if_authorized(epoch, source, |capture| {
                capture.capture_fps = base_fps + 3;
            })
            .expect("save should succeed")
            .expect("epoch should still be authorized");
        assert_eq!(drain_config_changes(&mut events), vec![capture_change(3)]);

        let expected = Arc::clone(&manager.get());
        let mut capture = expected.capture.clone();
        capture.capture_fps = base_fps + 4;
        let staged = manager
            .stage_capture_config(&expected, capture)
            .expect("stage should succeed")
            .expect("snapshot should still be current");
        manager
            .commit_staged_capture_if_current(&expected, None, staged, |install| install())
            .expect("commit should succeed")
            .expect("snapshot should still be current");
        assert_eq!(drain_config_changes(&mut events), vec![capture_change(4)]);
    }

    #[test]
    fn reload_publishes_an_external_edit() {
        let (manager, mut events, _tempdir) = attached_manager();

        fs::write(
            manager.path(),
            "schema_version = 5\n[daemon]\ntarget_fps = 50\n",
        )
        .expect("external edit should write");
        manager.reload().expect("reload should succeed");

        assert_eq!(
            drain_config_changes(&mut events),
            vec![("daemon.target_fps".to_owned(), Some(json!(60)), json!(50))]
        );
    }

    #[test]
    fn a_manager_without_a_stream_saves_silently() {
        let tempdir = tempfile::tempdir().expect("tempdir should build");
        let manager = ConfigManager::new(tempdir.path().join("hypercolor.toml"))
            .expect("config manager should build");
        let bus = Arc::new(HypercolorBus::new());
        let mut events = bus.subscribe_all();

        manager.modify(|config| config.daemon.target_fps = 45);
        assert!(manager.save().expect("save should succeed").is_durable());

        assert!(drain_config_changes(&mut events).is_empty());
    }
}
