use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest as _, Sha256};

use crate::{
    MAX_MACOS_DESIGNATED_REQUIREMENT_HASH_BYTES, MAX_MACOS_EXECUTABLE_PATH_BYTES, MacosDaemonOwner,
    MacosOwnerExecutionError, MacosOwnerRecord, MacosOwnerStore, MacosOwnerStoreError,
    validate_bounded_identity_text,
};

/// Stable launchd label for Hypercolor's raw direct installation.
pub const MACOS_DIRECT_LAUNCHD_LABEL: &str = "tech.hyperbliss.hypercolor";

const MAX_LAUNCHCTL_OUTPUT_BYTES: usize = 64 * 1024;
const SHA256_HEX_BYTES: usize = 64;

fn is_sha256(value: &str) -> bool {
    value.len() == SHA256_HEX_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn requirement_hash(requirement: &str) -> String {
    let digest = Sha256::digest(requirement.as_bytes());
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing into a String cannot fail");
    }
    output
}

/// Exact loaded state of Hypercolor's direct per-user launchd service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosDirectLaunchdState {
    /// The fixed service label is absent from the current user's GUI domain.
    NotLoaded,
    /// The fixed service label is loaded with exactly one positive process ID.
    Loaded {
        /// Process ID reported by launchd for the loaded job.
        pid: u32,
    },
}

/// Injectable direct-launchd authority inspection used by exact owner proofs.
pub trait MacosDirectLaunchdInspector {
    /// Inspect the fixed direct-launchd label in the current user's GUI domain.
    fn inspect_direct_launchd(
        &mut self,
    ) -> Result<MacosDirectLaunchdState, MacosOwnerExecutionError>;

    /// Verify the complete recorded identity against the live process.
    fn live_identity_matches(
        &mut self,
        identity: &crate::MacosOwnerIdentity,
    ) -> Result<bool, MacosOwnerExecutionError>;

    /// Verify a publication against the retained immutable executable.
    fn publication_identity_matches(
        &mut self,
        identity: &crate::MacosOwnerIdentity,
        executable: &MacosDirectLaunchdExecutableExpectation,
    ) -> Result<bool, MacosOwnerExecutionError>;
}

/// Unforgeable result of corroborating one direct-launchd owner publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosDirectLaunchdOwnerProof {
    record: MacosOwnerRecord,
}

impl MacosDirectLaunchdOwnerProof {
    /// Borrow the exact corroborated owner record.
    #[must_use]
    pub const fn record(&self) -> &MacosOwnerRecord {
        &self.record
    }

    /// Consume the proof and return the exact corroborated owner record.
    #[must_use]
    pub fn into_record(self) -> MacosOwnerRecord {
        self.record
    }
}

/// Exact retained executable identity required from a direct publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosDirectLaunchdExecutableExpectation {
    path: PathBuf,
    designated_requirement: String,
    designated_requirement_hash: String,
    sha256: String,
    mode: u32,
    size: u64,
    device: u64,
    inode: u64,
}

