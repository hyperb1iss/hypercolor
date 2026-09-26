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

#[test]
fn bootstrap_rejects_a_real_foreign_owner_despite_safe_mode() {
    use hypercolor_platform_fs::ReadOnlyDirectoryAuthority;
    use std::path::Path;

    let root = ReadOnlyDirectoryAuthority::open(Path::new("/")).expect("root metadata");
    let root_metadata = root.metadata().expect("root metadata");
    let fixture = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("fixture");
    let (authority, path) = if root_metadata.is_owned_by_current_user() {
        // Root-run CI can create a foreign-owned fixture without touching host data.
        let owned = fixture.path().join("foreign");
        fs::create_dir(&owned).expect("foreign fixture");
        std::os::unix::fs::chown(&owned, Some(1), None).expect("set fixture owner");
        let authority = ReadOnlyDirectoryAuthority::open(&owned).expect("fixture authority");
        (authority, owned)
    } else {
        (root, Path::new("/").to_path_buf())
    };
    let metadata = authority.metadata().expect("fixture metadata");
    for role in [
        super::DirectoryRole::Ancestor,
        super::DirectoryRole::InstallerOwned,
    ] {
        assert!(matches!(
            super::require_safe_bootstrap_directory(
                &super::OwnershipPolicy::system(),
                &authority,
                metadata,
                &path,
                role,
            ),
            Err(InstallStoreError::UnsafeBootstrapDirectory(
                _,
                crate::DirectoryRefusal::ForeignOwner(_)
            ))
        ));
    }
}
