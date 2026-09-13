use std::fs;

use super::{InstallStore, InstallStoreError};

#[test]
fn bootstrap_state_replacement_is_rejected_before_lock_creation() {
    let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("home");
    let store = InstallStore::with_roots(
        home.path().join("releases"),
        home.path().join("state"),
        65536,
    )
    .expect("roots");
    let result = store.acquire_anchored_lock_after_bootstrap(home.path(), || {
        fs::rename(store.state_root(), home.path().join("displaced")).expect("displace state");
        fs::create_dir(store.state_root()).expect("replacement");
    });
    assert!(matches!(result, Err(InstallStoreError::BootstrapRoot(_))));
    assert!(!store.lock_path().exists());
}

#[test]
fn bootstrap_ancestor_replacement_cannot_preserve_leaf_authority() {
    for replace_state in [false, true] {
        let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("home");
        let store = InstallStore::with_roots(
            home.path().join("data/releases"),
            home.path().join("state/update"),
            65536,
        )
        .expect("roots");
        let result = store.acquire_anchored_lock_after_bootstrap(home.path(), || {
            let root = if replace_state {
                store.state_root()
            } else {
                store.root()
            };
            let parent = root.parent().expect("parent");
            let displaced = parent.with_extension("displaced");
            fs::rename(parent, &displaced).expect("displace parent");
            fs::create_dir(parent).expect("replacement parent");
            fs::rename(displaced.join(root.file_name().expect("root name")), root)
                .expect("preserve leaf");
        });
        assert!(matches!(result, Err(InstallStoreError::BootstrapRoot(_))));
        assert!(!store.lock_path().exists());
    }
}
