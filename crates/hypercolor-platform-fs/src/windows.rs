use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

const REPLACE_FLAGS: u32 = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;

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
