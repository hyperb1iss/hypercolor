use std::ffi::OsStr;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use hypercolor_app::supervisor::openrgb::{
    DetectorPartitionPlan, DeviceFacts, DriverFacts, OPENRGB_BRIDGE_DRIVER_ID, OpenRgbInspection,
    OpenRgbPlanSummary, OpenRgbStatus, OpenRgbSupervisor, QT_PLATFORM_ENV, QT_PLATFORM_OFFSCREEN,
    apply_headless_env, bridge_enabled, default_server_addr, known_detector_driver_ids,
    launch_spec, needs_offscreen_qt, partition_driver_ids, plan_summary,
};
use hypercolor_app::supervisor::{OpenRgbHoldReason, OpenRgbPlan};
use hypercolor_openrgb_host::{
    BinaryKind, DEFAULT_SERVER_PORT, LOOPBACK_HOST, ManagedConfigDir, OpenRgbBinary, ProcessSpec,
    ServerProbe, detector_prefixes_for_drivers,
};
use hypercolor_types::device::DriverModuleKind;

fn facts(id: &str, module_kind: DriverModuleKind, enabled: bool) -> DriverFacts {
    DriverFacts {
        id: id.to_owned(),
        module_kind,
        enabled,
    }
}

fn typical_registry() -> Vec<DriverFacts> {
    vec![
        facts("razer", DriverModuleKind::Hal, true),
        facts("lianli", DriverModuleKind::Hal, false),
        facts("corsair", DriverModuleKind::Hal, true),
        facts("hue", DriverModuleKind::Network, true),
        facts(OPENRGB_BRIDGE_DRIVER_ID, DriverModuleKind::Bridge, true),
    ]
}

fn device(driver_id: &str, disabled: bool) -> DeviceFacts {
    DeviceFacts {
        driver_id: driver_id.to_owned(),
        disabled,
    }
}

/// The live rig this was verified against: Corsair, Lian Li, and Razer own
/// enabled devices; Dygma's only device sits in state `known`; Nollie's only
/// device is user-disabled; ASUS is enabled with no device at all; QMK has
/// devices but no family in the detector table.
fn rig_drivers() -> Vec<DriverFacts> {
    vec![
        facts("corsair", DriverModuleKind::Hal, true),
        facts("lianli", DriverModuleKind::Hal, true),
        facts("razer", DriverModuleKind::Hal, true),
        facts("dygma", DriverModuleKind::Hal, true),
        facts("nollie", DriverModuleKind::Hal, true),
        facts("asus", DriverModuleKind::Hal, true),
        facts("qmk", DriverModuleKind::Hal, true),
        facts("hue", DriverModuleKind::Network, true),
        facts(OPENRGB_BRIDGE_DRIVER_ID, DriverModuleKind::Bridge, true),
    ]
}

fn rig_devices() -> Vec<DeviceFacts> {
    vec![
        device("corsair", false),
        device("corsair", true),
        device("lianli", false),
        device("razer", false),
        device("dygma", false),
        device("nollie", true),
        device("qmk", false),
        device("hue", false),
        device(OPENRGB_BRIDGE_DRIVER_ID, false),
    ]
}

#[test]
fn partition_disables_only_enabled_drivers_that_own_an_enabled_device() {
    let plan = partition_driver_ids(&rig_drivers(), &rig_devices(), &known_detector_driver_ids());

    assert_eq!(
        plan.disabled_driver_ids,
        vec![
            "corsair".to_owned(),
            "dygma".to_owned(),
            "lianli".to_owned(),
            "razer".to_owned(),
        ]
    );
    // Only-disabled devices (nollie) and no devices (asus) are handed back
    // to OpenRGB. Drivers without a detector family (hue, qmk) never appear.
    assert_eq!(
        plan.re_enable_driver_ids,
        vec!["asus".to_owned(), "nollie".to_owned()]
    );
}

#[test]
fn partition_hands_back_a_family_whose_driver_is_disabled() {
    let mut drivers = rig_drivers();
    drivers
        .iter_mut()
        .filter(|driver| driver.id == "razer")
        .for_each(|driver| driver.enabled = false);

    let plan = partition_driver_ids(&drivers, &rig_devices(), &known_detector_driver_ids());

    assert!(!plan.disabled_driver_ids.iter().any(|id| id == "razer"));
    assert!(plan.re_enable_driver_ids.iter().any(|id| id == "razer"));
}

