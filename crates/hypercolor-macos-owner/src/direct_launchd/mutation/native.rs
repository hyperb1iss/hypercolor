use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::Read;
use std::os::unix::fs::MetadataExt as _;
use std::process::{Child, Command, Stdio};
use std::sync::{Condvar, Mutex, TryLockError};
use std::time::{Duration, Instant};

use sha2::{Digest as _, Sha256};

use super::{
    DeadlineDirectLaunchdInspector, LaunchctlAction, LaunchctlCommandBoundary,
    MacosDirectLaunchdBootstrapExpectation, MacosDirectLaunchdMutationOutcome,
    MacosDirectLaunchdMutator, MutationController, SubmittedCommand,
};
use crate::{
    MACOS_DIRECT_LAUNCHD_LABEL, MacosDirectLaunchdExecutableExpectation,
    MacosDirectLaunchdInspector, MacosDirectLaunchdOwnerProof,
    MacosDirectLaunchdPublicationExpectation, MacosDirectLaunchdState, MacosOwnerExecutionError,
    MacosOwnerIdentity, MacosOwnerStore, NativeMacosDirectLaunchdInspector,
};

const COMMAND_OUTPUT_BYTES: usize = 64 * 1024;
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(5);
// Timed-out Security.framework and read-only launchctl probes cannot be cancelled.
// The install lock serializes callers, while this slot bounds any late worker.
static INSPECTION_WORKER_SLOT: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Native fixed-label launchd mutation authority for the effective user's GUI domain.
#[derive(Debug)]
pub struct NativeMacosDirectLaunchdMutator {
    store: MacosOwnerStore,
    inspector: DeadlineNativeMacosDirectLaunchdInspector,
    commands: NativeLaunchctlBoundary,
}

impl NativeMacosDirectLaunchdMutator {
    /// Construct an authority over Hypercolor's fixed direct-launchd label.
    #[must_use]
    pub fn new(store: MacosOwnerStore) -> Self {
        let uid = nix::unistd::Uid::effective().as_raw();
        Self {
            store,
            inspector: DeadlineNativeMacosDirectLaunchdInspector,
            commands: NativeLaunchctlBoundary { uid },
        }
    }

