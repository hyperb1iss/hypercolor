use std::path::Path;

use super::location::{InstallLocationError, LinuxInstallLocation};

#[test]
fn retained_location_rejects_changed_ancestry_after_acquisition() {
    use crate::InstallStore;
    use std::fs;
    use std::os::unix::fs::MetadataExt as _;

    let fixture = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("fixture");
    let home = fixture.path();
    let location = LinuxInstallLocation::new(
        home,
        &home.join("data"),
        &home.join("state"),
        &home.join("config"),
        fs::metadata(home).expect("owner").uid(),
    )
    .expect("location");
    for root in [
        location.data_root(),
        location.state_root(),
        location.release_root(),
        location.config_root(),
        &home.join(".local/lib/hypercolor"),
    ] {
        fs::create_dir_all(root).expect("prepare root");
    }
    let gate = InstallStore::new(home.join(".local/lib/hypercolor"), 65536)
        .acquire_anchored_lock(home)
        .expect("gate");
    let retained = location.retain_existing(home, &gate).expect("retain");
    retained.validate().expect("valid");
    let state_base = home.join("state");
    fs::rename(&state_base, home.join("displaced")).expect("displace state parent");
    fs::create_dir(&state_base).expect("replacement parent");
    fs::rename(
        home.join("displaced/hypercolor"),
        state_base.join("hypercolor"),
    )
    .expect("preserve state leaf");
    assert!(retained.validate().is_err());
}

#[test]
fn recorded_owner_must_match_retained_root_owner() {
    use crate::InstallStore;
    use std::fs;
    use std::os::unix::fs::MetadataExt as _;

    let fixture = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("fixture");
    let home = fixture.path();
    let wrong_uid = fs::metadata(home).expect("owner").uid().wrapping_add(1);
    let location = LinuxInstallLocation::new(
        home,
        &home.join("data"),
        &home.join("state"),
        &home.join("config"),
        wrong_uid,
    )
    .expect("location");
    for root in [
        location.data_root(),
        location.state_root(),
        location.release_root(),
        location.config_root(),
        &home.join(".local/lib/hypercolor"),
    ] {
        fs::create_dir_all(root).expect("prepare root");
    }
    let gate = InstallStore::new(home.join(".local/lib/hypercolor"), 65536)
        .acquire_anchored_lock(home)
        .expect("gate");
    assert!(matches!(
        location.retain_existing(home, &gate),
        Err(InstallLocationError::InvalidOwner(
            _,
            crate::DirectoryRefusal::RecordedOwnerMismatch { .. }
        ))
    ));
}

fn standard_location() -> LinuxInstallLocation {
    LinuxInstallLocation::new(
        Path::new("/home/test"),
        Path::new("/home/test/.local/share"),
        Path::new("/home/test/.local/state"),
        Path::new("/home/test/.config"),
        1000,
    )
    .expect("standard location")
}

#[test]
fn recorded_location_round_trips_without_recomputing_environment() {
    let location = standard_location();
    let bytes = serde_json::to_vec(&location).expect("serialize");
    let parsed = LinuxInstallLocation::parse(&bytes, Path::new("/home/test")).expect("parse");
    assert_eq!(location, parsed);
    assert!(!parsed.installation_id().is_nil());
    assert_eq!(parsed.uid(), 1000);
    assert_eq!(
        parsed.data_root(),
        Path::new("/home/test/.local/share/hypercolor")
    );
    assert_eq!(parsed.release_root(), parsed.data_root().join("releases"));
    assert_eq!(
        parsed.state_root(),
        Path::new("/home/test/.local/state/hypercolor/update")
    );
    assert_eq!(
        parsed.config_root(),
        Path::new("/home/test/.config/hypercolor")
    );
}

#[test]
fn shared_xdg_base_keeps_state_and_releases_as_siblings() {
    let location = LinuxInstallLocation::new(
        Path::new("/home/test"),
        Path::new("/volume/data"),
        Path::new("/volume/data"),
        Path::new("/volume/config"),
        1000,
    )
    .expect("shared base is allowed");
    assert_eq!(
        location.state_root(),
        Path::new("/volume/data/hypercolor/update")
    );
    assert_eq!(
        location.release_root(),
        Path::new("/volume/data/hypercolor/releases")
    );
}

