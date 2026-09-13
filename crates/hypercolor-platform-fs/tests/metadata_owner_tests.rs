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
