use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(any(test, target_os = "macos"))]
use std::time::Instant;

use super::{
    MACOS_DIRECT_LAUNCHD_LABEL, MacosDirectLaunchdOwnerProof,
    MacosDirectLaunchdPublicationExpectation,
};
#[cfg(any(test, target_os = "macos"))]
use super::{MacosDirectLaunchdInspector, MacosDirectLaunchdState, exact_published_proof};
#[cfg(any(test, target_os = "macos"))]
use crate::MacosOwnerStore;
use crate::{
    MAX_MACOS_EXECUTABLE_PATH_BYTES, MacosOwnerExecutionError, MacosOwnerStoreError,
    validate_bounded_identity_text,
};

const MAX_BOOTSTRAP_PLIST_BYTES: u64 = 256 * 1024;
const MAX_LAUNCHCTL_OUTPUT_BYTES: usize = 64 * 1024;
#[cfg(any(test, target_os = "macos"))]
const DEFAULT_INSPECTION_TIMEOUT: Duration = Duration::from_secs(10);

/// Exact immutable property-list identity submitted to launchd bootstrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosDirectLaunchdBootstrapExpectation {
    path: PathBuf,
    sha256: String,
    mode: u32,
    size: u64,
    device: u64,
    inode: u64,
}

impl MacosDirectLaunchdBootstrapExpectation {
    /// Bind one retained private property list to its exact bytes and inode metadata.
    pub fn new(
        path: impl Into<PathBuf>,
        sha256: impl Into<String>,
        mode: u32,
        size: u64,
        device: u64,
        inode: u64,
    ) -> Result<Self, MacosOwnerStoreError> {
        let path = path.into();
        let path_text = path
            .to_str()
            .ok_or(MacosOwnerStoreError::InvalidOwnerIdentity {
                field: "bootstrap_plist_path",
                detail: "must be valid UTF-8",
            })?;
        validate_bounded_identity_text(
            "bootstrap_plist_path",
            path_text,
            MAX_MACOS_EXECUTABLE_PATH_BYTES,
        )?;
        if !path.is_absolute() || path_text.as_bytes().contains(&0) {
            return Err(MacosOwnerStoreError::InvalidOwnerIdentity {
                field: "bootstrap_plist_path",
                detail: "must be absolute",
            });
        }
        let sha256 = sha256.into();
        if !super::is_sha256(&sha256) {
            return Err(MacosOwnerStoreError::InvalidOwnerIdentity {
                field: "bootstrap_plist_sha256",
                detail: "must be exactly 64 lowercase hexadecimal bytes",
            });
        }
        if mode > 0o777
            || mode & 0o400 == 0
            || mode & 0o022 != 0
            || size == 0
            || size > MAX_BOOTSTRAP_PLIST_BYTES
            || device == 0
            || inode == 0
        {
            return Err(MacosOwnerStoreError::InvalidOwnerIdentity {
                field: "bootstrap_plist_metadata",
                detail: "must identify one bounded, owner-readable regular file",
            });
        }
        Ok(Self {
            path,
            sha256,
            mode,
            size,
            device,
            inode,
        })
    }

    /// Absolute diagnostic path where the retained property list was opened.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// SHA-256 of the exact property-list bytes.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Exact ordinary permission bits.
    #[must_use]
    pub const fn mode(&self) -> u32 {
        self.mode
    }

    /// Exact property-list byte length.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Device identifier of the retained property-list inode.
    #[must_use]
    pub const fn device(&self) -> u64 {
        self.device
    }

    /// Inode identifier of the retained property-list inode.
    #[must_use]
    pub const fn inode(&self) -> u64 {
        self.inode
    }
}

/// Retained exact property-list object submitted to launchctl without reopening its path.
#[derive(Debug)]
pub struct MacosDirectLaunchdBootstrapSource {
    #[cfg_attr(
        not(target_os = "macos"),
        expect(
            dead_code,
            reason = "retained bootstrap descriptors are consumed only by the macOS mutator"
        )
    )]
    file: File,
    expectation: MacosDirectLaunchdBootstrapExpectation,
}

