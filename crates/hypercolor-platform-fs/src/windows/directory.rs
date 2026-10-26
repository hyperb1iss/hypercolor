//! Handle-relative directory authority on Windows.
//!
//! A [`WindowsDirectory`] retains one directory handle opened without
//! following reparse points, after refusing a reparse point anywhere on the
//! path to it. Every child operation is relative to that handle through
//! `NtCreateFile` with a root directory, takes one plain name, and never
//! follows a reparse point, so replacing a path component after the open
//! cannot redirect it.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle, RawHandle};
use std::path::{Component, Path, PathBuf};

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF,
    FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION, FILE_SYNCHRONOUS_IO_NONALERT,
    FileRenameInformation, NtCreateFile, NtSetInformationFile,
};
use windows_sys::Win32::Foundation::{
    DuplicateHandle, ERROR_LOCK_VIOLATION, ERROR_NO_MORE_FILES, GENERIC_READ, GENERIC_WRITE,
    HANDLE, INVALID_HANDLE_VALUE, LocalFree, OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError,
    UNICODE_STRING,
};
use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, CopySid, DACL_SECURITY_INFORMATION, GetAce, GetLengthSid,
    GetTokenInformation, INHERIT_ONLY_ACE, IsValidSid, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_APPEND_DATA,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_DELETE_CHILD, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_FULL_DIR_INFO, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
    FILE_ID_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_STANDARD_INFO, FILE_TRAVERSE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA,
    FILE_WRITE_EA, FileAttributeTagInfo, FileDispositionInfo, FileFullDirectoryInfo,
    FileFullDirectoryRestartInfo, FileIdInfo, FileStandardInfo, FlushFileBuffers,
    GetFileInformationByHandleEx, GetFinalPathNameByHandleW, LOCKFILE_EXCLUSIVE_LOCK,
    LOCKFILE_FAIL_IMMEDIATELY, LockFileEx, OPEN_EXISTING, SYNCHRONIZE, SetFileInformationByHandle,
};
use windows_sys::Win32::System::IO::{IO_STATUS_BLOCK, OVERLAPPED};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Rights that let a principal change a directory, a file, or its security.
const WRITE_CLASS_RIGHTS: u32 = FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_WRITE_EA
    | FILE_WRITE_ATTRIBUTES
    | FILE_DELETE_CHILD
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER
    | GENERIC_WRITE
    | GENERIC_ALL;
/// Rights that let a principal remove, rename or replace an entry or its
/// existing children, or take over its security.
const REPLACE_CLASS_RIGHTS: u32 =
    FILE_DELETE_CHILD | DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_ALL;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_EXECUTE: u32 = 0x2000_0000;
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
/// Allowed-object, allowed-callback and allowed-callback-object entries.
const ALLOW_ACE_TYPES_WITH_OTHER_LAYOUTS: [u8; 3] = [0x05, 0x09, 0x0B];
const READ_CONTROL: u32 = 0x0002_0000;
const ENUMERATION_BUFFER_BYTES: usize = 64 * 1024;
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const RESERVED_DEVICE_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Which operations a retained directory handle permits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsDirectoryAccess {
    /// List, inspect and open children for reading.
    Read,
    /// Also create, rename and delete children and flush the directory.
    Write,
}

/// What one directory entry is, judged without following it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsEntryKind {
    /// A directory that is not a reparse point.
    Directory,
    /// A regular file that is not a reparse point.
    File,
    /// A symlink, junction, mount point or any other reparse point.
    ReparsePoint,
}

/// Identity and shape of one entry, read through a handle that does not
/// follow reparse points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsEntryMetadata {
    kind: WindowsEntryKind,
    size: u64,
    links: u32,
    volume: u64,
    file_id: u128,
}

impl WindowsEntryMetadata {
    /// What the entry is.
    #[must_use]
    pub const fn kind(self) -> WindowsEntryKind {
        self.kind
    }

    /// The file's end-of-file position; zero for a directory.
    #[must_use]
    pub const fn size(self) -> u64 {
        self.size
    }

    /// How many names refer to the file.
    #[must_use]
    pub const fn link_count(self) -> u32 {
        self.links
    }

    /// Whether both describe the same file on the same volume.
    #[must_use]
    pub const fn same_entry(self, other: Self) -> bool {
        self.volume == other.volume && self.file_id == other.file_id
    }
}

