//! Coverage for the single load pipeline, overlay precedence,
//! provenance, the Boot/Live split, and restart reporting
//! (Spec 76 §3.1–§3.2).

use std::io::Write;

use hypercolor_core::config::{
    CliOverrides, ConfigManager, ConfigSources, EnvOverrides, SourceLayer,
};
use hypercolor_types::config::{HypercolorConfig, RenderAccelerationMode};

fn write_config(dir: &tempfile::TempDir, body: &str) -> std::path::PathBuf {
    let path = dir.path().join("hypercolor.toml");
    let mut file = std::fs::File::create(&path).expect("config file creates");
    writeln!(file, "schema_version = 5").expect("writes");
    file.write_all(body.as_bytes()).expect("writes");
    path
}

fn sources_for(path: std::path::PathBuf) -> ConfigSources {
    ConfigSources {
        file: Some(path),
        cli: CliOverrides::default(),
        env: EnvOverrides::default(),
        seed: None,
    }
}

#[test]
fn explicit_missing_file_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sources = sources_for(dir.path().join("absent.toml"));
    let error = ConfigManager::load_with_sources(sources).expect_err("missing file must fail");
    assert!(error.to_string().contains("absent.toml"));
}

#[test]
fn precedence_is_cli_over_env_over_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_config(
        &dir,
        "[effect_engine]\ncompositor_acceleration_mode = \"cpu\"\n[daemon]\nport = 9420\n",
    );
    let sources = ConfigSources {
        file: Some(path),
        cli: CliOverrides {
            compositor_acceleration_mode: Some(RenderAccelerationMode::Auto),
            servo_gpu_import_mode: None,
        },
        env: EnvOverrides {
            values: vec![
                ("daemon.port".to_owned(), serde_json::json!(9999)),
                (
                    "effect_engine.compositor_acceleration_mode".to_owned(),
                    serde_json::json!("cpu"),
                ),
            ],
        },
        seed: None,
    };
    let loaded = ConfigManager::load_with_sources(sources).expect("loads");

    // CLI beat the env overlay and the file for the mode; env beat
    // the file for the port.
    assert_eq!(
        loaded.boot.effect_engine.compositor_acceleration_mode,
        RenderAccelerationMode::Auto
    );
    assert_eq!(loaded.boot.daemon.port, 9999);
    assert_eq!(
        loaded
            .provenance
            .layer_for("effect_engine.compositor_acceleration_mode"),
        SourceLayer::Cli
    );
    assert_eq!(loaded.provenance.layer_for("daemon.port"), SourceLayer::Env);
    assert_eq!(
        loaded.provenance.layer_for("daemon.target_fps"),
        SourceLayer::Persisted
    );

    // The manager and the boot config agree at load time.
    assert_eq!(loaded.manager.live().daemon.port, 9999);
}

#[test]
fn seed_hook_runs_before_overlays() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_config(&dir, "");
    fn seed(config: &mut HypercolorConfig) {
        config.drivers.entry("wled".to_owned()).or_default();
    }
    let mut sources = sources_for(path);
    sources.seed = Some(seed);
    let loaded = ConfigManager::load_with_sources(sources).expect("loads");
    assert!(loaded.boot.drivers.contains_key("wled"));
}

#[test]
fn invalid_env_overlay_key_fails_the_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_config(&dir, "");
    let mut sources = sources_for(path);
    sources.env.values = vec![("daemon.port".to_owned(), serde_json::json!("not a number"))];
    let error = ConfigManager::load_with_sources(sources).expect_err("bad overlay must fail");
    assert!(error.to_string().contains("daemon.port"));
}

#[test]
fn malformed_overlay_keys_are_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_config(&dir, "");
    let mut sources = sources_for(path);
    sources.env.values = vec![("audio.".to_owned(), serde_json::json!("x"))];
    let error = ConfigManager::load_with_sources(sources).expect_err("trailing dot must fail");
    assert!(format!("{error:#}").contains("malformed config key"));
}

#[test]
fn boot_config_is_consumed_by_value() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_config(&dir, "");
    let loaded = ConfigManager::load_with_sources(sources_for(path)).expect("loads");
    // The enforcement is ownership: init takes the config out and the
    // handle is gone.
    let owned: HypercolorConfig = loaded.boot.into_inner();
    assert_eq!(owned.daemon.port, 9420);
}

#[test]
fn live_snapshot_derefs_without_exposing_the_swap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_config(&dir, "");
    let loaded = ConfigManager::load_with_sources(sources_for(path)).expect("loads");
    let snapshot = loaded.manager.live();
    assert_eq!(snapshot.daemon.port, 9420);
    let owned = snapshot.clone_inner();
    assert_eq!(owned.daemon.port, snapshot.daemon.port);
}

#[test]
fn pending_restart_flags_only_restart_classified_changes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_config(&dir, "");
    let loaded = ConfigManager::load_with_sources(sources_for(path)).expect("loads");
    let manager = loaded.manager;

    assert!(manager.pending_restart().is_empty(), "clean boot");

    // A live-classified exact key inside the restart-classified
    // daemon section must NOT flag a restart.
    manager.modify(|config| config.daemon.target_fps = 60);
    assert!(
        manager.pending_restart().is_empty(),
        "live render knobs never flag a restart"
    );

    // A genuinely boot-frozen key does.
    manager.modify(|config| config.daemon.port = 9421);
    let pending = manager.pending_restart();
    assert!(
        pending.iter().any(|root| root == "daemon"),
        "changed listener port must flag the daemon section, got {pending:?}"
    );
}

#[test]
fn pending_restart_reapplies_sticky_overlays() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_config(&dir, "[daemon]\nport = 9420\n");
    let mut sources = sources_for(path);
    sources.env.values = vec![("daemon.port".to_owned(), serde_json::json!(9999))];
    let loaded = ConfigManager::load_with_sources(sources).expect("loads");
    let manager = loaded.manager;

    // The daemon booted with the overlay in effect (9999). Persisting
    // a different port changes the FILE, but the overlay will mask it
    // again at the next start — no restart would change anything.
    manager.modify(|config| config.daemon.port = 9421);
    assert!(
        manager.pending_restart().is_empty(),
        "a persisted change the sticky overlay masks must not flag a restart"
    );

    // A restart-classified change the overlay does NOT mask still
    // flags.
    manager.modify(|config| config.daemon.listen_address = "0.0.0.0".to_owned());
    assert!(
        manager
            .pending_restart()
            .iter()
            .any(|root| root == "daemon")
    );
}

#[test]
fn managers_without_a_boot_baseline_report_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_config(&dir, "");
    let manager = ConfigManager::new(path).expect("legacy constructor");
    manager.modify(|config| config.daemon.port = 9422);
    assert!(
        manager.pending_restart().is_empty(),
        "no baseline, no claims"
    );
}
