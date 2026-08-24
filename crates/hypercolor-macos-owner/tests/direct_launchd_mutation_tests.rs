use std::path::PathBuf;

use hypercolor_macos_owner::{
    MACOS_DIRECT_LAUNCHD_LABEL, MacosDirectLaunchdBootstrapExpectation,
    parse_direct_launchd_autostart_state,
};

const SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn disabled_state_parser_accepts_current_legacy_and_missing_forms() {
    let current_disabled =
        format!("disabled services = {{\n\t\"{MACOS_DIRECT_LAUNCHD_LABEL}\" => disabled\n}}\n");
    assert!(
        !parse_direct_launchd_autostart_state(current_disabled.as_bytes())
            .expect("current disabled state should parse")
    );

    let current_enabled =
        format!("disabled services = {{\n\t\"{MACOS_DIRECT_LAUNCHD_LABEL}\" => enabled\n}}\n");
    assert!(
        parse_direct_launchd_autostart_state(current_enabled.as_bytes())
            .expect("current enabled state should parse")
    );

    let legacy_disabled =
        format!("disabled services = {{\n\t\"{MACOS_DIRECT_LAUNCHD_LABEL}\" => true\n}}\n");
    assert!(
        !parse_direct_launchd_autostart_state(legacy_disabled.as_bytes())
            .expect("legacy disabled state should parse")
    );

    assert!(
        parse_direct_launchd_autostart_state(b"disabled services = {\n}\n")
            .expect("missing label should retain launchd's enabled default")
    );
}

#[test]
fn disabled_state_parser_rejects_ambiguous_malformed_and_unbounded_output() {
    let duplicate = format!(
        "disabled services = {{\n\"{0}\" => enabled\n\"{0}\" => disabled\n}}\n",
        MACOS_DIRECT_LAUNCHD_LABEL
    );
    for invalid in [
        duplicate.into_bytes(),
        b"disabled services = {\n\"unterminated => enabled\n}\n".to_vec(),
        b"disabled services = {\n\"other\" => maybe\n}\n".to_vec(),
        b"disabled services = {\n".to_vec(),
        b"wrong header {\n}\n".to_vec(),
        vec![0xff],
        vec![b'x'; 64 * 1024 + 1],
    ] {
        assert!(parse_direct_launchd_autostart_state(&invalid).is_err());
    }
}

#[test]
fn bootstrap_expectation_requires_exact_bounded_private_snapshot_identity() {
    let exact = MacosDirectLaunchdBootstrapExpectation::new(
        "/private/hypercolor/unit/launchd.plist",
        SHA256,
        0o600,
        512,
        1,
        2,
    )
    .expect("exact private property list should validate");
    assert_eq!(
        exact.path(),
        PathBuf::from("/private/hypercolor/unit/launchd.plist")
    );
    assert_eq!(exact.sha256(), SHA256);
    assert_eq!(exact.mode(), 0o600);
    assert_eq!(exact.size(), 512);
    assert_eq!(exact.device(), 1);
    assert_eq!(exact.inode(), 2);

    for invalid in [
        MacosDirectLaunchdBootstrapExpectation::new("relative.plist", SHA256, 0o600, 1, 1, 1),
        MacosDirectLaunchdBootstrapExpectation::new("/absolute", "invalid", 0o600, 1, 1, 1),
        MacosDirectLaunchdBootstrapExpectation::new("/absolute", SHA256, 0o622, 1, 1, 1),
        MacosDirectLaunchdBootstrapExpectation::new("/absolute", SHA256, 0o200, 1, 1, 1),
        MacosDirectLaunchdBootstrapExpectation::new("/absolute", SHA256, 0o600, 0, 1, 1),
        MacosDirectLaunchdBootstrapExpectation::new(
            "/absolute",
            SHA256,
            0o600,
            256 * 1024 + 1,
            1,
            1,
        ),
        MacosDirectLaunchdBootstrapExpectation::new("/absolute", SHA256, 0o600, 1, 0, 1),
    ] {
        assert!(invalid.is_err());
    }
}
