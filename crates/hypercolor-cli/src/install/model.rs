use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
use hypercolor_platform_fs::{DirectoryEntryKind, ReadOnlyDirectoryAuthority};

use serde::{Deserialize, Serialize};

pub const INSTALL_JOURNAL_SCHEMA_VERSION: u32 = 2;
pub const MAX_INSTALL_JOURNAL_BYTES: usize = 64 * 1024;
pub const MAX_PLATFORM_TRANSACTION_RECORD_BYTES: usize = 12 * 1024;
pub const MAX_LAYOUT_OPERATIONS: u16 = 256;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct UnitId(String);

impl UnitId {
    pub fn new(value: impl Into<String>) -> Result<Self, InstallModelError> {
        let value = value.into();
        let digest = value.strip_prefix("legacy-").unwrap_or(&value);
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(InstallModelError::InvalidUnitId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for UnitId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// One content-addressed release unit and its retained directory authority.
#[derive(Clone)]
pub struct UnitRecord {
    id: UnitId,
    root_hint: PathBuf,
    #[cfg(unix)]
    directory: Arc<ReadOnlyDirectoryAuthority>,
    #[cfg(unix)]
    identity: UnitRootIdentity,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UnitRootIdentity {
    device: u64,
    inode: u64,
}

impl UnitRecord {
    /// Bind a unit record to an already-open exact directory.
    ///
    /// `root_hint` is retained only for diagnostics. Filesystem access must
    /// use [`Self::directory`].
    ///
    /// # Errors
    ///
    /// Returns an error when the retained handle does not identify a
    /// directory or its metadata cannot be inspected.
    #[cfg(unix)]
    pub(crate) fn new(
        id: UnitId,
        root_hint: impl Into<PathBuf>,
        directory: ReadOnlyDirectoryAuthority,
    ) -> std::io::Result<Self> {
        let metadata = directory.metadata()?;
        if metadata.kind() != DirectoryEntryKind::Directory {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "unit authority does not identify a directory",
            ));
        }
        Ok(Self {
            id,
            root_hint: root_hint.into(),
            directory: Arc::new(directory),
            identity: UnitRootIdentity {
                device: metadata.device(),
                inode: metadata.inode(),
            },
        })
    }

    /// Return the digest-derived unit identifier bound to this directory.
    #[must_use]
    pub fn id(&self) -> &UnitId {
        &self.id
    }

    /// Borrow the exact opened unit directory capability.
    #[must_use]
    #[cfg(unix)]
    pub fn directory(&self) -> &ReadOnlyDirectoryAuthority {
        &self.directory
    }
}

impl std::fmt::Debug for UnitRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("UnitRecord");
        debug
            .field("id", &self.id)
            .field("root_hint", &self.root_hint);
        #[cfg(unix)]
        debug.field("identity", &self.identity);
        debug.finish_non_exhaustive()
    }
}

impl PartialEq for UnitRecord {
    fn eq(&self, other: &Self) -> bool {
        if self.id != other.id {
            return false;
        }
        #[cfg(unix)]
        {
            self.identity == other.identity
        }
        #[cfg(not(unix))]
        {
            self.root_hint == other.root_hint
        }
    }
}

