use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::MacosOwnerExecutionError;

const MAX_USER_TEMP_DIRECTORY_BYTES: usize = 4_096;
const MACOS_DAEMON_GUARD_FILE_NAME: &str = "hypercolor-daemon.lock";
const RESOLUTION_TIMEOUT: Duration = Duration::from_secs(10);

/// Resolve the stable per-user daemon guard independently of `TMPDIR`.
pub fn canonical_macos_daemon_guard_path() -> Result<PathBuf, MacosOwnerExecutionError> {
    let mut child = Command::new("/usr/bin/getconf")
        .arg("DARWIN_USER_TEMP_DIR")
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    let Some(stdout) = child.stdout.take() else {
        kill_and_reap(&mut child);
        return Err(MacosOwnerExecutionError::new(
            "getconf stdout pipe is unavailable",
        ));
    };
    let Some(stderr) = child.stderr.take() else {
        kill_and_reap(&mut child);
        return Err(MacosOwnerExecutionError::new(
            "getconf stderr pipe is unavailable",
        ));
    };
    let stdout_reader = match std::thread::Builder::new()
        .name("getconf-stdout".to_owned())
        .spawn(move || read_bounded(stdout))
    {
        Ok(reader) => reader,
        Err(error) => {
            kill_and_reap(&mut child);
            return Err(MacosOwnerExecutionError::new(error.to_string()));
        }
    };
    let stderr_reader = match std::thread::Builder::new()
        .name("getconf-stderr".to_owned())
        .spawn(move || read_bounded(stderr))
    {
        Ok(reader) => reader,
        Err(error) => {
            kill_and_reap(&mut child);
            let _ = stdout_reader.join();
            return Err(MacosOwnerExecutionError::new(error.to_string()));
        }
    };
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                kill_and_reap(&mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(MacosOwnerExecutionError::new(error.to_string()));
            }
        }
        let Some(remaining) = RESOLUTION_TIMEOUT.checked_sub(started.elapsed()) else {
            kill_and_reap(&mut child);
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(MacosOwnerExecutionError::new(
                "canonical macOS user temporary directory resolution timed out",
            ));
        };
        std::thread::sleep(remaining.min(Duration::from_millis(5)));
    };
    let stdout = join_reader(stdout_reader);
    let stderr = join_reader(stderr_reader);
    let stdout = stdout?;
    let stderr = stderr?;
    parse_getconf_output(status.success(), &stdout, &stderr)
}

fn parse_getconf_output(
    success: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<PathBuf, MacosOwnerExecutionError> {
    if !success || !stderr.is_empty() {
        return Err(MacosOwnerExecutionError::new(
            "canonical macOS user temporary directory resolution failed",
        ));
    }
    let directory = std::str::from_utf8(&stdout)
        .map_err(|_| {
            MacosOwnerExecutionError::new(
                "canonical macOS user temporary directory is not valid UTF-8",
            )
        })?
        .strip_suffix('\n')
        .ok_or_else(|| {
            MacosOwnerExecutionError::new(
                "canonical macOS user temporary directory has no line terminator",
            )
        })?;
    if directory.is_empty() || directory.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(MacosOwnerExecutionError::new(
            "canonical macOS user temporary directory output is malformed",
        ));
    }
    let path = Path::new(directory);
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(MacosOwnerExecutionError::new(
            "canonical macOS user temporary directory is not absolute",
        ));
    }
    let mut canonical = PathBuf::from("/");
    let mut count = 0_usize;
    for component in components {
        let Component::Normal(component) = component else {
            return Err(MacosOwnerExecutionError::new(
                "canonical macOS user temporary directory has unsafe components",
            ));
        };
        canonical.push(component);
        count += 1;
    }
    if count == 0 {
        return Err(MacosOwnerExecutionError::new(
            "canonical macOS user temporary directory cannot be the filesystem root",
        ));
    }
    let canonical_text = canonical
        .to_str()
        .expect("canonical path built from valid UTF-8 remains valid UTF-8");
    if directory != canonical_text && directory.strip_suffix('/') != Some(canonical_text) {
        return Err(MacosOwnerExecutionError::new(
            "canonical macOS user temporary directory has an ambiguous representation",
        ));
    }
    Ok(canonical.join(MACOS_DAEMON_GUARD_FILE_NAME))
}

fn read_bounded(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(MAX_USER_TEMP_DIRECTORY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn join_reader(
    reader: std::thread::JoinHandle<std::io::Result<Vec<u8>>>,
) -> Result<Vec<u8>, MacosOwnerExecutionError> {
    let bytes = reader
        .join()
        .map_err(|_| MacosOwnerExecutionError::new("getconf output reader panicked"))?
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    if bytes.len() > MAX_USER_TEMP_DIRECTORY_BYTES {
        return Err(MacosOwnerExecutionError::new(
            "canonical macOS user temporary directory output is unbounded",
        ));
    }
    Ok(bytes)
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::parse_getconf_output;

    #[test]
    fn getconf_output_requires_one_canonical_absolute_directory() {
        assert_eq!(
            parse_getconf_output(true, b"/var/folders/ab/T/\n", b"")
                .expect("canonical trailing slash should parse"),
            std::path::Path::new("/var/folders/ab/T/hypercolor-daemon.lock")
        );
        for invalid in [
            b"/var/folders/ab/../T/\n".as_slice(),
            b"/var//folders/ab/T/\n",
            b"/var/folders/ab/T//\n",
            b"/var/folders/ab/T/\r\n",
            b"relative/T/\n",
            b"/\n",
            b"/var/folders/ab/T\nextra\n",
        ] {
            assert!(parse_getconf_output(true, invalid, b"").is_err());
        }
        assert!(parse_getconf_output(false, b"/var/folders/ab/T/\n", b"").is_err());
        assert!(parse_getconf_output(true, b"/var/folders/ab/T/\n", b"warning").is_err());
    }
}
