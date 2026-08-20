use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Write as _};
#[cfg(target_os = "linux")]
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path};

#[cfg(not(target_os = "linux"))]
use rustix::fs::Dir;
#[cfg(target_os = "linux")]
use rustix::fs::RawDir;
use rustix::fs::{AtFlags, FileType, Mode, OFlags, fchmod, fstat, openat, statat, unlinkat};
use rustix::io::Errno;

#[cfg(target_os = "linux")]
use super::DIRECTORY_BUFFER_BYTES;
use super::{
    ALL_PERMISSION_BITS, DirectoryEntryKind, DirectoryEntryMetadata, OpenedRegularFile,
    PERMISSION_BITS, PRIVATE_DIRECTORY_MODE, SECRET_FILE_MODE,
};

type OpenedAbsoluteDirectory = (File, Vec<(File, OsString, DirectoryEntryMetadata)>);
pub(super) fn duplicate_directory(directory: &File) -> io::Result<File> {
    openat(
        directory,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(io::Error::from)
}

pub(super) fn open_absolute_directory_components(
    path: &Path,
) -> io::Result<OpenedAbsoluteDirectory> {
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "public directory path must be absolute",
        ));
    }
    let root = openat(
        rustix::fs::CWD,
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(io::Error::from)?;
    let mut current = root;
    let mut ancestry = Vec::new();
    for component in components {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "public directory path must contain only normal absolute components",
            ));
        };
        let child = open_directory_at(&current, name)?;
        let expected = metadata_for_file(&child)?;
        let named = entry_metadata_at(&current, name)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "public directory component disappeared during acquisition",
            )
        })?;
        require_same_entry(
            expected,
            named,
            "public directory component changed during acquisition",
        )?;
        ancestry.push((current, name.to_os_string(), expected));
        current = child;
    }
    if ancestry.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to govern the filesystem root as a public directory",
        ));
    }
    Ok((current, ancestry))
}

pub(super) fn open_directory_at(directory: &File, name: &OsStr) -> io::Result<File> {
    let opened = openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(io::Error::from)?;
    let metadata = metadata_for_file(&opened)?;
    if metadata.kind != DirectoryEntryKind::Directory || metadata.mode & !PERMISSION_BITS != 0 {
        return Err(unsafe_entry(
            "entry is not a directory with ordinary permission bits",
        ));
    }
    Ok(opened)
}

pub(super) fn open_regular_file_at(
    directory: &File,
    name: &OsStr,
) -> io::Result<OpenedRegularFile> {
    let file = openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(io::Error::from)?;
    let metadata = metadata_for_file(&file)?;
    if metadata.kind != DirectoryEntryKind::RegularFile
        || metadata.link_count != 1
        || metadata.mode & !PERMISSION_BITS != 0
    {
        return Err(unsafe_entry("entry is not a single-link regular file"));
    }
    Ok(OpenedRegularFile { file, metadata })
}

pub(super) fn entry_metadata_at(
    directory: &File,
    name: &OsStr,
) -> io::Result<Option<DirectoryEntryMetadata>> {
    match statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(metadata) => metadata_from_stat(&metadata).map(Some),
        Err(Errno::NOENT) => Ok(None),
        Err(error) => Err(io::Error::from(error)),
    }
}

pub(super) fn metadata_for_file(file: &File) -> io::Result<DirectoryEntryMetadata> {
    fstat(file)
        .map_err(io::Error::from)
        .and_then(|metadata| metadata_from_stat(&metadata))
}

fn metadata_from_stat(metadata: &rustix::fs::Stat) -> io::Result<DirectoryEntryMetadata> {
    let file_type = FileType::from_raw_mode(metadata.st_mode);
    let kind = if file_type.is_file() {
        DirectoryEntryKind::RegularFile
    } else if file_type.is_dir() {
        DirectoryEntryKind::Directory
    } else if file_type.is_symlink() {
        DirectoryEntryKind::SymbolicLink
    } else {
        DirectoryEntryKind::Special
    };
    Ok(DirectoryEntryMetadata {
        kind,
        mode: widen_to_u32(metadata.st_mode) & ALL_PERMISSION_BITS,
        size: checked_to_u64(metadata.st_size, "negative entry size")?,
        link_count: widen_to_u64(metadata.st_nlink),
        device: checked_to_u64(metadata.st_dev, "negative device number")?,
        inode: widen_to_u64(metadata.st_ino),
    })
}