impl MacosDirectLaunchdBootstrapSource {
    /// Bind an already-open property-list object to its exact expected identity.
    #[must_use]
    pub const fn new(file: File, expectation: MacosDirectLaunchdBootstrapExpectation) -> Self {
        Self { file, expectation }
    }

    /// Exact byte and inode identity required from the retained object.
    #[must_use]
    pub const fn expectation(&self) -> &MacosDirectLaunchdBootstrapExpectation {
        &self.expectation
    }

    #[cfg(target_os = "macos")]
    fn try_clone_file(&self) -> Result<File, MacosOwnerExecutionError> {
        self.file
            .try_clone()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
    }
}

/// Reconciled result of one launchd mutation submission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacosDirectLaunchdMutationOutcome<T> {
    /// The exact requested terminal state was proven.
    Complete(T),
    /// A command was submitted, but its exact terminal state could not be proven.
    SubmittedUnknown,
}

/// Exact direct-launchd mutation authority used by a raw macOS installer.
pub trait MacosDirectLaunchdMutator {
    /// Inspect the fixed service label's persisted launchd autostart state.
    fn autostart_enabled(&mut self) -> Result<bool, MacosOwnerExecutionError>;

    /// Set and exactly reconcile the fixed service label's autostart state.
    fn set_autostart(
        &mut self,
        enabled: bool,
        timeout: Duration,
    ) -> Result<MacosDirectLaunchdMutationOutcome<()>, MacosOwnerExecutionError>;

    /// Stop only the exact corroborated owner publication.
    fn bootout_exact(
        &mut self,
        expected: &MacosDirectLaunchdOwnerProof,
        timeout: Duration,
    ) -> Result<MacosDirectLaunchdMutationOutcome<()>, MacosOwnerExecutionError>;

    /// Bootstrap and start one exact retained immutable unit.
    fn bootstrap_and_kickstart_exact(
        &mut self,
        source: &mut MacosDirectLaunchdBootstrapSource,
        expected: &MacosDirectLaunchdPublicationExpectation,
        timeout: Duration,
    ) -> Result<
        MacosDirectLaunchdMutationOutcome<MacosDirectLaunchdOwnerProof>,
        MacosOwnerExecutionError,
    >;
}

/// Parse bounded `launchctl print-disabled` output for the fixed service label.
///
/// A missing label has launchd's default enabled state. Current symbolic values
/// and the legacy boolean representation are accepted with identical meaning.
pub fn parse_direct_launchd_autostart_state(
    stdout: &[u8],
) -> Result<bool, MacosOwnerExecutionError> {
    if stdout.len() > MAX_LAUNCHCTL_OUTPUT_BYTES {
        return Err(MacosOwnerExecutionError::new(
            "launchctl disabled-state output exceeds 64 KiB",
        ));
    }
    let stdout = std::str::from_utf8(stdout).map_err(|_| {
        MacosOwnerExecutionError::new("launchctl disabled-state output is not UTF-8")
    })?;
    let mut lines = stdout.lines();
    if lines.next().map(str::trim) != Some("disabled services = {") {
        return Err(MacosOwnerExecutionError::new(
            "launchctl disabled-state output has the wrong header",
        ));
    }
    let mut target = None;
    let mut closed = false;
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line == "}" {
            closed = true;
            if lines.any(|remaining| !remaining.trim().is_empty()) {
                return Err(MacosOwnerExecutionError::new(
                    "launchctl disabled-state output has trailing content",
                ));
            }
            break;
        }
        if line.is_empty() {
            continue;
        }
        let Some((quoted_label, value)) = line.split_once(" => ") else {
            return Err(MacosOwnerExecutionError::new(
                "launchctl disabled-state output has a malformed entry",
            ));
        };
        let Some(label) = quoted_label
            .strip_prefix('"')
            .and_then(|label| label.strip_suffix('"'))
        else {
            return Err(MacosOwnerExecutionError::new(
                "launchctl disabled-state output has a malformed label",
            ));
        };
        if label.is_empty()
            || label.contains('"')
            || label.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(MacosOwnerExecutionError::new(
                "launchctl disabled-state output has an invalid label",
            ));
        }
        let enabled = match value {
            "enabled" | "false" => true,
            "disabled" | "true" => false,
            _ => {
                return Err(MacosOwnerExecutionError::new(
                    "launchctl disabled-state output has an unknown value",
                ));
            }
        };
        if label == MACOS_DIRECT_LAUNCHD_LABEL && target.replace(enabled).is_some() {
            return Err(MacosOwnerExecutionError::new(
                "launchctl disabled-state output repeats the direct service label",
            ));
        }
    }
    if !closed {
        return Err(MacosOwnerExecutionError::new(
            "launchctl disabled-state output is not terminated",
        ));
    }
    Ok(target.unwrap_or(true))
}