/// One principal named by a security descriptor, held as an owned SID.
#[derive(Clone, PartialEq, Eq)]
pub struct WindowsSid(Vec<u8>);

impl WindowsSid {
    /// Copy a SID from a pointer into owned bytes.
    ///
    /// # Safety
    ///
    /// `sid` must point to a valid SID for the duration of the call.
    unsafe fn copy_from(sid: PSID) -> io::Result<Self> {
        // SAFETY: The caller guarantees `sid` is valid for the call.
        if unsafe { IsValidSid(sid) } == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "security descriptor names an invalid SID",
            ));
        }
        // SAFETY: `sid` is a valid SID, as checked above.
        let length = unsafe { GetLengthSid(sid) };
        let mut bytes = vec![0_u8; length as usize];
        // SAFETY: The destination holds exactly `length` writable bytes and
        // `sid` is valid for reading that many bytes.
        if unsafe { CopySid(length, bytes.as_mut_ptr().cast(), sid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(bytes))
    }

    /// The user the current process runs as.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when the process token cannot be
    /// queried.
    pub fn current_user() -> io::Result<Self> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: The process pseudo handle is always valid and `token` is a
        // live out-parameter.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: OpenProcessToken returned a unique owned handle.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut buffer = vec![0_u64; 128];
        let mut needed = 0_u32;
        // SAFETY: The token is live, the buffer is writable and aligned for
        // TOKEN_USER, and its byte length is passed exactly.
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * size_of::<u64>()) as u32,
                &mut needed,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GetTokenInformation wrote a TOKEN_USER at the buffer start,
        // whose SID pointer refers into the same buffer.
        unsafe { Self::copy_from((*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid) }
    }

    /// Parse the string form of a SID, such as `S-1-5-18`.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not a SID.
    pub fn parse(text: &str) -> io::Result<Self> {
        let mut parts = text.split('-');
        let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "not a SID string");
        if parts.next() != Some("S") || parts.next() != Some("1") {
            return Err(invalid());
        }
        let authority: u64 = parts
            .next()
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        if authority >= 1 << 48 {
            return Err(invalid());
        }
        let subauthorities = parts
            .map(|part| part.parse::<u32>().map_err(|_| invalid()))
            .collect::<io::Result<Vec<_>>>()?;
        if subauthorities.is_empty() || subauthorities.len() > 15 {
            return Err(invalid());
        }
        let mut bytes = vec![1_u8, subauthorities.len() as u8];
        bytes.extend_from_slice(&authority.to_be_bytes()[2..]);
        for subauthority in subauthorities {
            bytes.extend_from_slice(&subauthority.to_le_bytes());
        }
        Ok(Self(bytes))
    }

    /// The SID's string form.
    #[must_use]
    pub fn to_string_form(&self) -> String {
        let count = self.0.get(1).copied().unwrap_or(0) as usize;
        let mut authority = [0_u8; 8];
        if let Some(bytes) = self.0.get(2..8) {
            authority[2..].copy_from_slice(bytes);
        }
        let mut text = format!("S-{}-{}", self.0[0], u64::from_be_bytes(authority));
        for index in 0..count {
            let start = 8 + index * 4;
            if let Some(bytes) = self.0.get(start..start + 4) {
                let mut value = [0_u8; 4];
                value.copy_from_slice(bytes);
                text.push('-');
                text.push_str(&u32::from_le_bytes(value).to_string());
            }
        }
        text
    }
}

impl std::fmt::Debug for WindowsSid {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.to_string_form())
    }
}

/// One allow entry that applies to the entry itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsGrant {
    /// The principal the entry names.
    pub sid: WindowsSid,
    /// The access mask it grants.
    pub mask: u32,
}

impl WindowsGrant {
    /// Whether the grant lets its principal change the entry's contents,
    /// attributes, children, name or security in any way.
    #[must_use]
    pub const fn writes(&self) -> bool {
        self.mask & WRITE_CLASS_RIGHTS != 0
    }

    /// Whether the grant lets its principal delete, rename or replace the
    /// entry or its existing children, or change its security. Adding new
    /// children is not among them.
    #[must_use]
    pub const fn replaces(&self) -> bool {
        self.mask & REPLACE_CLASS_RIGHTS != 0
    }

