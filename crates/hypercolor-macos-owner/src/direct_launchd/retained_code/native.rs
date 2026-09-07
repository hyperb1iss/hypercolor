use std::ffi::CString;
use std::io::Read as _;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::spawn::{PosixSpawnAttr, PosixSpawnFileActions, PosixSpawnFlags, posix_spawn};
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;
use security_framework::os::macos::code_signing::{
    Flags as CodeSigningFlags, GuestAttributes, SecCode, SecRequirement,
};

use super::{AcceptedImageProof, require_time};
use crate::MacosOwnerExecutionError;

const POSIX_SPAWN_START_SUSPENDED: i32 = 0x0080;
const POSIX_SPAWN_CLOEXEC_DEFAULT: i32 = 0x4000;
const MAX_CODESIGN_OUTPUT_BYTES: usize = 16 * 1_024;

pub(super) struct NativeAcceptedImageProof;

impl AcceptedImageProof for NativeAcceptedImageProof {
    fn prove(
        &mut self,
        path: &Path,
        designated_requirement: &str,
        cdhash: &str,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError> {
        let requirement = designated_requirement
            .parse::<SecRequirement>()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        let mut child = SuspendedChild::spawn(path, deadline)?;
        let proof = (|| {
            child.require_stopped(deadline)?;
            if !dynamic_code_matches(child.pid(), &requirement)? {
                return Ok(false);
            }
            child.require_stopped(deadline)?;
            if dynamic_cdhash(child.pid(), deadline)?.as_deref() != Some(cdhash) {
                return Ok(false);
            }
            child.require_stopped(deadline)?;
            if !dynamic_code_matches(child.pid(), &requirement)? {
                return Ok(false);
            }
            child.require_stopped(deadline)?;
            Ok(true)
        })();
        let terminal = child.kill_and_reap();
        match (proof, terminal) {
            (Ok(matches), Ok(())) => Ok(matches),
            (Err(error), Ok(())) => Err(error),
            (_, Err(error)) => Err(error),
        }
    }
}

struct SuspendedChild {
    pid: Option<Pid>,
}

impl SuspendedChild {
    fn spawn(path: &Path, deadline: Instant) -> Result<Self, MacosOwnerExecutionError> {
        let path = executable_path(path)?;
        Self::spawn_arguments(&path, &[path.as_c_str()], deadline)
    }

    fn spawn_arguments(
        path: &CString,
        arguments: &[&std::ffi::CStr],
        deadline: Instant,
    ) -> Result<Self, MacosOwnerExecutionError> {
        require_time(deadline)?;
        let actions = PosixSpawnFileActions::init()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        let mut attributes = PosixSpawnAttr::init()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        let flags = PosixSpawnFlags::from_bits_retain(POSIX_SPAWN_START_SUSPENDED)
            | PosixSpawnFlags::from_bits_retain(POSIX_SPAWN_CLOEXEC_DEFAULT);
        attributes
            .set_flags(flags)
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        let environment: [&std::ffi::CStr; 0] = [];
        let pid = posix_spawn(
            path.as_c_str(),
            &actions,
            &attributes,
            arguments,
            &environment,
        )
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        // Darwin returns only after START_SUSPENDED has stopped the image before main. Its
        // synthetic wait status carries signal zero, which nix intentionally cannot decode.
        Ok(Self { pid: Some(pid) })
    }

    fn pid(&self) -> Pid {
        self.pid.expect("live suspended child retains its pid")
    }

    fn require_stopped(&self, deadline: Instant) -> Result<(), MacosOwnerExecutionError> {
        require_time(deadline)?;
        match wait_child_state(self.pid())? {
            WaitStatus::StillAlive => Ok(()),
            status => Err(MacosOwnerExecutionError::new(format!(
                "suspended executable state drifted during validation: {status:?}"
            ))),
        }
    }

    fn kill_and_reap(&mut self) -> Result<(), MacosOwnerExecutionError> {
        let pid = self.pid.ok_or_else(|| {
            MacosOwnerExecutionError::new("suspended executable was already reaped")
        })?;
        kill(pid, Signal::SIGKILL)
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        loop {
            match waitpid(pid, None) {
                Ok(WaitStatus::Signaled(observed, Signal::SIGKILL, _)) if observed == pid => {
                    self.pid = None;
                    return Ok(());
                }
                Ok(status) => {
                    self.pid = None;
                    return Err(MacosOwnerExecutionError::new(format!(
                        "suspended executable did not terminate through SIGKILL: {status:?}"
                    )));
                }
                Err(Errno::EINTR) => {}
                Err(error) => return Err(MacosOwnerExecutionError::new(error.to_string())),
            }
        }
    }
}

fn executable_path(path: &Path) -> Result<CString, MacosOwnerExecutionError> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| MacosOwnerExecutionError::new("executable path contains an embedded NUL"))
}