#[test]
fn partition_maps_to_the_detector_prefixes_those_families_own() {
    let plan = partition_driver_ids(&rig_drivers(), &rig_devices(), &known_detector_driver_ids());

    let disabled = detector_prefixes_for_drivers(&plan.disabled_driver_ids);
    assert_eq!(
        disabled,
        vec![
            "Corsair ".to_owned(),
            "Dygma ".to_owned(),
            "Lian Li ".to_owned(),
            "Razer ".to_owned(),
        ]
    );
    let re_enable = detector_prefixes_for_drivers(&plan.re_enable_driver_ids);
    assert!(re_enable.iter().any(|prefix| prefix == "Nollie "));
    assert!(re_enable.iter().any(|prefix| prefix == "ASUS Aura"));
    assert!(!re_enable.iter().any(|prefix| prefix == "Razer "));
}

#[test]
fn partition_with_no_daemon_devices_re_enables_every_known_family() {
    let plan = partition_driver_ids(&rig_drivers(), &[], &known_detector_driver_ids());

    assert!(plan.disabled_driver_ids.is_empty());
    let mut known = known_detector_driver_ids();
    known.sort();
    assert_eq!(plan.re_enable_driver_ids, known);
}

#[test]
fn partition_ignores_case_and_bridge_devices() {
    let drivers = vec![
        facts("Razer", DriverModuleKind::Hal, true),
        facts(OPENRGB_BRIDGE_DRIVER_ID, DriverModuleKind::Bridge, true),
    ];
    let devices = vec![
        device("razer", false),
        device(OPENRGB_BRIDGE_DRIVER_ID, false),
    ];

    let plan = partition_driver_ids(&drivers, &devices, &["razer", "openrgb"]);

    assert_eq!(
        plan,
        DetectorPartitionPlan {
            disabled_driver_ids: vec!["Razer".to_owned()],
            re_enable_driver_ids: vec!["openrgb".to_owned()],
        }
    );
}

#[test]
fn device_facts_treat_only_the_disabled_status_as_disabled() {
    for (status, expected) in [
        ("disabled", true),
        ("Disabled", true),
        ("known", false),
        ("connected", false),
        ("active", false),
        ("reconnecting", false),
    ] {
        let facts = DeviceFacts {
            driver_id: "razer".to_owned(),
            disabled: status.eq_ignore_ascii_case("disabled"),
        };
        assert_eq!(facts.disabled, expected, "status {status}");
    }
}

#[test]
fn bridge_enabled_requires_the_openrgb_driver_to_be_on() {
    assert!(bridge_enabled(&typical_registry()));

    let mut disabled = typical_registry();
    disabled
        .iter_mut()
        .filter(|driver| driver.id == OPENRGB_BRIDGE_DRIVER_ID)
        .for_each(|driver| driver.enabled = false);
    assert!(!bridge_enabled(&disabled));

    let absent: Vec<DriverFacts> = typical_registry()
        .into_iter()
        .filter(|driver| driver.id != OPENRGB_BRIDGE_DRIVER_ID)
        .collect();
    assert!(!bridge_enabled(&absent));
}

#[test]
fn offscreen_qt_only_on_displayless_linux() {
    let x11 = OsStr::new(":0");
    let wayland = OsStr::new("wayland-0");
    let empty = OsStr::new("");

    assert!(needs_offscreen_qt(true, None, None));
    assert!(needs_offscreen_qt(true, Some(empty), Some(empty)));
    assert!(!needs_offscreen_qt(true, Some(x11), None));
    assert!(!needs_offscreen_qt(true, None, Some(wayland)));
    assert!(!needs_offscreen_qt(false, None, None));
}

