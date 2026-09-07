//! Host permission and driver prerequisites for OpenRGB device access.
//!
//! Only Linux has anything to check: udev rules for HID and I2C nodes, the
//! `i2c-dev` module for SMBus, and write access to the device nodes. The
//! filesystem inspection is written against an injectable root so tests can
//! stage a fake tree; [`permission_checks`] runs it against `/` on Linux and
//! returns nothing elsewhere.
//!
//! The hidraw check only judges nodes whose USB VID:PID appears in the
//! installed OpenRGB rules file: every other HID device (root-only keyboards,
//! audio controls, hubs OpenRGB has no rule for) is normal and reported as
//! informational rather than as a failure.

use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use crate::types::PermissionCheck;

/// Check id: an OpenRGB udev rules file is installed.
pub const CHECK_UDEV_RULES: &str = "udev_rules";
/// Check id: the `i2c-dev` kernel module is loaded.
pub const CHECK_I2C_DEV_MODULE: &str = "i2c_dev_module";
/// Check id: every `/dev/i2c-*` node is writable by the current user.
pub const CHECK_I2C_NODES: &str = "i2c_nodes_writable";
/// Check id: every `/dev/hidraw*` node covered by the OpenRGB rules file is
/// writable by the current user.
pub const CHECK_HIDRAW_NODES: &str = "hidraw_nodes_writable";

/// Where OpenRGB's own instructions and the distro packages put the rules.
pub const UDEV_RULES_PATHS: [&str; 3] = [
    "etc/udev/rules.d/60-openrgb.rules",
    "usr/lib/udev/rules.d/60-openrgb.rules",
    "lib/udev/rules.d/60-openrgb.rules",
];

const UDEV_RULES_TARGET: &str = "/etc/udev/rules.d/60-openrgb.rules";

/// The rules file as shipped with the current OpenRGB release tag.
pub const UDEV_RULES_URL: &str =
    "https://gitlab.com/CalcProgrammer1/OpenRGB/-/raw/release_candidate_1.0rc3.1/60-openrgb.rules";
const UDEV_RELOAD: &str = "sudo udevadm control --reload-rules && sudo udevadm trigger";

