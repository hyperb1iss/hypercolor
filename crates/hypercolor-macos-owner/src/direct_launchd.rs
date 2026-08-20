use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::{
    MAX_MACOS_DESIGNATED_REQUIREMENT_HASH_BYTES, MAX_MACOS_EXECUTABLE_PATH_BYTES, MacosDaemonOwner,
    MacosOwnerExecutionError, MacosOwnerRecord, MacosOwnerStore, MacosOwnerStoreError,
    validate_bounded_identity_text,
};

/// Stable launchd label for Hypercolor's raw direct installation.
pub const MACOS_DIRECT_LAUNCHD_LABEL: &str = "tech.hyperbliss.hypercolor";

const MAX_LAUNCHCTL_OUTPUT_BYTES: usize = 64 * 1024;
const SHA256_HEX_BYTES: usize = 64;

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

    /// Verify the exact executable bytes against an immutable unit manifest.
    fn executable_digest_matches(
        &mut self,
        executable_path: &Path,
        expected_sha256: &str,
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

/// Immutable identity expected from a candidate or rollback unit publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosDirectLaunchdPublicationExpectation {
    after_epoch: u64,
    executable_path: PathBuf,
    designated_requirement_hash: String,
    executable_sha256: String,
}

impl MacosDirectLaunchdPublicationExpectation {
    /// Validate and construct an exact publication expectation.
    pub fn new(
        after_epoch: u64,
        executable_path: impl Into<PathBuf>,
        designated_requirement_hash: impl Into<String>,
        executable_sha256: impl Into<String>,
    ) -> Result<Self, MacosOwnerStoreError> {
        let executable_path = executable_path.into();
        let executable_text =
            executable_path
                .to_str()
                .ok_or(MacosOwnerStoreError::InvalidOwnerIdentity {
                    field: "executable_path",
                    detail: "must be valid UTF-8",
                })?;
        validate_bounded_identity_text(
            "executable_path",
            executable_text,
            MAX_MACOS_EXECUTABLE_PATH_BYTES,
        )?;
        if !executable_path.is_absolute() {
            return Err(MacosOwnerStoreError::InvalidOwnerIdentity {
                field: "executable_path",
                detail: "must be absolute",
            });
        }
        let designated_requirement_hash = designated_requirement_hash.into();
        validate_bounded_identity_text(
            "designated_requirement_hash",
            &designated_requirement_hash,
            MAX_MACOS_DESIGNATED_REQUIREMENT_HASH_BYTES,
        )?;
        let executable_sha256 = executable_sha256.into();
        if executable_sha256.len() != SHA256_HEX_BYTES
            || !executable_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(MacosOwnerStoreError::InvalidOwnerIdentity {
                field: "executable_sha256",
                detail: "must be exactly 64 lowercase hexadecimal bytes",
            });
        }
        Ok(Self {
            after_epoch,
            executable_path,
            designated_requirement_hash,
            executable_sha256,
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
        &self.executable_path
    }

    /// Exact designated-requirement hash expected from the immutable unit.
    #[must_use]
    pub fn designated_requirement_hash(&self) -> &str {
        &self.designated_requirement_hash
    }

    /// Exact executable SHA-256 expected by the immutable unit manifest.
    #[must_use]
    pub fn executable_sha256(&self) -> &str {
        &self.executable_sha256
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
    if record.active_identity.executable_path != expectation.executable_path {
        return Err(MacosOwnerExecutionError::new(
            "newer direct launchd publication has the wrong executable path",
        ));
    }
    if record.active_identity.designated_requirement_hash != expectation.designated_requirement_hash
    {
        return Err(MacosOwnerExecutionError::new(
            "newer direct launchd publication has the wrong designated requirement",
        ));
    }
    let proof = corroborate_direct_launchd_owner(record, inspector)?;
    if !inspector.executable_digest_matches(
        expectation.executable_path(),
        expectation.executable_sha256(),
    )? {
        return Err(MacosOwnerExecutionError::new(
            "direct launchd executable does not match the immutable unit manifest",
        ));
    }
    require_direct_launchd_pid(record, inspector)?;
    Ok(Some(proof))
}

/// Wait for a newer exact direct-launchd publication and return its record.
pub fn wait_for_exact_direct_launchd_publication(
    store: &MacosOwnerStore,
    expectation: &MacosDirectLaunchdPublicationExpectation,
    timeout: Duration,
    inspector: &mut impl MacosDirectLaunchdInspector,
) -> Result<Option<MacosOwnerRecord>, MacosOwnerExecutionError> {
    use notify::{RecursiveMode, Watcher};
    use std::sync::mpsc;

    if let Some(record) = exact_published_record(store, expectation, inspector)? {
        return Ok(Some(record));
    }
    if timeout.is_zero() {
        return Ok(None);
    }

    let owner_path = store.owner_record_path();
    let directory = owner_path
        .parent()
        .ok_or_else(|| MacosOwnerExecutionError::new("owner record has no parent directory"))?
        .to_path_buf();
    fs::create_dir_all(&directory)
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
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
    if let Some(record) = exact_published_record(store, expectation, inspector)? {
        return Ok(Some(record));
    }

    let started = Instant::now();
    loop {
        let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
            return Ok(None);
        };
        match signal_rx.recv_timeout(remaining) {
            Ok(()) => {
                if let Some(record) = exact_published_record(store, expectation, inspector)? {
                    return Ok(Some(record));
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

fn exact_published_record(
    store: &MacosOwnerStore,
    expectation: &MacosDirectLaunchdPublicationExpectation,
    inspector: &mut impl MacosDirectLaunchdInspector,
) -> Result<Option<MacosOwnerRecord>, MacosOwnerExecutionError> {
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
    Ok(Some(proof.into_record()))
}

#[cfg(target_os = "macos")]
mod native {
    use std::fmt::Write as _;
    use std::process::Command;

    use core_foundation::{base::TCFType, data::CFData};
    use security_framework::os::macos::code_signing::{
        Flags as CodeSigningFlags, GuestAttributes, SecCode, SecRequirement,
    };
    use sha2::{Digest, Sha256};

    use super::{
        MACOS_DIRECT_LAUNCHD_LABEL, MacosDirectLaunchdInspector, MacosDirectLaunchdState,
        MacosOwnerExecutionError, Path, parse_direct_launchd_service_state,
    };

    const MAX_CODESIGN_OUTPUT_BYTES: usize = 16 * 1024;
    const MAX_DESIGNATED_REQUIREMENT_BYTES: usize = 8 * 1024;

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
            let output = Command::new("/bin/launchctl")
                .args(["print", &self.target()])
                .output()
                .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
            parse_direct_launchd_service_state(
                self.uid,
                output.status.code(),
                &output.stdout,
                &output.stderr,
            )
        }

        fn live_identity_matches(
            &mut self,
            identity: &crate::MacosOwnerIdentity,
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
            let requirement_text = designated_requirement(&identity.executable_path)?;
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

        fn executable_digest_matches(
            &mut self,
            executable_path: &Path,
            expected_sha256: &str,
        ) -> Result<bool, MacosOwnerExecutionError> {
            executable_digest(executable_path).map(|digest| digest == expected_sha256)
        }
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

    fn code_path_matches(
        code: &SecCode,
        expected: &Path,
    ) -> Result<bool, MacosOwnerExecutionError> {
        let Some(observed) = code
            .path(CodeSigningFlags::NONE)
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?
            .to_path()
        else {
            return Ok(false);
        };
        Ok(match (observed.canonicalize(), expected.canonicalize()) {
            (Ok(observed), Ok(expected)) => observed == expected,
            _ => false,
        })
    }

    fn designated_requirement(path: &Path) -> Result<String, MacosOwnerExecutionError> {
        let output = Command::new("/usr/bin/codesign")
            .args(["-d", "-r-"])
            .arg(path)
            .output()
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        if output.stdout.len() > MAX_CODESIGN_OUTPUT_BYTES
            || output.stderr.len() > MAX_CODESIGN_OUTPUT_BYTES
        {
            return Err(MacosOwnerExecutionError::new(
                "codesign designated-requirement output exceeds 16 KiB",
            ));
        }
        if !output.status.success() {
            return Err(MacosOwnerExecutionError::new(format!(
                "codesign could not read the live designated requirement: {}",
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

    fn requirement_hash(requirement: &str) -> String {
        let digest = Sha256::digest(requirement.as_bytes());
        let mut output = String::with_capacity(digest.len() * 2);
        for byte in digest {
            write!(&mut output, "{byte:02x}").expect("writing into a String cannot fail");
        }
        output
    }

    fn executable_digest(path: &Path) -> Result<String, MacosOwnerExecutionError> {
        use std::io::Read as _;

        let mut executable = hypercolor_platform_fs::open_no_follow(path)
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            let read = executable
                .read(&mut buffer)
                .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let digest = hasher.finalize();
        let mut output = String::with_capacity(digest.len() * 2);
        for byte in digest {
            write!(&mut output, "{byte:02x}").expect("writing into a String cannot fail");
        }
        Ok(output)
    }
}

#[cfg(target_os = "macos")]
pub use native::NativeMacosDirectLaunchdInspector;
