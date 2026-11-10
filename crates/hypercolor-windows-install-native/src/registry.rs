//! Writing the registry's `SYSTEM` hive to disk.
//!
//! The service control manager keeps every service's registration under
//! `HKLM\SYSTEM\CurrentControlSet\Services`, and Windows Firewall keeps its
//! rules under the same hive. Windows writes registry changes to disk
//! lazily: on Windows Server 2025, a service created 2, 10 or 40 seconds
//! before a hard reset was gone after it, and one created and then flushed
//! survived. Flushing the hive's root key writes every change made in that
//! hive.

use std::io;

use windows::Win32::Foundation::{ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegFlushKey, RegOpenKeyExW,
};
use windows::core::{Owned, w};

fn os_result(status: WIN32_ERROR) -> io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status.0.cast_signed()))
    }
}

/// Write every change made in the `SYSTEM` hive to disk, returning once it
/// is there: service registrations, failure actions, service security and
/// firewall rules.
///
/// # Errors
///
/// Returns the operating-system error when the hive cannot be opened or
/// written.
pub fn flush_system_hive() -> io::Result<()> {
    let mut key = HKEY::default();
    // SAFETY: The predefined key and the NUL-terminated literal are valid
    // for the call, and the out parameter is a live local.
    os_result(unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            w!("SYSTEM"),
            None,
            KEY_READ,
            &raw mut key,
        )
    })?;
    // SAFETY: Nothing else owns the handle the call just opened.
    let key = unsafe { Owned::new(key) };
    // SAFETY: `key` is an open registry key for the whole call.
    os_result(unsafe { RegFlushKey(*key) })
}

#[cfg(test)]
mod tests {
    use super::flush_system_hive;

    #[test]
    fn the_system_hive_flushes() {
        flush_system_hive().expect("flush HKLM\\SYSTEM");
        flush_system_hive().expect("a hive with nothing new flushes too");
    }
}
