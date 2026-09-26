//! The managed package contract in Linux release manifests: strict for new
//! candidates, tolerant for installed releases, and the durable-data
//! compatibility decision built on it.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use hypercolor_cli::install::{
    CompatibilityDecision, CompatibilityRefusal, DeclaredCompatibility, InstallLock, InstallStore,
    MigrationMode, ReleasePayloadError, UnitId, UnitRecord, copy_installed_release_unit,
    declared_compatibility_from_manifest, evaluate_data_compatibility, read_declared_compatibility,
    retain_linux_unit, stage_release_payload,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

/// A named manifest edit and the refusal it must produce.
type ManifestCase = (&'static str, fn(&mut Value), &'static str);
/// A named components edit and the refusal it must produce.
type ComponentsCase = (
    &'static str,
    fn(&mut serde_json::Map<String, Value>),
    &'static str,
);

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn store_declaration(name: &str, min: u32, max: u32, written: u32, mode: &str) -> Value {
    json!({
        "name": name, "storage_format": "json",
        "readable_schema_min": min, "readable_schema_max": max,
        "written_schema": written, "migration_mode": mode,
    })
}

fn managed_package(stores: Vec<Value>) -> Value {
    json!({
        "schema_version": 1,
        "owner": "linux-user-tarball",
        "launcher_contract": 1,
        "components": {
            "daemon": "bin/hypercolor-daemon",
            "cli": "bin/hypercolor",
            "ui": "share/hypercolor/ui",
            "bundled_effects": "share/hypercolor/effects/bundled",
        },
        "compatibility": {"stores": stores},
    })
}

/// A complete Linux release tree on disk, before any manifest rewrite.
struct Release {
    root: tempfile::TempDir,
    members: Vec<Value>,
}

impl Release {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("release root");
        let directories = [
            "bin",
            "share",
            "share/hypercolor",
            "share/hypercolor/ui",
            "share/hypercolor/effects",
            "share/hypercolor/effects/bundled",
            "share/hypercolor/docs",
            "share/hypercolor/agents",
            "share/hypercolor/agents/skills",
            "share/hypercolor/agents/agents",
            "share/hypercolor/skills",
            "share/hypercolor/site",
        ];
        let files: [(&str, &[u8]); 10] = [
            ("bin/hypercolor-daemon", b"daemon"),
            ("bin/hypercolor", b"candidate"),
            ("bin/hypercolor-app", b"app"),
            ("bin/hypercolor-tui", b"tui"),
            ("bin/hypercolor-open", b"open"),
            ("share/hypercolor/ui/index.html", b"ui"),
            ("share/hypercolor/effects/bundled/effect.html", b"effect"),
            ("share/hypercolor/agents/skills/skill.md", b"skill"),
            ("share/hypercolor/agents/agents/agent.md", b"agent"),
            ("share/hypercolor/skills/skill.md", b"user skill"),
        ];
        let mut members = Vec::new();
        for directory in directories {
            let path = root.path().join(directory);
            fs::create_dir_all(&path).expect("release directory");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("mode");
            members.push(json!({"path": directory, "type": "directory", "mode": 0o755}));
        }
        for (path, bytes) in files {
            let mode = if path.starts_with("bin/") {
                0o755
            } else {
                0o644
            };
            fs::write(root.path().join(path), bytes).expect("release file");
            fs::set_permissions(root.path().join(path), fs::Permissions::from_mode(mode))
                .expect("mode");
            members.push(json!({
                "path": path, "type": "file", "mode": mode,
                "size": bytes.len(), "sha256": sha256(bytes),
            }));
        }
        members.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
        Self { root, members }
    }

    /// The manifest a current Linux producer writes for this tree.
    fn manifest(&self) -> Value {
        json!({
            "name": "hypercolor", "version": "0.6.0", "platform": "linux-amd64",
            "rust_target": "x86_64-unknown-linux-gnu",
            "binaries": ["hypercolor-daemon", "hypercolor", "hypercolor-app",
                "hypercolor-tui", "hypercolor-open"],
            "assets": {"ui_files": 1, "bundled_effect_files": 1, "docs_files": 0,
                "skill_files": 1, "user_skill_files": 1, "agent_files": 1, "site_files": 0},
            "members": self.members,
            "managed_package": managed_package(vec![
                store_declaration("library", 1, 2, 2, "backward_compatible"),
                store_declaration("scenes", 2, 2, 2, "backward_compatible"),
            ]),
        })
    }

    fn write(&self, manifest: &Value) -> (UnitId, File) {
        let bytes = serde_json::to_vec_pretty(manifest).expect("encode manifest");
        let path = self.root.path().join("manifest.json");
        fs::write(&path, &bytes).expect("write manifest");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("mode");
        (
            UnitId::new(sha256(&bytes)).expect("manifest digest"),
            File::open(self.root.path().join("bin/hypercolor")).expect("candidate"),
        )
    }

    fn stage(
        &self,
        store: &InstallStore,
        lock: &InstallLock,
        manifest: &Value,
    ) -> Result<UnitRecord, ReleasePayloadError> {
        let (unit, candidate) = self.write(manifest);
        stage_release_payload(store, lock, self.root.path(), &candidate, &unit)
    }
}

