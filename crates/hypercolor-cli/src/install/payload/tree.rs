use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Cursor, Read};
use std::os::unix::fs::{FileExt as _, MetadataExt as _};
use std::path::Path;

use hypercolor_platform_fs::{
    DirectoryAuthority, DirectoryEntryKind, DirectoryEntryMetadata, OpenedRegularFile,
    PrivateStagingDirectory, ReadOnlyDirectoryAuthority,
};
use sha2::{Digest as _, Sha256};

use super::manifest::{
    MAX_RELEASE_MANIFEST_BYTES, ValidatedManifest, ValidatedMember, hex_digest, split_parent,
};
use super::{MANIFEST_NAME, ReleasePayloadError};

const MANIFEST_SOURCE_MODE: u32 = 0o644;
const MANIFEST_INSTALLED_MODE: u32 = 0o444;
const UNIT_ROOT_MODE: u32 = 0o555;

pub(super) fn read_manifest_bytes(
    source: &ReadOnlyDirectoryAuthority,
) -> Result<Vec<u8>, ReleasePayloadError> {
    read_manifest_with_mode(source, MANIFEST_SOURCE_MODE)
}

pub(super) fn read_installed_manifest_bytes(
    root: &DirectoryAuthority,
) -> Result<Vec<u8>, ReleasePayloadError> {
    read_manifest_with_mode(root, MANIFEST_INSTALLED_MODE)
}

#[cfg(target_os = "macos")]
pub(super) fn read_retained_manifest_bytes(
    root: &ReadOnlyDirectoryAuthority,
) -> Result<Vec<u8>, ReleasePayloadError> {
    read_manifest_with_mode(root, MANIFEST_INSTALLED_MODE)
}

fn read_manifest_with_mode<T: ReadTree>(
    directory: &T,
    expected_mode: u32,
) -> Result<Vec<u8>, ReleasePayloadError> {
    let opened = directory
        .open_regular_file(Path::new(MANIFEST_NAME))
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "open manifest.json",
            source,
        })?;
    let metadata = opened.metadata();
    if metadata.mode() != expected_mode {
        return Err(ReleasePayloadError::InvalidManifest(format!(
            "manifest.json mode must be {expected_mode:#o}"
        )));
    }
    if metadata.size() > MAX_RELEASE_MANIFEST_BYTES as u64 {
        return Err(ReleasePayloadError::ManifestTooLarge {
            limit: MAX_RELEASE_MANIFEST_BYTES,
        });
    }
    read_opened_exact(&opened, metadata.size(), "read manifest.json")
}

pub(super) fn populate_staging(
    staging: &PrivateStagingDirectory,
    source: &ReadOnlyDirectoryAuthority,
    manifest: &ValidatedManifest,
) -> Result<(), ReleasePayloadError> {
    let root = staging.directory();
    let manifest_size = u64::try_from(manifest.bytes.len()).map_err(|_| {
        ReleasePayloadError::InvalidManifest("manifest length does not fit u64".to_owned())
    })?;
    root.create_regular_file(
        Path::new(MANIFEST_NAME),
        MANIFEST_INSTALLED_MODE,
        manifest_size,
        &mut Cursor::new(&manifest.bytes),
    )
    .map_err(|source| ReleasePayloadError::Filesystem {
        operation: "stage manifest.json",
        source,
    })?;

    let mut directories: Vec<_> = manifest
        .members
        .iter()
        .filter_map(|(path, member)| member.is_directory().then_some(path.as_str()))
        .collect();
    directories.sort_by_key(|path| path_depth(path));
    for path in directories {
        let (parent, name) = split_parent(path);
        with_directory(root, parent, |directory| {
            directory
                .create_child_directory(Path::new(name))
                .map(|_| ())
        })
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "create a staged release directory",
            source,
        })?;
    }

    for (path, member) in &manifest.members {
        let ValidatedMember::File {
            source_mode,
            size,
            sha256,
        } = member
        else {
            continue;
        };
        let (parent, name) = split_parent(path);
        let mut opened = with_read_directory(source, parent, |directory| {
            directory.open_regular_file(Path::new(name))
        })
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "open a release member for staging",
            source,
        })?;
        require_file_metadata(
            opened.metadata(),
            *source_mode,
            *size,
            path,
            TreeMode::Source,
        )?;
        let mut hashing = HashingReader::new(opened.file_mut());
        with_directory(root, parent, |directory| {
            directory
                .create_regular_file(
                    Path::new(name),
                    installed_mode(*source_mode),
                    *size,
                    &mut hashing,
                )
                .map(|_| ())
        })
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "copy a release member into private staging",
            source,
        })?;
        if hashing.finish_hex() != *sha256 {
            return Err(ReleasePayloadError::InvalidSource(format!(
                "release member {path} changed while it was copied"
            )));
        }
        require_std_file_metadata(
            opened
                .file()
                .metadata()
                .map_err(|source| ReleasePayloadError::Filesystem {
                    operation: "reinspect a copied source member",
                    source,
                })?,
            opened.metadata(),
            path,
            TreeMode::Source,
        )?;
    }
    Ok(())
}

