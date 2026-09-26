#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};

use hypercolor_platform_fs::{ExclusiveDirectory, PublicDirectoryAuthority};

struct Fixture {
    _temporary: tempfile::TempDir,
    gate: ExclusiveDirectory,
    public: PathBuf,
    outside: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::Builder::new()
            .prefix("platform-fs-tree-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("temporary directory");
        let root = fs::canonicalize(temporary.path()).expect("canonical root");
        let public = root.join("public");
        let outside = root.join("outside");
        fs::create_dir(&public).expect("public");
        fs::create_dir(&outside).expect("outside");
        fs::write(outside.join("keep"), b"outside").expect("outside file");
        let gate = ExclusiveDirectory::try_acquire(&root, Path::new("gate.lock"))
            .expect("gate")
            .expect("uncontended gate");
        Self {
            _temporary: temporary,
            gate,
            public,
            outside,
        }
    }

    fn authority(&self) -> PublicDirectoryAuthority {
        self.gate
            .open_public_directory(&self.public)
            .expect("public authority")
    }

    /// An immutable release-like tree: read-only directories and files, an
    /// active symlink, and a link that escapes the tree.
    fn populate(&self) -> PathBuf {
        let tree = self.public.join("releases");
        let unit = tree.join("units/a/bin");
        fs::create_dir_all(&unit).expect("unit");
        fs::write(unit.join("hypercolor-daemon"), b"daemon").expect("daemon");
        fs::set_permissions(
            unit.join("hypercolor-daemon"),
            fs::Permissions::from_mode(0o555),
        )
        .expect("daemon mode");
        symlink("units/a", tree.join("active")).expect("active");
        symlink(&self.outside, tree.join("escape")).expect("escape link");
        for directory in [unit.as_path(), unit.parent().expect("unit root")] {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o555)).expect("read-only");
        }
        tree
    }
}

#[test]
fn owned_tree_removal_unlinks_links_and_read_only_members_without_following() {
    let fixture = Fixture::new();
    let tree = fixture.populate();
    let authority = fixture.authority();
    assert!(
        authority
            .durable_remove_child_tree(Path::new("releases"))
            .expect("remove tree")
    );
    assert!(!tree.exists());
    assert_eq!(
        fs::read(fixture.outside.join("keep")).expect("outside survives"),
        b"outside"
    );
    assert!(
        !authority
            .durable_remove_child_tree(Path::new("releases"))
            .expect("absent tree")
    );
}

#[test]
fn owned_tree_removal_refuses_special_and_hardlinked_members_and_non_directories() {
    let fixture = Fixture::new();
    let tree = fixture.populate();
    let authority = fixture.authority();
    // macOS has no mknodat; the special-member refusal is exercised on Linux.
    #[cfg(target_os = "linux")]
    {
        rustix::fs::mknodat(
            rustix::fs::CWD,
            tree.join("fifo"),
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .expect("special member");
        assert!(
            authority
                .durable_remove_child_tree(Path::new("releases"))
                .is_err()
        );
        assert!(
            tree.join("fifo").exists(),
            "refusal keeps the special member"
        );
        fs::remove_file(tree.join("fifo")).expect("drop special member");
    }

    fs::write(fixture.public.join("shared"), b"linked").expect("shared file");
    fs::hard_link(fixture.public.join("shared"), tree.join("hardlink")).expect("hardlink");
    assert!(
        authority
            .durable_remove_child_tree(Path::new("releases"))
            .is_err()
    );
    assert_eq!(
        fs::read(fixture.public.join("shared")).expect("linked file survives"),
        b"linked"
    );
    fs::remove_file(tree.join("hardlink")).expect("drop hardlink");
    assert!(
        authority
            .durable_remove_child_tree(Path::new("releases"))
            .expect("resumed removal")
    );

    fs::write(fixture.public.join("file"), b"file").expect("regular child");
    symlink(&fixture.outside, fixture.public.join("link")).expect("link child");
    for name in ["file", "link", "../outside"] {
        assert!(
            authority
                .durable_remove_child_tree(Path::new(name))
                .is_err(),
            "{name} was removed as a tree"
        );
    }
    assert!(fixture.outside.join("keep").exists());
}

#[test]
fn owned_tree_removal_refuses_a_replaced_ancestry() {
    let fixture = Fixture::new();
    fixture.populate();
    let authority = fixture.authority();
    let displaced = fixture.public.with_extension("displaced");
    fs::rename(&fixture.public, &displaced).expect("displace public");
    fs::create_dir(&fixture.public).expect("replacement");
    assert!(
        authority
            .durable_remove_child_tree(Path::new("releases"))
            .is_err()
    );
    assert!(
        displaced
            .join("releases/units/a/bin/hypercolor-daemon")
            .exists()
    );
    for directory in [
        displaced.join("releases/units/a/bin"),
        displaced.join("releases/units/a"),
    ] {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).expect("cleanup");
    }
}

#[test]
fn owned_tree_removal_hides_the_name_first_and_finishes_leftover_tombstones() {
    let fixture = Fixture::new();
    let tree = fixture.populate();
    // An interrupted earlier removal left a tombstone behind.
    let leftover = fixture
        .public
        .join(".hypercolor-removing-releases.4242-7/units/a");
    fs::create_dir_all(&leftover).expect("leftover tombstone");
    fs::write(leftover.join("daemon"), b"old").expect("leftover member");
    fs::set_permissions(&leftover, fs::Permissions::from_mode(0o555)).expect("read-only");
    let unrelated = fixture.public.join(".hypercolor-removing-other.1-1");
    fs::create_dir(&unrelated).expect("unrelated tombstone");
    let authority = fixture.authority();
    assert!(
        authority
            .durable_remove_child_tree(Path::new("releases"))
            .expect("remove tree and leftovers")
    );
    assert!(!tree.exists());
    let remaining: Vec<_> = fs::read_dir(&fixture.public)
        .expect("public entries")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(
        remaining,
        vec![unrelated.file_name().expect("name").to_owned()]
    );

    // A leftover alone is still finished when the public name is gone.
    let orphan = fixture.public.join(".hypercolor-removing-releases.9-9");
    fs::create_dir(&orphan).expect("orphan tombstone");
    assert!(
        authority
            .durable_remove_child_tree(Path::new("releases"))
            .expect("finish orphan")
    );
    assert!(!orphan.exists());
    assert!(
        !authority
            .durable_remove_child_tree(Path::new("releases"))
            .expect("nothing left")
    );
}

#[test]
fn empty_child_removal_never_deletes_contents() {
    let fixture = Fixture::new();
    let empty = fixture.public.join("empty");
    let full = fixture.public.join("full");
    fs::create_dir(&empty).expect("empty");
    fs::create_dir(&full).expect("full");
    fs::write(full.join("keep"), b"keep").expect("content");
    symlink(&fixture.outside, fixture.public.join("link")).expect("link");
    let authority = fixture.authority();
    assert!(
        authority
            .durable_remove_empty_child(Path::new("empty"))
            .expect("empty removed")
    );
    assert!(!empty.exists());
    assert!(
        !authority
            .durable_remove_empty_child(Path::new("full"))
            .expect("full kept")
    );
    assert_eq!(fs::read(full.join("keep")).expect("content kept"), b"keep");
    assert!(
        !authority
            .durable_remove_empty_child(Path::new("absent"))
            .expect("absent")
    );
    assert!(
        authority
            .durable_remove_empty_child(Path::new("link"))
            .is_err()
    );
    assert!(fixture.outside.join("keep").exists());
}