fn new_store() -> (tempfile::TempDir, InstallStore) {
    let parent = tempfile::tempdir().expect("install parent");
    let store = InstallStore::new(parent.path().join("store"), 64 * 1024);
    (parent, store)
}

/// Stage a valid release, then rewrite its installed manifest the way an
/// older or newer installer would have left it, as the unit of that digest.
fn installed_with_manifest(
    store: &InstallStore,
    lock: &InstallLock,
    edit: impl FnOnce(&mut Value),
) -> UnitId {
    let release = Release::new();
    let staged = release
        .stage(store, lock, &release.manifest())
        .expect("stage the current release");
    let root = store.unit_path(staged.id());
    drop(staged);
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).expect("installed manifest"))
            .expect("decode installed manifest");
    edit(&mut manifest);
    let bytes = serde_json::to_vec_pretty(&manifest).expect("encode");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).expect("thaw unit");
    let path = root.join("manifest.json");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("thaw manifest");
    fs::write(&path, &bytes).expect("rewrite manifest");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).expect("freeze manifest");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).expect("freeze unit");
    let unit = UnitId::new(sha256(&bytes)).expect("installed digest");
    fs::rename(&root, store.unit_path(&unit)).expect("rename to its digest");
    unit
}

fn refusal(result: Result<UnitRecord, ReleasePayloadError>) -> String {
    match result {
        Ok(unit) => panic!("staged {} although it must be refused", unit.id().as_str()),
        Err(error) => error.to_string(),
    }
}

#[test]
fn a_linux_candidate_must_declare_its_managed_package() {
    let release = Release::new();
    let (_parent, store) = new_store();
    let lock = store.acquire_lock().expect("lock");
    let mut manifest = release.manifest();
    manifest
        .as_object_mut()
        .expect("object")
        .remove("managed_package");
    let error = refusal(release.stage(&store, &lock, &manifest));
    assert!(
        error.contains("a Linux release must declare its managed_package contract"),
        "{error}"
    );
    assert!(
        !store.root().join("units").exists()
            || fs::read_dir(store.root().join("units"))
                .expect("units")
                .next()
                .is_none(),
        "nothing was published"
    );
    let staged = release
        .stage(&store, &lock, &release.manifest())
        .expect("a complete declaration stages");
    assert_eq!(
        read_declared_compatibility(&staged)
            .expect("read the staged declaration")
            .declared()
            .expect("declared")
            .stores()
            .len(),
        2
    );
}

#[test]
fn missing_wrong_and_extra_components_are_refused() {
    let release = Release::new();
    let (_parent, store) = new_store();
    let lock = store.acquire_lock().expect("lock");
    let cases: [ComponentsCase; 4] = [
        (
            "missing",
            |components| {
                components.remove("bundled_effects");
            },
            "must name exactly",
        ),
        (
            "extra",
            |components| {
                components.insert("app".into(), json!("bin/hypercolor-app"));
            },
            "must name exactly",
        ),
        (
            "wrong executable",
            |components| {
                components.insert("daemon".into(), json!("bin/hypercolor-app"));
            },
            "must be bin/hypercolor-daemon",
        ),
        (
            "wrong tree",
            |components| {
                components.insert("ui".into(), json!("share/hypercolor/site"));
            },
            "must be share/hypercolor/ui",
        ),
    ];
    for (label, edit, message) in cases {
        let mut manifest = release.manifest();
        edit(
            manifest["managed_package"]["components"]
                .as_object_mut()
                .expect("components"),
        );
        let error = refusal(release.stage(&store, &lock, &manifest));
        assert!(error.contains(message), "{label}: {error}");
    }
}