    /// Whether the grant lets its principal read the entry's contents.
    #[must_use]
    pub const fn reads(&self) -> bool {
        let read = FILE_GENERIC_READ & !SYNCHRONIZE & !READ_CONTROL;
        self.mask & (GENERIC_ALL | GENERIC_READ) != 0 || self.mask & read == read
    }

    /// Whether the grant lets its principal read and execute the entry.
    #[must_use]
    pub const fn reads_and_executes(&self) -> bool {
        let execute = FILE_GENERIC_EXECUTE & !SYNCHRONIZE & !READ_CONTROL;
        let read = FILE_GENERIC_READ & !SYNCHRONIZE & !READ_CONTROL;
        self.mask & (GENERIC_ALL | GENERIC_EXECUTE) != 0
            || (self.mask & execute == execute && self.mask & read == read)
    }
}

/// An entry's owner and the allow entries that apply to it.
///
/// Deny entries are not credited: an allow entry is reported even when a
/// deny entry might mask it, so a judgement built on these grants can only
/// err toward refusing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsSecurity {
    /// The entry's owner, who can always rewrite its DACL.
    pub owner: WindowsSid,
    /// The entry has no DACL, which grants everyone every right.
    pub null_dacl: bool,
    /// Every allow entry that is not inherit-only.
    pub grants: Vec<WindowsGrant>,
}

/// An exclusive lock on one file, held until dropped.
#[derive(Debug)]
pub struct WindowsFileLock {
    file: File,
}

impl WindowsFileLock {
    /// The locked file.
    #[must_use]
    pub fn file(&self) -> &File {
        &self.file
    }
}

/// A retained directory handle and the path it was opened from.
#[derive(Debug)]
pub struct WindowsDirectory {
    handle: OwnedHandle,
    path: PathBuf,
    access: WindowsDirectoryAccess,
}

