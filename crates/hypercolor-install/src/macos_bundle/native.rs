//! The filesystem effects of an app bundle installation, through retained
//! directory handles.
//!
//! Every path below the installed bundle's parent is reached one component
//! at a time with `O_NOFOLLOW`, so a symlink planted anywhere in the
//! staging root or a bundle is refused, never followed. Files open with
//! `O_NONBLOCK`, so a FIFO in their place is refused, never waited on. The calls that are
//! macOS's own (the swapping rename, `clonefile`, `F_FULLFSYNC` and the
//! volume query) are confined to [`sys`].

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read as _;
use std::os::fd::{AsFd, OwnedFd};
use std::path::Path;

use hypercolor_platform_fs::ReadOnlyDirectoryAuthority;
use rustix::fs::{self as rfs, Access, AtFlags, FileType, Mode, OFlags, RenameFlags, Stat};
use rustix::io::Errno;
use sha2::{Digest as _, Sha256};

use super::super::{InstallPlatformError, UnitRecord};
use super::executor::MacosBundleExecutor;
use super::model::{
    MACOS_BUNDLE_DISCARD_SUFFIX, MACOS_BUNDLE_NAME, MACOS_BUNDLE_PARTIAL_SUFFIX,
    MACOS_BUNDLE_STAGING_ROOT_NAME, MacosBundleConfig, MacosBundleIdentity,
    MacosBundleMaterialization, MacosBundleMember, MacosBundleMemberKind, MacosBundleObservation,
    MacosBundleSlot, MacosBundleUnit, error,
};

const EXECUTABLES: [&str; 3] = ["hypercolor-app", "hypercolor-daemon", "hypercolor"];
const MAX_TREE_DEPTH: usize = 64;
const MAX_STAGED_COPIES: usize = 16;
const SCRATCH_DIRECTORY_MODE: u32 = 0o700;
const STAGING_ROOT_MODE: u32 = 0o755;

/// The native [`MacosBundleExecutor`]: the installed bundle's directory and
/// its staging root, on one APFS volume this user may write.
pub struct MacosBundleNativeExecutor {
    applications: OwnedFd,
    applications_device: u64,
    user: u32,
}

impl MacosBundleNativeExecutor {
    /// Open the directory holding the installed bundle and prove the
    /// installation can be swapped there: the volume exchanges directories
    /// atomically, this user may write the directory, and the installed
    /// bundle and staging root, where present, are this user's.
    ///
    /// # Errors
    /// Returns an error naming the first requirement that does not hold.
    pub fn new(config: &MacosBundleConfig) -> Result<Self, InstallPlatformError> {
        config.validate()?;
        let applications = rfs::open(
            config.applications(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io("open the installed bundle's directory"))?;
        rfs::accessat(
            &applications,
            ".",
            Access::WRITE_OK | Access::EXEC_OK,
            AtFlags::EACCESS,
        )
        .map_err(io(
            "this user may not write the installed bundle's directory",
        ))?;
        if !sys::volume(&applications)
            .map_err(io("inspect the installed bundle's volume"))?
            .atomic_swap
        {
            return Err(error(
                "the installed bundle's volume cannot swap directories atomically; it must be APFS",
            ));
        }
        let executor = Self {
            applications_device: device(
                &rfs::fstat(&applications)
                    .map_err(io("inspect the installed bundle's directory"))?,
            ),
            applications,
            user: rustix::process::geteuid().as_raw(),
        };
        if let Some(installed) = stat_entry(&executor.applications, MACOS_BUNDLE_NAME)? {
            executor.require_owned_directory(&installed, "the installed bundle")?;
        }
        executor.staging_root(false)?;
        Ok(executor)
    }

    fn require_owned_directory(&self, stat: &Stat, what: &str) -> Result<(), InstallPlatformError> {
        if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
            return Err(error(format!("{what} is not a directory")));
        }
        if stat.st_uid != self.user {
            return Err(error(format!(
                "{what} belongs to another user, so this installation is not managed here"
            )));
        }
        if device(stat) != self.applications_device {
            return Err(error(format!(
                "{what} is on another volume than /Applications"
            )));
        }
        Ok(())
    }

