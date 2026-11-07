//! Fixture isolation of every Hypercolor user directory.
//!
//! Every test here holds the process-wide claim, so they serialize.

use hypercolor_core::config::ConfigManager;
use hypercolor_core::config::paths::{self, IsolatedUserDirs, UserDirPaths};

#[test]
fn claimed_user_dirs_resolve_inside_the_fixture() {
    let root = tempfile::tempdir().expect("fixture root");
    let claim = IsolatedUserDirs::claim(root.path());
    let dirs = claim.paths();

    assert_eq!(ConfigManager::data_dir(), dirs.data);
    assert_eq!(ConfigManager::config_dir(), dirs.config);
    assert_eq!(ConfigManager::state_dir(), dirs.state);
    assert_eq!(ConfigManager::cache_dir(), dirs.cache);
    assert_eq!(paths::runtime_dir().as_ref(), Some(&dirs.runtime));
    assert!(paths::servo_runtime_cache_dir().starts_with(&dirs.cache));
    assert_eq!(*dirs, UserDirPaths::under(root.path()));
}

#[test]
fn a_claim_honors_exact_paths() {
    let root = tempfile::tempdir().expect("fixture root");
    let exact = UserDirPaths {
        data: root.path().join("xdg-data/hypercolor"),
        config: root.path().join("xdg-config/hypercolor"),
        state: root.path().join("xdg-state/hypercolor"),
        cache: root.path().join("xdg-cache/hypercolor"),
        runtime: root.path().join("xdg-runtime"),
    };

    let claim = IsolatedUserDirs::claim_paths(exact.clone());

    assert_eq!(claim.paths(), &exact);
    assert_eq!(ConfigManager::data_dir(), exact.data);
    assert_eq!(paths::runtime_dir(), Some(exact.runtime));
}

#[cfg(unix)]
#[test]
fn claimed_runtime_dir_hides_the_host_blocksd_socket() {
    let root = tempfile::tempdir().expect("fixture root");
    let claim = IsolatedUserDirs::claim(root.path());

    let socket = hypercolor_core::device::BlocksBackend::default_socket_path();

    assert_eq!(socket, claim.paths().runtime.join("blocksd/blocksd.sock"));
}

#[test]
fn dropping_the_claim_releases_every_directory() {
    let root = tempfile::tempdir().expect("fixture root");
    drop(IsolatedUserDirs::claim(root.path()));

    // The next claim proves the lock was released and sees only its own root.
    let next = tempfile::tempdir().expect("second fixture root");
    let claim = IsolatedUserDirs::claim(next.path());

    assert!(ConfigManager::data_dir().starts_with(next.path()));
    assert!(!claim.paths().data.starts_with(root.path()));
}