#[cfg(any(test, target_os = "macos"))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum LaunchctlAction {
    PrintDisabled,
    Enable,
    Disable,
    Bootstrap,
    Kickstart,
    Bootout,
}

#[cfg(any(test, target_os = "macos"))]
#[derive(Debug)]
enum SubmittedCommand {
    Completed {
        status: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    Unknown,
}

#[cfg(any(test, target_os = "macos"))]
trait LaunchctlCommandBoundary {
    fn run(
        &mut self,
        action: &LaunchctlAction,
        deadline: Instant,
    ) -> Result<SubmittedCommand, MacosOwnerExecutionError>;

    fn run_bootstrap(
        &mut self,
        source: &mut MacosDirectLaunchdBootstrapSource,
        deadline: Instant,
    ) -> Result<SubmittedCommand, MacosOwnerExecutionError>;

    fn bootstrap_source_matches(
        &mut self,
        source: &MacosDirectLaunchdBootstrapSource,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError>;
}

#[cfg(any(test, target_os = "macos"))]
trait DeadlineDirectLaunchdInspector {
    fn inspect_direct_launchd_until(
        &mut self,
        deadline: Instant,
    ) -> Result<MacosDirectLaunchdState, MacosOwnerExecutionError>;

    fn live_identity_matches_until(
        &mut self,
        identity: &crate::MacosOwnerIdentity,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError>;

    fn publication_identity_matches_until(
        &mut self,
        identity: &crate::MacosOwnerIdentity,
        executable: &super::MacosDirectLaunchdExecutableExpectation,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError>;
}

#[cfg(any(test, target_os = "macos"))]
struct DeadlineInspectorBridge<'a, I> {
    inspector: &'a mut I,
    deadline: Instant,
}

#[cfg(any(test, target_os = "macos"))]
impl<I: DeadlineDirectLaunchdInspector> MacosDirectLaunchdInspector
    for DeadlineInspectorBridge<'_, I>
{
    fn inspect_direct_launchd(
        &mut self,
    ) -> Result<MacosDirectLaunchdState, MacosOwnerExecutionError> {
        self.inspector.inspect_direct_launchd_until(self.deadline)
    }

    fn live_identity_matches(
        &mut self,
        identity: &crate::MacosOwnerIdentity,
    ) -> Result<bool, MacosOwnerExecutionError> {
        self.inspector
            .live_identity_matches_until(identity, self.deadline)
    }

    fn publication_identity_matches(
        &mut self,
        identity: &crate::MacosOwnerIdentity,
        executable: &super::MacosDirectLaunchdExecutableExpectation,
    ) -> Result<bool, MacosOwnerExecutionError> {
        self.inspector
            .publication_identity_matches_until(identity, executable, self.deadline)
    }
}

#[cfg(any(test, target_os = "macos"))]
struct MutationController<'a, I, C> {
    store: &'a MacosOwnerStore,
    inspector: &'a mut I,
    commands: &'a mut C,
}

#[cfg(any(test, target_os = "macos"))]
impl<I, C> MutationController<'_, I, C>
where
    I: DeadlineDirectLaunchdInspector,
    C: LaunchctlCommandBoundary,
{
    fn deadline(timeout: Duration) -> Result<Instant, MacosOwnerExecutionError> {
        Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| MacosOwnerExecutionError::new("launchd mutation deadline overflowed"))
    }

