#![cfg(unix)]

use clap::{CommandFactory as _, Parser as _};
use hypercolor_cli::{Cli, Commands};

const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn hidden_release_command_is_strict_and_absent_from_help() {
    let parsed = Cli::try_parse_from([
        "hypercolor",
        "__install-release",
        "--install-prefix",
        "/home/test/.local",
        "--install-dir",
        "/home/test/.local/bin",
        "--expected-manifest-sha256",
        DIGEST,
        "--no-service",
    ])
    .expect("hidden release command should parse");
    assert!(matches!(parsed.command, Commands::InstallRelease(_)));

    let uppercase = DIGEST.to_ascii_uppercase();
    assert!(
        Cli::try_parse_from([
            "hypercolor",
            "__install-release",
            "--install-prefix",
            "/home/test/.local",
            "--install-dir",
            "/home/test/.local/bin",
            "--expected-manifest-sha256",
            &uppercase,
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "hypercolor",
            "__install-release",
            "--install-prefix",
            "/home/test/.local",
            "--install-prefix",
            "/home/test/.local",
            "--install-dir",
            "/home/test/.local/bin",
            "--expected-manifest-sha256",
            DIGEST,
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "hypercolor",
            "__install-release",
            "--install-prefix",
            "/home/test/.local",
            "--install-dir",
            "/home/test/.local/bin",
            "--expected-manifest-sha256",
            DIGEST,
            "--no-service",
            "--no-service",
        ])
        .is_err()
    );
    let legacy = format!("legacy-{DIGEST}");
    assert!(
        Cli::try_parse_from([
            "hypercolor",
            "__install-release",
            "--install-prefix",
            "/home/test/.local",
            "--install-dir",
            "/home/test/.local/bin",
            "--expected-manifest-sha256",
            &legacy,
        ])
        .is_err()
    );

    let help = Cli::command().render_long_help().to_string();
    assert!(!help.contains("__install-release"));
}

#[test]
fn hidden_release_failure_bypasses_connection_setup_and_propagates() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::process::Command;

    let temp = tempfile::Builder::new()
        .prefix("candidate-dispatch-failure-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary install fixture");
    let release_root = temp.path().join("release");
    let candidate = release_root.join("bin/hypercolor");
    fs::create_dir_all(candidate.parent().expect("candidate parent"))
        .expect("create candidate directory");
    fs::copy(env!("CARGO_BIN_EXE_hypercolor"), &candidate).expect("copy candidate binary");
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o755))
        .expect("make candidate executable");

    let home = temp.path().join("home");
    let xdg_config = temp.path().join("xdg-config");
    fs::create_dir_all(home.join(".config/hypercolor")).expect("create HOME config sentinel");
    fs::create_dir_all(xdg_config.join("hypercolor")).expect("create CLI config directory");
    fs::write(
        xdg_config.join("hypercolor/cli.toml"),
        "this is not valid TOML",
    )
    .expect("write malformed CLI config");
    let sentinel = home.join(".config/hypercolor/sentinel");
    fs::write(&sentinel, b"untouched").expect("write user sentinel");

    let prefix = home.join(".local");
    let install_dir = prefix.join("bin");
    let output = Command::new(&candidate)
        .args([
            "__install-release",
            "--install-prefix",
            prefix.to_str().expect("UTF-8 prefix"),
            "--install-dir",
            install_dir.to_str().expect("UTF-8 install directory"),
            "--expected-manifest-sha256",
            DIGEST,
        ])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &xdg_config)
        .env("HYPERCOLOR_PROFILE", "must-not-resolve")
        .output()
        .expect("run copied release candidate");

    assert!(
        !output.status.success(),
        "invalid candidate unexpectedly succeeded"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let expected_failure = "release candidate validation failed before install bootstrap";
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let expected_failure = "raw release installation is unsupported on this platform";
    assert!(
        stderr.contains(expected_failure),
        "candidate failure was not propagated: {stderr}"
    );
    assert!(
        !stderr.contains("failed to load CLI config") && !stderr.contains("profile"),
        "hidden dispatch entered connection setup: {stderr}"
    );
    assert_eq!(
        fs::read(&sentinel).expect("read user sentinel"),
        b"untouched"
    );
    assert!(
        !prefix.exists(),
        "invalid candidate created install scaffolding before validation"
    );
    assert!(
        !home.join(".hypercolor-release-install.lock").exists(),
        "invalid candidate created the anchored install lock"
    );
}

#[test]
fn anchored_candidate_store_bootstraps_once_and_uses_one_canonical_lock() {
    use hypercolor_cli::install::{InstallStore, InstallStoreError, MAX_INSTALL_JOURNAL_BYTES};
    use std::fs;
    use std::os::unix::fs::MetadataExt as _;

    let temp = tempfile::Builder::new()
        .prefix("candidate-store-bootstrap-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary install fixture");
    let home = temp.path().join("home");
    fs::create_dir(&home).expect("create HOME anchor");
    let store = InstallStore::new(
        home.join(".local/lib/hypercolor"),
        MAX_INSTALL_JOURNAL_BYTES,
    );

    let lock = store
        .acquire_anchored_lock(&home)
        .expect("bootstrap the retained install store");
    let retained = lock
        .open_store_public_directory()
        .expect("canonical store path remains retained");
    let public = fs::metadata(store.root()).expect("public store metadata");
    let retained = retained.metadata().expect("retained store metadata");

    assert_eq!(
        (retained.device(), retained.inode()),
        (public.dev(), public.ino())
    );
    assert!(store.root().join("install.lock").is_file());
    assert!(home.join(".hypercolor-release-install.lock").is_file());
    assert!(matches!(
        store.acquire_anchored_lock(&home),
        Err(InstallStoreError::LockContended)
    ));
    assert!(matches!(
        store.acquire_lock(),
        Err(InstallStoreError::LockContended)
    ));
}