#[test]
fn unknown_contracts_fields_and_invalid_stores_are_refused_as_candidates() {
    let release = Release::new();
    let (_parent, store) = new_store();
    let lock = store.acquire_lock().expect("lock");
    let cases: [ManifestCase; 11] = [
        (
            "future schema",
            |m| m["managed_package"]["schema_version"] = json!(2),
            "schema_version 2 is not the supported 1",
        ),
        (
            "future launcher",
            |m| m["managed_package"]["launcher_contract"] = json!(2),
            "launcher_contract 2",
        ),
        (
            "foreign owner",
            |m| m["managed_package"]["owner"] = json!("distribution-package"),
            "is not \"linux-user-tarball\"",
        ),
        (
            "unknown package field",
            |m| m["managed_package"]["channel"] = json!("beta"),
            "unknown field \"channel\"",
        ),
        (
            "unknown store field",
            |m| m["managed_package"]["compatibility"]["stores"][0]["note"] = json!("x"),
            "unknown field \"note\"",
        ),
        (
            "unknown top-level field",
            |m| m["channel"] = json!("beta"),
            "unknown field",
        ),
        (
            "no stores",
            |m| m["managed_package"]["compatibility"]["stores"] = json!([]),
            "must declare 1..=",
        ),
        (
            "duplicate store",
            |m| {
                m["managed_package"]["compatibility"]["stores"] = json!([
                    store_declaration("library", 1, 2, 2, "backward_compatible"),
                    store_declaration("library", 1, 2, 2, "backward_compatible"),
                ]);
            },
            "declared twice",
        ),
        (
            "writes what it cannot read",
            |m| {
                m["managed_package"]["compatibility"]["stores"] =
                    json!([store_declaration("library", 1, 2, 3, "backward_compatible")]);
            },
            "must read the schema it writes",
        ),
        (
            "unknown migration mode",
            |m| {
                m["managed_package"]["compatibility"]["stores"] =
                    json!([store_declaration("library", 1, 2, 2, "eventually")]);
            },
            "malformed",
        ),
        (
            "store name",
            |m| {
                m["managed_package"]["compatibility"]["stores"] =
                    json!([store_declaration("Library", 1, 2, 2, "manual")]);
            },
            "durable store name",
        ),
    ];
    for (label, edit, message) in cases {
        let mut manifest = release.manifest();
        edit(&mut manifest);
        let error = refusal(release.stage(&store, &lock, &manifest));
        assert!(error.contains(message), "{label}: {error}");
    }
}

#[test]
fn an_installed_release_from_before_the_contract_is_undeclared_but_retained_and_adoptable() {
    let (_parent, store) = new_store();
    let lock = store.acquire_lock().expect("lock");
    let legacy = installed_with_manifest(&store, &lock, |manifest| {
        manifest
            .as_object_mut()
            .expect("object")
            .remove("managed_package");
    });
    let retained = retain_linux_unit(&store, &lock, &legacy).expect("retain a 0.5 release");
    assert_eq!(
        read_declared_compatibility(&retained).expect("read"),
        DeclaredCompatibility::Undeclared
    );

    let (_other_parent, destination) = new_store();
    let destination_lock = destination.acquire_lock().expect("destination lock");
    let copied = copy_installed_release_unit(&destination, &destination_lock, &retained)
        .expect("adoption copies a release from before the contract");
    assert_eq!(copied.id(), &legacy);
}

#[test]
fn an_installed_release_from_a_newer_contract_is_unrecognized_but_retained() {
    let (_parent, store) = new_store();
    let lock = store.acquire_lock().expect("lock");
    let newer = installed_with_manifest(&store, &lock, |manifest| {
        manifest["managed_package"]["schema_version"] = json!(2);
        manifest["managed_package"]["signing"] = json!({"key": "future"});
        manifest["channel"] = json!("beta");
    });
    let retained = retain_linux_unit(&store, &lock, &newer)
        .expect("an older installer still retains and can roll back a newer release");
    match read_declared_compatibility(&retained).expect("read") {
        DeclaredCompatibility::Unrecognized { reason } => {
            assert!(reason.contains("schema_version 2"), "{reason}");
        }
        other => panic!("expected an unrecognized declaration, got {other:?}"),
    }
}