    fn controller(
        &mut self,
    ) -> MutationController<'_, DeadlineNativeMacosDirectLaunchdInspector, NativeLaunchctlBoundary>
    {
        MutationController {
            store: &self.store,
            inspector: &mut self.inspector,
            commands: &mut self.commands,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct DeadlineNativeMacosDirectLaunchdInspector;

impl DeadlineDirectLaunchdInspector for DeadlineNativeMacosDirectLaunchdInspector {
    fn inspect_direct_launchd_until(
        &mut self,
        deadline: Instant,
    ) -> Result<MacosDirectLaunchdState, MacosOwnerExecutionError> {
        run_deadline_read(deadline, || {
            NativeMacosDirectLaunchdInspector::new().inspect_direct_launchd()
        })
    }

    fn live_identity_matches_until(
        &mut self,
        identity: &MacosOwnerIdentity,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError> {
        let identity = identity.clone();
        run_deadline_read(deadline, move || {
            NativeMacosDirectLaunchdInspector::new().live_identity_matches(&identity)
        })
    }

    fn publication_identity_matches_until(
        &mut self,
        identity: &MacosOwnerIdentity,
        executable: &MacosDirectLaunchdExecutableExpectation,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError> {
        let identity = identity.clone();
        let executable = executable.clone();
        run_deadline_read(deadline, move || {
            NativeMacosDirectLaunchdInspector::new()
                .publication_identity_matches(&identity, &executable)
        })
    }
}

fn run_deadline_read<T: Send + 'static>(
    deadline: Instant,
    operation: impl FnOnce() -> Result<T, MacosOwnerExecutionError> + Send + 'static,
) -> Result<T, MacosOwnerExecutionError> {
    deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| {
            MacosOwnerExecutionError::new("launchd inspection deadline expired before submission")
        })?;
    let worker_slot = InspectionWorkerSlot::acquire()?;
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("launchd-inspection".to_owned())
        .spawn(move || {
            let result = operation();
            drop(worker_slot);
            let _ = result_tx.send(result);
        })
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| {
            MacosOwnerExecutionError::new("launchd inspection exceeded its absolute deadline")
        })?;
    let result = result_rx
        .recv_timeout(remaining)
        .map_err(|error| match error {
            std::sync::mpsc::RecvTimeoutError::Timeout => {
                MacosOwnerExecutionError::new("launchd inspection exceeded its absolute deadline")
            }
            std::sync::mpsc::RecvTimeoutError::Disconnected => {
                MacosOwnerExecutionError::new("launchd inspection worker disconnected")
            }
        })?;
    if Instant::now() > deadline {
        return Err(MacosOwnerExecutionError::new(
            "launchd inspection exceeded its absolute deadline",
        ));
    }
    result
}

struct InspectionWorkerSlot;

impl InspectionWorkerSlot {
    fn acquire() -> Result<Self, MacosOwnerExecutionError> {
        let mut active = match INSPECTION_WORKER_SLOT.0.try_lock() {
            Ok(active) => active,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => {
                return Err(MacosOwnerExecutionError::new(
                    "launchd inspection worker slot is busy",
                ));
            }
        };
        if *active {
            return Err(MacosOwnerExecutionError::new(
                "launchd inspection worker slot is busy",
            ));
        }
        *active = true;
        Ok(Self)
    }
}

impl Drop for InspectionWorkerSlot {
    fn drop(&mut self) {
        let mut active = INSPECTION_WORKER_SLOT
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *active = false;
        INSPECTION_WORKER_SLOT.1.notify_all();
    }
}

impl MacosDirectLaunchdMutator for NativeMacosDirectLaunchdMutator {
    fn autostart_enabled(&mut self) -> Result<bool, MacosOwnerExecutionError> {
        self.controller().autostart_enabled()
    }

    fn set_autostart(
        &mut self,
        enabled: bool,
        timeout: Duration,
    ) -> Result<MacosDirectLaunchdMutationOutcome<()>, MacosOwnerExecutionError> {
        self.controller().set_autostart(enabled, timeout)
    }

    fn bootout_exact(
        &mut self,
        expected: &MacosDirectLaunchdOwnerProof,
        timeout: Duration,
    ) -> Result<MacosDirectLaunchdMutationOutcome<()>, MacosOwnerExecutionError> {
        self.controller().bootout_exact(expected, timeout)
    }

    fn bootstrap_and_kickstart_exact(
        &mut self,
        source: &MacosDirectLaunchdBootstrapExpectation,
        expected: &MacosDirectLaunchdPublicationExpectation,
        timeout: Duration,
    ) -> Result<
        MacosDirectLaunchdMutationOutcome<MacosDirectLaunchdOwnerProof>,
        MacosOwnerExecutionError,
    > {
        self.controller()
            .bootstrap_and_kickstart_exact(source, expected, timeout)
    }
}

#[derive(Debug)]
struct NativeLaunchctlBoundary {
    uid: u32,
}

impl NativeLaunchctlBoundary {
    fn domain(&self) -> String {
        format!("gui/{}", self.uid)
    }

    fn target(&self) -> String {
        format!("{}/{MACOS_DIRECT_LAUNCHD_LABEL}", self.domain())
    }

    fn arguments(&self, action: &LaunchctlAction) -> Vec<OsString> {
        let domain = self.domain();
        let target = self.target();
        match action {
            LaunchctlAction::PrintDisabled => vec!["print-disabled".into(), domain.into()],
            LaunchctlAction::Enable => vec!["enable".into(), target.into()],
            LaunchctlAction::Disable => vec!["disable".into(), target.into()],
            LaunchctlAction::Bootstrap(path) => {
                vec!["bootstrap".into(), domain.into(), path.as_os_str().into()]
            }
            LaunchctlAction::Kickstart => vec!["kickstart".into(), "-p".into(), target.into()],
            LaunchctlAction::Bootout => vec!["bootout".into(), "--wait".into(), target.into()],
        }
    }

    fn run_launchctl(
        &self,
        action: &LaunchctlAction,
        deadline: Instant,
    ) -> Result<SubmittedCommand, MacosOwnerExecutionError> {
        if Instant::now() >= deadline {
            return Err(MacosOwnerExecutionError::new(
                "launchd mutation deadline expired before command submission",
            ));
        }
        let mut command = Command::new("/bin/launchctl");
        command.args(self.arguments(action));
        let child = command
            .env_clear()
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        Ok(reap_submitted_command(child, deadline))
    }
}

impl LaunchctlCommandBoundary for NativeLaunchctlBoundary {
    fn run(
        &mut self,
        action: &LaunchctlAction,
        deadline: Instant,
    ) -> Result<SubmittedCommand, MacosOwnerExecutionError> {
        self.run_launchctl(action, deadline)
    }

    fn bootstrap_source_matches(
        &mut self,
        source: &MacosDirectLaunchdBootstrapExpectation,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError> {
        let source = source.clone();
        run_deadline_read(deadline, move || exact_bootstrap_source_matches(&source))
    }
}

fn reap_submitted_command(mut child: Child, deadline: Instant) -> SubmittedCommand {
    let Some(stdout) = child.stdout.take() else {
        kill_and_reap(&mut child);
        return SubmittedCommand::Unknown;
    };
    let Some(stderr) = child.stderr.take() else {
        kill_and_reap(&mut child);
        return SubmittedCommand::Unknown;
    };
    let Ok(stdout_reader) = std::thread::Builder::new()
        .name("launchctl-stdout".to_owned())
        .spawn(move || read_bounded(stdout))
    else {
        kill_and_reap(&mut child);
        return SubmittedCommand::Unknown;
    };
    let Ok(stderr_reader) = std::thread::Builder::new()
        .name("launchctl-stderr".to_owned())
        .spawn(move || read_bounded(stderr))
    else {
        kill_and_reap(&mut child);
        let _ = stdout_reader.join();
        return SubmittedCommand::Unknown;
    };

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) if Instant::now() <= deadline => break Some(status.code()),
            Ok(Some(_)) => break None,
            Ok(None) => {}
            Err(_) => {
                kill_and_reap(&mut child);
                break None;
            }
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            kill_and_reap(&mut child);
            break None;
        };
        std::thread::sleep(remaining.min(COMMAND_POLL_INTERVAL));
    };