impl MacosDirectLaunchdExecutableExpectation {
    /// Bind one immutable executable path to its retained identity and signing requirement.
    pub fn new(
        path: impl Into<PathBuf>,
        designated_requirement: impl Into<String>,
        designated_requirement_hash: impl Into<String>,
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
                field: "executable_path",
                detail: "must be valid UTF-8",
            })?;
        validate_bounded_identity_text(
            "executable_path",
            path_text,
            MAX_MACOS_EXECUTABLE_PATH_BYTES,
        )?;
        if !path.is_absolute() {
            return Err(MacosOwnerStoreError::InvalidOwnerIdentity {
                field: "executable_path",
                detail: "must be absolute",
            });
        }
        let designated_requirement = designated_requirement.into();
        validate_bounded_identity_text(
            "designated_requirement",
            &designated_requirement,
            8 * 1024,
        )?;
        let designated_requirement_hash = designated_requirement_hash.into();
        validate_bounded_identity_text(
            "designated_requirement_hash",
            &designated_requirement_hash,
            MAX_MACOS_DESIGNATED_REQUIREMENT_HASH_BYTES,
        )?;
        if requirement_hash(&designated_requirement) != designated_requirement_hash {
            return Err(MacosOwnerStoreError::InvalidOwnerIdentity {
                field: "designated_requirement_hash",
                detail: "must be the SHA-256 of the exact designated requirement",
            });
        }
        let sha256 = sha256.into();
        if !is_sha256(&sha256) {
            return Err(MacosOwnerStoreError::InvalidOwnerIdentity {
                field: "executable_sha256",
                detail: "must be exactly 64 lowercase hexadecimal bytes",
            });
        }
        if mode > 0o777
            || mode & 0o222 != 0
            || mode & 0o400 == 0
            || mode & 0o100 == 0
            || size == 0
            || size == u64::MAX
            || device == 0
            || inode == 0
        {
            return Err(MacosOwnerStoreError::InvalidOwnerIdentity {
                field: "executable_metadata",
                detail: "must identify one nonempty immutable regular file",
            });
        }
        Ok(Self {
            path,
            designated_requirement,
            designated_requirement_hash,
            sha256,
            mode,
            size,
            device,
            inode,
        })
    }

    /// Absolute immutable-unit executable path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Exact designated requirement bound by the signed release provenance.
    #[must_use]
    pub fn designated_requirement(&self) -> &str {
        &self.designated_requirement
    }

    /// SHA-256 of the exact designated requirement.
    #[must_use]
    pub fn designated_requirement_hash(&self) -> &str {
        &self.designated_requirement_hash
    }

    /// SHA-256 of the exact retained executable bytes.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Exact immutable executable permission bits.
    #[must_use]
    pub const fn mode(&self) -> u32 {
        self.mode
    }

    /// Exact immutable executable byte length.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Device identifier of the retained executable inode.
    #[must_use]
    pub const fn device(&self) -> u64 {
        self.device
    }

    /// Inode identifier of the retained executable.
    #[must_use]
    pub const fn inode(&self) -> u64 {
        self.inode
    }
}

/// Immutable identity expected from a candidate or rollback unit publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosDirectLaunchdPublicationExpectation {
    after_epoch: u64,
    executable: MacosDirectLaunchdExecutableExpectation,
}

impl MacosDirectLaunchdPublicationExpectation {
    /// Validate and construct an exact publication expectation.
    pub fn new(
        after_epoch: u64,
        executable: MacosDirectLaunchdExecutableExpectation,
    ) -> Result<Self, MacosOwnerStoreError> {
        Ok(Self {
            after_epoch,
            executable,
        })
    }

    /// Epoch that a matching publication must exceed.
    #[must_use]
    pub const fn after_epoch(&self) -> u64 {
        self.after_epoch
    }

    /// Exact executable path expected from the immutable unit.
    #[must_use]
    pub fn executable_path(&self) -> &Path {
        self.executable.path()
    }

    /// Exact designated-requirement hash expected from the immutable unit.
    #[must_use]
    pub fn designated_requirement_hash(&self) -> &str {
        self.executable.designated_requirement_hash()
    }

    /// Exact executable SHA-256 expected by the immutable unit manifest.
    #[must_use]
    pub fn executable_sha256(&self) -> &str {
        self.executable.sha256()
    }

    /// Complete retained executable expectation.
    #[must_use]
    pub const fn executable(&self) -> &MacosDirectLaunchdExecutableExpectation {
        &self.executable
    }
}

