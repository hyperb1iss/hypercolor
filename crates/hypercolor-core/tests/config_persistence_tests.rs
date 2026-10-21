//! A config save the file system refuses stays live and reports the lag.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::time::Duration;

use hypercolor_core::config::{ConfigManager, ConfigPersistence};
use hypercolor_core::persistence::AtomicFileWriter;
use hypercolor_types::config::DEFAULT_TARGET_FPS;

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .expect("test directory permissions should change");
}

/// Make `path` read-only, or return false when the process can still
/// create files there (root, or a filesystem that ignores mode bits).
fn lock_directory(path: &Path) -> bool {
    set_mode(path, 0o500);
    let probe = path.join(".write-probe");
    if fs::File::create(&probe).is_ok() {
        drop(fs::remove_file(&probe));
        set_mode(path, 0o700);
        return false;
    }
    true
}

#[test]
fn save_into_a_read_only_directory_is_live_and_lagging_until_the_retry_lands() {
    let root = tempfile::tempdir().expect("tempdir");
    let config_dir = root.path().join("config");
    fs::create_dir_all(&config_dir).expect("config directory");
    let path = config_dir.join("hypercolor.toml");
    let manager = ConfigManager::new(path.clone()).expect("config manager");
    assert!(manager.save().expect("baseline save").is_durable());
    assert!(manager.persistence_status().is_durable());

    if !lock_directory(&config_dir) {
        return;
    }
    manager.modify(|config| config.daemon.target_fps = 45);
    let saved = manager.save();
    let status = manager.persistence_status();
    set_mode(&config_dir, 0o700);

    let saved = saved.expect("a refused file write keeps the live change");
    let ConfigPersistence::Lagging {
        path: lagging,
        error,
    } = &saved
    else {
        panic!("a read-only directory cannot hold the write: {saved:?}");
    };
    assert_eq!(lagging, &path);
    assert!(error.contains("hypercolor.toml"), "{error}");
    assert_eq!(status, saved, "the status reports the same lag");
    assert_eq!(manager.get().daemon.target_fps, 45);
    assert_eq!(
        ConfigManager::load(&path)
            .expect("the previous file stays readable")
            .daemon
            .target_fps,
        DEFAULT_TARGET_FPS
    );

    AtomicFileWriter::new(&path)
        .expect("config destination")
        .flush(Duration::from_secs(5))
        .expect("the retry lands once the directory is writable");
    assert!(manager.persistence_status().is_durable());
    assert_eq!(
        ConfigManager::load(&path)
            .expect("converged config")
            .daemon
            .target_fps,
        45
    );
}