pub(super) fn finalize_staging(
    staging: &PrivateStagingDirectory,
    manifest: &ValidatedManifest,
) -> Result<(), ReleasePayloadError> {
    let mut directories: Vec<_> = manifest
        .members
        .iter()
        .filter_map(|(path, member)| match member {
            ValidatedMember::Directory { source_mode } => Some((path.as_str(), *source_mode)),
            ValidatedMember::File { .. } => None,
        })
        .collect();
    directories.sort_by_key(|(path, _)| std::cmp::Reverse(path_depth(path)));
    for (path, source_mode) in directories {
        with_directory(staging.directory(), path, |directory| {
            directory.set_mode(installed_mode(source_mode))
        })
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "finalize a staged directory mode",
            source,
        })?;
    }
    staging
        .directory()
        .set_mode(UNIT_ROOT_MODE)
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "finalize the staged unit root",
            source,
        })
}

pub(super) fn bind_candidate_executable(
    source: &ReadOnlyDirectoryAuthority,
    candidate: &File,
    manifest: &ValidatedManifest,
) -> Result<(), ReleasePayloadError> {
    let member = manifest
        .members
        .get("bin/hypercolor")
        .ok_or_else(|| ReleasePayloadError::InvalidManifest("missing bin/hypercolor".to_owned()))?;
    let ValidatedMember::File { size, sha256, .. } = member else {
        return Err(ReleasePayloadError::InvalidManifest(
            "bin/hypercolor must be a regular file".to_owned(),
        ));
    };
    let bin = source
        .open_child_directory(Path::new("bin"))
        .and_then(|directory| directory.open_regular_file(Path::new("hypercolor")))
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "open source bin/hypercolor",
            source,
        })?;
    let candidate_metadata =
        candidate
            .metadata()
            .map_err(|source| ReleasePayloadError::Filesystem {
                operation: "inspect the already-open candidate executable",
                source,
            })?;
    if !candidate_metadata.file_type().is_file()
        || candidate_metadata.dev() != bin.metadata().device()
        || candidate_metadata.ino() != bin.metadata().inode()
        || candidate_metadata.len() != *size
    {
        return Err(ReleasePayloadError::CandidateMismatch);
    }
    let source_digest = hash_file_exact(bin.file(), *size, "hash source bin/hypercolor")?;
    let candidate_digest = hash_file_exact(candidate, *size, "hash candidate executable")?;
    if source_digest != *sha256 || candidate_digest != *sha256 {
        return Err(ReleasePayloadError::CandidateMismatch);
    }
    let after = candidate
        .metadata()
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "reinspect the already-open candidate executable",
            source,
        })?;
    if after.dev() != candidate_metadata.dev()
        || after.ino() != candidate_metadata.ino()
        || after.len() != candidate_metadata.len()
    {
        return Err(ReleasePayloadError::CandidateMismatch);
    }
    Ok(())
}

pub(super) fn validate_source(
    root: &ReadOnlyDirectoryAuthority,
    manifest: &ValidatedManifest,
) -> Result<(), ReleasePayloadError> {
    validate_tree(root, manifest, TreeMode::Source, EnumerationMode::Existing)
}

pub(super) fn validate_installed(
    root: &DirectoryAuthority,
    manifest: &ValidatedManifest,
) -> Result<(), ReleasePayloadError> {
    validate_tree(
        root,
        manifest,
        TreeMode::Installed,
        EnumerationMode::Existing,
    )
}