/// Parse bounded `launchctl print` output for the fixed direct service label.
///
/// Status 113 is accepted only with launchctl's exact missing-service line.
/// Successful output must contain exactly one positive process ID.
pub fn parse_direct_launchd_service_state(
    uid: u32,
    exit_code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<MacosDirectLaunchdState, MacosOwnerExecutionError> {
    if stdout.len() > MAX_LAUNCHCTL_OUTPUT_BYTES || stderr.len() > MAX_LAUNCHCTL_OUTPUT_BYTES {
        return Err(MacosOwnerExecutionError::new(
            "launchctl output exceeds 64 KiB",
        ));
    }
    let stdout = std::str::from_utf8(stdout)
        .map_err(|_| MacosOwnerExecutionError::new("launchctl output is not UTF-8"))?;
    let stderr = std::str::from_utf8(stderr)
        .map_err(|_| MacosOwnerExecutionError::new("launchctl error output is not UTF-8"))?;
    match exit_code {
        Some(0) => {}
        Some(113) => {
            let expected = format!(
                "Could not find service \"{MACOS_DIRECT_LAUNCHD_LABEL}\" in domain for user gui: {uid}"
            );
            let normalized_stderr = stderr.strip_suffix('\n').unwrap_or(stderr);
            if stdout.is_empty() && normalized_stderr == expected {
                return Ok(MacosDirectLaunchdState::NotLoaded);
            }
            return Err(MacosOwnerExecutionError::new(
                "launchctl returned status 113 without exact missing-service output",
            ));
        }
        code => {
            return Err(MacosOwnerExecutionError::new(format!(
                "launchctl inspection failed with status {code:?}: {}",
                stderr.trim()
            )));
        }
    }

    let pids = stdout
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("pid = "))
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| MacosOwnerExecutionError::new("launchctl returned a malformed service pid"))?;
    match pids.as_slice() {
        [pid] if *pid > 0 => Ok(MacosDirectLaunchdState::Loaded { pid: *pid }),
        [] => Err(MacosOwnerExecutionError::new(
            "launchctl loaded service has no process pid",
        )),
        [_] => Err(MacosOwnerExecutionError::new(
            "launchctl returned a zero service pid",
        )),
        _ => Err(MacosOwnerExecutionError::new(
            "launchctl returned ambiguous service pids",
        )),
    }
}

/// Corroborate a durable record against the exact loaded direct-launchd owner.
pub fn corroborate_direct_launchd_owner(
    record: &MacosOwnerRecord,
    inspector: &mut impl MacosDirectLaunchdInspector,
) -> Result<MacosDirectLaunchdOwnerProof, MacosOwnerExecutionError> {
    if record.active_owner != MacosDaemonOwner::DirectLaunchd {
        return Err(MacosOwnerExecutionError::new(
            "macOS owner record is not the direct launchd topology",
        ));
    }
    require_direct_launchd_pid(record, inspector)?;
    if !inspector.live_identity_matches(&record.active_identity)? {
        return Err(MacosOwnerExecutionError::new(
            "direct launchd live process identity does not match the owner record",
        ));
    }
    require_direct_launchd_pid(record, inspector)?;
    Ok(MacosDirectLaunchdOwnerProof {
        record: record.clone(),
    })
}

fn require_direct_launchd_pid(
    record: &MacosOwnerRecord,
    inspector: &mut impl MacosDirectLaunchdInspector,
) -> Result<(), MacosOwnerExecutionError> {
    match inspector.inspect_direct_launchd()? {
        MacosDirectLaunchdState::NotLoaded => Err(MacosOwnerExecutionError::new(
            "direct launchd service is not loaded",
        )),
        MacosDirectLaunchdState::Loaded { pid } if pid != record.active_identity.pid => Err(
            MacosOwnerExecutionError::new("direct launchd pid does not match the owner record"),
        ),
        MacosDirectLaunchdState::Loaded { .. } => Ok(()),
    }
}

/// Corroborate one newer publication against an immutable unit expectation.
pub fn corroborate_newer_direct_launchd_owner(
    record: &MacosOwnerRecord,
    expectation: &MacosDirectLaunchdPublicationExpectation,
    inspector: &mut impl MacosDirectLaunchdInspector,
) -> Result<Option<MacosDirectLaunchdOwnerProof>, MacosOwnerExecutionError> {
    if record.owner_epoch <= expectation.after_epoch {
        return Ok(None);
    }
    if record.active_owner != MacosDaemonOwner::DirectLaunchd {
        return Err(MacosOwnerExecutionError::new(
            "newer macOS owner publication is not direct launchd",
        ));
    }
    if record.active_identity.executable_path != expectation.executable.path {
        return Err(MacosOwnerExecutionError::new(
            "newer direct launchd publication has the wrong executable path",
        ));
    }
    if record.active_identity.designated_requirement_hash
        != expectation.executable.designated_requirement_hash
    {
        return Err(MacosOwnerExecutionError::new(
            "newer direct launchd publication has the wrong designated requirement",
        ));
    }
    require_direct_launchd_pid(record, inspector)?;
    if !inspector.publication_identity_matches(&record.active_identity, expectation.executable())? {
        return Err(MacosOwnerExecutionError::new(
            "direct launchd process does not match the retained immutable executable",
        ));
    }
    require_direct_launchd_pid(record, inspector)?;
    Ok(Some(MacosDirectLaunchdOwnerProof {
        record: record.clone(),
    }))
}

