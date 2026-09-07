use std::path::Path;

use hypercolor_openrgb_host::{
    CHECK_HIDRAW_NODES, CHECK_I2C_DEV_MODULE, CHECK_I2C_NODES, CHECK_UDEV_RULES, PermissionCheck,
    UDEV_RULES_PATHS, UDEV_RULES_URL, linux_permission_checks_at, permission_checks,
    udev_rules_remedy,
};

fn check<'a>(checks: &'a [PermissionCheck], id: &str) -> &'a PermissionCheck {
    checks
        .iter()
        .find(|check| check.id == id)
        .unwrap_or_else(|| panic!("missing check {id}"))
}

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, "").expect("touch");
}

#[test]
fn empty_root_fails_udev_and_module_but_passes_absent_nodes() {
    let root = tempfile::tempdir().expect("tempdir");
    let checks = linux_permission_checks_at(root.path());
    assert_eq!(checks.len(), 4);
    let ids: Vec<&str> = checks.iter().map(|check| check.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            CHECK_UDEV_RULES,
            CHECK_I2C_DEV_MODULE,
            CHECK_I2C_NODES,
            CHECK_HIDRAW_NODES
        ]
    );

    let udev = check(&checks, CHECK_UDEV_RULES);
    assert!(!udev.ok);
    assert_eq!(
        udev.remedy.as_deref(),
        Some(
            "sudo curl -fsSL -o /etc/udev/rules.d/60-openrgb.rules https://gitlab.com/CalcProgrammer1/OpenRGB/-/raw/release_candidate_1.0rc3.1/60-openrgb.rules && sudo udevadm control --reload-rules && sudo udevadm trigger"
        )
    );

    let module = check(&checks, CHECK_I2C_DEV_MODULE);
    assert!(!module.ok);
    assert_eq!(
        module.remedy.as_deref(),
        Some("sudo modprobe i2c-dev && echo i2c-dev | sudo tee /etc/modules-load.d/i2c.conf")
    );
    assert!(module.detail.contains("i2c-i801"));
    assert!(module.detail.contains("i2c-piix4"));

    for id in [CHECK_I2C_NODES, CHECK_HIDRAW_NODES] {
        let nodes = check(&checks, id);
        assert!(nodes.ok, "{id} passes vacuously when no nodes exist");
        assert!(nodes.remedy.is_none());
        assert!(nodes.detail.contains("not exposed yet"));
    }
}

#[test]
fn udev_rules_are_found_in_any_documented_location() {
    for relative in UDEV_RULES_PATHS {
        let root = tempfile::tempdir().expect("tempdir");
        touch(&root.path().join(relative));
        let checks = linux_permission_checks_at(root.path());
        let udev = check(&checks, CHECK_UDEV_RULES);
        assert!(udev.ok, "{relative} should satisfy the check");
        assert_eq!(udev.detail, format!("found /{relative}"));
        assert!(udev.remedy.is_none());
    }
}

#[test]
fn i2c_dev_module_is_detected_via_sysfs_or_procfs() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(root.path().join("sys/module/i2c_dev")).expect("mkdir");
    assert!(
        check(
            &linux_permission_checks_at(root.path()),
            CHECK_I2C_DEV_MODULE
        )
        .ok
    );

    let root = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(root.path().join("proc")).expect("mkdir");
    std::fs::write(
        root.path().join("proc/modules"),
        "i2c_i801 40960 0 - Live 0x0000000000000000\ni2c_dev 24576 0 - Live 0x0000000000000000\n",
    )
    .expect("write");
    assert!(
        check(
            &linux_permission_checks_at(root.path()),
            CHECK_I2C_DEV_MODULE
        )
        .ok
    );

    let root = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(root.path().join("proc")).expect("mkdir");
    std::fs::write(
        root.path().join("proc/modules"),
        "i2c_i801 40960 0 - Live 0x0000000000000000\ni2c_dev_helper 1 0 - Live 0x0\n",
    )
    .expect("write");
    assert!(
        !check(
            &linux_permission_checks_at(root.path()),
            CHECK_I2C_DEV_MODULE
        )
        .ok
    );
}

#[test]
fn device_nodes_report_writable_and_unwritable_entries() {
    let root = tempfile::tempdir().expect("tempdir");
    touch(&root.path().join("dev/i2c-0"));
    touch(&root.path().join("dev/i2c-1"));
    touch(&root.path().join("dev/i2c-dev-not-a-node"));
    touch(&root.path().join("dev/hidraw3"));
    // A directory cannot be opened for writing regardless of privileges, so it
    // stands in for a device node the current user may not write.
    std::fs::create_dir_all(root.path().join("dev/hidraw0")).expect("mkdir");

    let checks = linux_permission_checks_at(root.path());

    let i2c = check(&checks, CHECK_I2C_NODES);
    assert!(i2c.ok);
    assert_eq!(i2c.detail, "2 /dev/i2c-* node(s) writable");

    let hidraw = check(&checks, CHECK_HIDRAW_NODES);
    assert!(!hidraw.ok);
    assert!(hidraw.detail.contains("/dev/hidraw0"));
    assert!(!hidraw.detail.contains("/dev/hidraw3"));
    assert_eq!(
        hidraw.remedy.as_deref(),
        Some(
            "sudo curl -fsSL -o /etc/udev/rules.d/60-openrgb.rules https://gitlab.com/CalcProgrammer1/OpenRGB/-/raw/release_candidate_1.0rc3.1/60-openrgb.rules && sudo udevadm control --reload-rules && sudo udevadm trigger"
        )
    );
}

#[test]
fn udev_remedy_installs_the_release_rules_file_and_reloads() {
    let remedy = udev_rules_remedy();
    assert_eq!(
        remedy,
        "sudo curl -fsSL -o /etc/udev/rules.d/60-openrgb.rules https://gitlab.com/CalcProgrammer1/OpenRGB/-/raw/release_candidate_1.0rc3.1/60-openrgb.rules && sudo udevadm control --reload-rules && sudo udevadm trigger"
    );
    assert!(remedy.contains(UDEV_RULES_URL));
    assert!(
        !remedy.contains("--generate-udev-rules") && !remedy.contains("--print-udev-rules"),
        "released OpenRGB has neither flag"
    );
}

#[test]
fn host_entry_point_matches_the_platform_contract() {
    let checks = permission_checks();
    if cfg!(target_os = "linux") {
        assert_eq!(checks.len(), 4);
    } else {
        assert!(checks.is_empty());
    }
}

#[test]
fn permission_check_serializes_with_optional_remedy() {
    let json: serde_json::Value = serde_json::from_str(
        r#"{"id":"udev_rules","ok":true,"detail":"found /etc/udev/rules.d/60-openrgb.rules"}"#,
    )
    .expect("json");
    let check: PermissionCheck = serde_json::from_value(json).expect("remedy defaults to None");
    assert_eq!(check.remedy, None);
}