#[cfg(target_os = "macos")]
pub(super) fn validate_retained(
    root: &ReadOnlyDirectoryAuthority,
    manifest: &ValidatedManifest,
) -> Result<(), ReleasePayloadError> {
    validate_tree(
        root,
        manifest,
        TreeMode::Installed,
        EnumerationMode::Retained,
    )
}

#[derive(Debug, Clone, Copy)]
enum TreeMode {
    Source,
    Installed,
}

#[derive(Debug, Clone, Copy)]
enum EnumerationMode {
    Existing,
    #[cfg(target_os = "macos")]
    Retained,
}

fn validate_tree<T: ReadTree>(
    root: &T,
    manifest: &ValidatedManifest,
    mode: TreeMode,
    enumeration: EnumerationMode,
) -> Result<(), ReleasePayloadError> {
    let root_metadata = root
        .metadata()
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "inspect a release tree root",
            source,
        })?;
    if root_metadata.kind() != DirectoryEntryKind::Directory
        || matches!(mode, TreeMode::Installed) && root_metadata.mode() != UNIT_ROOT_MODE
    {
        return Err(tree_error(mode, "release tree root metadata is invalid"));
    }
    validate_directory(root, "", manifest, mode, enumeration)
}

fn validate_directory<T: ReadTree>(
    directory: &T,
    prefix: &str,
    manifest: &ValidatedManifest,
    mode: TreeMode,
    enumeration: EnumerationMode,
) -> Result<(), ReleasePayloadError> {
    let actual_names = match enumeration {
        EnumerationMode::Existing => directory.entries(),
        #[cfg(target_os = "macos")]
        EnumerationMode::Retained => directory.child_names(),
    }
    .map_err(|source| ReleasePayloadError::Filesystem {
        operation: "enumerate a release tree directory",
        source,
    })?;
    let mut actual = BTreeSet::new();
    for name in actual_names {
        actual.insert(os_string_to_manifest_name(name, mode)?);
    }
    let mut expected: BTreeSet<String> = manifest
        .children
        .get(prefix)
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    if prefix.is_empty() {
        expected.insert(MANIFEST_NAME.to_owned());
    }
    if actual != expected {
        return Err(tree_error(
            mode,
            format!(
                "release member inventory mismatch below {prefix:?}: expected {expected:?}, found {actual:?}"
            ),
        ));
    }

    for name in expected {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if path == MANIFEST_NAME {
            validate_manifest_file(directory, manifest, mode)?;
            continue;
        }
        let member = manifest.members.get(&path).ok_or_else(|| {
            ReleasePayloadError::InvalidManifest(format!("missing validated member {path}"))
        })?;
        let metadata = directory
            .entry_metadata(Path::new(&name))
            .map_err(|source| ReleasePayloadError::Filesystem {
                operation: "inspect a release tree member",
                source,
            })?
            .ok_or_else(|| tree_error(mode, format!("release member {path} disappeared")))?;
        match member {
            ValidatedMember::Directory { source_mode } => {
                let expected_mode = mode.select_mode(*source_mode);
                if metadata.kind() != DirectoryEntryKind::Directory
                    || metadata.mode() != expected_mode
                {
                    return Err(tree_error(
                        mode,
                        format!("directory metadata mismatch for {path}"),
                    ));
                }
                let child = directory
                    .open_child_directory(Path::new(&name))
                    .map_err(|source| ReleasePayloadError::Filesystem {
                        operation: "open a release tree directory",
                        source,
                    })?;
                let opened_metadata =
                    child
                        .metadata()
                        .map_err(|source| ReleasePayloadError::Filesystem {
                            operation: "inspect an opened release directory",
                            source,
                        })?;
                require_same_identity(metadata, opened_metadata, &path, mode)?;
                if opened_metadata.mode() != expected_mode {
                    return Err(tree_error(
                        mode,
                        format!("directory mode changed for {path}"),
                    ));
                }
                validate_directory(&child, &path, manifest, mode, enumeration)?;
                require_same_metadata(
                    opened_metadata,
                    child
                        .metadata()
                        .map_err(|source| ReleasePayloadError::Filesystem {
                            operation: "reinspect an opened release directory",
                            source,
                        })?,
                    &path,
                    mode,
                )?;
                let named_after = directory
                    .entry_metadata(Path::new(&name))
                    .map_err(|source| ReleasePayloadError::Filesystem {
                        operation: "reinspect a release directory name",
                        source,
                    })?
                    .ok_or_else(|| tree_error(mode, format!("directory {path} disappeared")))?;
                require_same_metadata(metadata, named_after, &path, mode)?;
            }
            ValidatedMember::File {
                source_mode,
                size,
                sha256,
            } => validate_member_file(
                directory,
                &name,
                &path,
                mode.select_mode(*source_mode),
                *size,
                sha256,
                mode,
            )?,
        }
    }
    Ok(())
}

