//! Process inspection through procfs: the daemon's own memory, and the
//! other programs running beside it.

use hypercolor_types::host_software::HostProcess;

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

/// Every process the daemon can see, by name and command line.
///
/// Names come from `/proc/<pid>/comm` and command lines from
/// `/proc/<pid>/cmdline`, with arguments joined by spaces and quoted when
/// they contain whitespace. Kernel threads have no command line. A process
/// that exits mid-scan is skipped. Returns `None` off Linux or when `/proc`
/// cannot be listed.
#[must_use]
pub fn running_processes() -> Option<Vec<HostProcess>> {
    #[cfg(target_os = "linux")]
    {
        let entries = std::fs::read_dir("/proc").ok()?;
        let processes = entries
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.bytes().all(|byte| byte.is_ascii_digit()))
            })
            .filter_map(|entry| {
                let dir = entry.path();
                let comm = std::fs::read_to_string(dir.join("comm")).ok()?;
                let name = comm.trim_end_matches('\n').to_owned();
                let command_line = std::fs::read(dir.join("cmdline"))
                    .ok()
                    .and_then(|raw| join_cmdline(&raw));
                Some(HostProcess { name, command_line })
            })
            .collect();
        Some(processes)
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Turn NUL-separated `cmdline` bytes into one shell-like line.
#[cfg(target_os = "linux")]
fn join_cmdline(raw: &[u8]) -> Option<String> {
    let words: Vec<String> = raw
        .split(|byte| *byte == 0)
        .filter(|word| !word.is_empty())
        .map(|word| {
            let word = String::from_utf8_lossy(word);
            if word.contains(char::is_whitespace) {
                format!("\"{word}\"")
            } else {
                word.into_owned()
            }
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}
