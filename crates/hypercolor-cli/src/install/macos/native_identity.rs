use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::super::InstallPlatformError;
use super::model::error;

const MAX_CODESIGN_BYTES: usize = 16 * 1024;
const CODESIGN_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn codesign_requirement(path: &Path) -> Result<String, InstallPlatformError> {
    let started = Instant::now();
    let path = path
        .to_str()
        .ok_or_else(|| error("macOS codesign path is not exact UTF-8"))?;
    let mut child = Command::new("/usr/bin/codesign")
        .args(["-d", "-r-", path])
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(io_error)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| error("codesign stdout pipe is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| error("codesign stderr pipe is unavailable"))?;
    let stdout = std::thread::spawn(move || read_bounded(stdout));
    let stderr = std::thread::spawn(move || read_bounded(stderr));
    let status = loop {
        if let Some(status) = child.try_wait().map_err(io_error)? {
            break status;
        }
        if started.elapsed() >= CODESIGN_TIMEOUT {
            let kill = child.kill();
            let reap = child.wait();
            let stdout = stdout
                .join()
                .map_err(|_| error("codesign stdout reader panicked"))?;
            let stderr = stderr
                .join()
                .map_err(|_| error("codesign stderr reader panicked"))?;
            kill.map_err(io_error)?;
            reap.map_err(io_error)?;
            stdout?;
            stderr?;
            return Err(error(
                "codesign requirement inspection exceeded its deadline",
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let stdout = stdout
        .join()
        .map_err(|_| error("codesign stdout reader panicked"))??;
    let stderr = stderr
        .join()
        .map_err(|_| error("codesign stderr reader panicked"))??;
    if !status.success() {
        return Err(error("codesign requirement inspection failed"));
    }
    let mut requirements = Vec::new();
    for bytes in [&stdout, &stderr] {
        let output = std::str::from_utf8(bytes)
            .map_err(|_| error("codesign requirement output is not exact UTF-8"))?;
        requirements.extend(output.lines().filter_map(|line| {
            line.strip_prefix("designated => ")
                .or_else(|| line.strip_prefix("# designated => "))
        }));
    }
    match requirements.as_slice() {
        [requirement] if !requirement.is_empty() && requirement.len() <= 8 * 1024 => {
            Ok((*requirement).to_owned())
        }
        _ => Err(error(
            "codesign returned malformed or ambiguous requirements",
        )),
    }
}

fn read_bounded(mut reader: impl Read) -> Result<Vec<u8>, InstallPlatformError> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(MAX_CODESIGN_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > MAX_CODESIGN_BYTES {
        return Err(error("codesign output exceeds its byte bound"));
    }
    Ok(bytes)
}

fn io_error(source: std::io::Error) -> InstallPlatformError {
    error(source.to_string())
}