/// Permission checks for the current host.
///
/// Real on Linux, empty everywhere else: Windows and macOS have no udev or
/// device-node story for OpenRGB (Windows elevation and PawnIO are covered
/// by the install hints).
#[must_use]
pub fn permission_checks() -> Vec<PermissionCheck> {
    #[cfg(target_os = "linux")]
    {
        linux_permission_checks_at(Path::new("/"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

/// Linux permission checks evaluated against `root` as the filesystem root.
///
/// Available on every target so the logic stays testable; only
/// [`permission_checks`] decides whether the host has anything to inspect.
#[must_use]
pub fn linux_permission_checks_at(root: &Path) -> Vec<PermissionCheck> {
    let udev_remedy = udev_rules_remedy();
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
        hidraw_nodes_check(root, &udev_remedy),
    ]
}

/// Collect the USB `(idVendor, idProduct)` pairs an OpenRGB udev rules file
/// grants access to, as lowercase four-digit hex strings.
///
/// The file pairs `ATTRS{idVendor}=="xxxx"` with `ATTRS{idProduct}=="yyyy"`
/// on one line per device; lines without both are ignored.
#[must_use]
pub fn parse_rules_device_ids(rules: &str) -> BTreeSet<(String, String)> {
    rules
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| {
            let vendor = quoted_attr(line, "idVendor")?;
            let product = quoted_attr(line, "idProduct")?;
            Some((vendor, product))
        })
        .collect()
}

fn quoted_attr(line: &str, attr: &str) -> Option<String> {
    let marker = format!("{attr}}}==\"");
    let start = line.find(&marker)? + marker.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    let value = rest[..end].trim().to_ascii_lowercase();
    (value.len() == 4 && value.chars().all(|c| c.is_ascii_hexdigit())).then_some(value)
}

/// Parse a hidraw `uevent` file's `HID_ID=0003:0000XXXX:0000YYYY` line into
/// lowercase `(vid, pid)`.
#[must_use]
pub fn parse_hid_id(uevent: &str) -> Option<(String, String)> {
    let hid_id = uevent
        .lines()
        .find_map(|line| line.trim().strip_prefix("HID_ID="))?;
    let mut fields = hid_id.split(':');
    let _bus = fields.next()?;
    let vendor = fields.next()?;
    let product = fields.next()?;
    let tail = |field: &str| -> Option<String> {
        let field = field.trim();
        (field.len() >= 4 && field.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| field[field.len() - 4..].to_ascii_lowercase())
    };
    Some((tail(vendor)?, tail(product)?))
}

fn installed_rules_path(root: &Path) -> Option<PathBuf> {
    UDEV_RULES_PATHS
        .iter()
        .map(|relative| root.join(relative))
        .find(|path| path.is_file())
}

fn hidraw_nodes_check(root: &Path, udev_remedy: &str) -> PermissionCheck {
    let mut nodes = list_device_nodes(&root.join("dev"), "hidraw");
    nodes.sort();
    if nodes.is_empty() {
        return PermissionCheck {
            id: CHECK_HIDRAW_NODES.to_owned(),
            ok: true,
            detail: "no /dev/hidraw* nodes present (HID devices not exposed yet)".to_owned(),
            remedy: None,
        };
    }

    let Some(rules_path) = installed_rules_path(root) else {
        return PermissionCheck {
            id: CHECK_HIDRAW_NODES.to_owned(),
            ok: true,
            detail: format!(
                "{} /dev/hidraw* node(s) present; no OpenRGB rules file installed, so coverage is \
                 unknown (see {CHECK_UDEV_RULES})",
                nodes.len()
            ),
            remedy: None,
        };
    };
    let covered_ids = std::fs::read_to_string(&rules_path)
        .map(|rules| parse_rules_device_ids(&rules))
        .unwrap_or_default();

    let mut covered_writable = 0_usize;
    let mut uncovered = 0_usize;
    let mut failures: Vec<String> = Vec::new();
    for node in &nodes {
        let name = node
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let ids = std::fs::read_to_string(
            root.join("sys/class/hidraw")
                .join(name)
                .join("device/uevent"),
        )
        .ok()
        .and_then(|uevent| parse_hid_id(&uevent));
        match ids {
            Some((vendor, product)) if covered_ids.contains(&(vendor.clone(), product.clone())) => {
                if is_writable(node) {
                    covered_writable += 1;
                } else {
                    failures.push(format!(
                        "{} ({vendor}:{product})",
                        device_node_display(node)
                    ));
                }
            }
            _ => uncovered += 1,
        }
    }

    if failures.is_empty() {
        PermissionCheck {
            id: CHECK_HIDRAW_NODES.to_owned(),
            ok: true,
            detail: format!(
                "{covered_writable} node(s) covered by OpenRGB rules writable; {uncovered} not \
                 covered by OpenRGB rules"
            ),
            remedy: None,
        }
    } else {
        PermissionCheck {
            id: CHECK_HIDRAW_NODES.to_owned(),
            ok: false,
            detail: format!(
                "covered by OpenRGB rules but not writable by the current user: {}; reinstall the \
                 rules, then replug or reboot ({uncovered} other node(s) not covered)",
                failures.join(", ")
            ),
            remedy: Some(udev_remedy.to_owned()),
        }
    }
}

/// The command that installs OpenRGB's udev rules on a released build.
///
/// Released OpenRGB (1.0rc3 and earlier) ships no `--generate-udev-rules`
/// flag; that exists only on master. The portable remedy is to install the
/// rules file from the release tag and reload udev.
#[must_use]
pub fn udev_rules_remedy() -> String {
    format!("sudo curl -fsSL -o {UDEV_RULES_TARGET} {UDEV_RULES_URL} && {UDEV_RELOAD}")
}

fn udev_rules_check(root: &Path, remedy: &str) -> PermissionCheck {
    let found = UDEV_RULES_PATHS
        .iter()
        .find(|relative| root.join(relative).is_file());
    match found {
        Some(relative) => PermissionCheck {
            id: CHECK_UDEV_RULES.to_owned(),
            ok: true,
            detail: format!("found /{relative}"),
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
        .map(|node| device_node_display(node))
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

/// Render a device node as `/dev/<name>` regardless of the inspected root or
/// the host's path separator, so detail strings are stable everywhere.
fn device_node_display(node: &Path) -> String {
    match node.file_name().and_then(|name| name.to_str()) {
        Some(name) => format!("/dev/{name}"),
        None => node.display().to_string(),
    }
}
