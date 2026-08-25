use std::fs;

#[cfg(unix)]
use hypercolor_platform_fs::ExclusiveDirectory;
use hypercolor_platform_fs::{durable_replace, replace_file};

#[cfg(unix)]
fn acquire(directory: &std::path::Path) -> ExclusiveDirectory {
    ExclusiveDirectory::try_acquire(directory, std::path::Path::new("install.lock"))
        .expect("acquire directory authority")
        .expect("directory authority is uncontended")
}

#[test]
fn replacement_overwrites_destination_and_consumes_source() {
    let directory = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let source = directory.path().join("source.tmp");
    let destination = directory.path().join("state.json");
    fs::write(&source, b"new").expect("write source");
    fs::write(&destination, b"old").expect("write destination");

    durable_replace(&source, &destination).expect("replace destination");

    assert_eq!(fs::read(&destination).expect("read destination"), b"new");
    assert!(!source.exists());
}

#[test]
fn failed_replacement_preserves_existing_destination() {
    let directory = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let source = directory.path().join("missing.tmp");
    let destination = directory.path().join("state.json");
    fs::write(&destination, b"old").expect("write destination");

    durable_replace(&source, &destination).expect_err("missing source must fail");

    assert_eq!(fs::read(&destination).expect("read destination"), b"old");
}

#[test]
fn compatibility_entry_point_includes_the_durability_contract() {
    let directory = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let source = directory.path().join("source.tmp");
    let destination = directory.path().join("state.json");
    fs::write(&source, b"new").expect("write source");

    replace_file(&source, &destination).expect("replace destination durably");

    assert_eq!(fs::read(&destination).expect("read destination"), b"new");
}

#[cfg(unix)]
#[test]
fn relative_symlink_replacement_switches_one_active_entry() {
    let directory = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let units = directory.path().join("units");
    fs::create_dir(&units).expect("create units directory");
    fs::create_dir(units.join("old")).expect("create old unit");
    fs::create_dir(units.join("new")).expect("create new unit");
    let active = directory.path().join("active");
    std::os::unix::fs::symlink("units/old", &active).expect("create old active link");
    let authority = acquire(directory.path());

    authority
        .durable_replace_symlink(
            std::path::Path::new("units/new"),
            std::path::Path::new("active"),
        )
        .expect("replace active link durably");

    assert_eq!(
        fs::read_link(active).expect("read active link"),
        std::path::Path::new("units/new")
    );
    assert_eq!(
        authority
            .read_symlink(std::path::Path::new("active"))
            .expect("read active through authority"),
        Some(std::path::PathBuf::from("units/new"))
    );
}

#[cfg(unix)]
#[test]
fn symlink_replacement_rejects_targets_outside_the_install_root() {
    let directory = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let authority = acquire(directory.path());

    for target in ["", "/tmp/unit", "../unit", "units/../unit", "./unit"] {
        authority
            .durable_replace_symlink(std::path::Path::new(target), std::path::Path::new("active"))
            .expect_err("unsafe target must be rejected");
    }
    assert!(!directory.path().join("active").exists());
}

#[cfg(unix)]
#[test]
fn symlink_replacement_preserves_an_unexpected_regular_destination() {
    let directory = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let active = directory.path().join("active");
    fs::write(&active, b"unexpected").expect("write unexpected destination");
    let authority = acquire(directory.path());

    authority
        .durable_replace_symlink(
            std::path::Path::new("units/new"),
            std::path::Path::new("active"),
        )
        .expect_err("regular destination must be rejected");

    assert_eq!(fs::read(active).expect("read destination"), b"unexpected");
}

#[cfg(unix)]
#[test]
fn durable_removal_handles_files_links_and_missing_entries() {
    let directory = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let file = directory.path().join("launcher");
    let link = directory.path().join("active");
    fs::write(&file, b"launcher").expect("write launcher");
    std::os::unix::fs::symlink("units/old", &link).expect("create active link");
    let authority = acquire(directory.path());

    assert!(
        authority
            .durable_remove_file(std::path::Path::new("launcher"))
            .expect("remove launcher")
    );
    assert!(
        authority
            .durable_remove_file(std::path::Path::new("active"))
            .expect("remove active link")
    );
    assert!(
        !authority
            .durable_remove_file(std::path::Path::new("launcher"))
            .expect("missing launcher is unchanged")
    );
}

