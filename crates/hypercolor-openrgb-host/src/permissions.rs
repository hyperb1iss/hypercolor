//! Host permission and driver prerequisites for OpenRGB device access.
//!
//! Only Linux has anything to check: udev rules for HID and I2C nodes, the
//! `i2c-dev` module for SMBus, and write access to the device nodes. The
//! filesystem inspection is written against an injectable root so tests can
//! stage a fake tree; [`permission_checks`] runs it against `/` on Linux and
//! returns nothing elsewhere.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use crate::detect::FLATPAK_APP_ID;
use crate::types::{BinaryKind, OpenRgbBinary, PermissionCheck};

/// Check id: an OpenRGB udev rules file is installed.
pub const CHECK_UDEV_RULES: &str = "udev_rules";
/// Check id: the `i2c-dev` kernel module is loaded.
pub const CHECK_I2C_DEV_MODULE: &str = "i2c_dev_module";
/// Check id: every `/dev/i2c-*` node is writable by the current user.
pub const CHECK_I2C_NODES: &str = "i2c_nodes_writable";
/// Check id: every `/dev/hidraw*` node is writable by the current user.
pub const CHECK_HIDRAW_NODES: &str = "hidraw_nodes_writable";

/// Where OpenRGB's own instructions and the distro packages put the rules.
pub const UDEV_RULES_PATHS: [&str; 3] = [
    "etc/udev/rules.d/60-openrgb.rules",
    "usr/lib/udev/rules.d/60-openrgb.rules",
    "lib/udev/rules.d/60-openrgb.rules",
];

const UDEV_RULES_TARGET: &str = "/etc/udev/rules.d/60-openrgb.rules";
const UDEV_RELOAD: &str = "sudo udevadm control --reload-rules && sudo udevadm trigger";

/// Permission checks for the current host.
///
/// Real on Linux, empty everywhere else: Windows and macOS have no udev or
/// device-node story for OpenRGB (Windows elevation and PawnIO are covered
/// by the install hints).
#[must_use]
pub fn permission_checks(binary: Option<&OpenRgbBinary>) -> Vec<PermissionCheck> {
    #[cfg(target_os = "linux")]
    {
        linux_permission_checks_at(Path::new("/"), binary)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = binary;
        Vec::new()
    }
}

/// Linux permission checks evaluated against `root` as the filesystem root.
///
/// Available on every target so the logic stays testable; only
/// [`permission_checks`] decides whether the host has anything to inspect.
#[must_use]
pub fn linux_permission_checks_at(
    root: &Path,
    binary: Option<&OpenRgbBinary>,
) -> Vec<PermissionCheck> {
    let udev_remedy = udev_rules_remedy(binary);
    vec![
        udev_rules_check(root, &udev_remedy),
        i2c_dev_module_check(root),
        device_nodes_check(
            root,
            CHECK_I2C_NODES,
            "i2c-",
            "SMBus adapters",
            &udev_remedy,
        ),
        device_nodes_check(
            root,
            CHECK_HIDRAW_NODES,
            "hidraw",
            "HID devices",
            &udev_remedy,
        ),
    ]
}

/// The command that installs OpenRGB's udev rules for this binary.
#[must_use]
pub fn udev_rules_remedy(binary: Option<&OpenRgbBinary>) -> String {
    match binary {
        Some(binary) if binary.kind == BinaryKind::Flatpak => format!(
            "sudo sh -c 'flatpak run {FLATPAK_APP_ID} --print-udev-rules > {UDEV_RULES_TARGET}' && {UDEV_RELOAD}"
        ),
        Some(binary) => format!(
            "sudo {} --generate-udev-rules {UDEV_RULES_TARGET} && {UDEV_RELOAD}",
            binary.path.display()
        ),
        None => format!("sudo openrgb --generate-udev-rules {UDEV_RULES_TARGET} && {UDEV_RELOAD}"),
    }
}

fn udev_rules_check(root: &Path, remedy: &str) -> PermissionCheck {
    let found: Option<PathBuf> = UDEV_RULES_PATHS
        .iter()
        .map(|relative| root.join(relative))
        .find(|path| path.is_file());
    match found {
        Some(path) => PermissionCheck {
            id: CHECK_UDEV_RULES.to_owned(),
            ok: true,
            detail: format!("found {}", display_from_root(root, &path)),
            remedy: None,
        },
        None => PermissionCheck {
            id: CHECK_UDEV_RULES.to_owned(),
            ok: false,
            detail: "no 60-openrgb.rules under /etc/udev/rules.d, /usr/lib/udev/rules.d, or \
                     /lib/udev/rules.d; HID and I2C nodes will need root"
                .to_owned(),
            remedy: Some(remedy.to_owned()),
        },
    }
}

fn i2c_dev_module_check(root: &Path) -> PermissionCheck {
    let sysfs_loaded = root.join("sys/module/i2c_dev").is_dir();
    let procfs_loaded = std::fs::read_to_string(root.join("proc/modules"))
        .map(|modules| {
            modules
                .lines()
                .any(|line| line.split_whitespace().next() == Some("i2c_dev"))
        })
        .unwrap_or(false);
    if sysfs_loaded || procfs_loaded {
        PermissionCheck {
            id: CHECK_I2C_DEV_MODULE.to_owned(),
            ok: true,
            detail: "i2c-dev is loaded".to_owned(),
            remedy: None,
        }
    } else {
        PermissionCheck {
            id: CHECK_I2C_DEV_MODULE.to_owned(),
            ok: false,
            detail: "i2c-dev is not loaded; SMBus devices (motherboard headers, DRAM) stay \
                     invisible. Also load i2c-i801 (Intel) or i2c-piix4 (AMD) if /dev/i2c-* \
                     is still missing afterwards."
                .to_owned(),
            remedy: Some(
                "sudo modprobe i2c-dev && echo i2c-dev | sudo tee /etc/modules-load.d/i2c.conf"
                    .to_owned(),
            ),
        }
    }
}

fn device_nodes_check(
    root: &Path,
    id: &str,
    prefix: &str,
    purpose: &str,
    udev_remedy: &str,
) -> PermissionCheck {
    let mut nodes = list_device_nodes(&root.join("dev"), prefix);
    nodes.sort();
    if nodes.is_empty() {
        return PermissionCheck {
            id: id.to_owned(),
            ok: true,
            detail: format!("no /dev/{prefix}* nodes present ({purpose} not exposed yet)"),
            remedy: None,
        };
    }
    let unwritable: Vec<String> = nodes
        .iter()
        .filter(|node| !is_writable(node))
        .map(|node| display_from_root(root, node))
        .collect();
    if unwritable.is_empty() {
        PermissionCheck {
            id: id.to_owned(),
            ok: true,
            detail: format!("{} /dev/{prefix}* node(s) writable", nodes.len()),
            remedy: None,
        }
    } else {
        PermissionCheck {
            id: id.to_owned(),
            ok: false,
            detail: format!(
                "not writable by the current user: {} ({purpose}); the OpenRGB udev rules \
                 grant access, replug or reboot after installing them",
                unwritable.join(", ")
            ),
            remedy: Some(udev_remedy.to_owned()),
        }
    }
}

fn list_device_nodes(dev: &Path, prefix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dev) else {
        return Vec::new();
    };
    entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.strip_prefix(prefix).is_some_and(|rest| {
                        !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
                    })
                })
        })
        .collect()
}

fn is_writable(path: &Path) -> bool {
    OpenOptions::new().write(true).open(path).is_ok()
}

fn display_from_root(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| format!("/{}", relative.display()))
        .unwrap_or_else(|_| path.display().to_string())
}