/// Wait for a newer exact direct-launchd publication and return its record.
pub fn wait_for_exact_direct_launchd_publication(
    store: &MacosOwnerStore,
    expectation: &MacosDirectLaunchdPublicationExpectation,
    timeout: Duration,
    inspector: &mut impl MacosDirectLaunchdInspector,
) -> Result<Option<MacosOwnerRecord>, MacosOwnerExecutionError> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| MacosOwnerExecutionError::new("owner publication deadline overflowed"))?;
    wait_for_exact_direct_launchd_publication_proof(store, expectation, deadline, inspector)
        .map(|proof| proof.map(MacosDirectLaunchdOwnerProof::into_record))
}

fn wait_for_exact_direct_launchd_publication_proof(
    store: &MacosOwnerStore,
    expectation: &MacosDirectLaunchdPublicationExpectation,
    deadline: Instant,
    inspector: &mut impl MacosDirectLaunchdInspector,
) -> Result<Option<MacosDirectLaunchdOwnerProof>, MacosOwnerExecutionError> {
    use notify::{RecursiveMode, Watcher};
    use std::sync::mpsc;

    if let Some(proof) = exact_published_proof(store, expectation, inspector)? {
        return Ok(Some(proof));
    }
    if Instant::now() >= deadline {
        return Ok(None);
    }

    let owner_path = store.owner_record_path();
    let directory = owner_path
        .parent()
        .ok_or_else(|| MacosOwnerExecutionError::new("owner record has no parent directory"))?
        .to_path_buf();
    let metadata = fs::metadata(&directory)
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    if !metadata.is_dir() {
        return Err(MacosOwnerExecutionError::new(
            "owner record parent is not a directory",
        ));
    }
    let (signal_tx, signal_rx) = mpsc::sync_channel(1);
    let watched_path = owner_path.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok_and(|event| event.paths.iter().any(|path| path == &watched_path)) {
            let _ = signal_tx.try_send(());
        }
    })
    .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    watcher
        .watch(&directory, RecursiveMode::NonRecursive)
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    if let Some(proof) = exact_published_proof(store, expectation, inspector)? {
        return Ok(Some(proof));
    }

    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(None);
        };
        match signal_rx.recv_timeout(remaining) {
            Ok(()) => {
                if let Some(proof) = exact_published_proof(store, expectation, inspector)? {
                    return Ok(Some(proof));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => return Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(MacosOwnerExecutionError::new(
                    "owner publication watch disconnected",
                ));
            }
        }
    }
}

fn exact_published_proof(
    store: &MacosOwnerStore,
    expectation: &MacosDirectLaunchdPublicationExpectation,
    inspector: &mut impl MacosDirectLaunchdInspector,
) -> Result<Option<MacosDirectLaunchdOwnerProof>, MacosOwnerExecutionError> {
    let Some(record) = store
        .load_owner_record()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?
    else {
        return Ok(None);
    };
    let Some(proof) = corroborate_newer_direct_launchd_owner(&record, expectation, inspector)?
    else {
        return Ok(None);
    };
    let current = store
        .load_owner_record()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
    if current.as_ref() != Some(proof.record()) {
        return Ok(None);
    }
    Ok(Some(proof))
}

mod mutation;
#[cfg(target_os = "macos")]
mod native;
#[cfg(target_os = "macos")]
pub use mutation::NativeMacosDirectLaunchdMutator;
pub use mutation::{
    MacosDirectLaunchdBootstrapExpectation, MacosDirectLaunchdMutationOutcome,
    MacosDirectLaunchdMutator, parse_direct_launchd_autostart_state,
};
#[cfg(target_os = "macos")]
pub use native::NativeMacosDirectLaunchdInspector;
