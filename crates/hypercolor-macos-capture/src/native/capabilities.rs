use super::{
    AnyClass, CStr, MacosCaptureCapabilities, MacosCaptureError, MacosHostArchitecture,
    MacosRuntimeCapability, MacosTahoeRuntimeProbes, c_char, c_void, ptr, sel,
};

pub(super) fn native_capture_capabilities() -> Result<MacosCaptureCapabilities, MacosCaptureError> {
    let screenshot_configuration = AnyClass::get(c"SCScreenshotConfiguration");
    let screenshot_manager = AnyClass::get(c"SCScreenshotManager");
    let probes = MacosTahoeRuntimeProbes {
        content_tone_mapping_info_symbol: capability(
            crate::screenshot::tahoe_reference_output_symbols_present(),
        ),
        screenshot_configuration_class: capability(screenshot_configuration.is_some()),
        screenshot_dynamic_range_selector: capability(
            screenshot_configuration.is_some_and(|class| class.responds_to(sel!(setDynamicRange:))),
        ),
        screenshot_capture_selector: capability(screenshot_manager.is_some_and(|class| {
            class.metaclass().responds_to(sel!(
                captureScreenshotWithFilter:configuration:completionHandler:
            ))
        })),
    };
    capture_capabilities_from_probes(
        sysctl_i32(c"hw.optional.arm64", "hw.optional.arm64"),
        sysctl_i32(c"sysctl.proc_translated", "sysctl.proc_translated"),
        probes,
    )
}

const fn capability(present: bool) -> MacosRuntimeCapability {
    if present {
        MacosRuntimeCapability::Present
    } else {
        MacosRuntimeCapability::Absent
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SysctlI32Value {
    Present(i32),
    Missing,
}

pub(super) fn capture_capabilities_from_probes(
    arm64: Result<SysctlI32Value, MacosCaptureError>,
    translated: Result<SysctlI32Value, MacosCaptureError>,
    tahoe: MacosTahoeRuntimeProbes,
) -> Result<MacosCaptureCapabilities, MacosCaptureError> {
    let arm64 = arm64?;
    let translated_process = matches!(translated?, SysctlI32Value::Present(1));
    let host_architecture = if matches!(arm64, SysctlI32Value::Present(1)) || translated_process {
        MacosHostArchitecture::AppleSilicon
    } else {
        MacosHostArchitecture::Intel
    };
    Ok(MacosCaptureCapabilities::from_runtime(
        host_architecture,
        translated_process,
        tahoe,
    ))
}

fn sysctl_i32(name: &CStr, failure: &'static str) -> Result<SysctlI32Value, MacosCaptureError> {
    #[link(name = "System", kind = "dylib")]
    unsafe extern "C-unwind" {
        fn sysctlbyname(
            name: *const c_char,
            old_value: *mut c_void,
            old_length: *mut usize,
            new_value: *mut c_void,
            new_length: usize,
        ) -> i32;
    }

    let mut value = 0_i32;
    let mut length = std::mem::size_of::<i32>();
    // SAFETY: Both output pointers reference initialized writable storage, the
    // name is nul-terminated, and this query performs no mutation.
    let status = unsafe {
        sysctlbyname(
            name.as_ptr(),
            ptr::from_mut(&mut value).cast(),
            &mut length,
            ptr::null_mut(),
            0,
        )
    };
    if status == 0 && length == std::mem::size_of::<i32>() {
        Ok(SysctlI32Value::Present(value))
    } else if status != 0 {
        if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            Ok(SysctlI32Value::Missing)
        } else {
            Err(MacosCaptureError::CapabilityProbeFailed(failure))
        }
    } else {
        Err(MacosCaptureError::CapabilityProbeFailed("sysctl size"))
    }
}
