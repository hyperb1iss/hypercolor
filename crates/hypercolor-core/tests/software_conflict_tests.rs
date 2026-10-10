//! Competing-software detection: catalog validation, matching, and the
//! store's change events.

use std::sync::{Arc, LazyLock};

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::conflicts::CatalogError;
use hypercolor_core::device::{SoftwareCatalog, SoftwareConflictStore};
use hypercolor_types::event::HypercolorEvent;
use hypercolor_types::host_software::{HostProcess, HostSoftwareSnapshot};

const TEST_CATALOG: &str = r#"
[[software]]
id = "suite"
name = "Suite"
processes = ["Suite.exe", "SuiteTray.exe"]
services = ["Suite.Service"]
all_drivers = true
smbus = true
remedy = "Quit Suite."

[[software]]
id = "panels"
name = "Panel Tool"
processes = ["Panel Tool.exe"]
drivers = ["lianli"]
remedy = "Quit Panel Tool."

[[software]]
id = "scripted"
name = "Scripted Daemon"
processes = ["scripted-daemon"]
drivers = ["razer"]
remedy = "Stop scripted-daemon."
"#;

static CATALOG: LazyLock<SoftwareCatalog> =
    LazyLock::new(|| SoftwareCatalog::from_toml(TEST_CATALOG).expect("test catalog parses"));

fn snapshot(processes: Vec<HostProcess>, services: &[&str]) -> HostSoftwareSnapshot {
    HostSoftwareSnapshot {
        processes,
        services: services
            .iter()
            .map(|service| (*service).to_owned())
            .collect(),
    }
}

fn ids(snapshot: &HostSoftwareSnapshot) -> Vec<String> {
    CATALOG
        .detect(snapshot)
        .into_iter()
        .map(|conflict| conflict.id)
        .collect()
}

#[test]
fn the_builtin_catalog_parses() {
    let catalog = SoftwareCatalog::builtin();
    assert!(!catalog.entries().is_empty());
}

#[test]
fn the_builtin_catalog_leaves_openrgb_out() {
    let openrgb = snapshot(
        vec![
            HostProcess::named("OpenRGB.exe"),
            HostProcess::with_command_line("openrgb", "openrgb --server"),
        ],
        &[],
    );
    assert!(
        SoftwareCatalog::builtin().detect(&openrgb).is_empty(),
        "OpenRGB beside Hypercolor is a supported setup"
    );
}

#[test]
fn nothing_running_means_no_conflicts() {
    let quiet = snapshot(
        vec![
            HostProcess::named("explorer.exe"),
            HostProcess::with_command_line("bash", "bash -l"),
        ],
        &["Spooler"],
    );
    assert!(ids(&quiet).is_empty());
}

#[test]
fn process_names_match_without_case_or_exe() {
    assert_eq!(
        ids(&snapshot(vec![HostProcess::named("suite.EXE")], &[])),
        ["suite"]
    );
    assert_eq!(
        ids(&snapshot(vec![HostProcess::named("suite")], &[])),
        ["suite"]
    );
}

#[test]
fn a_service_alone_is_enough() {
    let detected = CATALOG.detect(&snapshot(Vec::new(), &["suite.service"]));
    assert_eq!(detected.len(), 1);
    assert_eq!(detected[0].matched, ["suite.service"]);
}

#[test]
fn matched_names_keep_the_hosts_spelling_and_collapse_duplicates() {
    let detected = CATALOG.detect(&snapshot(
        vec![
            HostProcess::named("Suite.exe"),
            HostProcess::named("Suite.exe"),
            HostProcess::named("SuiteTray.exe"),
        ],
        &["Suite.Service"],
    ));
    assert_eq!(
        detected[0].matched,
        ["Suite.Service", "Suite.exe", "SuiteTray.exe"]
    );
}

