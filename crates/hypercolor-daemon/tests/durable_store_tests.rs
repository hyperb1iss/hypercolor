//! The durable store inventory, its packaged declaration, and the on-disk
//! schema probe that runs before any store opens.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use hypercolor_daemon::durable_stores::{
    DURABLE_STORES, DurableStoreReport, StoreFound, StoreRoots, probe_durable_stores,
};
use serde_json::{Value, json};

fn packaged_declaration() -> Value {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/managed/durable-stores.json");
    serde_json::from_slice(&fs::read(&path).expect("read the packaged store declaration"))
        .expect("parse the packaged store declaration")
}

#[test]
fn packaged_declaration_is_exactly_the_store_inventory() {
    let packaged = packaged_declaration();
    let object = packaged.as_object().expect("declaration object");
    assert_eq!(
        object.keys().collect::<Vec<_>>(),
        ["stores"],
        "the packaged file is the manifest's compatibility object"
    );
    let expected: Vec<Value> = DURABLE_STORES
        .iter()
        .map(|store| {
            json!({
                "name": store.name,
                "storage_format": store.storage_format,
                "readable_schema_min": store.readable_schema_min,
                "readable_schema_max": store.readable_schema_max,
                "written_schema": store.written_schema,
                "migration_mode": store.migration_mode,
            })
        })
        .collect();
    assert_eq!(
        packaged["stores"].as_array().expect("stores array"),
        &expected,
        "packaging/managed/durable-stores.json must list DURABLE_STORES in order"
    );
}

#[test]
fn every_store_is_named_once_and_reads_what_it_writes() {
    let mut names = BTreeSet::new();
    for store in DURABLE_STORES {
        assert!(names.insert(store.name), "{} declared twice", store.name);
        assert!(
            store.readable_schema_min <= store.written_schema
                && store.written_schema <= store.readable_schema_max,
            "{} writes a schema it does not read",
            store.name
        );
        assert_eq!(
            store.migration_mode, "backward_compatible",
            "{}",
            store.name
        );
    }
}

#[test]
fn enforced_versions_come_from_the_stores_own_constants() {
    let find = |name: &str| {
        DURABLE_STORES
            .iter()
            .find(|store| store.name == name)
            .unwrap_or_else(|| panic!("{name} is inventoried"))
    };
    assert_eq!(
        find("config").written_schema,
        hypercolor_types::config::CURRENT_SCHEMA_VERSION
    );
    assert_eq!(
        find("asset-library").written_schema,
        hypercolor_core::asset::INDEX_VERSION
    );
    assert_eq!(
        find("device-settings").written_schema,
        hypercolor_daemon::device_settings::DEVICE_SETTINGS_SCHEMA_VERSION
    );
    for (name, min, max) in [
        ("config", 4, 5),
        ("scenes", 2, 2),
        ("device-settings", 1, 3),
        ("device-aliases", 2, 2),
        ("library", 1, 2),
        ("driver-inventory", 1, 1),
        ("device-binding-journal", 1, 1),
    ] {
        let store = find(name);
        assert_eq!(
            (store.readable_schema_min, store.readable_schema_max),
            (min, max),
            "{name}"
        );
    }
}

struct Roots {
    _temporary: tempfile::TempDir,
    roots: StoreRoots,
}

impl Roots {
    fn new() -> Self {
        let temporary = tempfile::tempdir().expect("temporary roots");
        let base = temporary.path();
        let roots = StoreRoots {
            config: base.join("config/hypercolor"),
            data: base.join("data/hypercolor"),
            state: base.join("state/hypercolor"),
            config_file: base.join("config/hypercolor/hypercolor.toml"),
        };
        for directory in [&roots.config, &roots.data, &roots.state] {
            fs::create_dir_all(directory).expect("create root");
        }
        Self {
            _temporary: temporary,
            roots,
        }
    }

    fn write(&self, path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(path, contents).expect("write store file");
    }

    fn data(&self, relative: &str, contents: &str) {
        self.write(&self.roots.data.join(relative), contents);
    }

    fn state(&self, relative: &str, contents: &str) {
        self.write(&self.roots.state.join(relative), contents);
    }

    fn config(&self, relative: &str, contents: &str) {
        self.write(&self.roots.config.join(relative), contents);
    }

    fn probe(&self) -> DurableStoreReport {
        probe_durable_stores(&self.roots)
    }
}

