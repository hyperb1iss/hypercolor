#![cfg(unix)]

use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use hypercolor_platform_fs::ReadOnlyDirectoryAuthority;

#[test]
fn owner_identity_comes_from_the_retained_directory() {
    let fixture = tempfile::tempdir().expect("fixture");
    let original = fixture.path().join("original");
    let retained_path = fixture.path().join("retained");
    fs::create_dir(&original).expect("directory");
    let expected = fs::metadata(&original).expect("metadata");
    let authority = ReadOnlyDirectoryAuthority::open(&original).expect("authority");
    fs::rename(&original, &retained_path).expect("move original");
    fs::create_dir(&original).expect("replacement");
    let observed = authority.metadata().expect("retained metadata");
    assert_eq!(observed.owner_uid(), expected.uid());
    assert_eq!(observed.owner_gid(), expected.gid());
    assert_eq!(observed.inode(), expected.ino());
    assert!(observed.is_owned_by_current_user());
}

#[test]
fn current_owner_check_compares_effective_user_instead_of_permission_bits() {
    let authority = ReadOnlyDirectoryAuthority::open(Path::new("/")).expect("root authority");
    let metadata = authority.metadata().expect("root metadata");
    assert_eq!(
        metadata.owner_uid(),
        fs::metadata("/").expect("root stat").uid()
    );
    assert_eq!(
        metadata.is_owned_by_current_user(),
        metadata.owner_uid() == rustix::process::geteuid().as_raw()
    );
}

#[cfg(target_os = "linux")]
fn posix_access_acl(named_uid: u32) -> Vec<u8> {
    const UNDEFINED_ID: u32 = u32::MAX;
    let entries: [(u16, u16, u32); 5] = [
        (0x01, 0o7, UNDEFINED_ID),
        (0x02, 0o7, named_uid),
        (0x04, 0o5, UNDEFINED_ID),
        (0x10, 0o7, UNDEFINED_ID),
        (0x20, 0o5, UNDEFINED_ID),
    ];
    let mut bytes = 2_u32.to_le_bytes().to_vec();
    for (tag, perm, id) in entries {
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&perm.to_le_bytes());
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    bytes
}

#[cfg(target_os = "linux")]
#[test]
fn extended_access_acl_probe_reads_the_retained_directory() {
    use std::os::unix::fs::PermissionsExt as _;

    use hypercolor_platform_fs::ExclusiveDirectory;

    let fixture = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).expect("fixture");
    let plain = fixture.path().join("plain");
    let granted = fixture.path().join("granted");
    for directory in [&plain, &granted] {
        fs::create_dir(directory).expect("directory");
        fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).expect("mode");
    }
    let plain_authority = ReadOnlyDirectoryAuthority::open(&plain).expect("plain authority");
    assert!(
        !plain_authority
            .has_extended_access_acl()
            .expect("plain probe")
    );

    let granted_file = fs::File::open(&granted).expect("granted handle");
    let named = if rustix::process::geteuid().as_raw() == 65_534 {
        65_533
    } else {
        65_534
    };
    match rustix::fs::fsetxattr(
        &granted_file,
        "system.posix_acl_access",
        &posix_access_acl(named),
        rustix::fs::XattrFlags::empty(),
    ) {
        Ok(()) => {}
        Err(rustix::io::Errno::OPNOTSUPP) => {
            eprintln!("skipping ACL probe: fixture filesystem does not support POSIX ACLs");
            return;
        }
        Err(error) => panic!("set fixture ACL: {error}"),
    }
    let retained = ReadOnlyDirectoryAuthority::open(&granted).expect("granted authority");
    assert!(retained.has_extended_access_acl().expect("granted probe"));
    assert_eq!(
        fs::metadata(&granted).expect("mode").permissions().mode() & 0o777,
        0o775,
        "the ACL mask surfaces as group write permission"
    );

    let gate = ExclusiveDirectory::try_acquire(fixture.path(), Path::new("probe.lock"))
        .expect("lock")
        .expect("uncontended lock");
    let public = gate
        .open_public_directory(&granted)
        .expect("public authority");
    assert!(public.has_extended_access_acl().expect("public probe"));
    let public_plain = gate.open_public_directory(&plain).expect("plain public");
    assert!(
        !public_plain
            .has_extended_access_acl()
            .expect("plain public probe")
    );
}
