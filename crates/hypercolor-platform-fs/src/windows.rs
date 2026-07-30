use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;

use windows_sys::Win32::Foundation::{
    ERROR_CALL_NOT_IMPLEMENTED, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER,
    ERROR_NOT_SUPPORTED, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_CASE_SENSITIVE_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FileCaseSensitiveInfo, FileIdInfo, GetFileInformationByHandleEx, MOVEFILE_REPLACE_EXISTING,
    MOVEFILE_WRITE_THROUGH, MoveFileExW, OPEN_EXISTING,
};

const REPLACE_FLAGS: u32 = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;
const FILE_CS_FLAG_CASE_SENSITIVE_DIR: u32 = 1;

/// Filesystem identity and name-comparison rules for one destination.
#[derive(Debug, Clone)]
pub struct DestinationIdentity {
    parent_volume_serial: u64,
    parent_file_id: [u8; 16],
    file_name: Vec<u16>,
    case_sensitive: bool,
}

impl DestinationIdentity {
    /// Resolve a destination name relative to an existing canonical parent.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when the parent identity or its
    /// per-directory case-sensitivity policy cannot be queried.
    pub fn resolve(canonical_parent: &Path, file_name: &std::ffi::OsStr) -> io::Result<Self> {
        let parent = open_directory(canonical_parent)?;
        let mut information = FILE_ID_INFO::default();
        // SAFETY: The handle is live for the call and `information` points to
        // writable storage matching the requested information class.
        if unsafe {
            GetFileInformationByHandleEx(
                parent.as_raw_handle(),
                FileIdInfo,
                std::ptr::addr_of_mut!(information).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }

        let mut case_information = FILE_CASE_SENSITIVE_INFO::default();
        // SAFETY: The handle is live for the call, the buffer matches the
        // requested information class, and its byte length is exact.
        let case_sensitive = if unsafe {
            GetFileInformationByHandleEx(
                parent.as_raw_handle(),
                FileCaseSensitiveInfo,
                std::ptr::addr_of_mut!(case_information).cast(),
                size_of::<FILE_CASE_SENSITIVE_INFO>() as u32,
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            if case_sensitivity_query_is_unsupported(&error) {
                false
            } else {
                return Err(error);
            }
        } else {
            case_information.Flags & FILE_CS_FLAG_CASE_SENSITIVE_DIR != 0
        };

        let file_name = file_name.encode_wide().collect::<Vec<_>>();
        i32::try_from(file_name.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Windows destination name exceeds the ordinal comparison limit",
            )
        })?;

        Ok(Self {
            parent_volume_serial: information.VolumeSerialNumber,
            parent_file_id: information.FileId.Identifier,
            file_name,
            case_sensitive,
        })
    }

    /// Return whether both identities name the same filesystem destination.
    #[must_use]
    pub fn equivalent(&self, other: &Self) -> bool {
        if self.parent_volume_serial != other.parent_volume_serial
            || self.parent_file_id != other.parent_file_id
            || self.case_sensitive != other.case_sensitive
        {
            return false;
        }
        if self.case_sensitive {
            return self.file_name == other.file_name;
        }

        let left_len = i32::try_from(self.file_name.len())
            .expect("destination name length was validated during construction");
        let right_len = i32::try_from(other.file_name.len())
            .expect("destination name length was validated during construction");
        // SAFETY: Both slices remain live for the call and their explicit
        // lengths were validated to fit the API's signed length parameters.
        unsafe {
            CompareStringOrdinal(
                self.file_name.as_ptr(),
                left_len,
                other.file_name.as_ptr(),
                right_len,
                1,
            ) == CSTR_EQUAL
        }
    }
}

fn case_sensitivity_query_is_unsupported(error: &io::Error) -> bool {
    error.raw_os_error().is_some_and(|code| {
        [
            ERROR_INVALID_FUNCTION,
            ERROR_NOT_SUPPORTED,
            ERROR_INVALID_PARAMETER,
            ERROR_CALL_NOT_IMPLEMENTED,
        ]
        .contains(&(code as u32))
    })
}

fn open_directory(path: &Path) -> io::Result<OwnedHandle> {
    let path = wide_path(path)?;
    // SAFETY: The path is NUL-terminated and live for the call. Access and
    // sharing flags open an existing directory for metadata queries only.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `CreateFileW` returned a unique owned handle which is transferred
    // exactly once into `OwnedHandle` for automatic closure.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

pub(super) fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    let source = wide_path(source)?;
    let destination = wide_path(destination)?;

    // SAFETY: Both buffers are NUL-terminated, live for the call, and point to
    // immutable UTF-16 path data. The flags request same-volume replacement
    // and write-through completion without retaining either pointer.
    let replaced = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), REPLACE_FLAGS) };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows paths cannot contain NUL code units",
        ));
    }
    wide.push(0);
    Ok(wide)
}
