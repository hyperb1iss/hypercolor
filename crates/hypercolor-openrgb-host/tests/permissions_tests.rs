use hypercolor_openrgb_host::{
    PermissionCheck, UDEV_RULES_URL, parse_hid_id, parse_rules_device_ids, permission_checks,
    udev_rules_remedy,
};

const RULES_FIXTURE: &str = r#"#---------------------------------------------------------------#
#  OpenRGB udev rules - Git Commit:                     #
#---------------------------------------------------------------#
KERNEL=="i2c-[0-99]*", TAG+="uaccess"

# SUBSYSTEMS=="usb|hidraw", ATTRS{idVendor}=="dead", ATTRS{idProduct}=="beef", TAG+="uaccess"
SUBSYSTEMS=="usb|hidraw", ATTRS{idVendor}=="0CF2", ATTRS{idProduct}=="A100", TAG+="uaccess", TAG+="Lian_Li_Uni_Hub"
SUBSYSTEMS=="usb|hidraw", ATTRS{idVendor}=="1532", ATTRS{idProduct}=="0226", TAG+="uaccess", TAG+="Razer_Huntsman"
SUBSYSTEMS=="usb", ATTRS{idVendor}=="1b1c", TAG+="uaccess"
"#;

#[test]
fn rules_parser_collects_vid_pid_pairs_lowercase_and_skips_comments() {
    let ids = parse_rules_device_ids(RULES_FIXTURE);
    assert_eq!(
        ids.len(),
        2,
        "vendor-only and commented lines are ignored: {ids:?}"
    );
    assert!(ids.contains(&("0cf2".to_owned(), "a100".to_owned())));
    assert!(ids.contains(&("1532".to_owned(), "0226".to_owned())));
}

#[test]
fn hid_id_parser_reads_the_kernel_uevent_shape() {
    assert_eq!(
        parse_hid_id("DRIVER=hid-generic\nHID_ID=0003:00001A86:00002107\nHID_NAME=LIANLI SLV3H\n"),
        Some(("1a86".to_owned(), "2107".to_owned()))
    );
    assert_eq!(parse_hid_id("DRIVER=hid-generic\n"), None);
    assert_eq!(parse_hid_id("HID_ID=0003:zzzz:0001"), None);
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

/// Fixture-tree tests stand in regular files and directories for device nodes
/// and rely on Unix open-for-write semantics, so they only run on Unix hosts.
#[cfg(unix)]
mod linux_tree {
    use std::path::Path;

    use hypercolor_openrgb_host::{
        CHECK_HIDRAW_NODES, CHECK_I2C_DEV_MODULE, CHECK_I2C_NODES, CHECK_UDEV_RULES,
        PermissionCheck, UDEV_RULES_PATHS, linux_permission_checks_at, udev_rules_remedy,
    };

    use super::RULES_FIXTURE;

    fn stage_hidraw(root: &Path, index: u32, hid_id: &str, writable: bool) {
        let node = root.join(format!("dev/hidraw{index}"));
        if writable {
            touch(&node);
        } else {
            // A directory cannot be opened for writing regardless of privileges,
            // so it stands in for a root-only device node.
            std::fs::create_dir_all(&node).expect("mkdir node");
        }
        let uevent = root.join(format!("sys/class/hidraw/hidraw{index}/device/uevent"));
        std::fs::create_dir_all(uevent.parent().expect("parent")).expect("mkdir sysfs");
        std::fs::write(
            uevent,
            format!("DRIVER=hid-generic\nHID_ID={hid_id}\nHID_NAME=Fixture\nHID_PHYS=usb-0000:00:14.0-2/input0\n"),
        )
        .expect("write uevent");
    }

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
    fn i2c_nodes_report_writable_entries_and_ignore_non_nodes() {
        let root = tempfile::tempdir().expect("tempdir");
        touch(&root.path().join("dev/i2c-0"));
        touch(&root.path().join("dev/i2c-1"));
        touch(&root.path().join("dev/i2c-dev-not-a-node"));
        let checks = linux_permission_checks_at(root.path());
        let i2c = check(&checks, CHECK_I2C_NODES);
        assert!(i2c.ok);
        assert_eq!(i2c.detail, "2 /dev/i2c-* node(s) writable");

        std::fs::create_dir_all(root.path().join("dev/i2c-2")).expect("mkdir unwritable node");
        let checks = linux_permission_checks_at(root.path());
        let i2c = check(&checks, CHECK_I2C_NODES);
        assert!(!i2c.ok);
        assert!(i2c.detail.contains("/dev/i2c-2"));
        assert_eq!(i2c.remedy.as_deref(), Some(udev_rules_remedy().as_str()));
    }

    #[test]
    fn hidraw_check_only_judges_nodes_covered_by_the_rules_file() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(root.path().join("etc/udev/rules.d")).expect("mkdir");
        std::fs::write(
            root.path().join("etc/udev/rules.d/60-openrgb.rules"),
            RULES_FIXTURE,
        )
        .expect("write rules");
        // Covered and writable: the happy path.
        stage_hidraw(root.path(), 0, "0003:00000CF2:0000A100", true);
        // Not covered (LIANLI SLV3H 1a86:2107, root-only on the live host): informational.
        stage_hidraw(root.path(), 14, "0003:00001A86:00002107", false);
        // Not covered, no sysfs entry at all: informational.
        touch(&root.path().join("dev/hidraw3"));

        let checks = linux_permission_checks_at(root.path());
        let hidraw = check(&checks, CHECK_HIDRAW_NODES);
        assert!(hidraw.ok, "{}", hidraw.detail);
        assert_eq!(
            hidraw.detail,
            "1 node(s) covered by OpenRGB rules writable; 2 not covered by OpenRGB rules"
        );
        assert!(hidraw.remedy.is_none());

        // Covered but root-only: the one case that fails.
        stage_hidraw(root.path(), 5, "0003:00001532:00000226", false);
        let checks = linux_permission_checks_at(root.path());
        let hidraw = check(&checks, CHECK_HIDRAW_NODES);
        assert!(!hidraw.ok);
        assert!(hidraw.detail.contains("/dev/hidraw5 (1532:0226)"));
        assert!(!hidraw.detail.contains("hidraw14"));
        assert!(hidraw.detail.contains("2 other node(s) not covered"));
        assert_eq!(hidraw.remedy.as_deref(), Some(udev_rules_remedy().as_str()));
    }

    #[test]
    fn hidraw_check_without_rules_file_is_informational() {
        let root = tempfile::tempdir().expect("tempdir");
        stage_hidraw(root.path(), 0, "0003:00000CF2:0000A100", false);
        let checks = linux_permission_checks_at(root.path());
        let hidraw = check(&checks, CHECK_HIDRAW_NODES);
        assert!(hidraw.ok);
        assert!(hidraw.detail.contains("no OpenRGB rules file installed"));
        assert!(
            !check(&checks, CHECK_UDEV_RULES).ok,
            "the rules check carries the remedy"
        );
    }
}
