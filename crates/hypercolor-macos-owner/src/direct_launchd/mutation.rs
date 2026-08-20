use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{
    MACOS_DIRECT_LAUNCHD_LABEL, MacosDirectLaunchdInspector, MacosDirectLaunchdOwnerProof,
    MacosDirectLaunchdPublicationExpectation, MacosDirectLaunchdState, exact_published_proof,
};
use crate::{
    MAX_MACOS_EXECUTABLE_PATH_BYTES, MacosOwnerExecutionError, MacosOwnerStore,
    MacosOwnerStoreError, validate_bounded_identity_text,
};

const MAX_BOOTSTRAP_PLIST_BYTES: u64 = 256 * 1024;
const MAX_LAUNCHCTL_OUTPUT_BYTES: usize = 64 * 1024;
const DEFAULT_INSPECTION_TIMEOUT: Duration = Duration::from_secs(10);

/// Exact immutable property-list identity submitted to launchd bootstrap.
///
/// The path must remain inside a private namespace protected by the install
/// transaction's cooperative lock. launchd opens the path independently, so
/// this expectation does not exclude a noncooperating same-user path swap.
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

    /// Absolute retained private property-list path passed to launchd.
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
        source: &MacosDirectLaunchdBootstrapExpectation,
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum LaunchctlAction {
    PrintDisabled,
    Enable,
    Disable,
    Bootstrap(PathBuf),
    Kickstart,
    Bootout,
}

#[derive(Debug)]
enum SubmittedCommand {
    Completed {
        status: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    Unknown,
}

trait LaunchctlCommandBoundary {
    fn run(
        &mut self,
        action: &LaunchctlAction,
        deadline: Instant,
    ) -> Result<SubmittedCommand, MacosOwnerExecutionError>;

    fn bootstrap_source_matches(
        &mut self,
        source: &MacosDirectLaunchdBootstrapExpectation,
        deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError>;
}

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

struct DeadlineInspectorBridge<'a, I> {
    inspector: &'a mut I,
    deadline: Instant,
}

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

struct MutationController<'a, I, C> {
    store: &'a MacosOwnerStore,
    inspector: &'a mut I,
    commands: &'a mut C,
}

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
        source: &MacosDirectLaunchdBootstrapExpectation,
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
        let bootstrap = self.commands.run(
            &LaunchctlAction::Bootstrap(source.path().to_path_buf()),
            deadline,
        )?;
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
        source: &MacosDirectLaunchdBootstrapExpectation,
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
        source: &MacosDirectLaunchdBootstrapExpectation,
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

#[cfg(test)]
mod tests;
