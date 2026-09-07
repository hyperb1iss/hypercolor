#![cfg(target_os = "macos")]

use std::process::Command;

use hypercolor_macos_owner::canonical_macos_daemon_guard_path;

const CHILD_MARKER: &str = "HYPERCOLOR_GUARD_COORDINATE_CHILD";
const EXPECTED_PATH: &str = "HYPERCOLOR_GUARD_COORDINATE_EXPECTED";

#[test]
fn canonical_guard_coordinate_ignores_poisoned_tmpdir() {
    if std::env::var_os(CHILD_MARKER).is_some() {
        let expected = std::env::var_os(EXPECTED_PATH).expect("expected path should be supplied");
        assert_eq!(
            canonical_macos_daemon_guard_path()
                .expect("canonical guard should resolve")
                .as_os_str(),
            expected
        );
        assert_ne!(
            canonical_macos_daemon_guard_path().expect("canonical guard should resolve"),
            std::path::Path::new("/tmp/poisoned-hypercolor").join("hypercolor-daemon.lock")
        );
        return;
    }

    let expected = canonical_macos_daemon_guard_path().expect("canonical guard should resolve");
    let output = Command::new(std::env::current_exe().expect("test executable should resolve"))
        .args([
            "--exact",
            "canonical_guard_coordinate_ignores_poisoned_tmpdir",
            "--nocapture",
        ])
        .env(CHILD_MARKER, "1")
        .env(EXPECTED_PATH, &expected)
        .env("TMPDIR", "/tmp/poisoned-hypercolor")
        .output()
        .expect("poisoned-environment child should run");
    assert!(
        output.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
