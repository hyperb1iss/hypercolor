//! Process self-inspection through procfs.

/// Resident set size of the current process in mebibytes.
///
/// Reads `VmRSS` from `/proc/self/status`. Returns `None` off Linux or when
/// procfs is unavailable or unparseable.
#[must_use]
pub fn process_resident_memory_mb() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
        let kb = line.split_whitespace().nth(1)?.parse::<f64>().ok()?;
        Some(kb / 1024.0)
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}
