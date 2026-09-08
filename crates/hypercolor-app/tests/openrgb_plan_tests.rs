use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use hypercolor_app::supervisor::{OpenRgbHoldReason, OpenRgbPlan, OpenRgbPlanInputs, openrgb_plan};
use hypercolor_openrgb_host::{
    BinaryKind, DEFAULT_SERVER_PORT, InstallHint, InstallMethod, OpenRgbBinary, PermissionCheck,
    Platform, ProcessSpec, ServerProbe,
};

fn addr() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_SERVER_PORT)
}

fn binary() -> OpenRgbBinary {
    OpenRgbBinary {
        path: PathBuf::from("/usr/bin/openrgb"),
        kind: BinaryKind::Native,
        version: Some("1.0rc2".to_owned()),
    }
}

fn spec() -> ProcessSpec {
    ProcessSpec {
        program: PathBuf::from("/usr/bin/openrgb"),
        args: vec!["--server".to_owned()],
        ..ProcessSpec::default()
    }
}

fn hint() -> InstallHint {
    InstallHint {
        platform: Platform::Linux,
        method: InstallMethod::Pacman,
        command: "sudo pacman -S openrgb".to_owned(),
        note: String::new(),
    }
}

fn check(id: &str, ok: bool) -> PermissionCheck {
    PermissionCheck {
        id: id.to_owned(),
        ok,
        detail: String::new(),
        remedy: (!ok).then(|| format!("fix {id}")),
    }
}

fn live_probe() -> ServerProbe {
    ServerProbe {
        reachable: true,
        protocol_version: Some(4),
        controller_count: Some(3),
        error: None,
    }
}

fn silent_probe() -> ServerProbe {
    ServerProbe {
        reachable: false,
        protocol_version: None,
        controller_count: None,
        error: Some("connection refused".to_owned()),
    }
}

/// A healthy host with nothing listening: the spawn baseline every other
/// case perturbs.
fn spawnable() -> OpenRgbPlanInputs {
    OpenRgbPlanInputs {
        bridge_enabled: true,
        binary: Some(binary()),
        addr: addr(),
        probe: silent_probe(),
        port_open: false,
        checks: vec![check("udev_rules", true), check("i2c_dev_module", true)],
        hints: vec![hint()],
        spawn: Some(spec()),
        managed_pid: None,
    }
}

#[test]
fn our_own_starting_child_holds_as_starting_not_foreign() {
    let mut inputs = spawnable();
    inputs.managed_pid = Some(4242);
    inputs.port_open = true;

    assert_eq!(
        openrgb_plan(inputs),
        OpenRgbPlan::Hold {
            reason: OpenRgbHoldReason::Starting { pid: 4242 },
        }
    );
}

#[test]
fn our_own_child_that_has_not_bound_the_port_yet_still_holds_as_starting() {
    let mut inputs = spawnable();
    inputs.managed_pid = Some(4242);
    inputs.port_open = false;

    assert!(matches!(
        openrgb_plan(inputs),
        OpenRgbPlan::Hold {
            reason: OpenRgbHoldReason::Starting { pid: 4242 },
        }
    ));
}

#[test]
fn our_own_child_that_answers_is_adopted_by_the_plan() {
    let mut inputs = spawnable();
    inputs.managed_pid = Some(4242);
    inputs.probe = live_probe();
    inputs.port_open = true;

    assert!(matches!(openrgb_plan(inputs), OpenRgbPlan::Adopt { .. }));
}

#[test]
fn starting_hold_serializes_with_the_pid() {
    let hold = serde_json::to_value(OpenRgbPlan::Hold {
        reason: OpenRgbHoldReason::Starting { pid: 4242 },
    })
    .expect("hold serializes");
    assert_eq!(hold["reason"]["kind"], "starting");
    assert_eq!(hold["reason"]["pid"], 4242);
}

#[test]
fn bridge_disabled_holds_before_anything_else() {
    let mut inputs = spawnable();
    inputs.bridge_enabled = false;
    inputs.probe = live_probe();

    assert_eq!(
        openrgb_plan(inputs),
        OpenRgbPlan::Hold {
            reason: OpenRgbHoldReason::BridgeDisabled,
        }
    );
}