    /// The staging root, created durably when `create` is set.
    fn staging_root(&self, create: bool) -> Result<Option<OwnedFd>, InstallPlatformError> {
        if stat_entry(&self.applications, MACOS_BUNDLE_STAGING_ROOT_NAME)?.is_none() {
            if !create {
                return Ok(None);
            }
            rfs::mkdirat(
                &self.applications,
                MACOS_BUNDLE_STAGING_ROOT_NAME,
                Mode::from_raw_mode(0o700),
            )
            .map_err(io("create the bundle staging root"))?;
            let root = open_directory(&self.applications, MACOS_BUNDLE_STAGING_ROOT_NAME)?;
            rfs::fchmod(&root, Mode::from_raw_mode(STAGING_ROOT_MODE as _))
                .map_err(io("set the bundle staging root's mode"))?;
            sync_directory(&root)?;
            sync_directory(&self.applications)?;
            sys::full_sync(&self.applications).map_err(io("flush the staging root"))?;
        }
        let root = open_directory(&self.applications, MACOS_BUNDLE_STAGING_ROOT_NAME)?;
        self.require_owned_directory(
            &rfs::fstat(&root).map_err(io("inspect the bundle staging root"))?,
            "the bundle staging root",
        )?;
        Ok(Some(root))
    }

    fn require_staging_root(&self) -> Result<OwnedFd, InstallPlatformError> {
        self.staging_root(false)?
            .ok_or_else(|| error("the bundle staging root is missing"))
    }

    /// Flush both directories one rename touched, then the volume.
    fn sync_rename(&self, staged: &OwnedFd) -> Result<(), InstallPlatformError> {
        sync_directory(staged)?;
        sync_directory(&self.applications)?;
        sys::full_sync(&self.applications).map_err(io("flush the bundle swap"))
    }
}

impl MacosBundleExecutor for MacosBundleNativeExecutor {
    fn observe(&mut self) -> Result<MacosBundleObservation, InstallPlatformError> {
        let installed = bundle_slot(&self.applications, MACOS_BUNDLE_NAME);
        let mut staged = BTreeMap::new();
        let mut interrupted = BTreeSet::new();
        if let Some(root) = self.staging_root(false)? {
            for name in entry_names(&root)? {
                if is_digest(&name) {
                    if staged.len() == MAX_STAGED_COPIES {
                        return Err(error("the bundle staging root holds too many copies"));
                    }
                    let slot = match open_directory(&root, &name) {
                        Ok(directory) => bundle_slot(&directory, MACOS_BUNDLE_NAME),
                        Err(source) => MacosBundleSlot::Unrecognized(source.to_string()),
                    };
                    staged.insert(name, slot);
                } else if [MACOS_BUNDLE_PARTIAL_SUFFIX, MACOS_BUNDLE_DISCARD_SUFFIX]
                    .into_iter()
                    .any(|suffix| name.strip_suffix(suffix).is_some_and(is_digest))
                {
                    interrupted.insert(name);
                }
            }
        }
        Ok(MacosBundleObservation {
            installed,
            staged,
            interrupted,
        })
    }

    fn available_bytes(&mut self) -> Result<u64, InstallPlatformError> {
        Ok(sys::volume(&self.applications)
            .map_err(io("inspect the bundle volume's free space"))?
            .available_bytes)
    }