fn widen_to_u32<T: Into<u32>>(value: T) -> u32 {
    value.into()
}

fn widen_to_u64<T: Into<u64>>(value: T) -> u64 {
    value.into()
}

fn checked_to_u64<T: TryInto<u64>>(value: T, message: &'static str) -> io::Result<u64> {
    value
        .try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, message))
}

#[cfg(target_os = "linux")]
pub(super) fn directory_entries(directory: &File) -> io::Result<Vec<OsString>> {
    let iteration = duplicate_directory(directory)?;
    let mut buffer = Vec::<u8>::with_capacity(DIRECTORY_BUFFER_BYTES);
    let mut names = Vec::new();
    loop {
        let mut resize = false;
        {
            let mut entries = RawDir::new(&iteration, buffer.spare_capacity_mut());
            while let Some(entry) = entries.next() {
                match entry {
                    Ok(entry) => push_directory_entry(entry.file_name().to_bytes(), &mut names)?,
                    Err(Errno::INVAL) => {
                        resize = true;
                        break;
                    }
                    Err(error) => return Err(io::Error::from(error)),
                }
            }
        }
        if !resize {
            break;
        }
        let additional = buffer.capacity().max(DIRECTORY_BUFFER_BYTES);
        buffer.reserve(additional);
    }
    names.sort_by(|left, right| left.as_encoded_bytes().cmp(right.as_encoded_bytes()));
    Ok(names)
}

#[cfg(target_os = "linux")]
pub(super) fn directory_is_empty(directory: &File) -> io::Result<bool> {
    let iteration = duplicate_directory(directory)?;
    let mut buffer = [MaybeUninit::<u8>::uninit(); DIRECTORY_BUFFER_BYTES];
    let mut entries = RawDir::new(&iteration, &mut buffer);
    while let Some(entry) = entries.next() {
        match entry {
            Ok(entry) if matches!(entry.file_name().to_bytes(), b"." | b"..") => {}
            Ok(_) => return Ok(false),
            Err(Errno::INVAL) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "directory entry exceeds the fixed proof buffer",
                ));
            }
            Err(error) => return Err(io::Error::from(error)),
        }
    }
    Ok(true)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn directory_entries(directory: &File) -> io::Result<Vec<OsString>> {
    let mut entries = Dir::read_from(directory).map_err(io::Error::from)?;
    let mut names = Vec::new();
    while let Some(entry) = entries.read() {
        let entry = entry.map_err(io::Error::from)?;
        push_directory_entry(entry.file_name().to_bytes(), &mut names)?;
    }
    names.sort_by(|left, right| left.as_encoded_bytes().cmp(right.as_encoded_bytes()));
    Ok(names)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn directory_is_empty(directory: &File) -> io::Result<bool> {
    let mut entries = Dir::read_from(directory).map_err(io::Error::from)?;
    while let Some(entry) = entries.read() {
        let entry = entry.map_err(io::Error::from)?;
        if !matches!(entry.file_name().to_bytes(), b"." | b"..") {
            return Ok(false);
        }
    }
    Ok(true)
}

fn push_directory_entry(bytes: &[u8], names: &mut Vec<OsString>) -> io::Result<()> {
    if matches!(bytes, b"." | b"..") {
        return Ok(());
    }
    let name = OsString::from_vec(bytes.to_vec());
    entry_name(Path::new(&name), "enumerated entry name")?;
    names.push(name);
    Ok(())
}

pub(super) fn remove_directory_tree(
    parent: &File,
    name: &OsStr,
    expected: DirectoryEntryMetadata,
) -> io::Result<()> {
    let directory = open_directory_at(parent, name)?;
    let opened_metadata = metadata_for_file(&directory)?;
    require_same_entry(expected, opened_metadata, "private staging source changed")?;
    set_exact_mode(&directory, PRIVATE_DIRECTORY_MODE)?;
    directory.sync_all()?;
    for child in directory_entries(&directory)? {
        let child_name = child.as_os_str();
        let metadata = entry_metadata_at(&directory, child_name)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "private tree member disappeared during cleanup",
            )
        })?;
        match metadata.kind {
            DirectoryEntryKind::Directory => {
                remove_directory_tree(&directory, child_name, metadata)?;
            }
            DirectoryEntryKind::RegularFile if metadata.link_count == 1 => {
                let opened = open_regular_file_at(&directory, child_name)?;
                require_same_entry(
                    metadata,
                    opened.metadata,
                    "private tree file changed during cleanup",
                )?;
                let current = entry_metadata_at(&directory, child_name)?.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        "private tree file disappeared during cleanup",
                    )
                })?;
                require_same_entry(
                    opened.metadata,
                    current,
                    "private tree file changed during cleanup",
                )?;
                drop(opened);
                unlinkat(&directory, child_name, AtFlags::empty()).map_err(io::Error::from)?;
            }
            DirectoryEntryKind::RegularFile => {
                return Err(unsafe_entry(
                    "private tree contains a multiply linked regular file",
                ));
            }
            DirectoryEntryKind::SymbolicLink => {
                return Err(unsafe_entry("private tree contains a symbolic link"));
            }
            DirectoryEntryKind::Special => {
                return Err(unsafe_entry("private tree contains a special file"));
            }
        }
    }
    directory.sync_all()?;
    let current_metadata = entry_metadata_at(parent, name)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "private tree root disappeared during cleanup",
        )
    })?;
    require_same_entry(
        opened_metadata,
        current_metadata,
        "private tree root changed during cleanup",
    )?;
    unlinkat(parent, name, AtFlags::REMOVEDIR).map_err(io::Error::from)?;
    parent.sync_all()
}