fn validate_member_file<T: ReadTree>(
    directory: &T,
    name: &str,
    path: &str,
    expected_mode: u32,
    expected_size: u64,
    expected_sha256: &str,
    tree_mode: TreeMode,
) -> Result<(), ReleasePayloadError> {
    let metadata = directory
        .entry_metadata(Path::new(name))
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "inspect a release file",
            source,
        })?
        .ok_or_else(|| tree_error(tree_mode, format!("release member {path} disappeared")))?;
    require_file_metadata(metadata, expected_mode, expected_size, path, tree_mode)?;
    let opened = directory
        .open_regular_file(Path::new(name))
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "open a release tree member",
            source,
        })?;
    require_same_metadata(metadata, opened.metadata(), path, tree_mode)?;
    let digest = hash_file_exact(opened.file(), expected_size, "hash a release tree member")?;
    if digest != expected_sha256 {
        return Err(tree_error(
            tree_mode,
            format!("SHA-256 mismatch for {path}"),
        ));
    }
    require_std_file_metadata(
        opened
            .file()
            .metadata()
            .map_err(|source| ReleasePayloadError::Filesystem {
                operation: "reinspect an opened release member",
                source,
            })?,
        opened.metadata(),
        path,
        tree_mode,
    )?;
    let named_after = directory
        .entry_metadata(Path::new(name))
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "reinspect a release member name",
            source,
        })?
        .ok_or_else(|| tree_error(tree_mode, format!("release member {path} disappeared")))?;
    require_same_metadata(opened.metadata(), named_after, path, tree_mode)
}

fn validate_manifest_file<T: ReadTree>(
    directory: &T,
    manifest: &ValidatedManifest,
    mode: TreeMode,
) -> Result<(), ReleasePayloadError> {
    let expected_mode = match mode {
        TreeMode::Source => MANIFEST_SOURCE_MODE,
        TreeMode::Installed => MANIFEST_INSTALLED_MODE,
    };
    let expected_size = u64::try_from(manifest.bytes.len()).map_err(|_| {
        ReleasePayloadError::InvalidManifest("manifest length does not fit u64".to_owned())
    })?;
    let metadata = directory
        .entry_metadata(Path::new(MANIFEST_NAME))
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "inspect manifest.json in a release tree",
            source,
        })?
        .ok_or_else(|| tree_error(mode, "manifest.json disappeared"))?;
    require_file_metadata(metadata, expected_mode, expected_size, MANIFEST_NAME, mode)?;
    let opened = directory
        .open_regular_file(Path::new(MANIFEST_NAME))
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "open manifest.json in a release tree",
            source,
        })?;
    require_same_metadata(metadata, opened.metadata(), MANIFEST_NAME, mode)?;
    let bytes = read_opened_exact(
        &opened,
        expected_size,
        "read manifest.json from a release tree",
    )?;
    if bytes != manifest.bytes {
        return Err(tree_error(mode, "manifest.json bytes changed"));
    }
    let named_after = directory
        .entry_metadata(Path::new(MANIFEST_NAME))
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "reinspect manifest.json name",
            source,
        })?
        .ok_or_else(|| tree_error(mode, "manifest.json disappeared"))?;
    require_same_metadata(opened.metadata(), named_after, MANIFEST_NAME, mode)
}

trait ReadTree: Sized {
    fn metadata(&self) -> io::Result<DirectoryEntryMetadata>;
    fn open_child_directory(&self, name: &Path) -> io::Result<Self>;
    fn entries(&self) -> io::Result<Vec<OsString>>;
    #[cfg(target_os = "macos")]
    fn child_names(&self) -> io::Result<Vec<OsString>>;
    fn entry_metadata(&self, name: &Path) -> io::Result<Option<DirectoryEntryMetadata>>;
    fn open_regular_file(&self, name: &Path) -> io::Result<OpenedRegularFile>;
}