    fn autostart_enabled_until(
        &mut self,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError> {
        let SubmittedCommand::Completed {
            status,
            stdout,
            stderr,
        } = self
            .commands
            .run(&LaunchctlAction::PrintDisabled, deadline)?
        else {
            return Err(MacosOwnerExecutionError::new(
                "launchctl disabled-state inspection did not complete",
            ));
        };
        if status != Some(0) {
            return Err(MacosOwnerExecutionError::new(format!(
                "launchctl disabled-state inspection failed with status {status:?}: {}",
                String::from_utf8_lossy(&stderr).trim()
            )));
        }
        parse_direct_launchd_autostart_state(&stdout)
    }

    #[cfg_attr(
        all(test, not(target_os = "macos")),
        expect(
            dead_code,
            reason = "the native inspector is unavailable in cross-target test builds"
        )
    )]
    fn autostart_enabled(&mut self) -> Result<bool, MacosOwnerExecutionError> {
        self.autostart_enabled_until(Self::deadline(DEFAULT_INSPECTION_TIMEOUT)?)
    }

    fn set_autostart(
        &mut self,
        enabled: bool,
        timeout: Duration,
    ) -> Result<MacosDirectLaunchdMutationOutcome<()>, MacosOwnerExecutionError> {
        let deadline = Self::deadline(timeout)?;
        if self.autostart_enabled_until(deadline)? == enabled {
            return Ok(MacosDirectLaunchdMutationOutcome::Complete(()));
        }
        let action = if enabled {
            LaunchctlAction::Enable
        } else {
            LaunchctlAction::Disable
        };
        let _submitted = self.commands.run(&action, deadline)?;
        if self.exact_autostart(enabled, deadline) {
            Ok(MacosDirectLaunchdMutationOutcome::Complete(()))
        } else {
            Ok(MacosDirectLaunchdMutationOutcome::SubmittedUnknown)
        }
    }

    fn exact_autostart(&mut self, enabled: bool, deadline: Instant) -> bool {
        self.autostart_enabled_until(deadline)
            .is_ok_and(|value| value == enabled)
            && self
                .autostart_enabled_until(deadline)
                .is_ok_and(|value| value == enabled)
    }

    fn bootout_exact(
        &mut self,
        expected: &MacosDirectLaunchdOwnerProof,
        timeout: Duration,
    ) -> Result<MacosDirectLaunchdMutationOutcome<()>, MacosOwnerExecutionError> {
        let deadline = Self::deadline(timeout)?;
        if self.exact_not_loaded(deadline) {
            return Ok(MacosDirectLaunchdMutationOutcome::Complete(()));
        }
        let mut submitted = false;
        let result = self
            .store
            .request_stop_if_current(&expected.record().incarnation(), || {
                let mut inspector = DeadlineInspectorBridge {
                    inspector: self.inspector,
                    deadline,
                };
                super::corroborate_direct_launchd_owner(expected.record(), &mut inspector)?;
                let _outcome = self.commands.run(&LaunchctlAction::Bootout, deadline)?;
                submitted = true;
                if self.exact_not_loaded(deadline) {
                    Ok(())
                } else {
                    Err(MacosOwnerExecutionError::new(
                        "submitted launchd bootout has no exact terminal proof",
                    ))
                }
            });
        match result {
            Ok(()) => Ok(MacosDirectLaunchdMutationOutcome::Complete(())),
            Err(_) if submitted => Ok(MacosDirectLaunchdMutationOutcome::SubmittedUnknown),
            Err(_) if self.exact_not_loaded(deadline) => {
                Ok(MacosDirectLaunchdMutationOutcome::Complete(()))
            }
            Err(error) => Err(error),
        }
    }