impl Drop for SuspendedChild {
    fn drop(&mut self) {
        let Some(pid) = self.pid.take() else {
            return;
        };
        let _ = kill(pid, Signal::SIGKILL);
        loop {
            match waitpid(pid, None) {
                Err(Errno::EINTR) => {}
                _ => return,
            }
        }
    }
}

fn wait_child_state(pid: Pid) -> Result<WaitStatus, MacosOwnerExecutionError> {
    waitpid(pid, Some(WaitPidFlag::WNOHANG | WaitPidFlag::WCONTINUED))
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
}

fn dynamic_code_matches(
    pid: Pid,
    requirement: &SecRequirement,
) -> Result<bool, MacosOwnerExecutionError> {
    let mut attributes = GuestAttributes::new();
    attributes.set_pid(pid.as_raw());
    let code = match SecCode::copy_guest_with_attribues(None, &attributes, CodeSigningFlags::NONE) {
        Ok(code) => code,
        Err(error) if error.code() == 100_003 => return Ok(false),
        Err(error) => return Err(MacosOwnerExecutionError::new(error.to_string())),
    };
    let flags = CodeSigningFlags::STRICT_VALIDATE | CodeSigningFlags::NO_NETWORK_ACCESS;
    Ok(code.check_validity(flags, requirement).is_ok())
}

fn dynamic_cdhash(pid: Pid, deadline: Instant) -> Result<Option<String>, MacosOwnerExecutionError> {
    require_time(deadline)?;
    let pid_argument = format!("+{}", pid.as_raw());
    let output = run_bounded_codesign(&pid_argument, deadline)?;
    if output.status != Some(0) || !output.stdout.is_empty() {
        return Ok(None);
    }
    parse_cdhash(&output.stderr)
}

pub(in crate::direct_launchd) fn dynamic_cdhash_for_pid(
    pid: u32,
    deadline: Instant,
) -> Result<Option<String>, MacosOwnerExecutionError> {
    let pid = i32::try_from(pid)
        .ok()
        .filter(|pid| *pid > 0)
        .map(Pid::from_raw)
        .ok_or_else(|| MacosOwnerExecutionError::new("dynamic code PID is invalid"))?;
    dynamic_cdhash(pid, deadline)
}

fn parse_cdhash(output: &[u8]) -> Result<Option<String>, MacosOwnerExecutionError> {
    let output = std::str::from_utf8(output)
        .map_err(|_| MacosOwnerExecutionError::new("codesign returned non-UTF-8 output"))?;
    let mut cdhash = None;
    for line in output.lines() {
        let Some(value) = line.strip_prefix("CDHash=") else {
            continue;
        };
        if cdhash.is_some()
            || value.len() != 40
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Ok(None);
        }
        cdhash = Some(value.to_owned());
    }
    Ok(cdhash)
}

struct BoundedOutput {
    status: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_bounded_codesign(
    pid_argument: &str,
    deadline: Instant,
) -> Result<BoundedOutput, MacosOwnerExecutionError> {
    require_time(deadline)?;
    let mut child = Command::new("/usr/bin/codesign")
        .args(["--display", "--verbose=4", pid_argument])
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    let Some(stdout) = child.stdout.take() else {
        kill_command(&mut child);
        return Err(MacosOwnerExecutionError::new(
            "codesign stdout pipe is unavailable",
        ));
    };
    let Some(stderr) = child.stderr.take() else {
        kill_command(&mut child);
        return Err(MacosOwnerExecutionError::new(
            "codesign stderr pipe is unavailable",
        ));
    };
    let stdout_reader = std::thread::spawn(move || read_bounded(stdout));
    let stderr_reader = std::thread::spawn(move || read_bounded(stderr));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status.code().unwrap_or(-1)),
            Ok(None) => {}
            Err(error) => {
                kill_command(&mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(MacosOwnerExecutionError::new(error.to_string()));
            }
        }
        if Instant::now() >= deadline {
            kill_command(&mut child);
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(MacosOwnerExecutionError::new(
                "codesign exceeded the code validation deadline",
            ));
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let stdout = join_reader(stdout_reader)?;
    let stderr = join_reader(stderr_reader)?;
    Ok(BoundedOutput {
        status,
        stdout,
        stderr,
    })
}

fn read_bounded(mut stream: impl std::io::Read) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::with_capacity(MAX_CODESIGN_OUTPUT_BYTES.min(8 * 1_024));
    stream
        .by_ref()
        .take((MAX_CODESIGN_OUTPUT_BYTES + 1) as u64)
        .read_to_end(&mut output)?;
    if output.len() > MAX_CODESIGN_OUTPUT_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "codesign output exceeded 16 KiB",
        ));
    }
    Ok(output)
}

