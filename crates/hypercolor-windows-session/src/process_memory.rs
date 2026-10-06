//! Process self-inspection through the Win32 process status API.

/// Resident set size of the current process in mebibytes.
///
/// Reads the working set size from `K32GetProcessMemoryInfo`. Returns `None`
/// off Windows or when the query fails.
#[must_use]
pub fn process_resident_memory_mb() -> Option<f64> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::ProcessStatus::{
            K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        let size = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?;
        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: size,
            PageFaultCount: 0,
            PeakWorkingSetSize: 0,
            WorkingSetSize: 0,
            QuotaPeakPagedPoolUsage: 0,
            QuotaPagedPoolUsage: 0,
            QuotaPeakNonPagedPoolUsage: 0,
            QuotaNonPagedPoolUsage: 0,
            PagefileUsage: 0,
            PeakPagefileUsage: 0,
        };
        // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no
        // closing, and K32GetProcessMemoryInfo writes at most `size` bytes
        // into the counters struct owned by this call.
        let succeeded =
            unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &raw mut counters, size) };
        if succeeded == 0 {
            return None;
        }
        let resident_kib = u32::try_from(counters.WorkingSetSize / 1024).ok()?;
        Some(f64::from(resident_kib) / 1024.0)
    }

    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}