#[test]
fn an_installed_release_whose_tree_changed_is_still_refused() {
    let (_parent, store) = new_store();
    let lock = store.acquire_lock().expect("lock");
    let newer = installed_with_manifest(&store, &lock, |manifest| {
        manifest["future_field"] = json!(true);
        for member in manifest["members"].as_array_mut().expect("members") {
            if member["path"] == "bin/hypercolor-daemon" {
                member["sha256"] = json!(sha256(b"another daemon"));
            }
        }
    });
    assert!(
        retain_linux_unit(&store, &lock, &newer).is_err(),
        "tolerating unknown fields never tolerates changed bytes"
    );
}

fn declared(stores: Vec<Value>) -> DeclaredCompatibility {
    let release = Release::new();
    let mut manifest = release.manifest();
    manifest["managed_package"] = managed_package(stores);
    declared_compatibility_from_manifest(serde_json::to_vec(&manifest).expect("encode"))
        .expect("parse")
}

fn backward(name: &str, min: u32, max: u32, written: u32) -> Value {
    store_declaration(name, min, max, written, "backward_compatible")
}

#[test]
fn the_manifest_reader_matches_what_the_producer_declared() {
    let compatibility = declared(vec![
        backward("config", 4, 5, 5),
        store_declaration("library", 1, 2, 2, "staged"),
    ]);
    let package = compatibility.declared().expect("declared");
    assert_eq!(package.launcher_contract(), 1);
    let config = package.store("config").expect("config");
    assert_eq!(config.storage_format(), "json");
    assert_eq!(
        (
            config.readable_schema_min(),
            config.readable_schema_max(),
            config.written_schema()
        ),
        (4, 5, 5)
    );
    assert_eq!(
        package.store("library").expect("library").migration_mode(),
        MigrationMode::Staged
    );
    assert!(package.store("scenes").is_none());
}

#[test]
fn a_compatible_pair_activates_automatically() {
    let running = declared(vec![
        backward("library", 1, 2, 2),
        backward("scenes", 2, 2, 2),
    ]);
    let target = declared(vec![
        backward("library", 1, 2, 2),
        backward("scenes", 2, 2, 2),
    ]);
    let high_water = BTreeMap::from([("library".to_owned(), 2)]);
    assert_eq!(
        evaluate_data_compatibility(&running, &target, &high_water),
        CompatibilityDecision::Automatic
    );
}

#[test]
fn a_schema_unsafe_predecessor_is_refused() {
    // N reads library 1..=2. Candidate N+1 writes 3, so the in-transaction
    // rollback to N would meet data N cannot read.
    let running = declared(vec![backward("library", 1, 2, 2)]);
    let target = declared(vec![backward("library", 2, 3, 3)]);
    assert_eq!(
        evaluate_data_compatibility(&running, &target, &BTreeMap::new()),
        CompatibilityDecision::Manual(vec![CompatibilityRefusal::RollbackUnreadable {
            store: "library".to_owned(),
            written_schema: 3,
            readable_schema_min: 1,
            readable_schema_max: 2,
        }])
    );
    // The same N+1 is safe to reach from a release that already reads 3.
    let newer_running = declared(vec![backward("library", 1, 3, 2)]);
    assert_eq!(
        evaluate_data_compatibility(&newer_running, &target, &BTreeMap::new()),
        CompatibilityDecision::Automatic
    );
}