#[test]
fn a_quoted_windows_path_matches_its_program() {
    let tool = HostProcess::with_command_line(
        "Panel Tool.exe",
        r#""C:\Program Files\Panel Tool\Panel Tool.exe" --minimized"#,
    );
    assert_eq!(ids(&snapshot(vec![tool], &[])), ["panels"]);

    let renamed = HostProcess::with_command_line(
        "launcher.exe",
        r#""C:\Program Files\Panel Tool\Panel Tool.exe" --minimized"#,
    );
    assert_eq!(
        ids(&snapshot(vec![renamed], &[])),
        ["panels"],
        "the launched program counts even when the process name differs"
    );
}

#[test]
fn an_interpreted_script_matches_its_file_name() {
    let daemon = HostProcess::with_command_line(
        "python3",
        "/usr/bin/python3 /usr/bin/scripted-daemon --foreground",
    );
    assert_eq!(ids(&snapshot(vec![daemon], &[])), ["scripted"]);

    let versioned = HostProcess::with_command_line("python3.12", "python3.12 /opt/scripted-daemon");
    assert_eq!(ids(&snapshot(vec![versioned], &[])), ["scripted"]);
}

#[test]
fn only_an_interpreter_lends_its_argument() {
    let viewer = HostProcess::with_command_line("less", "less /usr/bin/scripted-daemon");
    assert!(
        ids(&snapshot(vec![viewer], &[])).is_empty(),
        "reading the script is not running it"
    );
    let flag = HostProcess::with_command_line("python3", "python3 -m scripted-daemon");
    assert!(ids(&snapshot(vec![flag], &[])).is_empty());
}

#[test]
fn scope_fields_follow_the_catalog() {
    let detected = CATALOG.detect(&snapshot(
        vec![
            HostProcess::named("Suite.exe"),
            HostProcess::named("Panel Tool.exe"),
        ],
        &[],
    ));
    assert_eq!(detected.len(), 2);
    let suite = &detected[0];
    assert!(suite.all_drivers && suite.smbus);
    assert!(suite.driver_ids.is_empty());
    let panels = &detected[1];
    assert_eq!(panels.driver_ids, ["lianli"]);
    assert!(panels.affects("lianli", false));
    assert!(!panels.affects("razer", false));
}

#[test]
fn invalid_catalogs_are_rejected() {
    let duplicate = r#"
[[software]]
id = "a"
name = "A"
processes = ["a"]
all_drivers = true
remedy = "r"

[[software]]
id = "a"
name = "A again"
processes = ["b"]
all_drivers = true
remedy = "r"
"#;
    assert!(matches!(
        SoftwareCatalog::from_toml(duplicate),
        Err(CatalogError::DuplicateId(id)) if id == "a"
    ));

    let unmatchable = r#"
[[software]]
id = "ghost"
name = "Ghost"
all_drivers = true
remedy = "r"
"#;
    assert!(matches!(
        SoftwareCatalog::from_toml(unmatchable),
        Err(CatalogError::Unmatchable(_))
    ));

    let unscoped = r#"
[[software]]
id = "idle"
name = "Idle"
processes = ["idle"]
remedy = "r"
"#;
    assert!(matches!(
        SoftwareCatalog::from_toml(unscoped),
        Err(CatalogError::NoScope(_))
    ));

    let misspelled = r#"
[[software]]
id = "typo"
name = "Typo"
processes = ["typo"]
all_driver = true
remedy = "r"
"#;
    assert!(matches!(
        SoftwareCatalog::from_toml(misspelled),
        Err(CatalogError::Parse(_))
    ));
}