    fn materialize(
        &mut self,
        source: &UnitRecord,
        unit: &MacosBundleUnit,
        name: &str,
    ) -> Result<MacosBundleMaterialization, InstallPlatformError> {
        require_digest(name)?;
        let root = self
            .staging_root(true)?
            .ok_or_else(|| error("the bundle staging root could not be created"))?;
        if stat_entry(&root, name)?.is_some() {
            return Err(error("the staged bundle copy already exists"));
        }
        let partial = format!("{name}{MACOS_BUNDLE_PARTIAL_SUFFIX}");
        remove_entry(&root, &partial)?;
        remove_entry(&root, &format!("{name}{MACOS_BUNDLE_DISCARD_SUFFIX}"))?;
        rfs::mkdirat(
            &root,
            partial.as_str(),
            Mode::from_raw_mode(SCRATCH_DIRECTORY_MODE as _),
        )
        .map_err(io("create the bundle scratch copy"))?;
        let built = open_directory(&root, &partial).and_then(|scratch| {
            let copied = build_bundle(&scratch, source, unit)?;
            sync_directory(&scratch)?;
            sys::full_sync(&scratch).map_err(io("flush the bundle copy"))?;
            Ok(copied)
        });
        let copied = match built {
            Ok(copied) => copied,
            Err(failure) => {
                return match remove_entry(&root, &partial) {
                    Ok(()) => Err(failure),
                    Err(cleanup) => Err(error(format!(
                        "{failure}; removing the scratch copy also failed: {cleanup}"
                    ))),
                };
            }
        };
        rfs::renameat_with(&root, partial.as_str(), &root, name, RenameFlags::NOREPLACE)
            .map_err(io("publish the verified bundle copy"))?;
        sync_directory(&root)?;
        sys::full_sync(&root).map_err(io("flush the staged bundle copy"))?;
        Ok(copied)
    }

    fn exchange(&mut self, name: &str) -> Result<(), InstallPlatformError> {
        require_digest(name)?;
        let staged = open_directory(&self.require_staging_root()?, name)?;
        sys::exchange(
            &staged,
            MACOS_BUNDLE_NAME,
            &self.applications,
            MACOS_BUNDLE_NAME,
        )
        .map_err(io("swap the staged and installed bundles"))?;
        self.sync_rename(&staged)
    }

    fn install_staged(&mut self, name: &str) -> Result<(), InstallPlatformError> {
        require_digest(name)?;
        let staged = open_directory(&self.require_staging_root()?, name)?;
        rfs::renameat_with(
            &staged,
            MACOS_BUNDLE_NAME,
            &self.applications,
            MACOS_BUNDLE_NAME,
            RenameFlags::NOREPLACE,
        )
        .map_err(io("move the staged bundle into place"))?;
        self.sync_rename(&staged)
    }

    fn withdraw_installed(&mut self, name: &str) -> Result<(), InstallPlatformError> {
        require_digest(name)?;
        let staged = open_directory(&self.require_staging_root()?, name)?;
        rfs::renameat_with(
            &self.applications,
            MACOS_BUNDLE_NAME,
            &staged,
            MACOS_BUNDLE_NAME,
            RenameFlags::NOREPLACE,
        )
        .map_err(io("move the installed bundle back to its staging name"))?;
        self.sync_rename(&staged)
    }

    fn retire_staged(&mut self, name: &str) -> Result<(), InstallPlatformError> {
        require_digest(name)?;
        let root = self.require_staging_root()?;
        let discard = format!("{name}{MACOS_BUNDLE_DISCARD_SUFFIX}");
        remove_entry(&root, &discard)?;
        rfs::renameat_with(&root, name, &root, discard.as_str(), RenameFlags::NOREPLACE)
            .map_err(io("retire the staged bundle copy"))?;
        sync_directory(&root)?;
        sys::full_sync(&root).map_err(io("flush the retired bundle copy"))
    }

    fn discard(&mut self, name: &str) -> Result<(), InstallPlatformError> {
        require_digest(name)?;
        let Some(root) = self.staging_root(false)? else {
            return Ok(());
        };
        let discard = format!("{name}{MACOS_BUNDLE_DISCARD_SUFFIX}");
        remove_entry(&root, &discard)?;
        remove_entry(&root, &format!("{name}{MACOS_BUNDLE_PARTIAL_SUFFIX}"))?;
        if stat_entry(&root, name)?.is_some() {
            rfs::renameat_with(&root, name, &root, discard.as_str(), RenameFlags::NOREPLACE)
                .map_err(io("retire the staged bundle copy"))?;
            sync_directory(&root)?;
            remove_entry(&root, &discard)?;
        }
        sync_directory(&root)?;
        sys::full_sync(&root).map_err(io("flush the bundle removal"))
    }
}

