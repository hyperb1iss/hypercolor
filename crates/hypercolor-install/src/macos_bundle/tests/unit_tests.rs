//! What an app bundle unit is: its manifest, its store inventory and its
//! bundle, and nothing else.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;

use serde_json::Value;

use super::fixture::{UnitTree, thin_macho};
use crate::{InstallStore, ReleasePayloadError, stage_macos_bundle_unit};

fn stage_tree(tree: &UnitTree) -> Result<crate::UnitRecord, ReleasePayloadError> {
    stage_with(tree, |source| source)
}

fn stage_with(
    tree: &UnitTree,
    arrange: impl FnOnce(&std::path::Path) -> &std::path::Path,
) -> Result<crate::UnitRecord, ReleasePayloadError> {
    let store_root = tempfile::tempdir().expect("store root");
    let store = InstallStore::new(store_root.path().join("releases"), 64 * 1024);
    let source = tempfile::tempdir().expect("source");
    let id = tree.write(source.path());
    let lock = store.acquire_lock().expect("lock");
    let staged = stage_macos_bundle_unit(&store, &lock, arrange(source.path()), &id);
    unlock(source.path());
    staged
}

/// Give every directory back to its owner so the temporary tree can go.
fn unlock(root: &std::path::Path) {
    for entry in fs::read_dir(root).expect("list").flatten() {
        let path = entry.path();
        if entry.file_type().expect("type").is_dir() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("unlock");
            unlock(&path);
        }
    }
}

fn edit_manifest(tree: &UnitTree, edit: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut manifest: Value = serde_json::from_slice(&tree.manifest()).expect("manifest");
    edit(&mut manifest);
    serde_json::to_vec(&manifest).expect("encode")
}

fn stage_manifest(
    tree: &UnitTree,
    manifest: &[u8],
) -> Result<crate::UnitRecord, ReleasePayloadError> {
    let store_root = tempfile::tempdir().expect("store root");
    let store = InstallStore::new(store_root.path().join("releases"), 64 * 1024);
    let source = tempfile::tempdir().expect("source");
    let id = tree.write_with_manifest(source.path(), manifest);
    let lock = store.acquire_lock().expect("lock");
    stage_macos_bundle_unit(&store, &lock, source.path(), &id)
}

#[test]
fn a_bundle_unit_binds_its_three_executables_and_installed_bytes() {
    let tree = UnitTree::new("seed", "daemon", "1.2.3");
    let unit = stage_tree(&tree).expect("stage");
    let bound = crate::payload::bind_macos_bundle_unit(&unit).expect("bind");
    assert_eq!(bound.version(), "1.2.3");
    assert_eq!(bound.installed_bytes(), tree.installed_bytes());
    assert_eq!(bound.identity().app, thin_macho(b"seed:hypercolor-app").1);
    assert_eq!(
        bound.identity().daemon,
        thin_macho(b"daemon:hypercolor-daemon").1
    );
    assert_eq!(bound.identity().cli, thin_macho(b"seed:hypercolor").1);
    assert_eq!(bound.bundle_mode(), 0o755);
    assert!(
        bound
            .members()
            .iter()
            .all(|member| !member.path.starts_with("Hypercolor.app")),
        "members are relative to the bundle"
    );
    assert!(
        bound
            .members()
            .iter()
            .any(|member| member.path == "Contents/MacOS/hypercolor")
    );
}

#[test]
fn a_bundle_unit_holds_nothing_beside_the_bundle_but_its_store_inventory() {
    let mut tree = UnitTree::new("seed", "seed", "1.0.0");
    tree.files
        .push(("README".to_owned(), 0o644, b"extra".to_vec()));
    let error = stage_tree(&tree).expect_err("a file beside the bundle");
    assert!(
        matches!(error, ReleasePayloadError::InvalidManifest(_)),
        "{error}"
    );

    let mut tree = UnitTree::new("seed", "seed", "1.0.0");
    tree.files
        .retain(|(path, _, _)| !path.ends_with("durable-stores.json"));
    let error = stage_tree(&tree).expect_err("no store inventory");
    assert!(error.to_string().contains("durable-stores.json"), "{error}");

    let mut tree = UnitTree::new("seed", "seed", "1.0.0");
    tree.directories
        .push(("share/hypercolor/ui".to_owned(), 0o755));
    let error = stage_tree(&tree).expect_err("the standalone layout");
    assert!(
        matches!(error, ReleasePayloadError::InvalidManifest(_)),
        "{error}"
    );
}

#[test]
fn a_bundle_unit_refuses_files_its_manifest_does_not_cover_and_links() {
    let tree = UnitTree::new("seed", "seed", "1.0.0");
    let error = stage_with(&tree, |source| {
        fs::write(source.join("Hypercolor.app/Contents/added"), b"unsealed").expect("add file");
        source
    })
    .expect_err("an unlisted file inside the bundle");
    assert!(
        matches!(error, ReleasePayloadError::InvalidSource(_)),
        "{error}"
    );

    let error = stage_with(&tree, |source| {
        std::os::unix::fs::symlink("Info.plist", source.join("Hypercolor.app/Contents/link"))
            .expect("add link");
        source
    })
    .expect_err("a symlink inside the bundle");
    assert!(
        matches!(error, ReleasePayloadError::InvalidSource(_)),
        "{error}"
    );
}

