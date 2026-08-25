use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

mod tree;

pub use tree::{
    DirectoryAuthority, DirectoryEntryKind, DirectoryEntryMetadata, EntryReplacement,
    ExactDirectoryEntry, ExactEntry, ExclusiveDirectory, MAX_EXACT_ENTRY_BYTES,
    MAX_PUBLIC_DIRECTORY_CHILD_COUNT, MAX_PUBLIC_DIRECTORY_CHILD_NAMES_BYTES, OpenedRegularFile,
    PrivateStagingDirectory, PublicDirectoryAuthority, ReadOnlyDirectoryAuthority,
};

const SECRET_FILE_MODE: u32 = 0o600;
#[cfg(any(target_os = "macos", target_os = "ios"))]
const OPEN_NO_FOLLOW: i32 = 0x0100;
#[cfg(any(target_os = "linux", target_os = "android"))]
const OPEN_NO_FOLLOW: i32 = 0x0002_0000;

pub(super) fn durable_replace(source: &Path, destination: &Path) -> io::Result<()> {
    durable_replace_with(source, destination, sync_directory)
}

fn durable_replace_with(
    source: &Path,
    destination: &Path,
    sync: impl FnOnce(File) -> io::Result<()>,
) -> io::Result<()> {
    let parent = File::open(destination_parent(destination))?;
    fs::rename(source, destination)?;
    sync(parent)
}

pub(super) fn write_secret(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_secret_with(path, contents, tree::write_secret_contents)
}

fn write_secret_with(
    path: &Path,
    contents: &[u8],
    write: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(SECRET_FILE_MODE)
        .open(path)?;
    if let Err(error) = write(&mut file, contents) {
        drop(file);
        drop(fs::remove_file(path));
        return Err(error);
    }
    Ok(())
}

pub(super) fn open_no_follow(path: &Path) -> io::Result<File> {
    #[cfg(any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    ))]
    {
        OpenOptions::new()
            .read(true)
            .custom_flags(OPEN_NO_FOLLOW)
            .open(path)
    }

    #[cfg(not(any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "symlink-refusing file open is unsupported on this Unix platform",
        ))
    }
}

fn destination_parent(destination: &Path) -> PathBuf {
    destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn sync_directory(directory: File) -> io::Result<()> {
    directory.sync_all()
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn replacement_runs_parent_sync_after_rename() {
        let directory = tempfile::Builder::new()
            .prefix("platform-fs-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("temporary directory");
        let source = directory.path().join("source.tmp");
        let destination = directory.path().join("state.json");
        fs::write(&source, b"new").expect("write source");
        fs::write(&destination, b"old").expect("write destination");
        let sync_called = Cell::new(false);

        durable_replace_with(&source, &destination, |parent| {
            assert!(parent.metadata()?.is_dir());
            assert_eq!(fs::read(&destination)?, b"new");
            sync_called.set(true);
            Ok(())
        })
        .expect("replace and sync destination");

        assert!(sync_called.get());
    }

    #[test]
    fn parent_sync_failure_is_reported_after_atomic_replacement() {
        let directory = tempfile::Builder::new()
            .prefix("platform-fs-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("temporary directory");
        let source = directory.path().join("source.tmp");
        let destination = directory.path().join("state.json");
        fs::write(&source, b"new").expect("write source");
        fs::write(&destination, b"old").expect("write destination");

        let error = durable_replace_with(&source, &destination, |_| {
            Err(io::Error::other("injected parent sync failure"))
        })
        .expect_err("parent sync failure must propagate");

        assert_eq!(error.to_string(), "injected parent sync failure");
        assert_eq!(fs::read(&destination).expect("read destination"), b"new");
        assert!(!source.exists());
    }

    #[test]
    fn failed_secret_write_removes_the_partial_file() {
        use std::io::Write as _;

        let directory = tempfile::Builder::new()
            .prefix("platform-fs-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("temporary directory");
        let path = directory.path().join("credential");

        let error = write_secret_with(&path, b"private material", |file, contents| {
            file.write_all(&contents[..4])?;
            Err(io::Error::other("injected write failure"))
        })
        .expect_err("injected write failure must propagate");

        assert_eq!(error.to_string(), "injected write failure");
        assert!(!path.exists(), "partial secret must be removed");
        write_secret(&path, b"private material").expect("path is reusable after cleanup");
        assert_eq!(fs::read(&path).expect("read secret"), b"private material");
    }

    #[test]
    fn failed_secret_sync_removes_the_complete_file() {
        use std::io::Write as _;

        let directory = tempfile::Builder::new()
            .prefix("platform-fs-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("temporary directory");
        let path = directory.path().join("credential");

        write_secret_with(&path, b"private material", |file, contents| {
            file.write_all(contents)?;
            Err(io::Error::other("injected sync failure"))
        })
        .expect_err("injected sync failure must propagate");

        assert!(!path.exists(), "unsynced secret must be removed");
    }
}