fn found(report: &DurableStoreReport, name: &str) -> StoreFound {
    report
        .found(name)
        .unwrap_or_else(|| panic!("{name} is probed"))
        .clone()
}

#[test]
fn empty_roots_hold_no_store() {
    let roots = Roots::new();
    let report = roots.probe();
    assert_eq!(report.observations.len(), DURABLE_STORES.len());
    for observation in &report.observations {
        assert_eq!(
            observation.found,
            StoreFound::Absent,
            "{}",
            observation.name
        );
    }
}

#[test]
fn versioned_stores_report_their_field_and_their_readers_default() {
    let roots = Roots::new();
    roots.write(&roots.roots.config_file, "schema_version = 4\n");
    roots.data("library.json", r#"{"favorites":[]}"#);
    roots.data("scenes.json", r#"{"schema_version":2,"scenes":{}}"#);
    roots.state("device-aliases.json", r#"{"aliases":{}}"#);
    roots.state(
        "device-settings.json",
        r#"{"schema_version":3,"devices":{}}"#,
    );
    roots.config("assets/index.json", r#"{"records":[]}"#);
    roots.state(
        "device-binding-migration.json",
        r#"{"schema_version":1,"remaps":[]}"#,
    );
    let report = roots.probe();
    assert_eq!(found(&report, "config"), StoreFound::Schema(4));
    assert_eq!(
        found(&report, "library"),
        StoreFound::Schema(1),
        "library reads a missing `version` as its legacy schema 1"
    );
    assert_eq!(found(&report, "scenes"), StoreFound::Schema(2));
    assert_eq!(
        found(&report, "device-aliases"),
        StoreFound::Schema(2),
        "device aliases read a missing version as 2"
    );
    assert_eq!(found(&report, "device-settings"), StoreFound::Schema(3));
    assert_eq!(found(&report, "asset-library"), StoreFound::Schema(1));
    assert_eq!(
        found(&report, "device-binding-journal"),
        StoreFound::Schema(1)
    );
}

#[test]
fn a_newer_schema_on_disk_is_reported_as_found() {
    let roots = Roots::new();
    roots.write(&roots.roots.config_file, "schema_version = 9\n");
    roots.data("library.json", r#"{"version":7}"#);
    roots.state("device-settings.json", r#"{"schema_version":4}"#);
    let report = roots.probe();
    assert_eq!(found(&report, "config"), StoreFound::Schema(9));
    assert_eq!(found(&report, "library"), StoreFound::Schema(7));
    assert_eq!(found(&report, "device-settings"), StoreFound::Schema(4));
}

#[test]
fn stores_whose_reader_refuses_a_missing_version_are_unreadable() {
    let roots = Roots::new();
    roots.write(&roots.roots.config_file, "[daemon]\n");
    roots.data("scenes.json", r#"{"scenes":{}}"#);
    roots.state("device-binding-migration.json", r#"{"remaps":[]}"#);
    let report = roots.probe();
    for name in ["config", "scenes", "device-binding-journal"] {
        assert!(
            matches!(found(&report, name), StoreFound::Unreadable(_)),
            "{name}: {:?}",
            found(&report, name)
        );
    }
}

#[test]
fn an_empty_scene_document_holds_nothing() {
    let roots = Roots::new();
    roots.data("scenes.json", "{}");
    assert_eq!(found(&roots.probe(), "scenes"), StoreFound::Absent);
}

#[test]
fn record_stores_report_their_newest_record() {
    let roots = Roots::new();
    roots.data(
        "layouts.json",
        r#"[{"id":"a","version":1},{"id":"b","version":3}]"#,
    );
    roots.data(
        "attachment-profiles.json",
        r#"{"one":{"schema_version":2},"two":{}}"#,
    );
    roots.data("attachments/strips/one.toml", "schema_version = 1\n");
    roots.data("attachments/two.toml", "name = \"no version\"\n");
    roots.data("attachments/readme.txt", "not a template");
    let report = roots.probe();
    assert_eq!(found(&report, "layouts"), StoreFound::Schema(3));
    assert_eq!(found(&report, "attachment-profiles"), StoreFound::Schema(2));
    assert_eq!(
        found(&report, "attachment-templates"),
        StoreFound::Schema(1)
    );
}

#[test]
fn a_record_without_a_required_version_makes_its_store_unreadable() {
    let roots = Roots::new();
    roots.data("layouts.json", r#"[{"id":"a","version":1},{"id":"b"}]"#);
    assert!(matches!(
        found(&roots.probe(), "layouts"),
        StoreFound::Unreadable(_)
    ));
}

#[test]
fn unversioned_stores_hold_schema_zero_whatever_their_contents() {
    let roots = Roots::new();
    roots.data("instance_id", "not even a uuid");
    roots.data("credentials.json.enc", "\u{1}\u{2}ciphertext");
    roots.data("effects/user/aurora.html", "<html></html>");
    roots.state("runtime-state.json", "{ corrupt");
    roots.config("cli.toml", "[defaults]\n");
    roots.data("openrgb/OpenRGB.json", "{}");
    let report = roots.probe();
    for name in [
        "instance-id",
        "credentials",
        "user-effects",
        "runtime-state",
        "cli-config",
        "openrgb-config",
    ] {
        assert_eq!(found(&report, name), StoreFound::Schema(0), "{name}");
    }
    assert_eq!(found(&report, "logical-devices"), StoreFound::Absent);
}

#[test]
fn state_stores_fall_back_to_their_previous_data_file() {
    let roots = Roots::new();
    roots.data("device-settings.json", r#"{"schema_version":2}"#);
    roots.data("driver-inventory.json", r#"{"drivers":{}}"#);
    let report = roots.probe();
    assert_eq!(found(&report, "device-settings"), StoreFound::Schema(2));
    assert_eq!(found(&report, "driver-inventory"), StoreFound::Schema(1));

    roots.state("device-settings.json", r#"{"schema_version":3}"#);
    assert_eq!(
        found(&roots.probe(), "device-settings"),
        StoreFound::Schema(3),
        "the state-root file wins once it exists"
    );
}

#[test]
fn corrupt_versioned_stores_are_unreadable_not_absent() {
    let roots = Roots::new();
    roots.data("library.json", "{ not json");
    roots.state("device-settings.json", r#"{"schema_version":"three"}"#);
    let report = roots.probe();
    assert!(matches!(
        found(&report, "library"),
        StoreFound::Unreadable(_)
    ));
    assert!(matches!(
        found(&report, "device-settings"),
        StoreFound::Unreadable(_)
    ));
}

#[test]
fn probing_never_writes() {
    let roots = Roots::new();
    roots.data("library.json", r#"{"favorites":[]}"#);
    roots.write(&roots.roots.config_file, "schema_version = 4\n");
    let before: Vec<_> = [
        roots.roots.data.join("library.json"),
        roots.roots.config_file.clone(),
    ]
    .iter()
    .map(|path| fs::read(path).expect("read before"))
    .collect();
    let _ = roots.probe();
    let after: Vec<_> = [
        roots.roots.data.join("library.json"),
        roots.roots.config_file.clone(),
    ]
    .iter()
    .map(|path| fs::read(path).expect("read after"))
    .collect();
    assert_eq!(before, after);
    assert!(
        !roots.roots.state.join("device-settings.json").exists(),
        "a probe creates nothing"
    );
}

fn declared(name: &str) -> &'static hypercolor_daemon::durable_stores::DurableStore {
    DURABLE_STORES
        .iter()
        .find(|store| store.name == name)
        .unwrap_or_else(|| panic!("{name} is inventoried"))
}

/// Every version from one below the declared range to one above it.
fn around(name: &str) -> std::ops::RangeInclusive<u32> {
    let store = declared(name);
    store.readable_schema_min.saturating_sub(1)..=store.readable_schema_max + 1
}

fn assert_reader_matches_declaration(name: &str, accepts: impl Fn(u32) -> bool) {
    let store = declared(name);
    for version in around(name) {
        let declared_readable =
            store.readable_schema_min <= version && version <= store.readable_schema_max;
        assert_eq!(
            accepts(version),
            declared_readable,
            "{name}: the real reader and the declaration disagree about schema {version}"
        );
    }
}

#[test]
fn the_real_readers_accept_exactly_the_declared_ranges() {
    assert_reader_matches_declaration("config", |version| {
        hypercolor_core::config::ConfigManager::parse_toml(&format!("schema_version = {version}\n"))
            .is_ok()
    });

    let directory = tempfile::tempdir().expect("store directory");
    let write = |name: &str, document: Value| {
        let path = directory.path().join(name);
        fs::write(&path, serde_json::to_vec(&document).expect("encode")).expect("write store");
        path
    };
    assert_reader_matches_declaration("scenes", |version| {
        hypercolor_daemon::scene_store::load(&write(
            "scenes.json",
            json!({"schema_version": version, "scenes": {}}),
        ))
        .is_ok()
    });
    assert_reader_matches_declaration("device-aliases", |version| {
        hypercolor_daemon::device_aliases::load(&write(
            "device-aliases.json",
            json!({"schema_version": version, "aliases": {}, "quarantined_keys": [], "collisions": []}),
        ))
        .is_ok()
    });
    assert_reader_matches_declaration("library", |version| {
        let path = write("library.json", json!({"version": version}));
        hypercolor_daemon::library::JsonLibraryStore::open(path).is_ok()
    });
}

#[test]
fn an_empty_document_reads_the_way_each_reader_takes_it() {
    let roots = Roots::new();
    roots.data("library.json", "{}");
    roots.state("device-binding-migration.json", "{}");
    roots.state("device-settings.json", "{}");
    let report = roots.probe();
    assert_eq!(
        found(&report, "library"),
        StoreFound::Schema(1),
        "the library reads a bare object as its legacy schema"
    );
    assert!(
        matches!(
            found(&report, "device-binding-journal"),
            StoreFound::Unreadable(_)
        ),
        "the binding journal refuses a document without its version"
    );
    assert_eq!(found(&report, "device-settings"), StoreFound::Schema(1));
}

#[test]
fn a_moved_store_reports_the_newer_of_its_two_files() {
    let roots = Roots::new();
    roots.state("device-settings.json", r#"{"schema_version":2}"#);
    roots.data("device-settings.json", r#"{"schema_version":3}"#);
    assert_eq!(
        found(&roots.probe(), "device-settings"),
        StoreFound::Schema(3),
        "the reader takes a newer previous file, so the probe reports it"
    );
}

#[cfg(unix)]
#[test]
fn a_special_file_is_unreadable_without_being_opened() {
    let roots = Roots::new();
    for (root, name) in [
        (&roots.roots.data, "library.json"),
        (&roots.roots.config, "cli.toml"),
    ] {
        let path = root.join(name);
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("mkfifo");
        assert!(status.success());
    }
    // Opening a FIFO for reading blocks until a writer appears; the probe
    // must not open one at all.
    let (sender, receiver) = std::sync::mpsc::channel();
    let probe_roots = roots.roots.clone();
    std::thread::spawn(move || {
        let _ = sender.send(probe_durable_stores(&probe_roots));
    });
    let report = receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the probe never blocks on a special file");
    for name in ["library", "cli-config"] {
        assert!(
            matches!(found(&report, name), StoreFound::Unreadable(_)),
            "{name}: {:?}",
            found(&report, name)
        );
    }
}

#[cfg(unix)]
#[test]
fn directory_stores_are_read_the_way_their_readers_walk_them() {
    let roots = Roots::new();
    let elsewhere = roots.roots.data.join("real-attachments");
    fs::create_dir_all(elsewhere.join("nested")).expect("templates");
    fs::write(elsewhere.join("nested/strip.TOML"), "schema_version = 2\n").expect("template");
    for index in 0..10 {
        fs::write(
            elsewhere.join(format!("note-{index}.txt")),
            "not a template",
        )
        .expect("note");
    }
    std::os::unix::fs::symlink(&elsewhere, roots.roots.data.join("attachments"))
        .expect("linked store root");
    assert_eq!(
        found(&roots.probe(), "attachment-templates"),
        StoreFound::Schema(2),
        "a linked root and an upper-case extension still load, as in the reader"
    );
}

#[test]
fn a_probe_before_initialization_reports_what_migration_then_rewrites() {
    let roots = Roots::new();
    roots.data("library.json", r#"{"favorites":[]}"#);
    let before = roots.probe();
    hypercolor_daemon::library::JsonLibraryStore::open(roots.roots.data.join("library.json"))
        .expect("the library opens and migrates in place");
    let rewritten: Value =
        serde_json::from_slice(&fs::read(roots.roots.data.join("library.json")).expect("library"))
            .expect("library JSON");
    assert_eq!(rewritten["version"], 2, "opening rewrote the store");
    assert_eq!(
        found(&before, "library"),
        StoreFound::Schema(1),
        "the report keeps what was on disk before the store opened"
    );
    assert_eq!(found(&roots.probe(), "library"), StoreFound::Schema(2));
}