impl WindowsDirectory {
    /// Open an absolute directory path, refusing a reparse point at the
    /// directory or at any ancestor.
    ///
    /// # Errors
    ///
    /// Returns an error for a relative or non-normal path, a reparse point
    /// anywhere on the path, something other than a directory, or an open
    /// failure.
    pub fn open(path: &Path, access: WindowsDirectoryAccess) -> io::Result<Self> {
        require_normal_absolute(path)?;
        let mut prefix = PathBuf::new();
        for component in path.components() {
            prefix.push(component.as_os_str());
            if matches!(component, Component::Normal(_)) {
                let probe = open_path(&prefix, FILE_READ_ATTRIBUTES)?;
                if attributes(&probe)? & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    return Err(reparse_refused());
                }
            }
        }
        let handle = open_path(path, directory_rights(access))?;
        // An ancestor swapped for a reparse point after its probe would
        // change the path the handle resolves to.
        if !final_path_matches(&handle, path)? {
            return Err(reparse_refused());
        }
        let directory = Self {
            handle,
            path: path.to_path_buf(),
            access,
        };
        if directory.metadata()?.kind != WindowsEntryKind::Directory {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "the path does not name a directory",
            ));
        }
        Ok(directory)
    }

    /// The path this directory was opened from, for diagnostics and for
    /// composing child paths that other processes will be handed.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Identity and shape of the directory itself.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when the handle cannot be queried.
    pub fn metadata(&self) -> io::Result<WindowsEntryMetadata> {
        metadata(&self.handle)
    }

    /// Owner and allow entries of the directory itself.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when the security descriptor
    /// cannot be read, or an error for a DACL holding an allow entry whose
    /// layout this reader cannot judge.
    pub fn security(&self) -> io::Result<WindowsSecurity> {
        security(&self.handle)
    }

    /// Open a child directory without following it.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name, a missing child, a child that is
    /// a reparse point or not a directory, or a write request through a
    /// read-only parent.
    pub fn open_child_directory(
        &self,
        name: &OsStr,
        access: WindowsDirectoryAccess,
    ) -> io::Result<Self> {
        if access == WindowsDirectoryAccess::Write {
            self.require_write()?;
        }
        let handle = self.nt_open(
            name,
            directory_rights(access),
            FILE_OPEN,
            FILE_DIRECTORY_FILE,
        )?;
        let child = Self {
            handle,
            path: self.path.join(name),
            access,
        };
        if child.metadata()?.kind != WindowsEntryKind::Directory {
            return Err(reparse_refused());
        }
        Ok(child)
    }

    /// Create a new child directory, durably, and open it for writing.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::AlreadyExists`] when the name is taken, and
    /// the operating-system error for any other failure.
    pub fn create_child_directory(&self, name: &OsStr) -> io::Result<Self> {
        self.require_write()?;
        let handle = self.nt_open(
            name,
            directory_rights(WindowsDirectoryAccess::Write),
            FILE_CREATE,
            FILE_DIRECTORY_FILE,
        )?;
        self.sync()?;
        Ok(Self {
            handle,
            path: self.path.join(name),
            access: WindowsDirectoryAccess::Write,
        })
    }

    /// Reopen this directory read-only through its retained handle.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when the handle cannot be reopened.
    pub fn read_only(&self) -> io::Result<Self> {
        let mut duplicate: HANDLE = std::ptr::null_mut();
        // SAFETY: Both process handles are the current-process pseudo handle
        // and the source handle is live; the duplicate carries only the
        // read rights requested.
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.handle.as_raw_handle(),
                GetCurrentProcess(),
                &mut duplicate,
                directory_rights(WindowsDirectoryAccess::Read),
                0,
                0,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: DuplicateHandle returned a unique owned handle.
        let handle = unsafe { OwnedHandle::from_raw_handle(duplicate) };
        Ok(Self {
            handle,
            path: self.path.clone(),
            access: WindowsDirectoryAccess::Read,
        })
    }

    /// Every child name, in no particular order.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when enumeration fails, or an error
    /// for a child name that is not valid UTF-16.
    pub fn entries(&self) -> io::Result<Vec<OsString>> {
        let mut names = Vec::new();
        let mut buffer = vec![0_u64; ENUMERATION_BUFFER_BYTES / size_of::<u64>()];
        let mut class = FileFullDirectoryRestartInfo;
        loop {
            // SAFETY: The handle is live, the buffer is writable for its
            // byte length and 8-byte aligned as the information class needs.
            let ok = unsafe {
                GetFileInformationByHandleEx(
                    self.handle.as_raw_handle(),
                    class,
                    buffer.as_mut_ptr().cast(),
                    ENUMERATION_BUFFER_BYTES as u32,
                )
            };
            if ok == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                    return Ok(names);
                }
                return Err(error);
            }
            class = FileFullDirectoryInfo;
            let bytes = buffer.as_ptr().cast::<u8>();
            let mut offset = 0_usize;
            loop {
                // SAFETY: `offset` always points at a FILE_FULL_DIR_INFO record
                // the call wrote inside the buffer: zero first, then each
                // record's own NextEntryOffset.
                let record = unsafe { &*bytes.add(offset).cast::<FILE_FULL_DIR_INFO>() };
                let name_offset = offset + offset_of!(FILE_FULL_DIR_INFO, FileName);
                let name_units = record.FileNameLength as usize / size_of::<u16>();
                // SAFETY: The record's name lies inside the buffer the call
                // filled; FileNameLength is its byte length.
                let name = unsafe {
                    std::slice::from_raw_parts(bytes.add(name_offset).cast::<u16>(), name_units)
                };
                if name != [u16::from(b'.')] && name != [u16::from(b'.'), u16::from(b'.')] {
                    names.push(OsString::from_wide(name));
                }
                if record.NextEntryOffset == 0 {
                    break;
                }
                offset += record.NextEntryOffset as usize;
            }
        }
    }

    /// Metadata of one child without following it, or `None` when absent.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name or any failure other than absence.
    pub fn entry_metadata(&self, name: &OsStr) -> io::Result<Option<WindowsEntryMetadata>> {
        match self.nt_open(name, FILE_READ_ATTRIBUTES | SYNCHRONIZE, FILE_OPEN, 0) {
            Ok(handle) => metadata(&handle).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Owner and allow entries of one child, read without following it.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid or missing name, an unreadable
    /// security descriptor, or a DACL this reader cannot judge.
    pub fn entry_security(&self, name: &OsStr) -> io::Result<WindowsSecurity> {
        let handle = self.nt_open(name, READ_CONTROL | SYNCHRONIZE, FILE_OPEN, 0)?;
        security(&handle)
    }

    /// Open an existing regular child file for reading.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name, a missing child, or a child that
    /// is a reparse point or a directory.
    pub fn open_file(&self, name: &OsStr) -> io::Result<File> {
        let handle = self.nt_open(name, GENERIC_READ, FILE_OPEN, FILE_NON_DIRECTORY_FILE)?;
        if metadata(&handle)?.kind != WindowsEntryKind::File {
            return Err(reparse_refused());
        }
        Ok(File::from(handle))
    }

    /// Create a new regular child file for writing.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::AlreadyExists`] when the name is taken, and
    /// the operating-system error for any other failure.
    pub fn create_file(&self, name: &OsStr) -> io::Result<File> {
        self.require_write()?;
        let handle = self.nt_open(
            name,
            GENERIC_READ | GENERIC_WRITE | DELETE,
            FILE_CREATE,
            FILE_NON_DIRECTORY_FILE,
        )?;
        Ok(File::from(handle))
    }

    /// Open or create a regular child file and lock it exclusively without
    /// waiting. Returns `None` when another handle holds the lock.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name, a child that is a reparse point
    /// or directory, or a lock failure other than contention.
    pub fn try_lock_file(&self, name: &OsStr) -> io::Result<Option<WindowsFileLock>> {
        self.require_write()?;
        // Without delete sharing, nobody can unlink or rename the lock file
        // while it is held, so a second holder can never lock a new file of
        // the same name.
        let handle = self.nt_open_shared(
            name,
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN_IF,
            FILE_NON_DIRECTORY_FILE,
        )?;
        if metadata(&handle)?.kind != WindowsEntryKind::File {
            return Err(reparse_refused());
        }
        let mut overlapped = OVERLAPPED::default();
        // SAFETY: The handle is live and synchronous; the overlapped structure
        // only carries the zero lock offset and outlives the call.
        let locked = unsafe {
            LockFileEx(
                handle.as_raw_handle(),
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                u32::MAX,
                u32::MAX,
                &mut overlapped,
            )
        };
        if locked == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
                return Ok(None);
            }
            return Err(error);
        }
        Ok(Some(WindowsFileLock {
            file: File::from(handle),
        }))
    }

    /// Rename one child to another name in this directory.
    ///
    /// With `replace`, an existing regular file at `to` is replaced in one
    /// step; without it, an existing `to` fails with
    /// [`io::ErrorKind::AlreadyExists`]. This directory is flushed before
    /// returning, so a caller that flushed the source's contents first gets
    /// a durable replacement.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid names, a missing or reparse-point
    /// source, a taken destination without `replace`, or a flush failure.
    pub fn rename(&self, from: &OsStr, to: &OsStr, replace: bool) -> io::Result<()> {
        self.require_write()?;
        let to_wide = validate_name(to)?;
        let source = self.nt_open(
            from,
            DELETE | SYNCHRONIZE | FILE_READ_ATTRIBUTES,
            FILE_OPEN,
            0,
        )?;
        if metadata(&source)?.kind == WindowsEntryKind::ReparsePoint {
            return Err(reparse_refused());
        }
        let name_bytes = std::mem::size_of_val(to_wide.as_slice());
        let header = offset_of!(FILE_RENAME_INFORMATION, FileName);
        let total = header + name_bytes + size_of::<u16>();
        let mut buffer = vec![0_u64; total.div_ceil(size_of::<u64>())];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        // SAFETY: The buffer is zeroed, 8-byte aligned and large enough for
        // the header, the name and a terminator; each write stays inside it.
        unsafe {
            (*info).Anonymous.ReplaceIfExists = replace;
            (*info).RootDirectory = self.handle.as_raw_handle();
            (*info).FileNameLength = name_bytes as u32;
            std::ptr::copy_nonoverlapping(
                to_wide.as_ptr(),
                buffer.as_mut_ptr().cast::<u8>().add(header).cast::<u16>(),
                to_wide.len(),
            );
        }
        let mut status_block = IO_STATUS_BLOCK::default();
        // SAFETY: The handle is live and opened with DELETE, and the buffer
        // holds one complete FILE_RENAME_INFORMATION of `total` bytes whose
        // root directory handle is this live directory.
        let status = unsafe {
            NtSetInformationFile(
                source.as_raw_handle(),
                &mut status_block,
                buffer.as_ptr().cast(),
                total as u32,
                FileRenameInformation,
            )
        };
        nt_result(status)?;
        drop(source);
        self.sync()
    }

    /// Remove one regular child file and flush this directory.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid or missing name, a child that is not a
    /// regular file, or a removal failure.
    pub fn remove_file(&self, name: &OsStr) -> io::Result<()> {
        self.require_write()?;
        let handle = self.nt_open(
            name,
            DELETE | SYNCHRONIZE,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
        )?;
        if metadata(&handle)?.kind != WindowsEntryKind::File {
            return Err(reparse_refused());
        }
        mark_for_deletion(&handle)?;
        drop(handle);
        self.sync()
    }

    /// Remove one child directory and everything beneath it, without
    /// following any reparse point inside, then flush this directory.
    ///
    /// A reparse point inside the tree is unlinked itself; its target is
    /// never visited.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid or missing name or any removal failure.
    pub fn remove_tree(&self, name: &OsStr) -> io::Result<()> {
        self.require_write()?;
        let child = self.open_child_directory(name, WindowsDirectoryAccess::Write)?;
        child.clear()?;
        let handle = self.nt_open(name, DELETE | SYNCHRONIZE, FILE_OPEN, FILE_DIRECTORY_FILE)?;
        drop(child);
        mark_for_deletion(&handle)?;
        drop(handle);
        self.sync()
    }

    fn clear(&self) -> io::Result<()> {
        for name in self.entries()? {
            let Some(entry) = self.entry_metadata(&name)? else {
                continue;
            };
            if entry.kind == WindowsEntryKind::Directory {
                self.open_child_directory(&name, WindowsDirectoryAccess::Write)?
                    .clear()?;
            }
            let handle = self.nt_open(&name, DELETE | SYNCHRONIZE, FILE_OPEN, 0)?;
            mark_for_deletion(&handle)?;
        }
        Ok(())
    }

    /// Flush this directory's metadata.
    ///
    /// # Errors
    ///
    /// Returns an error for a read-only handle or a flush failure.
    pub fn sync(&self) -> io::Result<()> {
        self.require_write()?;
        flush(&self.handle)
    }

    fn require_write(&self) -> io::Result<()> {
        if self.access == WindowsDirectoryAccess::Write {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the directory authority is read-only",
            ))
        }
    }

    fn nt_open(
        &self,
        name: &OsStr,
        access: u32,
        disposition: u32,
        options: u32,
    ) -> io::Result<OwnedHandle> {
        self.nt_open_shared(name, access, SHARE_ALL, disposition, options)
    }

    fn nt_open_shared(
        &self,
        name: &OsStr,
        access: u32,
        share: u32,
        disposition: u32,
        options: u32,
    ) -> io::Result<OwnedHandle> {
        let wide = validate_name(name)?;
        nt_create(
            Some(&self.handle),
            &wide,
            access,
            share,
            disposition,
            options,
        )
    }
}