#[test]
fn the_high_water_mark_refuses_a_target_that_cannot_read_what_ran_before() {
    // RFC 57 section 10: N-1 updates to N, which writes 5 and rolls back.
    // A later restore to N-2, which reads at most 4, is refused even though
    // N-1 only ever wrote 4.
    let n_minus_1 = declared(vec![backward("library", 3, 5, 4)]);
    let n_minus_2 = declared(vec![backward("library", 3, 4, 4)]);
    let high_water = BTreeMap::from([("library".to_owned(), 5)]);
    assert_eq!(
        evaluate_data_compatibility(&n_minus_1, &n_minus_2, &high_water),
        CompatibilityDecision::Manual(vec![CompatibilityRefusal::HighWaterUnreadable {
            store: "library".to_owned(),
            high_water: 5,
            readable_schema_min: 3,
            readable_schema_max: 4,
        }])
    );
    assert_eq!(
        evaluate_data_compatibility(
            &n_minus_1,
            &n_minus_2,
            &BTreeMap::from([("library".to_owned(), 4)])
        ),
        CompatibilityDecision::Automatic
    );
}

#[test]
fn staged_and_manual_migrations_never_activate_automatically() {
    let running = declared(vec![backward("library", 1, 3, 2)]);
    for (mode, expected) in [
        ("staged", MigrationMode::Staged),
        ("manual", MigrationMode::Manual),
    ] {
        let target = declared(vec![store_declaration("library", 2, 3, 3, mode)]);
        assert_eq!(
            evaluate_data_compatibility(&running, &target, &BTreeMap::new()),
            CompatibilityDecision::Manual(vec![CompatibilityRefusal::NotBackwardCompatible {
                store: "library".to_owned(),
                migration_mode: expected,
            }])
        );
    }
}

#[test]
fn missing_metadata_on_either_side_is_manual() {
    let current = declared(vec![backward("library", 1, 2, 2)]);
    let undeclared = DeclaredCompatibility::Undeclared;
    let unrecognized = DeclaredCompatibility::Unrecognized {
        reason: "schema_version 2".to_owned(),
    };
    assert_eq!(
        evaluate_data_compatibility(&undeclared, &current, &BTreeMap::new()),
        CompatibilityDecision::Manual(vec![CompatibilityRefusal::RunningUndeclared])
    );
    assert_eq!(
        evaluate_data_compatibility(&current, &unrecognized, &BTreeMap::new()),
        CompatibilityDecision::Manual(vec![CompatibilityRefusal::TargetUndeclared])
    );
    assert_eq!(
        evaluate_data_compatibility(&undeclared, &unrecognized, &BTreeMap::new()),
        CompatibilityDecision::Manual(vec![
            CompatibilityRefusal::TargetUndeclared,
            CompatibilityRefusal::RunningUndeclared,
        ])
    );
}

#[test]
fn stores_only_one_side_declares_do_not_block() {
    // The target no longer reads `legacy-profiles`, and the running
    // release never wrote `layouts`; neither constrains the other.
    let running = declared(vec![
        backward("library", 1, 2, 2),
        backward("legacy-profiles", 0, 0, 0),
    ]);
    let target = declared(vec![
        backward("library", 1, 2, 2),
        backward("layouts", 1, 1, 1),
    ]);
    let high_water = BTreeMap::from([("legacy-profiles".to_owned(), 0)]);
    assert_eq!(
        evaluate_data_compatibility(&running, &target, &high_water),
        CompatibilityDecision::Automatic
    );
}

#[test]
fn the_repository_inventory_is_a_valid_declaration() {
    let inventory: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../packaging/managed/durable-stores.json"),
        )
        .expect("read the packaged inventory"),
    )
    .expect("decode the packaged inventory");
    let release = Release::new();
    let mut manifest = release.manifest();
    manifest["managed_package"]["compatibility"] = inventory.clone();
    let (_parent, store) = new_store();
    let lock = store.acquire_lock().expect("lock");
    let staged = release
        .stage(&store, &lock, &manifest)
        .expect("the packaged inventory passes the candidate validator");
    let compatibility = read_declared_compatibility(&staged).expect("read");
    let names: Vec<&str> = compatibility
        .declared()
        .expect("declared")
        .stores()
        .iter()
        .map(hypercolor_cli::install::DurableStoreDeclaration::name)
        .collect();
    let expected: Vec<&str> = inventory["stores"]
        .as_array()
        .expect("stores")
        .iter()
        .map(|store| store["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, expected);
    assert_eq!(
        evaluate_data_compatibility(&compatibility, &compatibility, &BTreeMap::new()),
        CompatibilityDecision::Automatic,
        "a release is always compatible with itself"
    );
}