    fn exact_not_loaded(&mut self, deadline: Instant) -> bool {
        self.inspector
            .inspect_direct_launchd_until(deadline)
            .is_ok_and(|state| state == MacosDirectLaunchdState::NotLoaded)
            && self
                .inspector
                .inspect_direct_launchd_until(deadline)
                .is_ok_and(|state| state == MacosDirectLaunchdState::NotLoaded)
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
        let deadline = Self::deadline(timeout)?;
        if let Some(proof) = self.exact_published_proof_until(expected, deadline)? {
            return Ok(MacosDirectLaunchdMutationOutcome::Complete(proof));
        }
        if self.inspector.inspect_direct_launchd_until(deadline)?
            != MacosDirectLaunchdState::NotLoaded
        {
            return Err(MacosOwnerExecutionError::new(
                "direct launchd service is already loaded without the exact publication",
            ));
        }
        if !self.commands.bootstrap_source_matches(source, deadline)? {
            return Err(MacosOwnerExecutionError::new(
                "bootstrap property list does not match its retained expectation",
            ));
        }
        let bootstrap = self.commands.run_bootstrap(source, deadline)?;
        if !matches!(
            bootstrap,
            SubmittedCommand::Completed {
                status: Some(0),
                ..
            }
        ) {
            return Ok(self.reconcile_publication(source, expected, deadline));
        }
        match self.exact_published_proof_until(expected, deadline) {
            Ok(Some(proof)) => return Ok(self.finish_publication(source, proof, deadline)),
            Ok(None) => {}
            Err(_) => return Ok(MacosDirectLaunchdMutationOutcome::SubmittedUnknown),
        }
        if Instant::now() >= deadline {
            return Ok(MacosDirectLaunchdMutationOutcome::SubmittedUnknown);
        }
        if self
            .commands
            .run(&LaunchctlAction::Kickstart, deadline)
            .is_err()
        {
            return Ok(MacosDirectLaunchdMutationOutcome::SubmittedUnknown);
        }
        Ok(self.reconcile_publication(source, expected, deadline))
    }

    fn reconcile_publication(
        &mut self,
        source: &MacosDirectLaunchdBootstrapSource,
        expected: &MacosDirectLaunchdPublicationExpectation,
        deadline: Instant,
    ) -> MacosDirectLaunchdMutationOutcome<MacosDirectLaunchdOwnerProof> {
        match self.exact_published_proof_until(expected, deadline) {
            Ok(Some(proof)) => return self.finish_publication(source, proof, deadline),
            Ok(None) => {}
            Err(_) => return MacosDirectLaunchdMutationOutcome::SubmittedUnknown,
        }
        let proof = self.wait_for_exact_publication_until(expected, deadline);
        match proof {
            Ok(Some(proof)) => self.finish_publication(source, proof, deadline),
            Ok(None) | Err(_) => MacosDirectLaunchdMutationOutcome::SubmittedUnknown,
        }
    }

    fn finish_publication(
        &mut self,
        source: &MacosDirectLaunchdBootstrapSource,
        proof: MacosDirectLaunchdOwnerProof,
        deadline: Instant,
    ) -> MacosDirectLaunchdMutationOutcome<MacosDirectLaunchdOwnerProof> {
        if self
            .commands
            .bootstrap_source_matches(source, deadline)
            .unwrap_or(false)
        {
            MacosDirectLaunchdMutationOutcome::Complete(proof)
        } else {
            MacosDirectLaunchdMutationOutcome::SubmittedUnknown
        }
    }

    fn exact_published_proof_until(
        &mut self,
        expected: &MacosDirectLaunchdPublicationExpectation,
        deadline: Instant,
    ) -> Result<Option<MacosDirectLaunchdOwnerProof>, MacosOwnerExecutionError> {
        let mut inspector = DeadlineInspectorBridge {
            inspector: self.inspector,
            deadline,
        };
        exact_published_proof(self.store, expected, &mut inspector)
    }

    fn wait_for_exact_publication_until(
        &mut self,
        expected: &MacosDirectLaunchdPublicationExpectation,
        deadline: Instant,
    ) -> Result<Option<MacosDirectLaunchdOwnerProof>, MacosOwnerExecutionError> {
        let mut inspector = DeadlineInspectorBridge {
            inspector: self.inspector,
            deadline,
        };
        super::wait_for_exact_direct_launchd_publication_proof(
            self.store,
            expected,
            deadline,
            &mut inspector,
        )
    }
}

#[cfg(target_os = "macos")]
mod native;
#[cfg(target_os = "macos")]
pub use native::NativeMacosDirectLaunchdMutator;
#[cfg(target_os = "macos")]
pub(super) use native::run_deadline_read;
#[cfg(all(test, target_os = "macos"))]
pub(super) use native::{INSPECTION_WORKER_TEST_GATE, wait_for_inspection_worker_idle};

#[cfg(test)]
mod tests;
