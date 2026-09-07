use super::identity::{dynamic_codesign_verification_args, plist_true_keys};

#[test]
fn compact_entitlement_plist_preserves_exact_true_keys() {
    let xml = concat!(
        "<plist><dict>",
        "<key>com.apple.security.device.usb</key><true/>",
        "<key>com.apple.security.cs.allow-jit</key><true />",
        "</dict></plist>"
    );

    assert_eq!(
        plist_true_keys(xml).expect("compact entitlement plist should parse"),
        [
            "com.apple.security.cs.allow-jit".to_owned(),
            "com.apple.security.device.usb".to_owned(),
        ]
    );
}

#[test]
fn entitlement_plist_rejects_non_true_values() {
    assert!(plist_true_keys("<plist><dict><key>unsafe</key><false/></dict></plist>").is_err());
}

#[test]
fn dynamic_codesign_verification_uses_the_nonverbose_live_pid_form() {
    assert_eq!(
        dynamic_codesign_verification_args("+42"),
        ["--verify", "+42"]
    );
}