/// Build `Hypercolor.app` inside `scratch` from `unit`, then verify every
/// byte, mode and entry of the result against it.
fn build_bundle(
    scratch: &OwnedFd,
    source: &UnitRecord,
    unit: &MacosBundleUnit,
) -> Result<MacosBundleMaterialization, InstallPlatformError> {
    let mut children: BTreeMap<&str, Vec<&MacosBundleMember>> = BTreeMap::new();
    for member in unit.members() {
        children
            .entry(parent_of(&member.path))
            .or_default()
            .push(member);
    }
    let source_bundle = source
        .directory()
        .open_child_directory(Path::new(MACOS_BUNDLE_NAME))
        .map_err(io_error("open the unit's bundle"))?;
    rfs::mkdirat(
        scratch,
        MACOS_BUNDLE_NAME,
        Mode::from_raw_mode(SCRATCH_DIRECTORY_MODE as _),
    )
    .map_err(io("create the bundle copy"))?;
    let bundle = open_directory(scratch, MACOS_BUNDLE_NAME)?;
    let mut copier = Copier {
        children: &children,
        clone: true,
        result: MacosBundleMaterialization::default(),
    };
    copier.directory(&source_bundle, &bundle, "", 0)?;
    set_mode(&bundle, unit.bundle_mode())?;
    sync_directory(&bundle)?;
    verify_bundle(scratch, unit)?;
    Ok(copier.result)
}

struct Copier<'a> {
    children: &'a BTreeMap<&'a str, Vec<&'a MacosBundleMember>>,
    clone: bool,
    result: MacosBundleMaterialization,
}

