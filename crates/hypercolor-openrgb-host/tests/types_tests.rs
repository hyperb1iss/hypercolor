use std::path::PathBuf;

use hypercolor_openrgb_host::{
    BinaryKind, InstallHint, InstallMethod, OpenRgbBinary, Platform, ServerProbe,
};
use serde_json::json;

#[test]
fn binary_kind_and_platform_use_snake_case() {
    assert_eq!(
        serde_json::to_value(BinaryKind::AppImage).expect("serialize"),
        json!("app_image")
    );
    assert_eq!(
        serde_json::to_value(BinaryKind::Flatpak).expect("serialize"),
        json!("flatpak")
    );
    assert_eq!(
        serde_json::to_value(Platform::Macos).expect("serialize"),
        json!("macos")
    );
    assert_eq!(
        serde_json::to_value(InstallMethod::DirectDownload).expect("serialize"),
        json!("direct_download")
    );
}

#[test]
fn binary_round_trips_and_tolerates_missing_version() {
    let binary = OpenRgbBinary {
        path: PathBuf::from("/usr/bin/openrgb"),
        kind: BinaryKind::Native,
        version: Some("1.0rc3".to_owned()),
    };
    let json = serde_json::to_string(&binary).expect("serialize");
    let back: OpenRgbBinary = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, binary);

    let without_version: OpenRgbBinary =
        serde_json::from_value(json!({"path": "/usr/bin/openrgb", "kind": "native"}))
            .expect("version defaults");
    assert_eq!(without_version.version, None);
}

#[test]
fn server_probe_defaults_are_unreachable_and_empty() {
    let probe = ServerProbe::default();
    assert!(!probe.reachable);
    assert_eq!(probe.protocol_version, None);
    assert_eq!(probe.controller_count, None);
    assert_eq!(probe.error, None);

    let minimal: ServerProbe =
        serde_json::from_value(json!({"reachable": true})).expect("optional fields default");
    assert!(minimal.reachable);
    assert_eq!(minimal.protocol_version, None);
}

#[test]
fn install_hint_note_defaults_to_empty() {
    let hint: InstallHint = serde_json::from_value(json!({
        "platform": "linux",
        "method": "pacman",
        "command": "sudo pacman -S openrgb"
    }))
    .expect("note defaults");
    assert_eq!(hint.note, "");
    assert_eq!(hint.platform, Platform::Linux);
}

#[test]
fn current_platform_matches_compile_target() {
    let expected = if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    };
    assert_eq!(Platform::current(), expected);
}
