use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read, Write as _};
use std::os::unix::fs::{FileExt as _, MetadataExt as _};
use std::process::{Child, Command, Stdio};
use std::sync::{Condvar, Mutex, TryLockError};
use std::time::{Duration, Instant};

use sha2::{Digest as _, Sha256};

use super::{
    DeadlineDirectLaunchdInspector, LaunchctlAction, LaunchctlCommandBoundary,
    MacosDirectLaunchdBootstrapExpectation, MacosDirectLaunchdBootstrapSource,
    MacosDirectLaunchdMutationOutcome, MacosDirectLaunchdMutator, MutationController,
    SubmittedCommand,
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

#[cfg(test)]
pub(in crate::direct_launchd) static INSPECTION_WORKER_TEST_GATE: Mutex<()> = Mutex::new(());

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
            NativeMacosDirectLaunchdInspector::new()
                .live_identity_matches_until(&identity, deadline)
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
            NativeMacosDirectLaunchdInspector::new().publication_identity_matches_until(
                &identity,
                &executable,
                deadline,
            )
        })
    }
}

pub(in crate::direct_launchd) fn run_deadline_read<T: Send + 'static>(
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

#[cfg(test)]
pub(in crate::direct_launchd) fn wait_for_inspection_worker_idle(timeout: Duration) -> bool {
    let active = INSPECTION_WORKER_SLOT
        .0
        .lock()
        .expect("worker slot should lock");
    let (active, wait) = INSPECTION_WORKER_SLOT
        .1
        .wait_timeout_while(active, timeout, |active| *active)
        .expect("worker slot wait should succeed");
    !wait.timed_out() && !*active
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
        source: &mut MacosDirectLaunchdBootstrapSource,
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
            LaunchctlAction::Bootstrap => {
                vec!["bootstrap".into(), domain.into(), "/dev/fd/0".into()]
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

    fn run_bootstrap_launchctl(
        &self,
        source: &MacosDirectLaunchdBootstrapSource,
        deadline: Instant,
    ) -> Result<SubmittedCommand, MacosOwnerExecutionError> {
        let file = source.try_clone_file()?;
        let expectation = source.expectation().clone();
        let Some(snapshot) = run_deadline_read(deadline, move || {
            exact_bootstrap_snapshot(&file, &expectation)
        })?
        else {
            return Err(MacosOwnerExecutionError::new(
                "bootstrap property list changed before command submission",
            ));
        };
        run_snapshot_stdin_command(
            "/bin/launchctl",
            &self.arguments(&LaunchctlAction::Bootstrap),
            snapshot,
            deadline,
        )
    }
}

fn run_snapshot_stdin_command(
    program: &str,
    arguments: &[OsString],
    snapshot: Vec<u8>,
    deadline: Instant,
) -> Result<SubmittedCommand, MacosOwnerExecutionError> {
    if Instant::now() >= deadline {
        return Err(MacosOwnerExecutionError::new(
            "launchd mutation deadline expired before command submission",
        ));
    }
    let mut child = Command::new(program)
        .args(arguments)
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    let Some(mut stdin) = child.stdin.take() else {
        kill_and_reap(&mut child);
        return Ok(SubmittedCommand::Unknown);
    };
    let Ok(writer) = std::thread::Builder::new()
        .name("launchctl-stdin".to_owned())
        .spawn(move || stdin.write_all(&snapshot))
    else {
        kill_and_reap(&mut child);
        return Ok(SubmittedCommand::Unknown);
    };
    Ok(reap_submitted_command_with_writer(
        child,
        Some(writer),
        deadline,
    ))
}

impl LaunchctlCommandBoundary for NativeLaunchctlBoundary {
    fn run(
        &mut self,
        action: &LaunchctlAction,
        deadline: Instant,
    ) -> Result<SubmittedCommand, MacosOwnerExecutionError> {
        self.run_launchctl(action, deadline)
    }

    fn run_bootstrap(
        &mut self,
        source: &mut MacosDirectLaunchdBootstrapSource,
        deadline: Instant,
    ) -> Result<SubmittedCommand, MacosOwnerExecutionError> {
        self.run_bootstrap_launchctl(source, deadline)
    }

    fn bootstrap_source_matches(
        &mut self,
        source: &MacosDirectLaunchdBootstrapSource,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError> {
        let file = source.try_clone_file()?;
        let expectation = source.expectation().clone();
        run_deadline_read(deadline, move || {
            exact_bootstrap_source_matches(&file, &expectation)
        })
    }
}

fn reap_submitted_command(child: Child, deadline: Instant) -> SubmittedCommand {
    reap_submitted_command_with_writer(child, None, deadline)
}

fn reap_submitted_command_with_writer(
    mut child: Child,
    mut writer: Option<std::thread::JoinHandle<std::io::Result<()>>>,
    deadline: Instant,
) -> SubmittedCommand {
    let Some(stdout) = child.stdout.take() else {
        kill_and_reap(&mut child);
        let _ = join_writer(writer.take());
        return SubmittedCommand::Unknown;
    };
    let Some(stderr) = child.stderr.take() else {
        kill_and_reap(&mut child);
        let _ = join_writer(writer.take());
        return SubmittedCommand::Unknown;
    };
    let Ok(stdout_reader) = std::thread::Builder::new()
        .name("launchctl-stdout".to_owned())
        .spawn(move || read_bounded(stdout))
    else {
        kill_and_reap(&mut child);
        let _ = join_writer(writer.take());
        return SubmittedCommand::Unknown;
    };
    let Ok(stderr_reader) = std::thread::Builder::new()
        .name("launchctl-stderr".to_owned())
        .spawn(move || read_bounded(stderr))
    else {
        kill_and_reap(&mut child);
        let _ = stdout_reader.join();
        let _ = join_writer(writer.take());
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
    let input = join_writer(writer);
    match (status, stdout, stderr, input) {
        (Some(status), Some(stdout), Some(stderr), Some(())) => SubmittedCommand::Completed {
            status,
            stdout,
            stderr,
        },
        _ => SubmittedCommand::Unknown,
    }
}

fn join_writer(writer: Option<std::thread::JoinHandle<std::io::Result<()>>>) -> Option<()> {
    writer.map_or(Some(()), |writer| writer.join().ok()?.ok())
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
    file: &File,
    source: &MacosDirectLaunchdBootstrapExpectation,
) -> Result<bool, MacosOwnerExecutionError> {
    Ok(exact_bootstrap_snapshot(file, source)?.is_some())
}

fn exact_bootstrap_snapshot(
    file: &File,
    source: &MacosDirectLaunchdBootstrapExpectation,
) -> Result<Option<Vec<u8>>, MacosOwnerExecutionError> {
    let before = file
        .metadata()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    if !bootstrap_metadata_matches(&before, source) {
        return Ok(None);
    }
    let mut hasher = Sha256::new();
    let mut snapshot = Vec::with_capacity(
        usize::try_from(source.size()).expect("bounded property-list size fits usize"),
    );
    let mut buffer = [0_u8; 8 * 1024];
    let mut offset = 0_u64;
    while offset <= source.size() {
        let remaining = source.size() + 1 - offset;
        let limit = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded read limit fits usize");
        let read = file
            .read_at(&mut buffer[..limit], offset)
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        snapshot.extend_from_slice(&buffer[..read]);
        offset += u64::try_from(read).expect("read length fits u64");
    }
    if offset != source.size() || hex_digest(&hasher.finalize()) != source.sha256() {
        return Ok(None);
    }
    let after = file
        .metadata()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    Ok((bootstrap_metadata_matches(&after, source)
        && before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.mode() == after.mode()
        && before.len() == after.len()
        && before.nlink() == after.nlink())
    .then_some(snapshot))
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
        INSPECTION_WORKER_SLOT, INSPECTION_WORKER_TEST_GATE, LaunchctlAction,
        NativeLaunchctlBoundary, SubmittedCommand, exact_bootstrap_snapshot,
        exact_bootstrap_source_matches, hex_digest, reap_submitted_command, run_deadline_read,
        run_snapshot_stdin_command,
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
        let _test_gate = INSPECTION_WORKER_TEST_GATE
            .lock()
            .expect("worker test gate should lock");
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
            boundary.arguments(&LaunchctlAction::Bootstrap),
            ["bootstrap", "gui/501", "/dev/fd/0"]
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
        let retained = fs::File::open(&path).expect("property list should retain");
        assert!(
            exact_bootstrap_source_matches(&retained, &expectation)
                .expect("exact proof should run")
        );

        let retained_path = directory.path().join("retained.plist");
        fs::rename(&path, &retained_path).expect("retained path should move");
        fs::write(&path, b"wrong plist").expect("replacement should write");
        assert!(
            exact_bootstrap_source_matches(&retained, &expectation)
                .expect("retained inode should remain exact")
        );

        fs::write(&retained_path, b"wrong plist").expect("retained inode should drift");
        assert!(
            !exact_bootstrap_source_matches(&retained, &expectation)
                .expect("digest drift should run")
        );

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("replacement mode should set");
        let replacement = fs::File::open(&path).expect("replacement should open");
        assert!(
            !exact_bootstrap_source_matches(&replacement, &expectation)
                .expect("replacement inode drift should run")
        );
    }

    #[test]
    fn bootstrap_command_reads_only_the_retained_descriptor() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let path = directory.path().join("launchd.plist");
        fs::write(&path, b"retained plist").expect("property list should write");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("property-list mode should set");
        let metadata = fs::metadata(&path).expect("property-list metadata should read");
        let expectation = MacosDirectLaunchdBootstrapExpectation::new(
            &path,
            hex_digest(&Sha256::digest(b"retained plist")),
            0o600,
            metadata.len(),
            metadata.dev(),
            metadata.ino(),
        )
        .expect("expectation should build");
        let retained = fs::File::open(&path).expect("property list should retain");
        fs::rename(&path, directory.path().join("retained.plist"))
            .expect("retained path should move");
        fs::write(&path, b"attacker plist").expect("attacker replacement should write");
        let snapshot = exact_bootstrap_snapshot(&retained, &expectation)
            .expect("snapshot should read")
            .expect("retained source should match");
        fs::write(directory.path().join("retained.plist"), b"attacker plist")
            .expect("retained inode should drift after snapshot");
        let result = run_snapshot_stdin_command(
            "/bin/cat",
            &["/dev/fd/0".into()],
            snapshot,
            Instant::now() + Duration::from_secs(1),
        )
        .expect("fake launchctl should read inherited descriptor");
        let SubmittedCommand::Completed { status, stdout, .. } = result else {
            panic!("fake launchctl should complete");
        };
        assert_eq!(status, Some(0));
        assert_eq!(stdout, b"retained plist");
    }
}