impl Copier<'_> {
    fn directory(
        &mut self,
        source: &ReadOnlyDirectoryAuthority,
        destination: &OwnedFd,
        prefix: &str,
        depth: usize,
    ) -> Result<(), InstallPlatformError> {
        if depth > MAX_TREE_DEPTH {
            return Err(error("the bundle exceeds its depth bound"));
        }
        for member in self.children.get(prefix).into_iter().flatten() {
            let name = leaf_of(&member.path);
            match &member.kind {
                MacosBundleMemberKind::Directory { mode } => {
                    rfs::mkdirat(
                        destination,
                        name,
                        Mode::from_raw_mode(SCRATCH_DIRECTORY_MODE as _),
                    )
                    .map_err(io("create a bundle directory"))?;
                    let child = open_directory(destination, name)?;
                    let child_source = source
                        .open_child_directory(Path::new(name))
                        .map_err(io_error("open a unit directory"))?;
                    self.directory(&child_source, &child, &member.path, depth + 1)?;
                    set_mode(&child, *mode)?;
                    sync_directory(&child)?;
                }
                MacosBundleMemberKind::File { mode, size, .. } => {
                    let opened = source
                        .open_regular_file(Path::new(name))
                        .map_err(io_error("open a unit file"))?;
                    if opened.metadata().size() != *size {
                        return Err(error(format!(
                            "unit member {} differs from its manifest size",
                            member.path
                        )));
                    }
                    self.file(opened.file(), destination, name, *mode)?;
                }
            }
        }
        Ok(())
    }

    fn file(
        &mut self,
        source: &File,
        destination: &OwnedFd,
        name: &str,
        mode: u32,
    ) -> Result<(), InstallPlatformError> {
        if self.clone {
            if sys::clone_file(source, destination, name).map_err(io_error("clone a unit file"))? {
                let cloned = rfs::openat(
                    destination,
                    name,
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(io("open a cloned bundle file"))?;
                set_mode(&cloned, mode)?;
                rfs::fsync(&cloned).map_err(io("sync a cloned bundle file"))?;
                self.result.cloned_files += 1;
                return Ok(());
            }
            // This volume pair cannot clone; copy every remaining file.
            self.clone = false;
        }
        let created = rfs::openat(
            destination,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(io("create a bundle file"))?;
        let mut output = File::from(created);
        std::io::copy(&mut &*source, &mut output).map_err(io_error("copy a bundle file"))?;
        set_mode(&output, mode)?;
        // A plain fsync per file; the one full flush before the copy is
        // published covers every file at once.
        rfs::fsync(&output).map_err(io("sync a copied bundle file"))?;
        self.result.copied_files += 1;
        Ok(())
    }
}

/// Prove the copy at `scratch/Hypercolor.app` is exactly `unit`'s bundle:
/// the same entries and no others, each with its recorded type and mode,
/// and every file with its recorded size and SHA-256.
fn verify_bundle(scratch: &OwnedFd, unit: &MacosBundleUnit) -> Result<(), InstallPlatformError> {
    let bundle = open_directory(scratch, MACOS_BUNDLE_NAME)?;
    if mode_of(&rfs::fstat(&bundle).map_err(io("inspect the bundle copy"))?) != unit.bundle_mode() {
        return Err(error("the bundle copy's root mode differs from its unit"));
    }
    let expected = unit
        .members()
        .iter()
        .map(|member| (member.path.as_str(), member))
        .collect::<BTreeMap<_, _>>();
    let mut seen = 0_usize;
    verify_directory(&bundle, "", &expected, &mut seen, 0)?;
    if seen != expected.len() {
        return Err(error("the bundle copy is missing members of its unit"));
    }
    Ok(())
}

fn verify_directory(
    directory: &OwnedFd,
    prefix: &str,
    expected: &BTreeMap<&str, &MacosBundleMember>,
    seen: &mut usize,
    depth: usize,
) -> Result<(), InstallPlatformError> {
    if depth > MAX_TREE_DEPTH {
        return Err(error("the bundle copy exceeds its depth bound"));
    }
    for name in entry_names(directory)? {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let member = expected.get(path.as_str()).ok_or_else(|| {
            error(format!(
                "the bundle copy holds {path}, which its unit does not"
            ))
        })?;
        let stat = rfs::statat(directory, name.as_str(), AtFlags::SYMLINK_NOFOLLOW)
            .map_err(io("inspect a bundle copy entry"))?;
        let kind = FileType::from_raw_mode(stat.st_mode);
        match &member.kind {
            MacosBundleMemberKind::Directory { mode }
                if kind == FileType::Directory && mode_of(&stat) == *mode =>
            {
                let child = open_directory(directory, &name)?;
                verify_directory(&child, &path, expected, seen, depth + 1)?;
            }
            MacosBundleMemberKind::File { mode, size, sha256 }
                if kind == FileType::RegularFile
                    && mode_of(&stat) == *mode
                    && u64::try_from(stat.st_size).ok() == Some(*size) =>
            {
                let opened = rfs::openat(
                    directory,
                    name.as_str(),
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(io("open a bundle copy file"))?;
                if sha256_of(File::from(opened))? != *sha256 {
                    return Err(error(format!(
                        "the bundle copy of {path} differs from its unit"
                    )));
                }
            }
            _ => {
                return Err(error(format!(
                    "the bundle copy of {path} has another type or mode than its unit"
                )));
            }
        }
        *seen += 1;
    }
    Ok(())
}

/// What `name` inside `directory` holds, read as a bundle.
fn bundle_slot(directory: &OwnedFd, name: &str) -> MacosBundleSlot {
    match stat_entry(directory, name) {
        Ok(None) => MacosBundleSlot::Absent,
        Ok(Some(stat)) if FileType::from_raw_mode(stat.st_mode) == FileType::Directory => {
            match bundle_identity(directory, name) {
                Ok(identity) => MacosBundleSlot::Bundle(identity),
                Err(failure) => MacosBundleSlot::Unrecognized(failure.to_string()),
            }
        }
        Ok(Some(_)) => MacosBundleSlot::Unrecognized("not a directory".to_owned()),
        Err(failure) => MacosBundleSlot::Unrecognized(failure.to_string()),
    }
}

fn bundle_identity(
    directory: &OwnedFd,
    name: &str,
) -> Result<MacosBundleIdentity, InstallPlatformError> {
    let bundle = open_directory(directory, name)?;
    let executables = open_directory(&open_directory(&bundle, "Contents")?, "MacOS")?;
    let [app, daemon, cli] = EXECUTABLES.map(|executable| {
        let opened = rfs::openat(
            &executables,
            executable,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io("open a bundle executable"))?;
        let stat = rfs::fstat(&opened).map_err(io("inspect a bundle executable"))?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(error(format!(
                "bundle executable {executable} is not a regular file"
            )));
        }
        let size = u64::try_from(stat.st_size)
            .map_err(|_| error("bundle executable reports a negative size"))?;
        crate::macho_cdhash::thin_macho_cdhash(&mut File::from(opened), size)
    });
    Ok(MacosBundleIdentity {
        app: app?,
        daemon: daemon?,
        cli: cli?,
    })
}

/// Remove `name` and everything below it, if it exists, never following a
/// symlink. Directories the owner cannot empty are opened up first.
fn remove_entry(parent: &OwnedFd, name: &str) -> Result<(), InstallPlatformError> {
    remove_entry_at(parent, name, 0)?;
    sync_directory(parent)
}

fn remove_entry_at(parent: &OwnedFd, name: &str, depth: usize) -> Result<(), InstallPlatformError> {
    if depth > MAX_TREE_DEPTH {
        return Err(error("a staged bundle exceeds its removal depth bound"));
    }
    let Some(stat) = stat_entry(parent, name)? else {
        return Ok(());
    };
    if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
        let directory = open_directory(parent, name)?;
        let mode = mode_of(&stat);
        if mode & 0o700 != 0o700 {
            set_mode(&directory, mode | 0o700)?;
        }
        for child in entry_names(&directory)? {
            remove_entry_at(&directory, &child, depth + 1)?;
        }
        rfs::unlinkat(parent, name, AtFlags::REMOVEDIR).map_err(io("remove a staged directory"))
    } else {
        rfs::unlinkat(parent, name, AtFlags::empty()).map_err(io("remove a staged file"))
    }
}

