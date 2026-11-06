//! Who may touch a service, which the safe service API does not reach.

use std::io;

use windows_service::service::Service;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
use windows_sys::Win32::System::Services::SetServiceObjectSecurity;

/// Replace the DACL of `service`, whose handle holds `WRITE_DAC`, with the
/// one `sddl` describes.
pub(crate) fn set_service_dacl(service: &Service, sddl: &str) -> io::Result<()> {
    let wide = sddl.encode_utf16().chain([0]).collect::<Vec<_>>();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: The string is NUL-terminated and live for the call, the out
    // parameter is a live local, and the size is not requested.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: The service handle is live for the call, and the descriptor
    // came from the conversion above.
    let result = if unsafe {
        SetServiceObjectSecurity(service.raw_handle(), DACL_SECURITY_INFORMATION, descriptor)
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    };
    // SAFETY: The conversion allocated the descriptor for the caller to free
    // once, and nothing uses it after this.
    unsafe { LocalFree(descriptor) };
    result
}
