use std::fmt::Write as _;
use std::io::Read;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use core_foundation::{base::TCFType, data::CFData};
use security_framework::os::macos::code_signing::{
    Flags as CodeSigningFlags, GuestAttributes, SecCode, SecRequirement,
};
use sha2::{Digest, Sha256};

use super::{
    MACOS_DIRECT_LAUNCHD_LABEL, MacosDirectLaunchdExecutableExpectation,
    MacosDirectLaunchdInspector, MacosDirectLaunchdState, MacosOwnerExecutionError,
    parse_direct_launchd_service_state, requirement_hash,
};

const MAX_CODESIGN_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_DESIGNATED_REQUIREMENT_BYTES: usize = 8 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Native exact-identity inspector for the current user's direct service.
#[derive(Debug, Clone, Copy)]
pub struct NativeMacosDirectLaunchdInspector {
    uid: u32,
}

impl NativeMacosDirectLaunchdInspector {
    /// Construct an inspector for the effective user's launchd GUI domain.
    #[must_use]
    pub fn new() -> Self {
        Self {
            uid: nix::unistd::Uid::effective().as_raw(),
        }
    }

    fn target(&self) -> String {
        format!("gui/{}/{MACOS_DIRECT_LAUNCHD_LABEL}", self.uid)
    }
}

impl Default for NativeMacosDirectLaunchdInspector {
    fn default() -> Self {
        Self::new()
    }
}

impl MacosDirectLaunchdInspector for NativeMacosDirectLaunchdInspector {
    fn inspect_direct_launchd(
        &mut self,
    ) -> Result<MacosDirectLaunchdState, MacosOwnerExecutionError> {
        let target = self.target();
        let output = run_bounded_command(
            "/bin/launchctl",
            &["print", &target],
            super::MAX_LAUNCHCTL_OUTPUT_BYTES,
            super::MAX_LAUNCHCTL_OUTPUT_BYTES,
            COMMAND_TIMEOUT,
        )?;
        parse_direct_launchd_service_state(self.uid, output.status, &output.stdout, &output.stderr)
    }

    fn live_identity_matches(
        &mut self,
        identity: &crate::MacosOwnerIdentity,
    ) -> Result<bool, MacosOwnerExecutionError> {
        let deadline = Instant::now()
            .checked_add(COMMAND_TIMEOUT)
            .ok_or_else(|| MacosOwnerExecutionError::new("inspection deadline overflowed"))?;
        let identity = identity.clone();
        run_identity_inspection(deadline, move || {
            Self::new().live_identity_matches_until(&identity, deadline)
        })
    }

    fn publication_identity_matches(
        &mut self,
        identity: &crate::MacosOwnerIdentity,
        executable: &MacosDirectLaunchdExecutableExpectation,
    ) -> Result<bool, MacosOwnerExecutionError> {
        let deadline = Instant::now()
            .checked_add(COMMAND_TIMEOUT)
            .ok_or_else(|| MacosOwnerExecutionError::new("inspection deadline overflowed"))?;
        let identity = identity.clone();
        let executable = executable.clone();
        run_identity_inspection(deadline, move || {
            Self::new().publication_identity_matches_until(&identity, &executable, deadline)
        })
    }
}

fn run_identity_inspection(
    deadline: Instant,
    operation: impl FnOnce() -> Result<bool, MacosOwnerExecutionError> + Send + 'static,
) -> Result<bool, MacosOwnerExecutionError> {
    super::mutation::run_deadline_read(deadline, operation)
}

