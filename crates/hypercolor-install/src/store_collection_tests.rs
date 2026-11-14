use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::{DESKTOP_ENTRIES_DIRECTORY, InstallStore, InstallStoreError, UnitId};

fn unit(byte: &str) -> UnitId {
    UnitId::new(byte.repeat(32)).expect("unit ID")
}

fn install(root: &Path, unit: &UnitId) {
    fs::create_dir_all(root.join("units").join(unit.as_str()).join("bin")).expect("unit");
}

fn render(root: &Path, name: &str) -> PathBuf {
    let directory = root.join(DESKTOP_ENTRIES_DIRECTORY).join(name);
    fs::create_dir_all(&directory).expect("rendered directory");
    fs::write(directory.join("hypercolor.desktop"), b"[Desktop Entry]\n").expect("entry");
    directory
}

fn names(directory: &Path) -> BTreeSet<String> {
    fs::read_dir(directory)
        .expect("directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .into_string()
                .expect("UTF-8")
        })
        .collect()
}

#[test]
fn collection_removes_what_was_rendered_for_every_unit_it_would_remove() {
    let fixture = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("fixture");
    let root = fixture.path().join("store");
    let store = InstallStore::new(&root, 64 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let (active, retained, stale, gone) = (unit("aa"), unit("bb"), unit("cc"), unit("dd"));
    for unit in [&active, &retained, &stale] {
        install(&root, unit);
        render(&root, unit.as_str());
    }
    let gone_entry = render(&root, gone.as_str());
    let tombstone = render(&root, ".hypercolor-removing-x.1");
    render(&root, "notes");
    store.set_active(Some(&active), &lock).expect("active");

    let collection = store
        .collect_units(&lock, std::slice::from_ref(&retained))
        .expect("collection");

    assert_eq!(collection.removed_units, std::slice::from_ref(&stale));
    assert!(collection.refused.is_empty(), "{:?}", collection.refused);
    let rendered = root.join(DESKTOP_ENTRIES_DIRECTORY);
    assert_eq!(
        collection.removed_leftovers.iter().collect::<BTreeSet<_>>(),
        [&rendered.join(stale.as_str()), &gone_entry, &tombstone]
            .into_iter()
            .collect(),
    );
    assert_eq!(
        names(&rendered),
        [active.as_str(), retained.as_str(), "notes"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        "kept units keep theirs, and names the installer never makes stay"
    );
    assert_eq!(
        names(&root.join("units")),
        [active.as_str(), retained.as_str()]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
}

#[test]
fn removing_a_unit_removes_what_was_rendered_for_it_and_refuses_a_referenced_one() {
    let fixture = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("fixture");
    let root = fixture.path().join("store");
    let store = InstallStore::new(&root, 64 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let (active, stale, gone) = (unit("aa"), unit("cc"), unit("dd"));
    for unit in [&active, &stale] {
        install(&root, unit);
        render(&root, unit.as_str());
    }
    let gone_entry = render(&root, gone.as_str());
    store.set_active(Some(&active), &lock).expect("active");

    assert!(store.remove_unit(&lock, &stale).expect("remove"));
    assert!(!root.join("units").join(stale.as_str()).exists());
    assert!(
        !root
            .join(DESKTOP_ENTRIES_DIRECTORY)
            .join(stale.as_str())
            .exists()
    );

    assert!(
        !store.remove_unit(&lock, &gone).expect("an absent unit"),
        "the unit itself was not installed"
    );
    assert!(
        !gone_entry.exists(),
        "what an interrupted removal left goes"
    );

    assert!(matches!(
        store.remove_unit(&lock, &active),
        Err(InstallStoreError::UnitReferenced(_))
    ));
    assert!(
        root.join(DESKTOP_ENTRIES_DIRECTORY)
            .join(active.as_str())
            .join("hypercolor.desktop")
            .is_file()
    );
}

#[test]
fn a_store_that_never_rendered_anything_collects_as_before() {
    let fixture = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("fixture");
    let root = fixture.path().join("store");
    let store = InstallStore::new(&root, 64 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let stale = unit("cc");
    install(&root, &stale);

    let collection = store.collect_units(&lock, &[]).expect("collection");

    assert_eq!(collection.removed_units, std::slice::from_ref(&stale));
    assert!(collection.removed_leftovers.is_empty());
    assert!(
        store
            .remove_unit(&lock, &stale)
            .is_ok_and(|removed| !removed)
    );
    assert!(!root.join(DESKTOP_ENTRIES_DIRECTORY).exists());
}