pub(super) fn validate_mode(mode: u32) -> io::Result<()> {
    if mode & !PERMISSION_BITS != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "mode must contain only permission bits",
        ));
    }
    Ok(())
}

pub(super) fn copy_exact(
    source: &mut impl Read,
    destination: &mut File,
    expected_size: u64,
) -> io::Result<()> {
    let copied = {
        let mut limited = source.take(expected_size);
        io::copy(&mut limited, destination)?
    };
    if copied != expected_size {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "source ended before the expected size",
        ));
    }
    let mut extra = [0_u8; 1];
    if source.read(&mut extra)? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source exceeds the expected size",
        ));
    }
    Ok(())
}

pub(super) fn require_same_entry(
    expected: DirectoryEntryMetadata,
    actual: DirectoryEntryMetadata,
    message: &'static str,
) -> io::Result<()> {
    if expected.kind != actual.kind
        || expected.device != actual.device
        || expected.inode != actual.inode
    {
        return Err(unsafe_entry(message));
    }
    Ok(())
}

pub(super) fn set_exact_mode(file: &File, mode: u32) -> io::Result<()> {
    fchmod(file, rustix_mode(mode)?).map_err(io::Error::from)
}

pub(super) fn rustix_mode(mode: u32) -> io::Result<Mode> {
    let raw_mode = rustix::fs::RawMode::try_from(mode).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "mode does not fit the platform mode type",
        )
    })?;
    Ok(Mode::from_raw_mode(raw_mode))
}

pub(super) fn unsafe_entry(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

pub(super) fn entry_name<'a>(path: &'a Path, description: &str) -> io::Result<&'a OsStr> {
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(name)), None) => Ok(name),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{description} must be one normal path component"),
        )),
    }
}

pub(super) fn validate_symlink_target(target: &Path) -> io::Result<()> {
    if target.as_os_str().is_empty()
        || target
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "symbolic-link target must be a nonempty normal relative path",
        ));
    }
    Ok(())
}

pub(in crate::unix) fn write_secret_contents(file: &mut File, contents: &[u8]) -> io::Result<()> {
    file.set_permissions(fs::Permissions::from_mode(SECRET_FILE_MODE))?;
    file.write_all(contents)?;
    file.sync_all()
}