#[cfg(unix)]
#[test]
fn durable_removal_refuses_directories() {
    let directory = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    fs::create_dir(directory.path().join("unit")).expect("create nested directory");
    let authority = acquire(directory.path());

    authority
        .durable_remove_file(std::path::Path::new("unit"))
        .expect_err("directory must be rejected");
    assert!(directory.path().join("unit").is_dir());
}

#[cfg(unix)]
#[test]
fn directory_authority_is_exclusive() {
    let directory = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let authority = acquire(directory.path());

    assert!(
        ExclusiveDirectory::try_acquire(directory.path(), std::path::Path::new("install.lock"))
            .expect("probe contended directory authority")
            .is_none()
    );

    drop(authority);
    assert!(
        ExclusiveDirectory::try_acquire(directory.path(), std::path::Path::new("install.lock"))
            .expect("reacquire released directory authority")
            .is_some()
    );
}

#[cfg(unix)]
#[test]
fn directory_authority_refuses_to_mutate_its_lock_entry() {
    let directory = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    fs::write(directory.path().join("replacement"), b"replacement").expect("write replacement");
    let authority = acquire(directory.path());

    authority
        .durable_remove_file(std::path::Path::new("install.lock"))
        .expect_err("held lock removal must be rejected");
    authority
        .durable_replace_file(
            std::path::Path::new("replacement"),
            std::path::Path::new("install.lock"),
        )
        .expect_err("held lock replacement must be rejected");

    assert!(directory.path().join("install.lock").is_file());
    assert!(directory.path().join("replacement").is_file());
}

#[cfg(unix)]
#[test]
fn directory_authority_survives_parent_path_replacement() {
    let root = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let current = root.path().join("current");
    let original = root.path().join("original");
    fs::create_dir(&current).expect("create governed directory");
    let authority = acquire(&current);

    fs::rename(&current, &original).expect("rename governed directory");
    fs::create_dir(&current).expect("create replacement directory");
    authority
        .durable_replace_symlink(
            std::path::Path::new("units/new"),
            std::path::Path::new("active"),
        )
        .expect("mutate opened directory incarnation");

    assert_eq!(
        fs::read_link(original.join("active")).expect("read original directory link"),
        std::path::Path::new("units/new")
    );
    assert!(!current.join("active").exists());
}

#[test]
fn directory_source_cannot_replace_a_regular_destination() {
    let directory = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let source = directory.path().join("source.dir");
    let destination = directory.path().join("state.json");
    fs::create_dir(&source).expect("create directory source");
    fs::write(&destination, b"old").expect("write destination");

    durable_replace(&source, &destination).expect_err("directory source must fail");

    assert_eq!(fs::read(&destination).expect("read destination"), b"old");
    assert!(source.is_dir());
}

#[test]
fn replacement_into_a_missing_parent_preserves_the_source() {
    let directory = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let source = directory.path().join("source.tmp");
    let destination = directory.path().join("missing").join("state.json");
    fs::write(&source, b"new").expect("write source");

    durable_replace(&source, &destination).expect_err("missing parent must fail");

    assert_eq!(fs::read(&source).expect("read source"), b"new");
    assert!(!destination.exists());
}

#[cfg(unix)]
#[test]
fn unopenable_parent_fails_before_the_destination_changes() {
    use std::os::unix::fs::PermissionsExt as _;

    if rustix::process::geteuid().is_root() {
        eprintln!("skipping: root bypasses directory permission checks");
        return;
    }

    let directory = tempfile::Builder::new()
        .prefix("platform-fs-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory");
    let parent = directory.path().join("parent");
    fs::create_dir(&parent).expect("create parent");
    let source = directory.path().join("source.tmp");
    let destination = parent.join("state.json");
    fs::write(&source, b"new").expect("write source");
    fs::write(&destination, b"old").expect("write destination");
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o000)).expect("seal parent");

    let result = durable_replace(&source, &destination);

    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).expect("unseal parent");
    result.expect_err("unopenable parent must fail");
    assert_eq!(fs::read(&destination).expect("read destination"), b"old");
    assert_eq!(fs::read(&source).expect("read source"), b"new");
}