#[test]
fn protected_roots_cannot_be_nested_in_cleanup_owned_roots() {
    for (field, path) in [
        (
            "state_root",
            "/home/test/.local/share/hypercolor/releases/state",
        ),
        ("state_root", "/home/test/.local/share/hypercolor"),
        (
            "config_root",
            "/home/test/.local/share/hypercolor/releases/config",
        ),
        (
            "config_root",
            "/home/test/.local/state/hypercolor/update/config",
        ),
        ("state_root", "/home/test/.local/lib/hypercolor/state"),
        ("state_root", "/home/test/.local/lib"),
    ] {
        let mut value = serde_json::to_value(standard_location()).expect("value");
        value[field] = path.into();
        let bytes = serde_json::to_vec(&value).expect("serialize");
        assert!(
            matches!(
                LinuxInstallLocation::parse(&bytes, Path::new("/home/test")),
                Err(InstallLocationError::OverlappingRoots)
            ),
            "{field}={path}"
        );
    }
}

#[test]
fn invalid_contracts_and_paths_do_not_fall_back_to_legacy() {
    for (field, replacement) in [
        ("schema_version", serde_json::json!(3)),
        ("launcher_contract", serde_json::json!(2)),
        ("service_name", serde_json::json!("other.service")),
        (
            "installation_id",
            serde_json::json!("00000000-0000-0000-0000-000000000000"),
        ),
        ("kind", serde_json::json!("unknown")),
        ("state_root", serde_json::json!("relative/state")),
        ("state_root", serde_json::json!("/state/../state")),
        ("state_root", serde_json::json!("/state/./update")),
        ("state_root", serde_json::json!("/state//update")),
        ("state_root", serde_json::json!("/state/update/")),
        ("state_root", serde_json::json!("/state\nupdate")),
        ("unexpected", serde_json::json!(true)),
    ] {
        let mut value = serde_json::to_value(standard_location()).expect("value");
        value[field] = replacement;
        assert!(
            LinuxInstallLocation::parse(
                &serde_json::to_vec(&value).expect("serialize"),
                Path::new("/home/test")
            )
            .is_err(),
            "invalid {field}"
        );
    }
    assert!(matches!(
        LinuxInstallLocation::parse(&vec![b' '; 32769], Path::new("/home/test")),
        Err(InstallLocationError::TooLarge)
    ));
}

#[test]
fn home_and_recorded_roots_are_bounded_before_any_write() {
    let home = format!("/{}", "h".repeat(255));
    let home = Path::new(&home);
    let base = |suffix: usize, letter: &str| {
        home.join(letter.repeat(512 - suffix - home.as_os_str().len() - 1))
    };
    let longest = LinuxInstallLocation::new(
        home,
        &base("/hypercolor/releases".len(), "d"),
        &base("/hypercolor/update".len(), "s"),
        &base("/hypercolor".len(), "c"),
        1000,
    )
    .expect("the longest supported roots");
    for root in [
        longest.release_root(),
        longest.state_root(),
        longest.config_root(),
    ] {
        assert_eq!(root.as_os_str().len(), 512);
    }
    assert!(matches!(
        LinuxInstallLocation::new(
            home,
            &base("/hypercolor/releases".len() - 1, "d"),
            &home.join("state"),
            &home.join("config"),
            1000,
        ),
        Err(InstallLocationError::PathTooLong { limit: 512, .. })
    ));
    let longer_home = format!("/{}", "h".repeat(256));
    let longer_home = Path::new(&longer_home);
    assert!(matches!(
        LinuxInstallLocation::new(
            longer_home,
            &longer_home.join("data"),
            &longer_home.join("state"),
            &longer_home.join("config"),
            1000,
        ),
        Err(InstallLocationError::PathTooLong { limit: 256, .. })
    ));
}

#[test]
fn recorded_roots_are_limited_to_bytes_systemd_never_reinterprets() {
    let home = Path::new("/home/test");
    for data in [
        "/home/test/My Data",
        "/home/test/%h",
        "/home/test/$HOME",
        "/home/test/a\\b",
        "/home/test/quote\"d",
        "/home/test/semi;colon",
        "/home/test/caf\u{e9}",
    ] {
        assert!(
            matches!(
                LinuxInstallLocation::new(
                    home,
                    Path::new(data),
                    Path::new("/home/test/state"),
                    Path::new("/home/test/config"),
                    1000,
                ),
                Err(InstallLocationError::InvalidPath(_))
            ),
            "{data:?} was accepted"
        );
    }
    assert!(matches!(
        LinuxInstallLocation::new(
            Path::new("/home/te st"),
            Path::new("/data"),
            Path::new("/state"),
            Path::new("/config"),
            1000,
        ),
        Err(InstallLocationError::InvalidPath(_))
    ));
    assert!(
        LinuxInstallLocation::new(
            home,
            Path::new("/srv/users/test-1.data_home"),
            Path::new("/home/test/.local/state"),
            Path::new("/home/test/.config"),
            1000,
        )
        .is_ok()
    );
}
