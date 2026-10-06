//! Process self-inspection through Mach task info.

/// Resident set size of the current process in mebibytes.
///
/// Reads `resident_size` from the task's `MACH_TASK_BASIC_INFO`. Returns
/// `None` off macOS or when the kernel refuses the query.
#[must_use]
pub fn process_resident_memory_mb() -> Option<f64> {
    #[cfg(target_os = "macos")]
    {
        use mach2::kern_return::KERN_SUCCESS;
        use mach2::task::task_info;
        use mach2::task_info::{
            MACH_TASK_BASIC_INFO, MACH_TASK_BASIC_INFO_COUNT, mach_task_basic_info,
        };
        use mach2::traps::mach_task_self;
        use mach2::vm_types::integer_t;

        let mut info = mach_task_basic_info::default();
        let mut count = MACH_TASK_BASIC_INFO_COUNT;
        // SAFETY: task_info writes at most MACH_TASK_BASIC_INFO_COUNT natural_t
        // values into a mach_task_basic_info owned by this call and stores the
        // number written in count; both outlive the call.
        let result = unsafe {
            task_info(
                mach_task_self(),
                MACH_TASK_BASIC_INFO,
                std::ptr::from_mut(&mut info).cast::<integer_t>(),
                &mut count,
            )
        };
        if result != KERN_SUCCESS || count != MACH_TASK_BASIC_INFO_COUNT {
            return None;
        }
        // The struct is packed, so the field is copied out rather than borrowed.
        let resident_bytes = info.resident_size;
        let resident_kib = u32::try_from(resident_bytes / 1024).ok()?;
        Some(f64::from(resident_kib) / 1024.0)
    }

    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