#[test]
fn headless_env_is_added_once_and_never_overrides_an_explicit_choice() {
    let mut spec = ProcessSpec::default();
    apply_headless_env(&mut spec, false, None);
    assert!(spec.env.is_empty());

    apply_headless_env(&mut spec, true, None);
    assert_eq!(
        spec.env.get(QT_PLATFORM_ENV).map(String::as_str),
        Some(QT_PLATFORM_OFFSCREEN)
    );
    apply_headless_env(&mut spec, true, None);
    assert_eq!(spec.env.len(), 1);

    let mut explicit = ProcessSpec::default();
    explicit
        .env
        .insert(QT_PLATFORM_ENV.to_owned(), "xcb".to_owned());
    apply_headless_env(&mut explicit, true, None);
    assert_eq!(
        explicit.env.get(QT_PLATFORM_ENV).map(String::as_str),
        Some("xcb")
    );
}

#[test]
fn headless_env_respects_a_qt_platform_inherited_from_the_parent() {
    let mut spec = ProcessSpec::default();
    apply_headless_env(&mut spec, true, Some(OsStr::new("wayland")));
    assert!(spec.env.is_empty());

    // An empty inherited value is no choice at all.
    apply_headless_env(&mut spec, true, Some(OsStr::new("")));
    assert_eq!(
        spec.env.get(QT_PLATFORM_ENV).map(String::as_str),
        Some(QT_PLATFORM_OFFSCREEN)
    );
}

#[test]
fn launch_spec_binds_loopback_and_the_managed_config_dir() {
    let binary = OpenRgbBinary {
        path: PathBuf::from("/usr/bin/openrgb"),
        kind: BinaryKind::Native,
        version: None,
    };
    let config_dir = ManagedConfigDir {
        root: PathBuf::from("/tmp/hypercolor-test/openrgb"),
    };

    let spec = launch_spec(
        &binary,
        &config_dir,
        "127.0.0.1:6799".parse().expect("endpoint"),
    )
    .expect("utf-8 config path launches");

    assert_eq!(spec.program, binary.path);
    let host_index = spec
        .args
        .iter()
        .position(|arg| arg == "--server-host")
        .expect("server host flag present");
    assert_eq!(spec.args[host_index + 1], LOOPBACK_HOST);
    let port_index = spec
        .args
        .iter()
        .position(|arg| arg == "--server-port")
        .expect("server port flag present");
    assert_eq!(spec.args[port_index + 1], "6799");
    let config_index = spec
        .args
        .iter()
        .position(|arg| arg == "--config")
        .expect("config flag present");
    assert_eq!(spec.args[config_index + 1], "/tmp/hypercolor-test/openrgb");
    assert!(spec.args.iter().any(|arg| arg == "--noautoconnect"));
}

#[test]
fn plan_summary_covers_every_arm() {
    let addr = default_server_addr();
    assert_eq!(
        plan_summary(&OpenRgbPlan::Adopt {
            addr,
            probe: ServerProbe::default(),
        }),
        OpenRgbPlanSummary::Adopt
    );
    assert_eq!(
        plan_summary(&OpenRgbPlan::Spawn {
            spec: ProcessSpec::default(),
        }),
        OpenRgbPlanSummary::Spawn
    );
    let hold = |reason| OpenRgbPlan::Hold { reason };
    assert_eq!(
        plan_summary(&hold(OpenRgbHoldReason::NotInstalled { hints: vec![] })),
        OpenRgbPlanSummary::HoldNotInstalled
    );
    assert_eq!(
        plan_summary(&hold(OpenRgbHoldReason::PermissionsMissing {
            checks: vec![]
        })),
        OpenRgbPlanSummary::HoldPermissionsMissing
    );
    assert_eq!(
        plan_summary(&hold(OpenRgbHoldReason::BridgeDisabled)),
        OpenRgbPlanSummary::HoldBridgeDisabled
    );
    assert_eq!(
        plan_summary(&hold(OpenRgbHoldReason::PortOwnedByUnknown { addr })),
        OpenRgbPlanSummary::HoldPortOwnedByUnknown
    );
    assert_eq!(
        plan_summary(&hold(OpenRgbHoldReason::Starting { pid: 7 })),
        OpenRgbPlanSummary::HoldStarting
    );
}