    let stdout = join_reader(stdout_reader);
    let stderr = join_reader(stderr_reader);
    match (status, stdout, stderr) {
        (Some(status), Some(stdout), Some(stderr)) => SubmittedCommand::Completed {
            status,
            stdout,
            stderr,
        },
        _ => SubmittedCommand::Unknown,
    }
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn read_bounded(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(COMMAND_OUTPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn join_reader(reader: std::thread::JoinHandle<std::io::Result<Vec<u8>>>) -> Option<Vec<u8>> {
    reader
        .join()
        .ok()?
        .ok()
        .filter(|bytes| bytes.len() <= COMMAND_OUTPUT_BYTES)
}

fn exact_bootstrap_source_matches(
    source: &MacosDirectLaunchdBootstrapExpectation,
) -> Result<bool, MacosOwnerExecutionError> {
    let mut opened = hypercolor_platform_fs::open_no_follow(source.path())
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    let before = opened
        .metadata()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    if !bootstrap_metadata_matches(&before, source) {
        return Ok(false);
    }
    let mut hasher = Sha256::new();
    let mut remaining = source.size() + 1;
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
    if remaining != 1 || hex_digest(&hasher.finalize()) != source.sha256() {
        return Ok(false);
    }
    let after = opened
        .metadata()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    Ok(bootstrap_metadata_matches(&after, source)
        && before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.mode() == after.mode()
        && before.len() == after.len()
        && before.nlink() == after.nlink())
}

fn bootstrap_metadata_matches(
    metadata: &std::fs::Metadata,
    source: &MacosDirectLaunchdBootstrapExpectation,
) -> bool {
    metadata.is_file()
        && metadata.mode() & 0o7777 == source.mode()
        && metadata.len() == source.size()
        && metadata.nlink() == 1
        && metadata.dev() == source.device()
        && metadata.ino() == source.inode()
}

fn hex_digest(digest: &[u8]) -> String {
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("writing into a String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use sha2::{Digest as _, Sha256};

    use super::{
        INSPECTION_WORKER_SLOT, LaunchctlAction, NativeLaunchctlBoundary, SubmittedCommand,
        exact_bootstrap_source_matches, hex_digest, reap_submitted_command, run_deadline_read,
    };
    use crate::MacosDirectLaunchdBootstrapExpectation;

    #[test]
    fn submitted_child_is_killed_and_reaped_at_the_absolute_deadline() {
        let child = Command::new("/bin/sh")
            .args(["-c", "exec /bin/sleep 1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("test child should spawn");
        let started = Instant::now();
        assert!(matches!(
            reap_submitted_command(child, started + Duration::from_millis(25)),
            SubmittedCommand::Unknown
        ));
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn timed_out_inspections_have_one_live_worker_and_recover_the_slot() {
        let live = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        let worker_live = Arc::clone(&live);
        let worker_maximum = Arc::clone(&maximum);
        let started = Instant::now();
        let caller = std::thread::spawn(move || {
            run_deadline_read(started + Duration::from_millis(100), move || {
                let current = worker_live.fetch_add(1, Ordering::SeqCst) + 1;
                worker_maximum.fetch_max(current, Ordering::SeqCst);
                started_tx.send(()).expect("worker start should be visible");
                release_rx.recv().expect("worker should be released");
                worker_live.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
        });
        started_rx
            .recv_timeout(Duration::from_millis(500))
            .expect("worker should start before its deadline");
        let error = caller
            .join()
            .expect("deadline caller should not panic")
            .expect_err("slow inspection must time out");
        assert!(error.to_string().contains("absolute deadline"));
        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(live.load(Ordering::SeqCst), 1);

        let second_live = Arc::clone(&live);
        let second = run_deadline_read(Instant::now() + Duration::from_secs(1), move || {
            second_live.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .expect_err("occupied slot must reject a second worker");
        assert!(second.to_string().contains("slot is busy"));
        assert_eq!(live.load(Ordering::SeqCst), 1);

        release_tx.send(()).expect("worker should finish");
        let active = INSPECTION_WORKER_SLOT
            .0
            .lock()
            .expect("worker slot should lock");
        let (active, wait) = INSPECTION_WORKER_SLOT
            .1
            .wait_timeout_while(active, Duration::from_secs(1), |active| *active)
            .expect("worker slot wait should succeed");
        assert!(!wait.timed_out());
        assert!(!*active);
        assert_eq!(live.load(Ordering::SeqCst), 0);
        drop(active);

        let recovered_live = Arc::clone(&live);
        let recovered_maximum = Arc::clone(&maximum);
        run_deadline_read(Instant::now() + Duration::from_secs(1), move || {
            let current = recovered_live.fetch_add(1, Ordering::SeqCst) + 1;
            recovered_maximum.fetch_max(current, Ordering::SeqCst);
            recovered_live.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        })
        .expect("released slot should accept the next inspection");
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn launchctl_mutation_arguments_are_fixed_and_non_escalating() {
        let boundary = NativeLaunchctlBoundary { uid: 501 };
        let target = "gui/501/tech.hyperbliss.hypercolor";
        assert_eq!(
            boundary.arguments(&LaunchctlAction::Kickstart),
            ["kickstart", "-p", target]
        );
        assert_eq!(
            boundary.arguments(&LaunchctlAction::Bootout),
            ["bootout", "--wait", target]
        );
        assert_eq!(
            boundary.arguments(&LaunchctlAction::Bootstrap(
                "/private/unit/launchd.plist".into()
            )),
            ["bootstrap", "gui/501", "/private/unit/launchd.plist"]
        );
    }

    #[test]
    fn bootstrap_source_proof_rejects_byte_metadata_and_inode_drift() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let path = directory.path().join("launchd.plist");
        fs::write(&path, b"exact plist").expect("property list should write");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("property-list mode should set");
        let metadata = fs::metadata(&path).expect("property-list metadata should read");
        let expectation = MacosDirectLaunchdBootstrapExpectation::new(
            &path,
            hex_digest(&Sha256::digest(b"exact plist")),
            0o600,
            metadata.len(),
            metadata.dev(),
            metadata.ino(),
        )
        .expect("expectation should build");
        assert!(exact_bootstrap_source_matches(&expectation).expect("exact proof should run"));

        fs::write(&path, b"wrong plist").expect("property list should drift");
        assert!(!exact_bootstrap_source_matches(&expectation).expect("digest drift should run"));

        fs::remove_file(&path).expect("drifted property list should remove");
        fs::write(&path, b"exact plist").expect("replacement should write");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("replacement mode should set");
        assert!(!exact_bootstrap_source_matches(&expectation).expect("inode drift should run"));
    }
}