#[tokio::test]
async fn the_store_publishes_only_when_the_set_changes() {
    let bus = Arc::new(HypercolorBus::new());
    let mut events = bus.subscribe_all();
    let store = SoftwareConflictStore::with_catalog(&CATALOG).with_event_bus(Arc::clone(&bus));

    let before = store.status();
    assert!(!before.scanned && !before.supported);

    store.record(Some(&snapshot(Vec::new(), &[])));
    let status = store.status();
    assert!(status.scanned && status.supported && status.conflicts.is_empty());
    assert!(
        events.try_recv().is_err(),
        "an empty first scan changes nothing"
    );

    let running = snapshot(vec![HostProcess::named("Panel Tool.exe")], &[]);
    let changes = store.record(Some(&running));
    assert_eq!(changes.appeared.len(), 1);
    assert_eq!(changes.appeared[0].id, "panels");
    assert!(changes.cleared.is_empty());
    let event = events.try_recv().expect("a new conflict publishes");
    assert!(matches!(
        event.event,
        HypercolorEvent::SoftwareConflictsChanged { count: 1 }
    ));

    assert!(store.record(Some(&running)).is_empty());
    assert!(events.try_recv().is_err(), "the same set publishes nothing");

    assert_eq!(store.affecting("lianli", false).len(), 1);
    assert!(store.affecting("razer", false).is_empty());

    let changes = store.record(Some(&snapshot(Vec::new(), &[])));
    assert_eq!(changes.cleared.len(), 1);
    assert!(changes.appeared.is_empty());
    let event = events.try_recv().expect("a cleared conflict publishes");
    assert!(matches!(
        event.event,
        HypercolorEvent::SoftwareConflictsChanged { count: 0 }
    ));
}

#[test]
fn a_host_without_an_inventory_is_unsupported() {
    let store = SoftwareConflictStore::with_catalog(&CATALOG);
    store.record(None);
    let status = store.status();
    assert!(status.scanned);
    assert!(!status.supported);
    assert!(status.conflicts.is_empty());
}

#[tokio::test]
async fn scan_requests_coalesce_and_wake_the_scanner() {
    let store = SoftwareConflictStore::with_catalog(&CATALOG);
    store.request_scan();
    store.request_scan();
    tokio::time::timeout(std::time::Duration::from_secs(1), store.scan_requested())
        .await
        .expect("a pending request wakes the scanner");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), store.scan_requested())
            .await
            .is_err(),
        "two requests before the wait collapse into one scan"
    );
}

#[test]
fn the_builtin_catalog_recognizes_the_real_spellings() {
    let catalog = SoftwareCatalog::builtin();
    let detect = |process: HostProcess, services: &[&str]| -> Vec<String> {
        catalog
            .detect(&snapshot(vec![process], services))
            .into_iter()
            .map(|conflict| conflict.id)
            .collect()
    };

    assert_eq!(
        detect(HostProcess::named("SignalRgbLauncher.exe"), &[]),
        ["signalrgb"]
    );
    assert_eq!(
        detect(
            HostProcess::with_command_line(
                "L-Connect-Service.exe",
                r#""C:\Program Files\L-Connect 3\L-Connect-Service.exe""#,
            ),
            &[],
        ),
        ["lian_li_l_connect"]
    );
    assert_eq!(
        detect(
            HostProcess::named("explorer.exe"),
            &["Razer Synapse Service"]
        ),
        ["razer_synapse"],
        "service names with spaces match whole"
    );
    assert_eq!(
        detect(HostProcess::named("explorer.exe"), &["CorsairService"]),
        ["corsair_icue"]
    );
    assert!(
        detect(HostProcess::named("explorer.exe"), &["Corsair Service"]).is_empty(),
        "a display name is not a service name"
    );
    assert_eq!(
        detect(
            HostProcess::with_command_line("openrazer-daemo", "openrazer-daemon"),
            &[],
        ),
        ["openrazer"],
        "a comm truncated to 15 characters still matches through argv[0]"
    );
    assert_eq!(
        detect(
            HostProcess::with_command_line("OpenLinkHub", "/usr/bin/openlinkhub"),
            &[],
        ),
        ["openlinkhub"],
        "the lowercase Arch binary matches too"
    );
    assert!(
        detect(HostProcess::named("gcc.exe"), &[]).is_empty(),
        "generic names stay out of the catalog"
    );
}

#[test]
fn every_builtin_entry_has_a_remedy_without_dashes() {
    for entry in SoftwareCatalog::builtin().entries() {
        assert!(
            !entry.remedy.trim().is_empty(),
            "{} needs a remedy",
            entry.id
        );
        assert!(
            !entry.remedy.contains(['\u{2013}', '\u{2014}']),
            "{} remedy uses an en or em dash",
            entry.id
        );
    }
}