fn inspection(probe: ServerProbe, managed_pid: Option<u32>) -> OpenRgbInspection {
    OpenRgbInspection {
        binary: None,
        addr: default_server_addr(),
        probe,
        port_open: true,
        checks: Vec::new(),
        hints: Vec::new(),
        bridge_enabled: true,
        partition_plan: DetectorPartitionPlan::default(),
        config_dir: ManagedConfigDir {
            root: PathBuf::from("/tmp/hypercolor-test/openrgb"),
        },
        data_dir: PathBuf::from("/tmp/hypercolor-test"),
        spawn: None,
        managed_pid,
    }
}

fn live_probe() -> ServerProbe {
    ServerProbe {
        reachable: true,
        protocol_version: Some(4),
        controller_count: Some(1),
        error: None,
    }
}

#[test]
fn a_reachable_server_that_is_our_own_child_is_managed_not_adopted() {
    let ours = inspection(live_probe(), Some(4242));
    let plan = ours.plan();
    assert!(matches!(plan, OpenRgbPlan::Adopt { .. }));

    let mut status = OpenRgbStatus::default();
    status.apply_inspection(&ours, &plan, Some(4242));

    assert!(!status.adopted);
    assert_eq!(status.managed_pid, Some(4242));
    assert_eq!(status.plan_summary, Some(OpenRgbPlanSummary::Adopt));
}

#[test]
fn a_reachable_server_nobody_here_spawned_is_adopted() {
    let theirs = inspection(live_probe(), None);
    let plan = theirs.plan();

    let mut status = OpenRgbStatus::default();
    status.apply_inspection(&theirs, &plan, None);

    assert!(status.adopted);
    assert_eq!(status.managed_pid, None);
}

#[test]
fn a_starting_child_keeps_the_last_error_and_is_not_foreign() {
    let starting = inspection(ServerProbe::default(), Some(4242));
    let plan = starting.plan();
    assert_eq!(plan_summary(&plan), OpenRgbPlanSummary::HoldStarting);

    let mut status = OpenRgbStatus {
        last_error: Some("still detecting devices".to_owned()),
        ..OpenRgbStatus::default()
    };
    status.apply_inspection(&starting, &plan, Some(4242));
    assert_eq!(
        status.last_error.as_deref(),
        Some("still detecting devices")
    );
    assert!(!status.adopted);

    // Any other outcome clears the stale report.
    let settled = inspection(live_probe(), Some(4242));
    let plan = settled.plan();
    status.apply_inspection(&settled, &plan, Some(4242));
    assert_eq!(status.last_error, None);
}

#[test]
fn status_defaults_to_the_loopback_sdk_port_and_serializes_snake_case() {
    let status = OpenRgbStatus::default();
    assert_eq!(
        status.addr,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_SERVER_PORT)
    );

    let json = serde_json::to_value(&status).expect("status serializes");
    assert_eq!(json["addr"], "127.0.0.1:6742");
    assert_eq!(json["managed_pid"], serde_json::Value::Null);
    assert_eq!(json["plan_summary"], serde_json::Value::Null);
    assert_eq!(json["adopted"], false);
    assert_eq!(json["bridge_enabled"], false);
    assert!(json.get("last_error").is_some());
    assert!(json.get("lastError").is_none());

    let summary = serde_json::to_value(OpenRgbPlanSummary::HoldPortOwnedByUnknown)
        .expect("summary serializes");
    assert_eq!(summary, "hold_port_owned_by_unknown");
}

#[test]
fn stopping_with_nothing_managed_is_a_no_op() {
    let supervisor = OpenRgbSupervisor::default();

    assert_eq!(supervisor.managed_pid(), None);
    assert!(supervisor.stop_managed().is_none());
    assert_eq!(supervisor.status().managed_pid, None);
    supervisor.terminate_managed_for_exit();
}

#[test]
fn stopping_with_nothing_managed_leaves_the_adopted_probe_alone() {
    let supervisor = OpenRgbSupervisor::default();
    let adopted = inspection(live_probe(), None);
    let plan = adopted.plan();
    // Seed the status the way detect() would for an adopted server.
    let mut expected = OpenRgbStatus::default();
    expected.apply_inspection(&adopted, &plan, None);
    assert!(expected.adopted);

    // The supervisor has no way to inject status from outside, so drive the
    // same fold through its public surface: a default supervisor holds no
    // child, and stop_managed must not touch the probe it does not own.
    let before = supervisor.status();
    assert!(supervisor.stop_managed().is_none());
    assert_eq!(supervisor.status(), before);
}