fn directory_rights(access: WindowsDirectoryAccess) -> u32 {
    let read =
        FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE;
    match access {
        WindowsDirectoryAccess::Read => read,
        WindowsDirectoryAccess::Write => {
            read | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY | FILE_DELETE_CHILD | FILE_WRITE_ATTRIBUTES
        }
    }
}

fn nt_create(
    root: Option<&OwnedHandle>,
    name: &[u16],
    access: u32,
    share: u32,
    disposition: u32,
    options: u32,
) -> io::Result<OwnedHandle> {
    let byte_length = u16::try_from(std::mem::size_of_val(name))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "child name is too long"))?;
    let object_name = UNICODE_STRING {
        Length: byte_length,
        MaximumLength: byte_length,
        Buffer: name.as_ptr().cast_mut(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: root.map_or(std::ptr::null_mut(), |handle| handle.as_raw_handle()),
        ObjectName: &object_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut handle: HANDLE = std::ptr::null_mut();
    let mut status_block = IO_STATUS_BLOCK::default();
    // SAFETY: Every pointer refers to a live local for the duration of the
    // call: the name buffer, the UNICODE_STRING, the attributes, the root
    // handle and the out-parameters. No EA buffer is passed.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            // Synchronous I/O, which std's File expects, needs SYNCHRONIZE.
            access | SYNCHRONIZE,
            &attributes,
            &mut status_block,
            std::ptr::null(),
            0,
            share,
            disposition,
            options | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            std::ptr::null(),
            0,
        )
    };
    nt_result(status)?;
    // SAFETY: NtCreateFile succeeded and returned a unique handle, which is
    // transferred exactly once into OwnedHandle.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) })
}