impl NativeMacosDirectLaunchdInspector {
    pub(in crate::direct_launchd) fn live_identity_matches_until(
        &mut self,
        identity: &crate::MacosOwnerIdentity,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError> {
        let Some(audit_token) = parse_audit_token(identity)? else {
            return Ok(false);
        };
        let Some(code) = code_for_audit_token(&audit_token)? else {
            return Ok(false);
        };
        if !code_path_matches(&code, &identity.executable_path)? {
            return Ok(false);
        }
        let requirement_text = designated_requirement(&identity.executable_path, deadline)?;
        if requirement_hash(&requirement_text) != identity.designated_requirement_hash {
            return Ok(false);
        }
        let requirement = requirement_text
            .parse::<SecRequirement>()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        if code
            .check_validity(CodeSigningFlags::STRICT_VALIDATE, &requirement)
            .is_err()
        {
            return Ok(false);
        }
        code_for_audit_token(&audit_token)?
            .map(|code| code_path_matches(&code, &identity.executable_path))
            .transpose()
            .map(Option::unwrap_or_default)
    }
    pub(in crate::direct_launchd) fn publication_identity_matches_until(
        &mut self,
        identity: &crate::MacosOwnerIdentity,
        executable: &MacosDirectLaunchdExecutableExpectation,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError> {
        let Some(audit_token) = parse_audit_token(identity)? else {
            return Ok(false);
        };
        let Some(code) = code_for_audit_token(&audit_token)? else {
            return Ok(false);
        };
        let requirement_before = publication_code_matches(&code, executable)?;
        if !requirement_before {
            return Ok(false);
        }
        let mut opened = hypercolor_platform_fs::open_no_follow(executable.path())
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        let before = opened
            .metadata()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        if !executable_metadata_matches(&before, executable) {
            return Ok(false);
        }
        let mut hasher = Sha256::new();
        let mut remaining = executable.size() + 1;
        let mut buffer = [0_u8; 8 * 1024];
        while remaining > 0 {
            let limit = usize::try_from(remaining.min(buffer.len() as u64))
                .expect("bounded read limit fits usize");
            let read = opened
                .read(&mut buffer[..limit])
                .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            remaining -= u64::try_from(read).expect("read length fits u64");
        }
        let digest = hasher.finalize();
        if remaining != 1 || hex_digest(&digest) != executable.sha256() {
            return Ok(false);
        }
        let after = opened
            .metadata()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        if !executable_metadata_matches(&after, executable)
            || before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.mode() != after.mode()
            || before.len() != after.len()
            || before.nlink() != after.nlink()
        {
            return Ok(false);
        }
        let observed_cdhash = super::retained_code::dynamic_cdhash_for_pid(identity.pid, deadline)?;
        let Some(code_after) = code_for_audit_token(&audit_token)? else {
            return Ok(false);
        };
        let requirement_after = publication_code_matches(&code_after, executable)?;
        Ok(publication_tuple_matches(
            requirement_before,
            observed_cdhash.as_deref(),
            requirement_after,
            executable.cdhash(),
        ))
    }
}

fn publication_tuple_matches(
    requirement_before: bool,
    observed_cdhash: Option<&str>,
    requirement_after: bool,
    expected_cdhash: &str,
) -> bool {
    requirement_before && observed_cdhash == Some(expected_cdhash) && requirement_after
}

fn parse_audit_token(
    identity: &crate::MacosOwnerIdentity,
) -> Result<Option<[u8; 32]>, MacosOwnerExecutionError> {
    let mut token = [0_u8; 32];
    let mut words = identity.audit_token_identity.split(':');
    for index in 0..8 {
        let Some(word) = words.next() else {
            return Ok(None);
        };
        if word.len() != 8 || !word.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(None);
        }
        let value = u32::from_str_radix(word, 16)
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        if index == 5 && value != identity.pid {
            return Ok(None);
        }
        token[index * 4..index * 4 + 4].copy_from_slice(&value.to_ne_bytes());
    }
    Ok(words.next().is_none().then_some(token))
}

fn code_for_audit_token(
    audit_token: &[u8; 32],
) -> Result<Option<SecCode>, MacosOwnerExecutionError> {
    let token_data = CFData::from_buffer(audit_token);
    let mut attributes = GuestAttributes::new();
    attributes.set_audit_token(token_data.as_concrete_TypeRef());
    match SecCode::copy_guest_with_attribues(None, &attributes, CodeSigningFlags::NONE) {
        Ok(code) => Ok(Some(code)),
        Err(error) if error.code() == 100_003 => Ok(None),
        Err(error) => Err(MacosOwnerExecutionError::new(error.to_string())),
    }
}

fn code_path_matches(code: &SecCode, expected: &Path) -> Result<bool, MacosOwnerExecutionError> {
    let Some(observed) = code
        .path(CodeSigningFlags::NONE)
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?
        .to_path()
    else {
        return Ok(false);
    };
    Ok(observed == expected)
}

fn designated_requirement(
    path: &Path,
    deadline: Instant,
) -> Result<String, MacosOwnerExecutionError> {
    let path = path.to_str().ok_or_else(|| {
        MacosOwnerExecutionError::new("codesign executable path is not valid UTF-8")
    })?;
    let timeout = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| {
            MacosOwnerExecutionError::new("live identity inspection exceeded its absolute deadline")
        })?;
    let output = run_bounded_command(
        "/usr/bin/codesign",
        &["-d", "-r-", path],
        MAX_CODESIGN_OUTPUT_BYTES,
        MAX_CODESIGN_OUTPUT_BYTES,
        timeout,
    )?;
    if output.status != Some(0) {
        return Err(MacosOwnerExecutionError::new(format!(
            "codesign could not read the live designated requirement with status {:?}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = std::str::from_utf8(&output.stdout).map_err(|_| {
        MacosOwnerExecutionError::new("codesign returned a non-UTF-8 designated requirement")
    })?;
    let requirement = stdout.lines().find_map(|line| {
        line.strip_prefix("designated => ")
            .or_else(|| line.strip_prefix("# designated => "))
    });
    let Some(requirement) = requirement else {
        return Err(MacosOwnerExecutionError::new(
            "codesign omitted the live designated requirement",
        ));
    };
    if requirement.is_empty() || requirement.len() > MAX_DESIGNATED_REQUIREMENT_BYTES {
        return Err(MacosOwnerExecutionError::new(
            "codesign designated requirement is empty or exceeds 8 KiB",
        ));
    }
    Ok(requirement.to_owned())
}

fn publication_code_matches(
    code: &SecCode,
    executable: &MacosDirectLaunchdExecutableExpectation,
) -> Result<bool, MacosOwnerExecutionError> {
    let Some(observed) = code
        .path(CodeSigningFlags::NONE)
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?
        .to_path()
    else {
        return Ok(false);
    };
    if observed != executable.path() {
        return Ok(false);
    }
    let requirement = executable
        .designated_requirement()
        .parse::<SecRequirement>()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    Ok(code
        .check_validity(CodeSigningFlags::STRICT_VALIDATE, &requirement)
        .is_ok())
}

fn executable_metadata_matches(
    metadata: &std::fs::Metadata,
    executable: &MacosDirectLaunchdExecutableExpectation,
) -> bool {
    metadata.is_file()
        && metadata.mode() & 0o7777 == executable.mode()
        && metadata.len() == executable.size()
        && metadata.nlink() == 1
        && metadata.dev() == executable.device()
        && metadata.ino() == executable.inode()
}

fn hex_digest(digest: &[u8]) -> String {
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("writing into a String cannot fail");
    }
    output
}

#[derive(Debug)]
struct BoundedCommandOutput {
    status: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_bounded_command(
    program: &str,
    args: &[&str],
    max_stdout: usize,
    max_stderr: usize,
    timeout: Duration,
) -> Result<BoundedCommandOutput, MacosOwnerExecutionError> {
    let mut child = Command::new(program)
        .args(args)
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| MacosOwnerExecutionError::new("command stdout pipe is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| MacosOwnerExecutionError::new("command stderr pipe is unavailable"))?;
    let stdout_reader = std::thread::spawn(move || read_bounded(stdout, max_stdout));
    let stderr_reader = std::thread::spawn(move || read_bounded(stderr, max_stderr));

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?
        {
            break status;
        }
        if started.elapsed() >= timeout {
            child
                .kill()
                .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
            child
                .wait()
                .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(MacosOwnerExecutionError::new(
                "macOS owner command exceeded its absolute deadline",
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    let stdout = join_bounded_reader(stdout_reader, "stdout", max_stdout)?;
    let stderr = join_bounded_reader(stderr_reader, "stderr", max_stderr)?;
    Ok(BoundedCommandOutput {
        status: status.code(),
        stdout,
        stderr,
    })
}

fn read_bounded(mut reader: impl Read, max_bytes: usize) -> Result<Vec<u8>, std::io::Error> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn join_bounded_reader(
    reader: std::thread::JoinHandle<Result<Vec<u8>, std::io::Error>>,
    stream: &'static str,
    max_bytes: usize,
) -> Result<Vec<u8>, MacosOwnerExecutionError> {
    let bytes = reader
        .join()
        .map_err(|_| MacosOwnerExecutionError::new("macOS owner output reader panicked"))?
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    if bytes.len() > max_bytes {
        return Err(MacosOwnerExecutionError::new(format!(
            "macOS owner command {stream} exceeds its byte bound"
        )));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn restored_path_with_same_requirement_but_different_cdhash_is_rejected() {
        assert!(!publication_tuple_matches(
            true,
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            true,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ));
    }

    #[test]
    fn blocked_identity_proof_times_out_without_queueing_and_releases_worker() {
        let _test_gate = super::super::mutation::INSPECTION_WORKER_TEST_GATE
            .lock()
            .expect("worker test gate should lock");
        let live = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let worker_live = Arc::clone(&live);
        let worker_maximum = Arc::clone(&maximum);
        let caller = std::thread::spawn(move || {
            run_identity_inspection(Instant::now() + Duration::from_millis(25), move || {
                let current = worker_live.fetch_add(1, Ordering::SeqCst) + 1;
                worker_maximum.fetch_max(current, Ordering::SeqCst);
                started_tx.send(()).expect("blocked proof should start");
                release_rx.recv().expect("blocked proof should release");
                worker_live.fetch_sub(1, Ordering::SeqCst);
                Ok(true)
            })
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocked proof should enter its worker");
        let error = caller
            .join()
            .expect("publication caller should not panic")
            .expect_err("blocked proof should exceed its caller deadline");
        assert!(error.to_string().contains("absolute deadline"));
        assert_eq!(live.load(Ordering::SeqCst), 1);

        let second = run_identity_inspection(Instant::now() + Duration::from_secs(1), || Ok(true))
            .expect_err("occupied worker must not queue another proof");
        assert!(second.to_string().contains("slot is busy"));
        assert_eq!(maximum.load(Ordering::SeqCst), 1);

        release_tx.send(()).expect("blocked proof should finish");
        assert!(super::super::mutation::wait_for_inspection_worker_idle(
            Duration::from_secs(1)
        ));
        assert_eq!(live.load(Ordering::SeqCst), 0);
        assert!(
            run_identity_inspection(Instant::now() + Duration::from_secs(1), || Ok(true),)
                .expect("released worker should accept the next proof")
        );
    }

    #[test]
    fn command_boundary_caps_output_and_wall_clock() {
        let oversized = run_bounded_command(
            "/bin/sh",
            &["-c", "printf 12345"],
            4,
            4,
            Duration::from_secs(1),
        )
        .expect_err("oversized stdout should fail");
        assert!(oversized.to_string().contains("stdout exceeds"));

        let started = Instant::now();
        let timeout = run_bounded_command(
            "/bin/sh",
            &["-c", "exec /bin/sleep 1"],
            4,
            4,
            Duration::from_millis(25),
        )
        .expect_err("slow command should fail");
        assert!(timeout.to_string().contains("absolute deadline"));
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
