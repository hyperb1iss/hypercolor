//! The durable store inventory and the declaration a Linux release ships.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use hypercolor_daemon::durable_stores::DURABLE_STORES;
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
        "the packaged file holds exactly the store declarations"
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