fn nt_result(status: i32) -> io::Result<()> {
    if status < 0 {
        // SAFETY: Converting an NTSTATUS has no preconditions.
        let code = unsafe { RtlNtStatusToDosError(status) };
        Err(io::Error::from_raw_os_error(code as i32))
    } else {
        Ok(())
    }
}

fn open_path(path: &Path, access: u32) -> io::Result<OwnedHandle> {
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows paths cannot contain NUL code units",
        ));
    }
    wide.push(0);
    // SAFETY: The path is NUL-terminated and live for the call; the flags
    // open the entry itself without following a reparse point.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateFileW returned a unique owned handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn final_path_matches(handle: &OwnedHandle, requested: &Path) -> io::Result<bool> {
    let mut buffer = vec![0_u16; 1024];
    loop {
        // SAFETY: The handle is live and the buffer is writable for its
        // length in UTF-16 units.
        let length = unsafe {
            GetFinalPathNameByHandleW(
                handle.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                0,
            )
        } as usize;
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        if length < buffer.len() {
            buffer.truncate(length);
            break;
        }
        buffer.resize(length + 1, 0);
    }
    let resolved = strip_verbatim(&buffer);
    let requested = requested.as_os_str().encode_wide().collect::<Vec<_>>();
    let requested = strip_verbatim(&requested);
    let resolved = resolved.strip_suffix(&[0x5C]).unwrap_or(resolved);
    let requested = requested.strip_suffix(&[0x5C]).unwrap_or(requested);
    Ok(ordinal_equal_ignore_case(resolved, requested))
}

