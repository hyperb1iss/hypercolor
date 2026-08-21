use std::io::{Cursor, Read as _};
use std::path::{Path, PathBuf};

use hypercolor_macos_owner::{
    MacosDirectLaunchdBootstrapExpectation, MacosDirectLaunchdBootstrapSource,
};
use hypercolor_platform_fs::{DirectoryAuthority, DirectoryEntryKind};

use super::super::InstallPlatformError;
use super::model::{
    MAX_LAUNCHER_BYTES, MacosFilePublication, MacosLauncherSnapshot, error, hex_digest,
    launcher_snapshot_id,
};

const LAUNCHD_DIRECTORY: &str = "launchd";

#[derive(Debug)]
pub(super) struct MacosLauncherStore {
    root_hint: PathBuf,
    directory: DirectoryAuthority,
}

impl MacosLauncherStore {
    pub(super) fn new(
        root_hint: PathBuf,
        root: &DirectoryAuthority,
    ) -> Result<Self, InstallPlatformError> {
        let directory = match root
            .entry_metadata(Path::new(LAUNCHD_DIRECTORY))
            .map_err(io_error)?
        {
            None => root
                .create_child_directory(Path::new(LAUNCHD_DIRECTORY))
                .map_err(io_error)?,
            Some(metadata) if metadata.kind() == DirectoryEntryKind::Directory => root
                .open_child_directory(Path::new(LAUNCHD_DIRECTORY))
                .map_err(io_error)?,
            Some(_) => {
                return Err(error(
                    "private macOS launchd snapshot root is not a directory",
                ));
            }
        };
        let metadata = directory.metadata().map_err(io_error)?;
        if metadata.mode() & 0o700 != 0o700 || metadata.mode() & 0o077 != 0 {
            return Err(error(
                "private macOS launchd snapshot root has unsafe permissions",
            ));
        }
        Ok(Self {
            root_hint,
            directory,
        })
    }

    pub(super) fn persist(
        &self,
        launcher: &MacosFilePublication,
    ) -> Result<MacosLauncherSnapshot, InstallPlatformError> {
        if launcher.contents.is_empty() || launcher.contents.len() > MAX_LAUNCHER_BYTES {
            return Err(error("private macOS launcher snapshot has invalid size"));
        }
        let snapshot_id = launcher_snapshot_id(launcher.mode, &launcher.contents);
        let name = format!("{snapshot_id}.plist");
        let path = Path::new(&name);
        let metadata = match self.directory.entry_metadata(path).map_err(io_error)? {
            None => self
                .directory
                .create_regular_file(
                    path,
                    launcher.mode,
                    launcher.contents.len() as u64,
                    &mut Cursor::new(&launcher.contents),
                )
                .map_err(io_error)?,
            Some(metadata) if metadata.kind() == DirectoryEntryKind::RegularFile => metadata,
            Some(_) => return Err(error("private macOS launcher snapshot has the wrong kind")),
        };
        let snapshot = MacosLauncherSnapshot {
            snapshot_id: snapshot_id.clone(),
            relative_path: format!("{LAUNCHD_DIRECTORY}/{name}"),
            content_sha256: hex_digest(&launcher.contents),
            mode: launcher.mode,
            size: launcher.contents.len() as u64,
            device: metadata.device(),
            inode: metadata.inode(),
        };
        self.validate(launcher, &snapshot)?;
        Ok(snapshot)
    }

    pub(super) fn validate(
        &self,
        launcher: &MacosFilePublication,
        snapshot: &MacosLauncherSnapshot,
    ) -> Result<(), InstallPlatformError> {
        let expected_id = launcher_snapshot_id(launcher.mode, &launcher.contents);
        let expected_relative = format!("{LAUNCHD_DIRECTORY}/{expected_id}.plist");
        if snapshot.snapshot_id != expected_id
            || snapshot.relative_path != expected_relative
            || snapshot.content_sha256 != hex_digest(&launcher.contents)
            || snapshot.mode != launcher.mode
            || snapshot.size != launcher.contents.len() as u64
            || snapshot.device == 0
            || snapshot.inode == 0
        {
            return Err(error(
                "private macOS launcher snapshot metadata is inconsistent",
            ));
        }
        let mut opened = self
            .directory
            .open_regular_file(Path::new(&format!("{expected_id}.plist")))
            .map_err(io_error)?;
        let metadata = opened.metadata();
        if metadata.mode() != snapshot.mode
            || metadata.size() != snapshot.size
            || metadata.device() != snapshot.device
            || metadata.inode() != snapshot.inode
        {
            return Err(error("private macOS launcher snapshot inode changed"));
        }
        let mut contents = Vec::with_capacity(launcher.contents.len());
        opened
            .file_mut()
            .take(snapshot.size.saturating_add(1))
            .read_to_end(&mut contents)
            .map_err(io_error)?;
        if contents != launcher.contents {
            return Err(error("private macOS launcher snapshot bytes changed"));
        }
        Ok(())
    }

    pub(super) fn bootstrap_source(
        &self,
        snapshot: &MacosLauncherSnapshot,
    ) -> Result<MacosDirectLaunchdBootstrapSource, InstallPlatformError> {
        let expected_relative = format!("{LAUNCHD_DIRECTORY}/{}.plist", snapshot.snapshot_id);
        if snapshot.relative_path != expected_relative
            || snapshot.content_sha256.len() != 64
            || snapshot.size == 0
            || snapshot.size > MAX_LAUNCHER_BYTES as u64
            || snapshot.device == 0
            || snapshot.inode == 0
        {
            return Err(error("private macOS launcher snapshot is not canonical"));
        }
        let mut opened = self
            .directory
            .open_regular_file(Path::new(&format!("{}.plist", snapshot.snapshot_id)))
            .map_err(io_error)?;
        let metadata = opened.metadata();
        if metadata.mode() != snapshot.mode
            || metadata.size() != snapshot.size
            || metadata.device() != snapshot.device
            || metadata.inode() != snapshot.inode
        {
            return Err(error("private macOS launcher snapshot inode changed"));
        }
        let mut contents = Vec::with_capacity(
            usize::try_from(snapshot.size)
                .map_err(|_| error("private macOS launcher snapshot exceeds this process"))?,
        );
        opened
            .file_mut()
            .take(snapshot.size.saturating_add(1))
            .read_to_end(&mut contents)
            .map_err(io_error)?;
        if contents.len() as u64 != snapshot.size
            || hex_digest(&contents) != snapshot.content_sha256
            || launcher_snapshot_id(snapshot.mode, &contents) != snapshot.snapshot_id
        {
            return Err(error("private macOS launcher snapshot bytes changed"));
        }
        let expectation = MacosDirectLaunchdBootstrapExpectation::new(
            self.absolute_path(snapshot)?,
            &snapshot.content_sha256,
            snapshot.mode,
            snapshot.size,
            snapshot.device,
            snapshot.inode,
        )
        .map_err(|source| error(source.to_string()))?;
        Ok(MacosDirectLaunchdBootstrapSource::new(
            opened.into_file(),
            expectation,
        ))
    }

    pub(super) fn absolute_path(
        &self,
        snapshot: &MacosLauncherSnapshot,
    ) -> Result<PathBuf, InstallPlatformError> {
        if snapshot.relative_path != format!("{LAUNCHD_DIRECTORY}/{}.plist", snapshot.snapshot_id) {
            return Err(error(
                "private macOS launcher snapshot path is not canonical",
            ));
        }
        Ok(self.root_hint.join(&snapshot.relative_path))
    }
}

fn io_error(source: std::io::Error) -> InstallPlatformError {
    error(source.to_string())
}