macro_rules! impl_read_tree {
    ($authority:ty) => {
        impl ReadTree for $authority {
            fn metadata(&self) -> io::Result<DirectoryEntryMetadata> {
                Self::metadata(self)
            }

            fn open_child_directory(&self, name: &Path) -> io::Result<Self> {
                Self::open_child_directory(self, name)
            }

            fn entries(&self) -> io::Result<Vec<OsString>> {
                Self::entries(self)
            }

            #[cfg(target_os = "macos")]
            fn child_names(&self) -> io::Result<Vec<OsString>> {
                Self::child_names(self)
            }

            fn entry_metadata(&self, name: &Path) -> io::Result<Option<DirectoryEntryMetadata>> {
                Self::entry_metadata(self, name)
            }

            fn open_regular_file(&self, name: &Path) -> io::Result<OpenedRegularFile> {
                Self::open_regular_file(self, name)
            }
        }
    };
}

impl_read_tree!(ReadOnlyDirectoryAuthority);
impl_read_tree!(DirectoryAuthority);

fn with_directory<T>(
    root: &DirectoryAuthority,
    path: &str,
    operation: impl FnOnce(&DirectoryAuthority) -> io::Result<T>,
) -> io::Result<T> {
    let mut current: Option<DirectoryAuthority> = None;
    for component in path_components(path) {
        let next = match current.as_ref() {
            Some(directory) => directory.open_child_directory(Path::new(component))?,
            None => root.open_child_directory(Path::new(component))?,
        };
        current = Some(next);
    }
    operation(current.as_ref().unwrap_or(root))
}

fn with_read_directory<T>(
    root: &ReadOnlyDirectoryAuthority,
    path: &str,
    operation: impl FnOnce(&ReadOnlyDirectoryAuthority) -> io::Result<T>,
) -> io::Result<T> {
    let mut current: Option<ReadOnlyDirectoryAuthority> = None;
    for component in path_components(path) {
        let next = match current.as_ref() {
            Some(directory) => directory.open_child_directory(Path::new(component))?,
            None => root.open_child_directory(Path::new(component))?,
        };
        current = Some(next);
    }
    operation(current.as_ref().unwrap_or(root))
}

fn path_components(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|component| !component.is_empty())
}

fn path_depth(path: &str) -> usize {
    path.bytes().filter(|byte| *byte == b'/').count() + 1
}

fn installed_mode(source_mode: u32) -> u32 {
    source_mode & !0o222
}

impl TreeMode {
    fn select_mode(self, source_mode: u32) -> u32 {
        match self {
            Self::Source => source_mode,
            Self::Installed => installed_mode(source_mode),
        }
    }
}

fn tree_error(mode: TreeMode, message: impl Into<String>) -> ReleasePayloadError {
    match mode {
        TreeMode::Source => ReleasePayloadError::InvalidSource(message.into()),
        TreeMode::Installed => ReleasePayloadError::InvalidUnit(message.into()),
    }
}

fn os_string_to_manifest_name(
    name: OsString,
    mode: TreeMode,
) -> Result<String, ReleasePayloadError> {
    name.into_string()
        .map_err(|_| tree_error(mode, "release tree contains a non-UTF-8 entry name"))
}

fn require_file_metadata(
    metadata: DirectoryEntryMetadata,
    mode: u32,
    size: u64,
    path: &str,
    tree_mode: TreeMode,
) -> Result<(), ReleasePayloadError> {
    if metadata.kind() != DirectoryEntryKind::RegularFile
        || metadata.link_count() != 1
        || metadata.mode() != mode
        || metadata.size() != size
    {
        return Err(tree_error(
            tree_mode,
            format!("file metadata mismatch for {path}"),
        ));
    }
    Ok(())
}

fn require_same_identity(
    expected: DirectoryEntryMetadata,
    actual: DirectoryEntryMetadata,
    path: &str,
    mode: TreeMode,
) -> Result<(), ReleasePayloadError> {
    if expected.kind() != actual.kind()
        || expected.device() != actual.device()
        || expected.inode() != actual.inode()
    {
        return Err(tree_error(
            mode,
            format!("entry identity changed for {path}"),
        ));
    }
    Ok(())
}