fn strip_verbatim(path: &[u16]) -> &[u16] {
    const VERBATIM: [u16; 4] = [0x5C, 0x5C, 0x3F, 0x5C];
    path.strip_prefix(&VERBATIM).unwrap_or(path)
}

fn ordinal_equal_ignore_case(left: &[u16], right: &[u16]) -> bool {
    let (Ok(left_len), Ok(right_len)) = (i32::try_from(left.len()), i32::try_from(right.len()))
    else {
        return false;
    };
    // SAFETY: Both slices are live for the call with their exact lengths.
    unsafe {
        CompareStringOrdinal(left.as_ptr(), left_len, right.as_ptr(), right_len, 1) == CSTR_EQUAL
    }
}

fn attributes(handle: &OwnedHandle) -> io::Result<u32> {
    let mut information = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: The handle is live and the buffer matches the information class.
    if unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileAttributeTagInfo,
            std::ptr::addr_of_mut!(information).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(information.FileAttributes)
}

fn metadata(handle: &OwnedHandle) -> io::Result<WindowsEntryMetadata> {
    let attributes = attributes(handle)?;
    let mut standard = FILE_STANDARD_INFO::default();
    // SAFETY: The handle is live and the buffer matches the information class.
    if unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileStandardInfo,
            std::ptr::addr_of_mut!(standard).cast(),
            size_of::<FILE_STANDARD_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut identity = FILE_ID_INFO::default();
    // SAFETY: The handle is live and the buffer matches the information class.
    if unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileIdInfo,
            std::ptr::addr_of_mut!(identity).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let kind = if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        WindowsEntryKind::ReparsePoint
    } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        WindowsEntryKind::Directory
    } else {
        WindowsEntryKind::File
    };
    Ok(WindowsEntryMetadata {
        kind,
        size: u64::try_from(standard.EndOfFile).unwrap_or(0),
        links: standard.NumberOfLinks,
        volume: identity.VolumeSerialNumber,
        file_id: u128::from_le_bytes(identity.FileId.Identifier),
    })
}

