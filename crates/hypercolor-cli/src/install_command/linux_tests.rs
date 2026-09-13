use std::fs;
use std::path::Path;

use super::*;

#[test]
fn proposed_xdg_roots_are_explicit_and_reject_relative_or_overlapping_roots() {
    let home = Path::new("/home/test");
    let defaults = proposed_location_with(home, 1000, |_| None).expect("default topology");
    assert_eq!(
        defaults.release_root(),
        home.join(".local/share/hypercolor/releases")
    );
    assert_eq!(
        defaults.state_root(),
        home.join(".local/state/hypercolor/update")
    );
    assert_eq!(defaults.config_root(), home.join(".config/hypercolor"));
    let custom = proposed_location_with(home, 1000, |name| {
        Some(
            match name {
                "XDG_DATA_HOME" => "/owned/data",
                "XDG_STATE_HOME" => "/other/state",
                "XDG_CONFIG_HOME" => "/owned/config",
                _ => unreachable!(),
            }
            .into(),
        )
    })
    .expect("separate roots");
    assert_eq!(
        custom.release_root(),
        Path::new("/owned/data/hypercolor/releases")
    );
    assert_eq!(
        custom.state_root(),
        Path::new("/other/state/hypercolor/update")
    );
    for state in ["relative", "/home/test/.local/share/hypercolor/releases"] {
        assert!(
            proposed_location_with(home, 1000, |name| (name == "XDG_STATE_HOME")
                .then(|| state.into()))
            .is_err()
        );
    }
}

#[test]
fn invalid_permanent_locator_refuses_normal_and_no_service_before_staging() {
    for no_service in [false, true] {
        let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("owned home");
        let prefix = home.path().join(".local");
        let old_root = prefix.join("lib/hypercolor");
        fs::create_dir_all(&old_root).expect("legacy discovery directory");
        fs::write(
            old_root.join("install-journal.json"),
            br#"{"schema_version":99}"#,
        )
        .expect("unknown permanent locator");
        let source = ReadOnlyDirectoryAuthority::open(home.path()).expect("source authority");
        let executable = File::open(old_root.join("install-journal.json")).expect("source file");
        let args = InstallReleaseArgs {
            install_prefix: prefix.clone(),
            install_dir: prefix.join("bin"),
            expected_manifest_sha256: crate::install::UnitId::new("a".repeat(64)).expect("id"),
            no_service,
        };
        let error = execute(&args, home.path(), &old_root, &source, &executable)
            .expect_err("unknown authority cannot fall back");
        assert!(error.to_string().contains("elect install authority"));
        assert!(!old_root.join("units").exists());
        assert!(!home.path().join(".local/share/hypercolor").exists());
        assert!(!home.path().join(".local/state/hypercolor").exists());
        assert!(!old_root.join("install.lock").exists());
    }
}