impl Eq for UnitRecord {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct InstallTransactionId(String);

impl InstallTransactionId {
    pub fn new(value: impl Into<String>) -> Result<Self, InstallModelError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(InstallModelError::InvalidTransactionId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for InstallTransactionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformState {
    pub layout_unit: Option<UnitId>,
    pub launcher_unit: Option<UnitId>,
    pub loaded: bool,
    pub running_unit: Option<UnitId>,
    pub autostart_enabled: bool,
}

impl PlatformState {
    pub fn validate(&self) -> Result<(), InstallModelError> {
        if self.running_unit.is_some() && !self.loaded {
            return Err(InstallModelError::RunningWithoutLoadedLauncher);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationState {
    pub active_unit: Option<UnitId>,
    pub platform: PlatformState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformCheckpoint {
    PriorOriginal,
    PriorUnloaded,
    CandidateLayout,
    CandidateLauncher,
    CandidateActive,
    CandidateManager,
    CandidateAutostart,
    CandidateRuntime,
    PriorActiveRestored,
    PriorLauncherRestored,
    PriorLayoutRestored,
    PriorManager,
    PriorAutostart,
    PriorRestored,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformTransitionStates {
    pub prior_unloaded: PlatformState,
    pub candidate_manager: PlatformState,
    pub candidate_autostart: PlatformState,
    pub prior_manager: PlatformState,
    pub prior_autostart: PlatformState,
}

impl PlatformTransitionStates {
    pub fn validate(
        &self,
        prior: &PlatformState,
        target: &PlatformState,
    ) -> Result<(), InstallModelError> {
        for state in [
            &self.prior_unloaded,
            &self.candidate_manager,
            &self.candidate_autostart,
            &self.prior_manager,
            &self.prior_autostart,
        ] {
            state.validate()?;
        }

        if self.prior_unloaded.layout_unit != prior.layout_unit
            || self.prior_unloaded.launcher_unit != prior.launcher_unit
            || self.prior_unloaded.running_unit.is_some()
            || self.prior_unloaded.autostart_enabled != prior.autostart_enabled
            || self.candidate_manager.layout_unit != target.layout_unit
            || self.candidate_manager.launcher_unit != target.launcher_unit
            || self.candidate_manager.autostart_enabled != prior.autostart_enabled
            || self.candidate_manager.running_unit.is_some()
            || self.candidate_autostart.layout_unit != target.layout_unit
            || self.candidate_autostart.launcher_unit != target.launcher_unit
            || self.candidate_autostart.loaded != self.candidate_manager.loaded
            || self.candidate_autostart.running_unit != self.candidate_manager.running_unit
            || self.candidate_autostart.autostart_enabled != target.autostart_enabled
            || self.prior_manager.layout_unit != prior.layout_unit
            || self.prior_manager.launcher_unit != prior.launcher_unit
            || self.prior_manager.autostart_enabled != target.autostart_enabled
            || self.prior_manager.running_unit.is_some()
            || self.prior_autostart.layout_unit != prior.layout_unit
            || self.prior_autostart.launcher_unit != prior.launcher_unit
            || self.prior_autostart.loaded != self.prior_manager.loaded
            || self.prior_autostart.running_unit != self.prior_manager.running_unit
            || self.prior_autostart.autostart_enabled != prior.autostart_enabled
        {
            return Err(InstallModelError::InvalidTransitionStates);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPlatformTransaction {
    pub record: PlatformTransactionRecord,
    pub transitions: PlatformTransitionStates,
    pub layout_operation_count: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "platform", rename_all = "snake_case")]
pub enum PlatformTransactionRecord {
    Linux {
        schema_version: u32,
        payload: Vec<u8>,
    },
    Macos {
        schema_version: u32,
        payload: Vec<u8>,
    },
}

impl PlatformTransactionRecord {
    pub fn linux(schema_version: u32, payload: Vec<u8>) -> Result<Self, InstallModelError> {
        let record = Self::Linux {
            schema_version,
            payload,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn macos(schema_version: u32, payload: Vec<u8>) -> Result<Self, InstallModelError> {
        let record = Self::Macos {
            schema_version,
            payload,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> Result<(), InstallModelError> {
        let (schema_version, payload) = match self {
            Self::Linux {
                schema_version,
                payload,
            }
            | Self::Macos {
                schema_version,
                payload,
            } => (*schema_version, payload),
        };
        if schema_version == 0 {
            return Err(InstallModelError::ZeroPlatformRecordSchema);
        }
        if payload.is_empty() {
            return Err(InstallModelError::EmptyPlatformRecord);
        }
        if payload.len() > MAX_PLATFORM_TRANSACTION_RECORD_BYTES {
            return Err(InstallModelError::PlatformRecordTooLarge {
                limit: MAX_PLATFORM_TRANSACTION_RECORD_BYTES,
            });
        }
        Ok(())
    }

    #[must_use]
    pub fn payload(&self) -> &[u8] {
        match self {
            Self::Linux { payload, .. } | Self::Macos { payload, .. } => payload,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallDisposition {
    Forward,
    Rollback,
    Committed,
    RolledBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallAction {
    PreflightCandidate,
    UnloadPrior,
    ProvePriorGuardReleased,
    InstallCandidateLayout,
    InstallCandidateLauncher,
    SwitchToCandidate,
    ReloadCandidateManager,
    RestoreCandidateAutostart,
    RestoreCandidateRuntime,
    ProveCandidate,
    Commit,
    UnloadCandidateRuntime,
    UnloadCandidateAutostart,
    UnloadCandidateManager,
    ProveCandidateGuardReleased,
    RestorePriorActive,
    RestorePriorLauncher,
    RestorePriorLayout,
    ReloadPriorManager,
    RestorePriorAutostart,
    RestorePriorRuntime,
    ProvePrior,
    FinishRollback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallTargetPolicy {
    Preserve,
    EnableOnFirstInstall,
    EnabledAndRunning,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallJournalV1 {
    pub schema_version: u32,
    pub revision: u64,
    pub transaction_id: InstallTransactionId,
    pub disposition: InstallDisposition,
    pub next_action: Option<InstallAction>,
    pub prior_active_unit: Option<UnitId>,
    pub candidate_unit: UnitId,
    pub target_policy: InstallTargetPolicy,
    pub prior_platform: PlatformState,
    pub target_platform: PlatformState,
    pub transition_states: PlatformTransitionStates,
    pub layout_operation_count: u16,
    pub layout_operation_index: u16,
    pub platform_record: PlatformTransactionRecord,
    pub failure: Option<String>,
}

impl InstallJournalV1 {
    pub fn new(
        transaction_id: InstallTransactionId,
        prior_active_unit: Option<UnitId>,
        candidate_unit: UnitId,
        prior_platform: PlatformState,
        target_policy: InstallTargetPolicy,
        transition_states: PlatformTransitionStates,
        layout_operation_count: u16,
        platform_record: PlatformTransactionRecord,
    ) -> Result<Self, InstallModelError> {
        prior_platform.validate()?;
        platform_record.validate()?;
        let target_platform = target_policy.target_platform(&prior_platform, &candidate_unit);
        transition_states.validate(&prior_platform, &target_platform)?;
        if layout_operation_count == 0 || layout_operation_count > MAX_LAYOUT_OPERATIONS {
            return Err(InstallModelError::InvalidLayoutOperationCount);
        }
        Ok(Self {
            schema_version: INSTALL_JOURNAL_SCHEMA_VERSION,
            revision: 1,
            transaction_id,
            disposition: InstallDisposition::Forward,
            next_action: Some(InstallAction::PreflightCandidate),
            prior_active_unit,
            candidate_unit,
            target_policy,
            prior_platform,
            target_platform,
            transition_states,
            layout_operation_count,
            layout_operation_index: 0,
            platform_record,
            failure: None,
        })
    }

    pub fn validate(&self) -> Result<(), InstallModelError> {
        if self.schema_version != INSTALL_JOURNAL_SCHEMA_VERSION {
            return Err(InstallModelError::UnsupportedJournalSchema(
                self.schema_version,
            ));
        }
        if self.revision == 0 {
            return Err(InstallModelError::ZeroJournalRevision);
        }
        self.prior_platform.validate()?;
        self.platform_record.validate()?;
        let expected_target = self
            .target_policy
            .target_platform(&self.prior_platform, &self.candidate_unit);
        if self.target_platform != expected_target {
            return Err(InstallModelError::InvalidTargetState);
        }
        self.target_platform.validate()?;
        self.transition_states
            .validate(&self.prior_platform, &self.target_platform)?;
        if self.layout_operation_count == 0
            || self.layout_operation_count > MAX_LAYOUT_OPERATIONS
            || self.layout_operation_index > self.layout_operation_count
        {
            return Err(InstallModelError::InvalidLayoutOperationCount);
        }
        let layout_cursor_matches_action = match (self.disposition, self.next_action) {
            (
                InstallDisposition::Forward,
                Some(
                    InstallAction::PreflightCandidate
                    | InstallAction::UnloadPrior
                    | InstallAction::ProvePriorGuardReleased,
                ),
            ) => self.layout_operation_index == 0,
            (InstallDisposition::Forward, Some(InstallAction::InstallCandidateLayout)) => {
                self.layout_operation_index < self.layout_operation_count
            }
            (InstallDisposition::Forward | InstallDisposition::Committed, _) => {
                self.layout_operation_index == self.layout_operation_count
            }
            (InstallDisposition::Rollback, Some(InstallAction::RestorePriorLayout)) => {
                self.layout_operation_index > 0
            }
            (
                InstallDisposition::Rollback,
                Some(
                    InstallAction::ReloadPriorManager
                    | InstallAction::RestorePriorAutostart
                    | InstallAction::RestorePriorRuntime
                    | InstallAction::ProvePrior
                    | InstallAction::FinishRollback,
                ),
            )
            | (InstallDisposition::RolledBack, None) => self.layout_operation_index == 0,
            (InstallDisposition::Rollback, Some(_)) => {
                self.layout_operation_index == self.layout_operation_count
            }
            _ => true,
        };
        if !layout_cursor_matches_action {
            return Err(InstallModelError::InvalidLayoutOperationCursor);
        }
        let terminal = matches!(
            self.disposition,
            InstallDisposition::Committed | InstallDisposition::RolledBack
        );
        if terminal != self.next_action.is_none() {
            return Err(InstallModelError::InvalidTerminalState);
        }
        let action_matches_disposition = match self.disposition {
            InstallDisposition::Forward => self.next_action.is_some_and(InstallAction::is_forward),
            InstallDisposition::Rollback => {
                self.next_action.is_some_and(InstallAction::is_rollback)
            }
            InstallDisposition::Committed | InstallDisposition::RolledBack => {
                self.next_action.is_none()
            }
        };
        if !action_matches_disposition {
            return Err(InstallModelError::InvalidDispositionAction);
        }
        let failure_matches_disposition = match self.disposition {
            InstallDisposition::Forward | InstallDisposition::Committed => self.failure.is_none(),
            InstallDisposition::Rollback | InstallDisposition::RolledBack => self.failure.is_some(),
        };
        if !failure_matches_disposition {
            return Err(InstallModelError::InvalidFailureDisposition);
        }
        if self
            .failure
            .as_ref()
            .is_some_and(|failure| failure.len() > 4_096)
        {
            return Err(InstallModelError::FailureDetailTooLarge);
        }
        Ok(())
    }

    pub fn advance(
        &mut self,
        disposition: InstallDisposition,
        next_action: Option<InstallAction>,
    ) -> Result<(), InstallModelError> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(InstallModelError::JournalRevisionOverflow)?;
        self.disposition = disposition;
        self.next_action = next_action;
        self.validate()
    }
}

impl InstallTargetPolicy {
    pub(super) fn target_platform(
        self,
        prior: &PlatformState,
        candidate: &UnitId,
    ) -> PlatformState {
        let (launcher_unit, loaded, running, autostart_enabled) = match self {
            Self::Preserve => (
                prior.launcher_unit.as_ref().map(|_| candidate.clone()),
                prior.loaded,
                prior.running_unit.is_some(),
                prior.autostart_enabled,
            ),
            Self::EnableOnFirstInstall if prior.launcher_unit.is_none() => {
                (Some(candidate.clone()), true, true, true)
            }
            Self::EnableOnFirstInstall => (
                Some(candidate.clone()),
                prior.loaded,
                prior.running_unit.is_some(),
                prior.autostart_enabled,
            ),
            Self::EnabledAndRunning => (Some(candidate.clone()), true, true, true),
            Self::Disabled => (Some(candidate.clone()), false, false, false),
        };
        PlatformState {
            layout_unit: Some(candidate.clone()),
            launcher_unit,
            loaded,
            running_unit: running.then(|| candidate.clone()),
            autostart_enabled,
        }
    }
}

impl InstallAction {
    fn is_forward(self) -> bool {
        matches!(
            self,
            Self::PreflightCandidate
                | Self::UnloadPrior
                | Self::ProvePriorGuardReleased
                | Self::InstallCandidateLayout
                | Self::InstallCandidateLauncher
                | Self::SwitchToCandidate
                | Self::ReloadCandidateManager
                | Self::RestoreCandidateAutostart
                | Self::RestoreCandidateRuntime
                | Self::ProveCandidate
                | Self::Commit
        )
    }

    fn is_rollback(self) -> bool {
        matches!(
            self,
            Self::UnloadCandidateRuntime
                | Self::UnloadCandidateAutostart
                | Self::UnloadCandidateManager
                | Self::ProveCandidateGuardReleased
                | Self::RestorePriorActive
                | Self::RestorePriorLauncher
                | Self::RestorePriorLayout
                | Self::ReloadPriorManager
                | Self::RestorePriorAutostart
                | Self::RestorePriorRuntime
                | Self::ProvePrior
                | Self::FinishRollback
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallRequest {
    pub transaction_id: InstallTransactionId,
    pub candidate: UnitRecord,
    pub target_policy: InstallTargetPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    Committed {
        active_unit: UnitId,
    },
    RolledBack {
        active_unit: Option<UnitId>,
        failure: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallModelError {
    #[error("unit ID must be 64 lowercase hexadecimal characters with an optional legacy- prefix")]
    InvalidUnitId,
    #[error("install transaction ID must be 1-64 ASCII letters, digits, '_' or '-'")]
    InvalidTransactionId,
    #[error("platform state cannot report a running unit while unloaded")]
    RunningWithoutLoadedLauncher,
    #[error("install journal schema {0} is unsupported")]
    UnsupportedJournalSchema(u32),
    #[error("install journal revision must be nonzero")]
    ZeroJournalRevision,
    #[error("install journal target state does not name the candidate unit")]
    InvalidTargetState,
    #[error("install journal platform transition states are inconsistent")]
    InvalidTransitionStates,
    #[error("install journal layout operation cursor is invalid")]
    InvalidLayoutOperationCount,
    #[error("install journal action is inconsistent with its layout operation cursor")]
    InvalidLayoutOperationCursor,
    #[error("install journal terminal disposition and next action disagree")]
    InvalidTerminalState,
    #[error("install journal disposition and next action disagree")]
    InvalidDispositionAction,
    #[error("install journal disposition and failure detail disagree")]
    InvalidFailureDisposition,
    #[error("install journal failure detail exceeds 4096 bytes")]
    FailureDetailTooLarge,
    #[error("install journal revision overflow")]
    JournalRevisionOverflow,
    #[error("platform transaction record schema must be nonzero")]
    ZeroPlatformRecordSchema,
    #[error("platform transaction record must not be empty")]
    EmptyPlatformRecord,
    #[error("platform transaction record exceeds {limit} bytes")]
    PlatformRecordTooLarge { limit: usize },
}

pub(crate) fn active_target(unit: &UnitId) -> PathBuf {
    Path::new("units").join(unit.as_str())
}