#[test]
fn a_bundle_unit_requires_its_executables_and_owner_access_to_its_directories() {
    let mut tree = UnitTree::new("seed", "seed", "1.0.0");
    for file in &mut tree.files {
        if file.0.ends_with("MacOS/hypercolor") {
            file.1 = 0o644;
        }
    }
    let error = stage_tree(&tree).expect_err("a non-executable CLI");
    assert!(error.to_string().contains("0755"), "{error}");

    let mut tree = UnitTree::new("seed", "seed", "1.0.0");
    tree.files
        .retain(|(path, _, _)| !path.ends_with("MacOS/hypercolor-app"));
    assert!(stage_tree(&tree).is_err(), "a missing app executable");

    let mut tree = UnitTree::new("seed", "seed", "1.0.0");
    for directory in &mut tree.directories {
        if directory.0 == "Hypercolor.app/Contents/Resources" {
            directory.1 = 0o555;
        }
    }
    let error = stage_tree(&tree).expect_err("a read-only bundle directory");
    assert!(error.to_string().contains("full access"), "{error}");
}

#[test]
fn a_bundle_unit_refuses_members_others_could_write() {
    for (path, mode) in [
        ("Hypercolor.app/Contents/MacOS", 0o775),
        ("Hypercolor.app/Contents/Info.plist", 0o664),
        ("Hypercolor.app/Contents/Resources/icon.icns", 0o646),
    ] {
        let mut tree = UnitTree::new("seed", "seed", "1.0.0");
        for directory in &mut tree.directories {
            if directory.0 == path {
                directory.1 = mode;
            }
        }
        for file in &mut tree.files {
            if file.0 == path {
                file.1 = mode;
            }
        }
        let error = stage_tree(&tree).expect_err(path);
        assert!(error.to_string().contains("writable"), "{path}: {error}");
    }
}

#[test]
fn a_bundle_unit_names_one_macos_target_and_no_standalone_fields() {
    let tree = UnitTree::new("seed", "seed", "1.0.0");
    let mismatched = edit_manifest(&tree, |manifest| {
        manifest["rust_target"] = "x86_64-apple-darwin".into();
    });
    assert!(
        stage_manifest(&tree, &mismatched).is_err(),
        "platform and target disagree"
    );

    let standalone = edit_manifest(&tree, |manifest| {
        manifest["binaries"] = serde_json::json!(["hypercolor"]);
    });
    let error = stage_manifest(&tree, &standalone).expect_err("standalone fields");
    assert!(
        matches!(error, ReleasePayloadError::DecodeManifest(_)),
        "{error}"
    );

    let mut intel = UnitTree::new("seed", "seed", "1.0.0");
    intel.platform = ("macos-x86_64", "x86_64-apple-darwin");
    assert!(stage_tree(&intel).is_ok());
}

#[test]
fn retaining_a_bundle_unit_rehashes_every_member() {
    let store_root = tempfile::tempdir().expect("store root");
    let store = InstallStore::new(store_root.path().join("releases"), 64 * 1024);
    let source = tempfile::tempdir().expect("source");
    let id = UnitTree::new("seed", "seed", "1.0.0").write(source.path());
    let lock = store.acquire_lock().expect("lock");
    stage_macos_bundle_unit(&store, &lock, source.path(), &id).expect("stage");
    crate::retain_macos_bundle_unit(&store, &lock, &id).expect("an intact unit");
    let plist = store
        .unit_path(&id)
        .join("Hypercolor.app/Contents/Info.plist");
    fs::set_permissions(&plist, fs::Permissions::from_mode(0o644)).expect("unlock member");
    fs::write(&plist, b"<plist>tampered</plist>").expect("tamper");
    fs::set_permissions(&plist, fs::Permissions::from_mode(0o444)).expect("relock member");
    assert!(crate::retain_macos_bundle_unit(&store, &lock, &id).is_err());
}

#[test]
fn a_bundle_config_refuses_a_translocated_bundle_and_an_unbounded_window() {
    let config = |installed_bundle: &str, probation| crate::macos_bundle::MacosBundleConfig {
        installed_bundle: std::path::PathBuf::from(installed_bundle),
        release_root: std::path::PathBuf::from("/Users/player/releases"),
        probation,
    };
    config(
        "/Applications/Hypercolor.app",
        crate::DEFAULT_PROBATION_WINDOW,
    )
    .validate()
    .expect("the installed bundle");
    let translocated = config(
        "/private/var/folders/ab/cdef/T/AppTranslocation/0F3A/d/Hypercolor.app",
        crate::DEFAULT_PROBATION_WINDOW,
    )
    .validate()
    .expect_err("a translocated bundle vanishes with the app");
    assert!(translocated.to_string().contains("translocated"));
    let unbounded = config(
        "/Applications/Hypercolor.app",
        crate::MAX_PROBATION_WINDOW + std::time::Duration::from_secs(1),
    )
    .validate()
    .expect_err("an unbounded window");
    assert!(unbounded.to_string().contains("probation"));
}