fn join_reader(
    reader: std::thread::JoinHandle<std::io::Result<Vec<u8>>>,
) -> Result<Vec<u8>, MacosOwnerExecutionError> {
    reader
        .join()
        .map_err(|_| MacosOwnerExecutionError::new("codesign output reader panicked"))?
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
}

fn kill_command(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill};
    use nix::sys::wait::{WaitPidFlag, waitpid};

    use super::{SuspendedChild, executable_path, parse_cdhash};

    #[test]
    fn cdhash_parser_requires_one_canonical_value() {
        assert_eq!(
            parse_cdhash(b"Identifier=x\nCDHash=0123456789abcdef0123456789abcdef01234567\n")
                .expect("canonical output should parse")
                .as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert!(
            parse_cdhash(b"CDHash=0123456789ABCDEF0123456789ABCDEF01234567\n")
                .expect("uppercase output should be ordinary mismatch")
                .is_none()
        );
        assert!(
            parse_cdhash(
                b"CDHash=0123456789abcdef0123456789abcdef01234567\nCDHash=0123456789abcdef0123456789abcdef01234567\n"
            )
            .expect("duplicate output should be ordinary mismatch")
            .is_none()
        );
    }

    #[test]
    fn executable_path_preserves_exact_nonlossy_bytes() {
        let path = Path::new("/private/unit/hypercolor-λ");
        assert_eq!(
            executable_path(path)
                .expect("UTF-8 path should encode")
                .as_bytes(),
            path.as_os_str().as_bytes()
        );
    }

    #[test]
    fn suspended_child_is_killed_and_reaped_without_executing_main() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let witness = directory.path().join("witness");
        let program = CString::new("/bin/sh").expect("program path should encode");
        let argument = CString::new("-c").expect("argument should encode");
        let script =
            CString::new(format!("touch '{}'", witness.display())).expect("script should encode");
        let mut child = SuspendedChild::spawn_arguments(
            &program,
            &[program.as_c_str(), argument.as_c_str(), script.as_c_str()],
            Instant::now() + Duration::from_secs(2),
        )
        .expect("test child should suspend");
        assert!(!witness.exists());
        let pid = child.pid();
        child.kill_and_reap().expect("test child should reap");
        assert_eq!(
            waitpid(pid, Some(WaitPidFlag::WNOHANG)).expect_err("child should already be reaped"),
            Errno::ECHILD
        );
        assert!(!witness.exists());
    }

    #[test]
    fn continued_child_is_detected_and_reaped() {
        let program = CString::new(Path::new("/bin/sleep").as_os_str().as_bytes())
            .expect("program path should encode");
        let duration = CString::new("10").expect("duration should encode");
        let mut child = SuspendedChild::spawn_arguments(
            &program,
            &[program.as_c_str(), duration.as_c_str()],
            Instant::now() + Duration::from_secs(2),
        )
        .expect("test child should suspend");
        let pid = child.pid();
        kill(pid, Signal::SIGCONT).expect("test child should continue");
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match child.require_stopped(deadline) {
                Ok(()) if Instant::now() < deadline => std::thread::yield_now(),
                Ok(()) => panic!("continued child state should become visible"),
                Err(error) => {
                    assert!(error.to_string().contains("state drifted"));
                    break;
                }
            }
        }
        child.kill_and_reap().expect("continued child should reap");
    }

    #[test]
    fn expired_child_proof_is_killed_and_reaped() {
        let program = CString::new("/bin/sleep").expect("program path should encode");
        let duration = CString::new("10").expect("duration should encode");
        let mut child = SuspendedChild::spawn_arguments(
            &program,
            &[program.as_c_str(), duration.as_c_str()],
            Instant::now() + Duration::from_secs(2),
        )
        .expect("test child should suspend");
        let pid = child.pid();
        let error = child
            .require_stopped(Instant::now())
            .expect_err("expired proof should fail");
        assert!(error.to_string().contains("deadline"));
        child.kill_and_reap().expect("expired child should reap");
        assert_eq!(
            waitpid(pid, Some(WaitPidFlag::WNOHANG)).expect_err("child should already be reaped"),
            Errno::ECHILD
        );
    }
}