fn security(handle: &OwnedHandle) -> io::Result<WindowsSecurity> {
    let mut owner: PSID = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: The handle is live and opened with READ_CONTROL; every out
    // pointer is a live local. The descriptor is freed below with LocalFree,
    // and `owner` and `dacl` point inside it.
    let status = unsafe {
        GetSecurityInfo(
            handle.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let result = (|| {
        // SAFETY: GetSecurityInfo returned a descriptor whose owner SID is
        // valid until the descriptor is freed.
        let owner = unsafe { WindowsSid::copy_from(owner) }?;
        if dacl.is_null() {
            return Ok(WindowsSecurity {
                owner,
                null_dacl: true,
                grants: Vec::new(),
            });
        }
        let mut grants = Vec::new();
        // SAFETY: `dacl` points to a valid ACL inside the descriptor.
        let count = u32::from(unsafe { (*dacl).AceCount });
        for index in 0..count {
            let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
            // SAFETY: The index is below the ACL's ACE count.
            if unsafe { GetAce(dacl, index, &mut ace) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: Every ACE begins with an ACE_HEADER.
            let header = unsafe { &*ace.cast::<ACE_HEADER>() };
            if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
                continue;
            }
            if header.AceType != ACCESS_ALLOWED_ACE_TYPE {
                // Object and callback allow entries carry their own layouts;
                // one might grant write, so refuse to judge the DACL at all.
                if ALLOW_ACE_TYPES_WITH_OTHER_LAYOUTS.contains(&header.AceType) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "the DACL carries an allow entry this policy cannot judge",
                    ));
                }
                continue;
            }
            // SAFETY: The ACE type was checked to have this layout.
            let entry = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
            let sid_pointer = std::ptr::addr_of!(entry.SidStart).cast_mut().cast();
            // SAFETY: The SID begins at SidStart inside the ACE.
            let sid = unsafe { WindowsSid::copy_from(sid_pointer) }?;
            grants.push(WindowsGrant {
                sid,
                mask: entry.Mask,
            });
        }
        Ok(WindowsSecurity {
            owner,
            null_dacl: false,
            grants,
        })
    })();
    // SAFETY: The descriptor came from GetSecurityInfo and is freed once.
    unsafe { LocalFree(descriptor) };
    result
}

fn flush(handle: &OwnedHandle) -> io::Result<()> {
    // SAFETY: The handle is live for the call.
    if unsafe { FlushFileBuffers(handle.as_raw_handle()) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn mark_for_deletion(handle: &OwnedHandle) -> io::Result<()> {
    let information = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: The handle is live and opened with DELETE; the buffer matches
    // the information class.
    if unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle(),
            FileDispositionInfo,
            std::ptr::addr_of!(information).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Whether `name` is one plain child name this authority accepts: no
/// separators, stream markers or reserved characters, no trailing dot or
/// space, and no reserved device name, so the Win32 layer reads it the way
/// the handle-relative NT calls do.
#[must_use]
pub fn windows_child_name_is_valid(name: &OsStr) -> bool {
    validate_name(name).is_ok()
}

/// Accept only one plain child name the Win32 layer reads the same way.
fn validate_name(name: &OsStr) -> io::Result<Vec<u16>> {
    let invalid = |reason: &str| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid child name {name:?}: {reason}"),
        )
    };
    let wide = name.encode_wide().collect::<Vec<_>>();
    if wide.is_empty() || wide.len() > 255 {
        return Err(invalid("it must hold 1 to 255 UTF-16 units"));
    }
    if wide.iter().any(|&unit| {
        unit < 0x20
            || matches!(
                unit,
                0x22 | 0x2A | 0x2F | 0x3A | 0x3C | 0x3E | 0x3F | 0x5C | 0x7C
            )
    }) {
        return Err(invalid(
            "it holds a separator, stream marker or reserved character",
        ));
    }
    if matches!(wide.last(), Some(0x20 | 0x2E)) {
        return Err(invalid("Win32 drops a trailing dot or space"));
    }
    if let Some(text) = name.to_str() {
        let stem = text.split('.').next().unwrap_or(text);
        if RESERVED_DEVICE_NAMES
            .iter()
            .any(|reserved| stem.eq_ignore_ascii_case(reserved))
        {
            return Err(invalid("it is a reserved device name"));
        }
    }
    Ok(wide)
}

fn require_normal_absolute(path: &Path) -> io::Result<()> {
    let mut components = path.components();
    let normal = matches!(components.next(), Some(Component::Prefix(_)))
        && matches!(components.next(), Some(Component::RootDir))
        && components.all(|component| match component {
            Component::Normal(name) => validate_name(name).is_ok(),
            _ => false,
        });
    if normal {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the directory path must be absolute and hold only plain names",
        ))
    }
}

fn reparse_refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "refusing a Windows reparse point or an entry of the wrong kind",
    )
}