fn open_directory(parent: &OwnedFd, name: &str) -> Result<OwnedFd, InstallPlatformError> {
    rfs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io("open a bundle directory without following links"))
}

fn stat_entry(parent: &OwnedFd, name: &str) -> Result<Option<Stat>, InstallPlatformError> {
    match rfs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => Ok(Some(stat)),
        Err(Errno::NOENT) => Ok(None),
        Err(source) => Err(io("inspect a bundle entry")(source)),
    }
}

fn entry_names(directory: &OwnedFd) -> Result<Vec<String>, InstallPlatformError> {
    let mut names = Vec::new();
    for entry in rfs::Dir::read_from(directory).map_err(io("list a bundle directory"))? {
        let entry = entry.map_err(io("list a bundle directory"))?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        names.push(
            std::str::from_utf8(name)
                .map_err(|_| error("a bundle entry name is not UTF-8"))?
                .to_owned(),
        );
    }
    names.sort();
    Ok(names)
}

fn set_mode(file: &impl AsFd, mode: u32) -> Result<(), InstallPlatformError> {
    rfs::fchmod(file, Mode::from_raw_mode(mode as _)).map_err(io("set a bundle entry's mode"))
}

fn sync_directory(directory: &OwnedFd) -> Result<(), InstallPlatformError> {
    rfs::fsync(directory).map_err(io("sync a bundle directory"))
}

fn sha256_of(mut file: File) -> Result<String, InstallPlatformError> {
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(io_error("hash a bundle copy file"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

#[allow(
    clippy::useless_conversion,
    reason = "mode_t is u16 on macOS and u32 on Linux"
)]
fn mode_of(stat: &Stat) -> u32 {
    u32::from(stat.st_mode) & 0o7777
}

#[allow(
    clippy::useless_conversion,
    clippy::unnecessary_cast,
    reason = "dev_t is i32 on macOS and u64 on Linux"
)]
fn device(stat: &Stat) -> u64 {
    stat.st_dev as u64
}

fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn leaf_of(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, leaf)| leaf)
}

fn is_digest(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn require_digest(name: &str) -> Result<(), InstallPlatformError> {
    if is_digest(name) {
        Ok(())
    } else {
        Err(error(
            "a staged bundle copy must be named by its unit digest",
        ))
    }
}

fn io(operation: &'static str) -> impl Fn(Errno) -> InstallPlatformError {
    move |source| error(format!("{operation}: {source}"))
}

fn io_error(operation: &'static str) -> impl Fn(std::io::Error) -> InstallPlatformError {
    move |source| error(format!("{operation}: {source}"))
}

/// The system calls macOS provides and the rest of this executor builds on.
///
/// Lib tests also build this executor on Linux, where the same rename
/// semantics exist (`renameat2` with `RENAME_EXCHANGE` and
/// `RENAME_NOREPLACE`); there `clonefile` is absent, so every file is
/// copied, and `fsync` is the strongest flush. Only macOS builds export it.
mod sys {
    use std::fs::File;
    use std::os::fd::AsFd;

    use rustix::fs::{self as rfs, RenameFlags};

    pub(super) struct Volume {
        /// The volume exchanges two directories in one rename.
        pub(super) atomic_swap: bool,
        pub(super) available_bytes: u64,
    }

    /// `renamex_np` with `RENAME_SWAP`, relative to two directory handles.
    pub(super) fn exchange(
        old_directory: &impl AsFd,
        old_name: &str,
        new_directory: &impl AsFd,
        new_name: &str,
    ) -> rustix::io::Result<()> {
        rfs::renameat_with(
            old_directory,
            old_name,
            new_directory,
            new_name,
            RenameFlags::EXCHANGE,
        )
    }

    /// `fclonefileat` without following links or copying ownership.
    /// `Ok(false)` means the volumes cannot clone, so the caller copies.
    #[cfg(target_os = "macos")]
    pub(super) fn clone_file(
        source: &File,
        directory: &impl AsFd,
        name: &str,
    ) -> std::io::Result<bool> {
        use rustix::fs::CloneFlags;
        use rustix::io::Errno;
        match rfs::fclonefileat(
            source,
            directory,
            name,
            CloneFlags::NOFOLLOW | CloneFlags::NOOWNERCOPY,
        ) {
            Ok(()) => Ok(true),
            Err(Errno::XDEV | Errno::NOTSUP | Errno::OPNOTSUPP) => Ok(false),
            Err(source) => Err(source.into()),
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[allow(clippy::unnecessary_wraps, reason = "keeps the macOS signature")]
    pub(super) fn clone_file(
        _source: &File,
        _directory: &impl AsFd,
        _name: &str,
    ) -> std::io::Result<bool> {
        Ok(false)
    }

    /// `fcntl(F_FULLFSYNC)`: the drive writes everything it has buffered.
    #[cfg(target_os = "macos")]
    pub(super) fn full_sync(file: &impl AsFd) -> rustix::io::Result<()> {
        rfs::fcntl_fullfsync(file)
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn full_sync(file: &impl AsFd) -> rustix::io::Result<()> {
        rfs::fsync(file)
    }

    #[cfg(target_os = "macos")]
    pub(super) fn volume(directory: &impl AsFd) -> rustix::io::Result<Volume> {
        let stat = rfs::fstatfs(directory)?;
        let kind = stat
            .f_fstypename
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect::<Vec<_>>();
        Ok(Volume {
            atomic_swap: kind == b"apfs",
            available_bytes: stat.f_bavail.saturating_mul(u64::from(stat.f_bsize)),
        })
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn volume(directory: &impl AsFd) -> rustix::io::Result<Volume> {
        let stat = rfs::fstatvfs(directory)?;
        Ok(Volume {
            atomic_swap: true,
            available_bytes: stat.f_bavail.saturating_mul(stat.f_frsize),
        })
    }
}