#[test]
fn live_sdk_server_is_adopted_never_spawned() {
    let mut inputs = spawnable();
    inputs.probe = live_probe();
    inputs.port_open = true;

    assert_eq!(
        openrgb_plan(inputs),
        OpenRgbPlan::Adopt {
            addr: addr(),
            probe: live_probe(),
        }
    );
}

#[test]
fn live_server_is_adopted_even_without_a_binary_or_permissions() {
    let mut inputs = spawnable();
    inputs.probe = live_probe();
    inputs.binary = None;
    inputs.spawn = None;
    inputs.checks = vec![check("udev_rules", false)];

    assert!(matches!(openrgb_plan(inputs), OpenRgbPlan::Adopt { .. }));
}

#[test]
fn open_port_that_fails_the_handshake_holds_as_foreign() {
    let mut inputs = spawnable();
    inputs.port_open = true;

    assert_eq!(
        openrgb_plan(inputs),
        OpenRgbPlan::Hold {
            reason: OpenRgbHoldReason::PortOwnedByUnknown { addr: addr() },
        }
    );
}

#[test]
fn missing_binary_holds_with_install_hints() {
    let mut inputs = spawnable();
    inputs.binary = None;
    inputs.spawn = None;

    assert_eq!(
        openrgb_plan(inputs),
        OpenRgbPlan::Hold {
            reason: OpenRgbHoldReason::NotInstalled {
                hints: vec![hint()],
            },
        }
    );
}

#[test]
fn binary_without_a_launch_spec_counts_as_not_installed() {
    let mut inputs = spawnable();
    inputs.spawn = None;

    assert!(matches!(
        openrgb_plan(inputs),
        OpenRgbPlan::Hold {
            reason: OpenRgbHoldReason::NotInstalled { .. },
        }
    ));
}

#[test]
fn any_failing_permission_check_holds_and_lists_only_the_failures() {
    let mut inputs = spawnable();
    inputs.checks = vec![
        check("udev_rules", false),
        check("i2c_dev_module", true),
        check("hidraw_nodes_writable", false),
    ];

    assert_eq!(
        openrgb_plan(inputs),
        OpenRgbPlan::Hold {
            reason: OpenRgbHoldReason::PermissionsMissing {
                checks: vec![
                    check("udev_rules", false),
                    check("hidraw_nodes_writable", false),
                ],
            },
        }
    );
}

#[test]
fn healthy_host_with_nothing_listening_spawns() {
    assert_eq!(
        openrgb_plan(spawnable()),
        OpenRgbPlan::Spawn { spec: spec() }
    );
}

#[test]
fn no_permission_checks_at_all_still_spawns() {
    let mut inputs = spawnable();
    inputs.checks.clear();

    assert!(matches!(openrgb_plan(inputs), OpenRgbPlan::Spawn { .. }));
}

#[test]
fn plan_serializes_with_snake_case_kind_tags() {
    let adopt = serde_json::to_value(OpenRgbPlan::Adopt {
        addr: addr(),
        probe: live_probe(),
    })
    .expect("adopt serializes");
    assert_eq!(adopt["kind"], "adopt");
    assert_eq!(adopt["addr"], "127.0.0.1:6742");
    assert_eq!(adopt["probe"]["protocol_version"], 4);

    let hold = serde_json::to_value(OpenRgbPlan::Hold {
        reason: OpenRgbHoldReason::PortOwnedByUnknown { addr: addr() },
    })
    .expect("hold serializes");
    assert_eq!(hold["kind"], "hold");
    assert_eq!(hold["reason"]["kind"], "port_owned_by_unknown");

    let spawn =
        serde_json::to_value(OpenRgbPlan::Spawn { spec: spec() }).expect("spawn serializes");
    assert_eq!(spawn["kind"], "spawn");
    assert_eq!(spawn["spec"]["args"][0], "--server");
}