fn require_same_metadata(
    expected: DirectoryEntryMetadata,
    actual: DirectoryEntryMetadata,
    path: &str,
    mode: TreeMode,
) -> Result<(), ReleasePayloadError> {
    if expected != actual {
        return Err(tree_error(
            mode,
            format!("entry metadata changed for {path}"),
        ));
    }
    Ok(())
}

fn require_std_file_metadata(
    actual: std::fs::Metadata,
    expected: DirectoryEntryMetadata,
    path: &str,
    mode: TreeMode,
) -> Result<(), ReleasePayloadError> {
    if !actual.file_type().is_file()
        || actual.mode() & 0o7777 != expected.mode()
        || actual.len() != expected.size()
        || actual.nlink() != expected.link_count()
        || actual.dev() != expected.device()
        || actual.ino() != expected.inode()
    {
        return Err(tree_error(
            mode,
            format!("opened file metadata changed for {path}"),
        ));
    }
    Ok(())
}

fn read_opened_exact(
    opened: &OpenedRegularFile,
    size: u64,
    operation: &'static str,
) -> Result<Vec<u8>, ReleasePayloadError> {
    let capacity = usize::try_from(size).map_err(|_| {
        ReleasePayloadError::InvalidManifest("file length does not fit memory bounds".to_owned())
    })?;
    let mut bytes = vec![0_u8; capacity];
    read_at_exact(opened.file(), &mut bytes, operation)?;
    let mut extra = [0_u8; 1];
    let extra_count = opened
        .file()
        .read_at(&mut extra, size)
        .map_err(|source| ReleasePayloadError::Filesystem { operation, source })?;
    if extra_count != 0 {
        return Err(ReleasePayloadError::InvalidSource(format!(
            "{operation} exceeded its declared size"
        )));
    }
    Ok(bytes)
}

fn hash_file_exact(
    file: &File,
    size: u64,
    operation: &'static str,
) -> Result<String, ReleasePayloadError> {
    let mut hasher = Sha256::new();
    let mut offset = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    while offset < size {
        let remaining = size - offset;
        let length = usize::try_from(remaining.min(buffer.len() as u64)).map_err(|_| {
            ReleasePayloadError::InvalidSource("release member size is not addressable".to_owned())
        })?;
        let count = file
            .read_at(&mut buffer[..length], offset)
            .map_err(|source| ReleasePayloadError::Filesystem { operation, source })?;
        if count == 0 {
            return Err(ReleasePayloadError::InvalidSource(format!(
                "{operation} ended before its declared size"
            )));
        }
        hasher.update(&buffer[..count]);
        offset += u64::try_from(count).map_err(|_| {
            ReleasePayloadError::InvalidSource("read length does not fit u64".to_owned())
        })?;
    }
    let mut extra = [0_u8; 1];
    if file
        .read_at(&mut extra, size)
        .map_err(|source| ReleasePayloadError::Filesystem { operation, source })?
        != 0
    {
        return Err(ReleasePayloadError::InvalidSource(format!(
            "{operation} exceeded its declared size"
        )));
    }
    Ok(hex_digest(&hasher.finalize()))
}

fn read_at_exact(
    file: &File,
    mut destination: &mut [u8],
    operation: &'static str,
) -> Result<(), ReleasePayloadError> {
    let mut offset = 0_u64;
    while !destination.is_empty() {
        let count = file
            .read_at(destination, offset)
            .map_err(|source| ReleasePayloadError::Filesystem { operation, source })?;
        if count == 0 {
            return Err(ReleasePayloadError::InvalidSource(format!(
                "{operation} ended before its declared size"
            )));
        }
        offset += u64::try_from(count).map_err(|_| {
            ReleasePayloadError::InvalidSource("read length does not fit u64".to_owned())
        })?;
        destination = &mut destination[count..];
    }
    Ok(())
}

struct HashingReader<'a> {
    source: &'a mut File,
    hasher: Sha256,
}

impl<'a> HashingReader<'a> {
    fn new(source: &'a mut File) -> Self {
        Self {
            source,
            hasher: Sha256::new(),
        }
    }

    fn finish_hex(self) -> String {
        hex_digest(&self.hasher.finalize())
    }
}

impl Read for HashingReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.source.read(buffer)?;
        self.hasher.update(&buffer[..count]);
        Ok(count)
    }
}