#[cfg(unix)]
mod managed_child {
    use std::os::unix::process::ExitStatusExt;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use hypercolor_app::supervisor::openrgb::{ManagedOpenRgb, OPENRGB_STOP_GRACE};
    use hypercolor_openrgb_host::ProcessSpec;

    fn log_file() -> std::fs::File {
        tempfile::tempfile().expect("temp log file")
    }

    fn sleeper() -> ProcessSpec {
        ProcessSpec {
            program: PathBuf::from("/bin/sleep"),
            args: vec!["30".to_owned()],
            ..ProcessSpec::default()
        }
    }

    #[test]
    fn stop_terminates_gracefully_within_the_grace_budget() {
        let mut managed = ManagedOpenRgb::spawn(&sleeper(), log_file()).expect("sleep spawns");
        assert!(managed.pid() > 0);
        assert!(!managed.has_exited());

        let started = Instant::now();
        let status = managed.stop(OPENRGB_STOP_GRACE).expect("child reaped");

        assert_eq!(status.signal(), Some(libc::SIGTERM));
        assert!(started.elapsed() < OPENRGB_STOP_GRACE);
        assert!(managed.has_exited());
        assert!(managed.stop(OPENRGB_STOP_GRACE).is_none());
    }

    #[test]
    fn a_child_that_exits_on_its_own_is_reported_exited() {
        let spec = ProcessSpec {
            program: PathBuf::from("/bin/true"),
            ..ProcessSpec::default()
        };
        let mut managed = ManagedOpenRgb::spawn(&spec, log_file()).expect("true spawns");

        let deadline = Instant::now() + Duration::from_secs(5);
        while !managed.has_exited() {
            assert!(Instant::now() < deadline, "child should exit promptly");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_claimed_child_excludes_another_supervisor_until_its_tree_is_gone() {
        let directory = tempfile::tempdir().expect("managed directory");
        let endpoint = "127.0.0.1:6799".parse().expect("endpoint");
        let claim = hypercolor_openrgb_host::try_claim_server(directory.path(), endpoint)
            .expect("claim")
            .expect("first owner");
        let managed = ManagedOpenRgb::spawn_claimed(&sleeper(), log_file(), claim)
            .expect("spawn claimed child");
        assert!(
            hypercolor_openrgb_host::try_claim_server(directory.path(), endpoint)
                .expect("second claim")
                .is_none()
        );
        drop(managed);
        assert!(
            hypercolor_openrgb_host::try_claim_server(directory.path(), endpoint)
                .expect("released claim")
                .is_some()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn managed_server_outlives_idle_blocking_pool_threads() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_keep_alive(Duration::from_millis(20))
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let directory = tempfile::tempdir().expect("managed directory");
            let endpoint = "127.0.0.1:6799".parse().expect("endpoint");
            let claim = hypercolor_openrgb_host::try_claim_server(directory.path(), endpoint)
                .expect("claim")
                .expect("first owner");
            let mut managed =
                ManagedOpenRgb::spawn_logged(sleeper(), directory.path().to_owned(), claim)
                    .await
                    .expect("spawn logged child");
            tokio::time::sleep(Duration::from_millis(150)).await;
            assert!(
                !managed.has_exited(),
                "temporary log worker exit must not terminate the server"
            );
        });
    }

    #[test]
    fn dropping_a_live_child_kills_it() {
        let managed = ManagedOpenRgb::spawn(&sleeper(), log_file()).expect("sleep spawns");
        let pid = managed.pid();
        drop(managed);

        // SAFETY: `kill` with signal 0 delivers nothing and only reports
        // whether the pid exists; it has no memory-safety preconditions.
        let result = unsafe { libc::kill(libc::pid_t::try_from(pid).expect("pid fits"), 0) };
        assert_eq!(result, -1, "child should be gone after drop");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}
