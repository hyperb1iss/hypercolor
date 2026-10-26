use std::fs;
use std::path::{Path, PathBuf};

use hypercolor_platform_fs::{WindowsDirectory, WindowsDirectoryAccess, WindowsSid};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    ReleasePayloadError, retain_installed_release_unit, stage_release_payload,
    validate_release_payload,
};
use crate::model::UnitId;
use crate::windows::WindowsOwnershipPolicy;
use crate::windows::manifest::ValidatedManifest;
use crate::windows::store::InstallStore;

const FILES: [(&str, &str, &[u8]); 7] = [
    ("hypercolor-app.exe", "program", b"app"),
    ("hypercolor-daemon.exe", "program", b"daemon"),
    ("hypercolor.exe", "program", b"cli"),
    ("vcruntime140.dll", "program", b"runtime"),
    ("ui/index.html", "data", b"<html></html>"),
    ("effects/bundled/rainbow.html", "data", b"rainbow"),
    ("tools/pawnio/module.bin", "data", b"module"),
];
/// A change made to an extracted source before it is staged.
type SourceMutation = (&'static str, fn(&Path));
/// A change made to a manifest that must make it invalid.
type ManifestEdit = (&'static str, Box<dyn Fn(&mut Value)>);

const DIRECTORIES: [&str; 5] = ["effects", "effects/bundled", "tools", "tools/pawnio", "ui"];

struct Fixture {
    _guard: tempfile::TempDir,
    base: PathBuf,
    store: InstallStore,
}

impl Fixture {
    fn new() -> Self {
        let guard = tempfile::tempdir().expect("temporary roots");
        let base = fs::canonicalize(guard.path()).expect("canonical roots");
        let base = PathBuf::from(
            base.to_str()
                .expect("UTF-8 temporary path")
                .trim_start_matches(r"\\?\"),
        );
        let release = base.join("Program Files").join("Hypercolor");
        let state = base.join("ProgramData").join("Hypercolor").join("update");
        fs::create_dir_all(&release).expect("release root");
        fs::create_dir_all(&state).expect("state root");
        let store = InstallStore::with_roots(&release, &state, 64 * 1024)
            .expect("store")
            .with_ownership_policy(WindowsOwnershipPolicy::trusting([
                WindowsSid::current_user().expect("current user"),
            ]));
        Self {
            _guard: guard,
            base,
            store,
        }
    }

    /// Write one extracted unit and return its directory and digest.
    fn source(&self, name: &str, manifest: &Value) -> (PathBuf, UnitId) {
        let root = self.base.join(name);
        for directory in DIRECTORIES {
            fs::create_dir_all(root.join(directory)).expect("source directory");
        }
        for (path, _, bytes) in FILES {
            fs::write(root.join(path), bytes).expect("source file");
        }
        let bytes = serde_json::to_vec(manifest).expect("manifest");
        fs::write(root.join("manifest.json"), &bytes).expect("write manifest");
        let unit = UnitId::new(hex::encode(Sha256::digest(&bytes))).expect("digest");
        (root, unit)
    }
}

fn manifest() -> Value {
    let mut members = DIRECTORIES
        .iter()
        .map(|path| json!({"path": path, "type": "directory"}))
        .collect::<Vec<_>>();
    for (path, class, bytes) in FILES {
        members.push(json!({
            "path": path,
            "type": "file",
            "class": class,
            "size": bytes.len(),
            "sha256": hex::encode(Sha256::digest(bytes)),
        }));
    }
    json!({
        "name": "hypercolor",
        "version": "1.5.0",
        "platform": "windows-x86_64",
        "rust_target": "x86_64-pc-windows-msvc",
        "programs": ["hypercolor-app.exe", "hypercolor-daemon.exe", "hypercolor.exe"],
        "members": members,
    })
}

fn open(path: &Path) -> WindowsDirectory {
    WindowsDirectory::open(path, WindowsDirectoryAccess::Read).expect("open source")
}

#[test]
fn a_unit_is_copied_under_its_digest_and_bound_again_by_identity() {
    let fixture = Fixture::new();
    let (source, unit) = fixture.source("extracted", &manifest());
    validate_release_payload(&open(&source), &unit).expect("preflight");
    let lock = fixture.store.acquire_lock().expect("lock");

    let staged =
        stage_release_payload(&fixture.store, &lock, &open(&source), &unit).expect("stage");
    assert_eq!(staged.id(), &unit);
    let installed = fixture.store.unit_path(&unit);
    for (path, _, bytes) in FILES {
        assert_eq!(
            fs::read(installed.join(path)).expect("installed file"),
            bytes
        );
    }

    let again = stage_release_payload(&fixture.store, &lock, &open(&source), &unit)
        .expect("restaging binds the existing unit");
    assert_eq!(again, staged);
    let retained = retain_installed_release_unit(&fixture.store, &lock, &unit).expect("retain");
    assert_eq!(retained, staged);
    let collection = fixture
        .store
        .collect_units(&lock, &[unit])
        .expect("collect");
    assert!(
        collection.removed_leftovers.is_empty(),
        "no staging is left behind"
    );
}

#[test]
fn a_source_that_differs_from_its_manifest_is_refused_and_leaves_nothing() {
    let fixture = Fixture::new();
    let lock = fixture.store.acquire_lock().expect("lock");
    let mutations: [SourceMutation; 4] = [
        ("tampered", |root| {
            fs::write(root.join("hypercolor.exe"), b"evil").expect("tamper");
        }),
        ("extra", |root| {
            fs::write(root.join("ui").join("extra.js"), b"extra").expect("extra");
        }),
        ("missing", |root| {
            fs::remove_file(root.join("vcruntime140.dll")).expect("remove");
        }),
        ("longer", |root| {
            fs::write(root.join("ui").join("index.html"), b"<html></html>!").expect("grow");
        }),
    ];
    for (name, mutate) in mutations {
        let (source, unit) = fixture.source(name, &manifest());
        mutate(&source);
        assert!(
            matches!(
                stage_release_payload(&fixture.store, &lock, &open(&source), &unit),
                Err(ReleasePayloadError::InvalidSource(_))
            ),
            "{name} must be refused"
        );
        assert!(validate_release_payload(&open(&source), &unit).is_err());
        assert!(!fixture.store.unit_path(&unit).exists());
    }
    let collection = fixture.store.collect_units(&lock, &[]).expect("collect");
    assert!(
        collection.removed_leftovers.is_empty(),
        "a refused stage cleans up after itself: {collection:?}"
    );

    let (source, _) = fixture.source("other-digest", &manifest());
    let other = UnitId::new("f".repeat(64)).expect("unit");
    assert!(matches!(
        stage_release_payload(&fixture.store, &lock, &open(&source), &other),
        Err(ReleasePayloadError::UnexpectedManifestDigest { .. })
    ));
}

#[test]
fn an_installed_unit_that_changed_is_never_bound() {
    let fixture = Fixture::new();
    let (source, unit) = fixture.source("extracted", &manifest());
    let lock = fixture.store.acquire_lock().expect("lock");
    stage_release_payload(&fixture.store, &lock, &open(&source), &unit).expect("stage");

    let installed = fixture.store.unit_path(&unit);
    fs::write(installed.join("hypercolor-daemon.exe"), b"swapped").expect("tamper");
    assert!(matches!(
        retain_installed_release_unit(&fixture.store, &lock, &unit),
        Err(ReleasePayloadError::InvalidUnit(_))
    ));
    assert!(matches!(
        stage_release_payload(&fixture.store, &lock, &open(&source), &unit),
        Err(ReleasePayloadError::InvalidUnit(_))
    ));

    fs::write(installed.join("hypercolor-daemon.exe"), b"daemon").expect("restore");
    fs::write(installed.join("ui").join("injected.js"), b"x").expect("inject");
    assert!(matches!(
        retain_installed_release_unit(&fixture.store, &lock, &unit),
        Err(ReleasePayloadError::InvalidUnit(_))
    ));
}

#[test]
fn a_reparse_point_in_the_source_is_refused() {
    let fixture = Fixture::new();
    let (source, unit) = fixture.source("extracted", &manifest());
    let outside = fixture.base.join("outside");
    fs::create_dir(&outside).expect("outside");
    fs::write(outside.join("index.html"), b"<html></html>").expect("outside file");
    fs::remove_dir_all(source.join("ui")).expect("drop ui");
    if let Err(error) = std::os::windows::fs::symlink_dir(&outside, source.join("ui")) {
        eprintln!("skipping reparse refusal: {error}");
        return;
    }
    let lock = fixture.store.acquire_lock().expect("lock");
    assert!(matches!(
        stage_release_payload(&fixture.store, &lock, &open(&source), &unit),
        Err(ReleasePayloadError::InvalidSource(_))
    ));
}

#[test]
fn manifests_that_a_windows_unit_cannot_honour_are_refused() {
    let valid = manifest();
    ValidatedManifest::parse(serde_json::to_vec(&valid).expect("bytes")).expect("valid");

    let edits: Vec<ManifestEdit> = vec![
        (
            "linux platform",
            Box::new(|value| value["platform"] = json!("linux-x86_64")),
        ),
        (
            "gnu target",
            Box::new(|value| value["rust_target"] = json!("x86_64-pc-windows-gnu")),
        ),
        (
            "missing program",
            Box::new(|value| value["programs"] = json!(["hypercolor-app.exe"])),
        ),
        (
            "unix mode",
            Box::new(|value| value["members"][0]["mode"] = json!(493)),
        ),
        (
            "classless file",
            Box::new(|value| {
                let members = value["members"].as_array_mut().expect("members");
                let file = members
                    .iter_mut()
                    .find(|member| member["type"] == "file")
                    .expect("file");
                file.as_object_mut().expect("object").remove("class");
            }),
        ),
        (
            "classed directory",
            Box::new(|value| value["members"][0]["class"] = json!("data")),
        ),
        (
            "data program",
            Box::new(|value| {
                let members = value["members"].as_array_mut().expect("members");
                let app = members
                    .iter_mut()
                    .find(|member| member["path"] == "hypercolor-app.exe")
                    .expect("app");
                app["class"] = json!("data");
            }),
        ),
        (
            "case collision",
            Box::new(|value| {
                value["members"]
                    .as_array_mut()
                    .expect("members")
                    .push(json!({"path": "UI", "type": "directory"}));
            }),
        ),
        (
            "missing parent",
            Box::new(|value| {
                value["members"].as_array_mut().expect("members").push(
                    json!({"path": "nowhere/file", "type": "file", "class": "data", "size": 0,
                    "sha256": hex::encode(Sha256::digest(b""))}),
                );
            }),
        ),
        (
            "no ui",
            Box::new(|value| {
                let members = value["members"].as_array_mut().expect("members");
                members
                    .retain(|member| member["path"] != "ui" && member["path"] != "ui/index.html");
            }),
        ),
    ];
    for name in [
        "CON",
        "a:b",
        "trailing.",
        "trailing ",
        "back\\slash",
        "manifest.json",
        "",
        "a//b",
    ] {
        let mut value = manifest();
        value["members"]
            .as_array_mut()
            .expect("members")
            .push(json!({"path": name, "type": "directory"}));
        assert!(
            ValidatedManifest::parse(serde_json::to_vec(&value).expect("bytes")).is_err(),
            "{name:?} must be refused"
        );
    }
    for (name, edit) in edits {
        let mut value = manifest();
        edit(&mut value);
        assert!(
            ValidatedManifest::parse(serde_json::to_vec(&value).expect("bytes")).is_err(),
            "{name} must be refused"
        );
    }
}
