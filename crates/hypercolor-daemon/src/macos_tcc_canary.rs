use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[cfg(feature = "screen-capture")]
use std::time::Duration;

use anyhow::{Context, Result};
use hypercolor_macos_owner::MacosDaemonOwner;
use serde::{Deserialize, Serialize};

#[cfg(feature = "screen-capture")]
use std::{
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(feature = "screen-capture")]
use core_foundation::{base::TCFType, data::CFData};
#[cfg(feature = "screen-capture")]
use hypercolor_macos_capture::{
    MacosCaptureCadence, MacosCaptureSelection, MacosCaptureSelector, MacosFrameEvent,
    MacosProtectedSourceState, MacosScreenCaptureSession, MacosStreamRequest,
};
#[cfg(feature = "screen-capture")]
use hypercolor_macos_input::{
    MacosInputConfig, MacosInputError, MacosInputPublicationOutcome, MacosInputSession,
    MacosWorkerState, current_process_audit_token_identity, input_monitoring_granted,
    request_input_monitoring,
};
#[cfg(feature = "screen-capture")]
use security_framework::os::macos::code_signing::{
    Flags as CodeSigningFlags, GuestAttributes, SecCode, SecRequirement,
};
use sha2::{Digest, Sha256};
#[cfg(feature = "screen-capture")]
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

pub const MACOS_TCC_CANARY_SCHEMA_VERSION: u32 = 2;
const REQUEST_FILE_NAME: &str = "request.json";
const MAX_REQUEST_BYTES: u64 = 64 * 1024;
const MAX_RECEIPT_BYTES: u64 = 128 * 1024;
const MAX_WITNESS_BYTES: u64 = 64 * 1024;
const MAX_WITNESS_EVIDENCE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_EVIDENCE_ARTIFACTS: usize = 2_048;
const MIN_OPERATION_TIMEOUT_MS: u64 = 1_000;
const MAX_OPERATION_TIMEOUT_MS: u64 = 300_000;
const REQUIRED_ENTITLEMENTS: [&str; 6] = [
    "com.apple.security.cs.allow-jit",
    "com.apple.security.cs.allow-unsigned-executable-memory",
    "com.apple.security.device.audio-input",
    "com.apple.security.device.usb",
    "com.apple.security.network.client",
    "com.apple.security.network.server",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacosTccCanaryCapability {
    Keyboard,
    Pointer,
    Picker,
    Stream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacosTccCanaryInstallationScenario {
    AppOnly,
    DirectLaunchdOnly,
    HomebrewOnly,
    StandaloneOnly,
    AppDirectAppEnabledFirst,
    AppDirectDirectEnabledFirst,
    AppHomebrew,
    DirectHomebrew,
    AppDirectHomebrew,
}

impl MacosTccCanaryInstallationScenario {
    const fn permits(self, topology: MacosDaemonOwner) -> bool {
        match self {
            Self::AppOnly => matches!(topology, MacosDaemonOwner::AppSidecar),
            Self::DirectLaunchdOnly => matches!(topology, MacosDaemonOwner::DirectLaunchd),
            Self::HomebrewOnly => matches!(topology, MacosDaemonOwner::Homebrew),
            Self::StandaloneOnly => matches!(topology, MacosDaemonOwner::Standalone),
            Self::AppDirectAppEnabledFirst | Self::AppDirectDirectEnabledFirst => matches!(
                topology,
                MacosDaemonOwner::AppSidecar | MacosDaemonOwner::DirectLaunchd
            ),
            Self::AppHomebrew => matches!(
                topology,
                MacosDaemonOwner::AppSidecar | MacosDaemonOwner::Homebrew
            ),
            Self::DirectHomebrew => matches!(
                topology,
                MacosDaemonOwner::DirectLaunchd | MacosDaemonOwner::Homebrew
            ),
            Self::AppDirectHomebrew => !matches!(topology, MacosDaemonOwner::Standalone),
        }
    }

    const fn needs_repeated_login_proof(self) -> bool {
        !matches!(
            self,
            Self::AppOnly | Self::DirectLaunchdOnly | Self::HomebrewOnly | Self::StandaloneOnly
        )
    }

    const fn installed_topologies(self) -> &'static [MacosDaemonOwner] {
        match self {
            Self::AppOnly => &[MacosDaemonOwner::AppSidecar],
            Self::DirectLaunchdOnly => &[MacosDaemonOwner::DirectLaunchd],
            Self::HomebrewOnly => &[MacosDaemonOwner::Homebrew],
            Self::StandaloneOnly => &[MacosDaemonOwner::Standalone],
            Self::AppDirectAppEnabledFirst | Self::AppDirectDirectEnabledFirst => &[
                MacosDaemonOwner::AppSidecar,
                MacosDaemonOwner::DirectLaunchd,
            ],
            Self::AppHomebrew => &[MacosDaemonOwner::AppSidecar, MacosDaemonOwner::Homebrew],
            Self::DirectHomebrew => &[MacosDaemonOwner::DirectLaunchd, MacosDaemonOwner::Homebrew],
            Self::AppDirectHomebrew => &[
                MacosDaemonOwner::AppSidecar,
                MacosDaemonOwner::DirectLaunchd,
                MacosDaemonOwner::Homebrew,
            ],
        }
    }

    fn enable_order_is_valid(self, order: &[MacosDaemonOwner]) -> bool {
        let installed = self.installed_topologies();
        if order.len() != installed.len()
            || order.iter().any(|owner| !installed.contains(owner))
            || order
                .iter()
                .enumerate()
                .any(|(index, owner)| order[..index].contains(owner))
        {
            return false;
        }
        match self {
            Self::AppDirectAppEnabledFirst => {
                order
                    == [
                        MacosDaemonOwner::AppSidecar,
                        MacosDaemonOwner::DirectLaunchd,
                    ]
            }
            Self::AppDirectDirectEnabledFirst => {
                order
                    == [
                        MacosDaemonOwner::DirectLaunchd,
                        MacosDaemonOwner::AppSidecar,
                    ]
            }
            _ => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacosTccCanaryLifecyclePhase {
    Grant,
    Deny,
    LaterGrant,
    RevokeWhileLive,
    GrantAfterRevocation,
    AppLaunch,
    OwnerRestart,
    AppRelaunch,
    ServiceInstall,
    LoginStart,
    ServiceRestart,
    SignedUpdate,
}

impl MacosTccCanaryLifecyclePhase {
    const fn needs_predecessor(self) -> bool {
        matches!(
            self,
            Self::LaterGrant
                | Self::GrantAfterRevocation
                | Self::OwnerRestart
                | Self::AppRelaunch
                | Self::ServiceRestart
                | Self::SignedUpdate
        )
    }

    const fn replaces_process(self) -> bool {
        self.needs_predecessor()
    }

    const fn needs_lifecycle_action_witness(self) -> bool {
        matches!(
            self,
            Self::AppLaunch | Self::ServiceInstall | Self::LoginStart
        )
    }

    const fn required_predecessor(self) -> Option<Self> {
        match self {
            Self::LaterGrant => Some(Self::Deny),
            Self::GrantAfterRevocation => Some(Self::RevokeWhileLive),
            Self::OwnerRestart | Self::AppRelaunch | Self::ServiceRestart | Self::SignedUpdate => {
                Some(Self::Grant)
            }
            Self::Grant
            | Self::Deny
            | Self::RevokeWhileLive
            | Self::AppLaunch
            | Self::ServiceInstall
            | Self::LoginStart => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosTccCanaryRequest {
    pub schema_version: u32,
    pub run_id: String,
    pub row_id: String,
    pub scenario_id: String,
    pub installation_scenario: MacosTccCanaryInstallationScenario,
    pub login_iteration: u32,
    pub expected_topology: MacosDaemonOwner,
    pub lifecycle_phase: MacosTccCanaryLifecyclePhase,
    pub predecessor_row_id: Option<String>,
    pub process_replacement_witness_id: Option<String>,
    pub lifecycle_action_witness_id: Option<String>,
    pub login_arbitration_witness_id: Option<String>,
    pub scored_capability: MacosTccCanaryCapability,
    pub capabilities: Vec<MacosTccCanaryCapability>,
    pub allow_input_prompt: bool,
    pub allow_screen_prompt: bool,
    pub allow_picker: bool,
    pub operation_timeout_ms: u64,
    pub fresh_tcc_reset_witness_id: Option<String>,
    pub system_settings_identity_witness_id: String,
    pub expected_prompt_text: String,
    pub expected_system_settings_entry: String,
}

impl MacosTccCanaryRequest {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.schema_version == MACOS_TCC_CANARY_SCHEMA_VERSION,
            "unsupported macOS TCC canary request schema {}",
            self.schema_version
        );
        validate_identifier(&self.run_id, "run_id")?;
        validate_identifier(&self.row_id, "row_id")?;
        validate_identifier(&self.scenario_id, "scenario_id")?;
        anyhow::ensure!(
            self.installation_scenario.permits(self.expected_topology),
            "installation scenario does not permit the expected topology"
        );
        anyhow::ensure!(self.login_iteration > 0, "login_iteration must be positive");
        if let Some(predecessor) = self.predecessor_row_id.as_deref() {
            validate_identifier(predecessor, "predecessor_row_id")?;
            anyhow::ensure!(predecessor != self.row_id, "a row cannot precede itself");
        }
        anyhow::ensure!(
            self.lifecycle_phase.needs_predecessor() == self.predecessor_row_id.is_some(),
            "predecessor_row_id is permitted exactly for process replacement phases"
        );
        anyhow::ensure!(
            self.lifecycle_phase.replaces_process()
                == self.process_replacement_witness_id.is_some(),
            "process replacement phases require exactly one process replacement witness"
        );
        if let Some(witness) = self.process_replacement_witness_id.as_deref() {
            validate_identifier(witness, "process_replacement_witness_id")?;
        }
        anyhow::ensure!(
            self.lifecycle_phase.needs_lifecycle_action_witness()
                == self.lifecycle_action_witness_id.is_some(),
            "app launch, service install, and login start require exactly one lifecycle action witness"
        );
        if let Some(witness) = self.lifecycle_action_witness_id.as_deref() {
            validate_identifier(witness, "lifecycle_action_witness_id")?;
        }
        anyhow::ensure!(
            self.installation_scenario.needs_repeated_login_proof()
                == self.login_arbitration_witness_id.is_some(),
            "mixed installation rows require exactly one login arbitration witness"
        );
        if let Some(witness) = self.login_arbitration_witness_id.as_deref() {
            validate_identifier(witness, "login_arbitration_witness_id")?;
        }
        if let Some(witness) = self.fresh_tcc_reset_witness_id.as_deref() {
            validate_identifier(witness, "fresh_tcc_reset_witness_id")?;
        }
        validate_identifier(
            &self.system_settings_identity_witness_id,
            "system_settings_identity_witness_id",
        )?;
        validate_observed_text(&self.expected_prompt_text, "expected_prompt_text")?;
        validate_observed_text(
            &self.expected_system_settings_entry,
            "expected_system_settings_entry",
        )?;
        anyhow::ensure!(
            (MIN_OPERATION_TIMEOUT_MS..=MAX_OPERATION_TIMEOUT_MS)
                .contains(&self.operation_timeout_ms),
            "operation_timeout_ms must be from {MIN_OPERATION_TIMEOUT_MS} through {MAX_OPERATION_TIMEOUT_MS}"
        );
        anyhow::ensure!(
            !self.capabilities.is_empty(),
            "capabilities cannot be empty"
        );
        let unique = self.capabilities.iter().copied().collect::<BTreeSet<_>>();
        anyhow::ensure!(
            unique.len() == self.capabilities.len(),
            "capabilities cannot contain duplicates"
        );
        anyhow::ensure!(
            !unique.contains(&MacosTccCanaryCapability::Stream)
                || unique.contains(&MacosTccCanaryCapability::Picker),
            "stream evidence requires picker evidence in the same process"
        );
        anyhow::ensure!(
            scored_capability_shape_is_valid(self.scored_capability, &unique),
            "each row scores one capability, except stream rows also carry picker evidence"
        );
        anyhow::ensure!(
            capability_phases(self.scored_capability).contains(&self.lifecycle_phase)
                || topology_phases(self.expected_topology).contains(&self.lifecycle_phase),
            "the lifecycle phase does not apply to the scored capability and topology"
        );
        anyhow::ensure!(
            (!unique.contains(&MacosTccCanaryCapability::Picker)
                && !unique.contains(&MacosTccCanaryCapability::Stream))
                || self.allow_picker,
            "screen evidence requires explicit allow_picker consent"
        );
        Ok(())
    }

    #[cfg(feature = "screen-capture")]
    fn timeout(&self) -> Duration {
        Duration::from_millis(self.operation_timeout_ms)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacosTccCanaryOutcome {
    Passed,
    Denied,
    Revoked,
    NeedsProcessRestart,
    Cancelled,
    TimedOut,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosTccCanaryCapabilityEvidence {
    pub capability: MacosTccCanaryCapability,
    pub outcome: MacosTccCanaryOutcome,
    pub resulting_api_state: String,
    pub typed_error: Option<String>,
    pub tcc_preflight_before: Option<bool>,
    pub tcc_request_result: Option<bool>,
    pub tcc_preflight_after: Option<bool>,
    pub requested_tap_mask: Option<u64>,
    pub tap_mask: Option<u64>,
    pub tap_created: Option<bool>,
    pub tap_enabled: Option<bool>,
    pub run_loop_started: Option<bool>,
    pub redacted_event_count: Option<u64>,
    pub picker_presented: Option<bool>,
    pub picker_selected: Option<bool>,
    pub stream_started: Option<bool>,
    pub first_complete_frame: Option<bool>,
    pub first_frame_monotonic_ns: Option<u64>,
    pub resource_live_before_revocation: Option<bool>,
    pub resource_failed_after_revocation: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosTccCanaryLauncherEvidence {
    pub actual_launcher: String,
    pub expected_label: Option<String>,
    pub parent_pid: Option<u32>,
    pub parent_executable_path: Option<PathBuf>,
    pub parent_signing: Option<MacosTccCanarySigningEvidence>,
    pub launchctl_pid_matches: Option<bool>,
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosTccCanarySigningEvidence {
    pub bundle_identifier: String,
    pub team_identifier: String,
    pub designated_requirement: String,
    pub designated_requirement_sha256: String,
    pub cdhash: String,
    pub process_bound_pid: u32,
    pub process_bound_fingerprint: String,
    pub process_bound_valid: bool,
    pub audit_token_bound_valid: bool,
    pub authorities: Vec<String>,
    pub entitlement_keys: Vec<String>,
    pub codesign_strict_valid: bool,
    pub hardened_runtime: bool,
    pub secure_timestamp: bool,
    pub spctl_accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosTccCanaryReceipt {
    pub schema_version: u32,
    pub run_id: String,
    pub row_id: String,
    pub scenario_id: String,
    pub installation_scenario: MacosTccCanaryInstallationScenario,
    pub login_iteration: u32,
    pub topology: MacosDaemonOwner,
    pub lifecycle_phase: MacosTccCanaryLifecyclePhase,
    pub predecessor_row_id: Option<String>,
    pub process_replacement_witness_id: Option<String>,
    pub lifecycle_action_witness_id: Option<String>,
    pub login_arbitration_witness_id: Option<String>,
    pub scored_capability: MacosTccCanaryCapability,
    pub fresh_tcc_reset_witness_id: Option<String>,
    pub system_settings_identity_witness_id: String,
    pub expected_prompt_text: String,
    pub expected_system_settings_entry: String,
    pub host_architecture: String,
    pub executable_slice: String,
    pub translated_process: bool,
    pub os_version: String,
    pub binary_version: String,
    pub pid: u32,
    pub process_fingerprint: String,
    pub audit_token_identity: String,
    pub executable_path: PathBuf,
    pub process_started_unix_ms: u64,
    pub operation_finished_unix_ms: u64,
    pub launcher: MacosTccCanaryLauncherEvidence,
    pub signing: MacosTccCanarySigningEvidence,
    pub capabilities: Vec<MacosTccCanaryCapabilityEvidence>,
    pub acceptance_claim: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacosTccCanaryWitnessKind {
    FreshTccReset,
    SystemSettingsIdentity,
    ProcessReplacement,
    LifecycleAction,
    LoginArbitration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosTccCanaryWitness {
    pub schema_version: u32,
    pub run_id: String,
    pub row_id: String,
    pub witness_id: String,
    pub kind: MacosTccCanaryWitnessKind,
    pub observer: String,
    pub observed_unix_ms: u64,
    pub evidence_sha256: String,
    pub prompt_text: Option<String>,
    pub system_settings_entry: Option<String>,
    #[serde(default)]
    pub observed_pid: Option<u32>,
    #[serde(default)]
    pub observed_audit_token_identity: Option<String>,
    #[serde(default)]
    pub observed_signing_audit_token_identity: Option<String>,
    #[serde(default)]
    pub observed_cdhash: Option<String>,
    #[serde(default)]
    pub observed_designated_requirement_sha256: Option<String>,
    #[serde(default)]
    pub observed_process_fingerprint: Option<String>,
    #[serde(default)]
    pub parent_pid: Option<u32>,
    #[serde(default)]
    pub parent_audit_token_identity: Option<String>,
    #[serde(default)]
    pub parent_signing_audit_token_identity: Option<String>,
    #[serde(default)]
    pub parent_cdhash: Option<String>,
    #[serde(default)]
    pub parent_designated_requirement_sha256: Option<String>,
    #[serde(default)]
    pub parent_process_fingerprint: Option<String>,
    pub fresh_tcc_database_observed: Option<bool>,
    pub predecessor_pid: Option<u32>,
    #[serde(default)]
    pub predecessor_audit_token_identity: Option<String>,
    #[serde(default)]
    pub predecessor_process_fingerprint: Option<String>,
    pub predecessor_exit_observed: Option<bool>,
    #[serde(default)]
    pub predecessor_parent_pid: Option<u32>,
    #[serde(default)]
    pub predecessor_parent_audit_token_identity: Option<String>,
    #[serde(default)]
    pub predecessor_parent_process_fingerprint: Option<String>,
    #[serde(default)]
    pub predecessor_parent_exit_observed: Option<bool>,
    pub launcher_action: Option<String>,
    #[serde(default)]
    pub installed_topologies: Option<Vec<MacosDaemonOwner>>,
    #[serde(default)]
    pub enable_order: Option<Vec<MacosDaemonOwner>>,
    #[serde(default)]
    pub selected_topology: Option<MacosDaemonOwner>,
    #[serde(default)]
    pub losing_topologies: Option<Vec<MacosDaemonOwner>>,
    #[serde(default)]
    pub owner_conflict_observed: Option<bool>,
    #[serde(default)]
    pub login_iteration: Option<u32>,
    #[serde(default)]
    pub login_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosTccCanaryValidation {
    pub schema_version: u32,
    pub receipt_structure_valid: bool,
    pub identity_consistent: bool,
    pub preferred_topology_eligible: bool,
    pub physical_acceptance_claimed: bool,
    pub receipt_count: usize,
    pub capability_qualifications: Vec<MacosTccCanaryCapabilityQualification>,
    pub missing_requirements: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacosTccCanaryCapabilityQualification {
    pub capability: MacosTccCanaryCapability,
    pub preferred_topology: Option<MacosDaemonOwner>,
    pub qualified_topologies: Vec<MacosDaemonOwner>,
    pub app_broker_required: bool,
}

pub fn macos_tcc_canary_directory(data_dir: &Path) -> PathBuf {
    data_dir.join("macos-tcc-canary")
}

pub fn macos_tcc_canary_request_path(data_dir: &Path) -> PathBuf {
    macos_tcc_canary_directory(data_dir).join(REQUEST_FILE_NAME)
}

pub fn validate_macos_tcc_canary_request(request_path: &Path) -> Result<()> {
    read_json_bounded::<MacosTccCanaryRequest>(request_path, MAX_REQUEST_BYTES)?.validate()
}

pub fn publish_macos_tcc_canary_artifact(
    canary_root: &Path,
    source: &Path,
    destination: &Path,
) -> Result<()> {
    ensure_real_directory(canary_root, false)?;
    let parent = destination
        .parent()
        .context("macOS TCC canary artifact destination has no parent")?;
    ensure_canary_descendant_directory(canary_root, parent)?;
    let file_name = destination
        .file_name()
        .context("macOS TCC canary artifact destination has no filename")?;
    anyhow::ensure!(
        matches!(file_name.to_str(), Some(name) if !name.is_empty() && name != "." && name != ".."),
        "macOS TCC canary artifact destination has an invalid filename"
    );
    let (file, metadata) = open_regular_file(source)?;
    anyhow::ensure!(
        metadata.len() <= MAX_WITNESS_EVIDENCE_BYTES,
        "macOS TCC canary artifact exceeds {MAX_WITNESS_EVIDENCE_BYTES} bytes"
    );
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(MAX_WITNESS_EVIDENCE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {}", source.display()))?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_WITNESS_EVIDENCE_BYTES,
        "macOS TCC canary artifact exceeds {MAX_WITNESS_EVIDENCE_BYTES} bytes"
    );
    write_bytes_new(destination, &bytes)
}

pub fn arm_macos_tcc_canary(data_dir: &Path, request_path: &Path) -> Result<PathBuf> {
    let request = read_json_bounded::<MacosTccCanaryRequest>(request_path, MAX_REQUEST_BYTES)?;
    request.validate()?;
    let canary_dir = macos_tcc_canary_directory(data_dir);
    ensure_real_directory(data_dir, false)?;
    ensure_real_directory(&canary_dir, true)?;
    ensure_existing_real_directory(&canary_dir.join("requests"))?;
    ensure_existing_real_directory(&canary_dir.join("receipts"))?;
    fs::set_permissions(&canary_dir, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to secure {}", canary_dir.display()))?;
    let destination = macos_tcc_canary_request_path(data_dir);
    write_json_new(&destination, &request)?;
    sync_parent(&canary_dir)?;
    Ok(destination)
}

pub fn validate_macos_tcc_canary_receipts(receipt_dir: &Path) -> Result<MacosTccCanaryValidation> {
    ensure_real_directory(receipt_dir, false)?;
    ensure_real_directory(&receipt_dir.join("evidence"), false)?;
    let mut artifact_paths = fs::read_dir(receipt_dir)
        .with_context(|| format!("failed to read {}", receipt_dir.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    artifact_paths.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
    });
    artifact_paths.sort();
    anyhow::ensure!(
        artifact_paths.len() <= MAX_EVIDENCE_ARTIFACTS,
        "receipt directory exceeds {MAX_EVIDENCE_ARTIFACTS} JSON files"
    );
    let mut receipts = Vec::new();
    let mut witnesses = Vec::new();
    for path in artifact_paths {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("macOS TCC evidence filename is not valid UTF-8")?;
        if file_name.ends_with(".receipt.json") {
            receipts.push(read_json_bounded::<MacosTccCanaryReceipt>(
                &path,
                MAX_RECEIPT_BYTES,
            )?);
        } else if file_name.ends_with(".witness.json") {
            let witness = read_json_bounded::<MacosTccCanaryWitness>(&path, MAX_WITNESS_BYTES)?;
            anyhow::ensure!(
                witness_evidence_matches(receipt_dir, &witness)?,
                "macOS TCC witness evidence hash does not match {}",
                witness.witness_id
            );
            witnesses.push(witness);
        } else {
            anyhow::bail!(
                "macOS TCC evidence JSON must end in .receipt.json or .witness.json: {}",
                path.display()
            );
        }
    }
    Ok(validate_receipt_set(&receipts, &witnesses))
}

#[cfg(feature = "screen-capture")]
pub fn run_armed_macos_tcc_canary(
    data_dir: &Path,
    actual_topology: MacosDaemonOwner,
) -> Result<bool> {
    let Some((request, archived_request_path)) = claim_request(data_dir, actual_topology)? else {
        return Ok(false);
    };
    let canary_dir = macos_tcc_canary_directory(data_dir);
    let receipt_path = canary_dir
        .join("receipts")
        .join(&request.run_id)
        .join(format!("{}.receipt.json", request.row_id));
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("hypercolor-macos-tcc-canary".to_owned())
        .spawn(move || {
            let result = execute_request(request, actual_topology).and_then(|mut receipt| {
                let parent = receipt_path
                    .parent()
                    .context("macOS TCC canary receipt path has no parent")?;
                ensure_canary_descendant_directory(&canary_dir, parent)?;
                let pending_path = parent.join(format!("{}.receipt.pending", receipt.row_id));
                write_json_new(&pending_path, &receipt)?;
                let live_validation = await_live_identity_witness(parent, &receipt);
                let identity_validated_unix_ms = match live_validation {
                    Ok(observed_unix_ms) => observed_unix_ms,
                    Err(error) => {
                        fs::remove_file(&pending_path).with_context(|| {
                            format!("failed to remove {}", pending_path.display())
                        })?;
                        sync_parent(parent)?;
                        return Err(error);
                    }
                };
                receipt.operation_finished_unix_ms = identity_validated_unix_ms;
                if let Some(parent_signing) = receipt.launcher.parent_signing.as_mut() {
                    parent_signing.audit_token_bound_valid = true;
                }
                write_json_new(&receipt_path, &receipt)?;
                fs::remove_file(&pending_path)
                    .with_context(|| format!("failed to remove {}", pending_path.display()))?;
                sync_parent(parent)?;
                Ok(receipt_path)
            });
            let _ = result_tx.send(result);
            dispatch2::run_on_main(|_mtm| {
                if let Some(run_loop) = objc2_core_foundation::CFRunLoop::main() {
                    run_loop.stop();
                }
            });
        })
        .context("failed to start the macOS TCC canary worker")?;
    objc2_core_foundation::CFRunLoop::run();
    let result = result_rx
        .recv()
        .context("macOS TCC canary worker exited without a result")?;
    worker
        .join()
        .map_err(|_| anyhow::anyhow!("macOS TCC canary worker panicked"))?;
    match result {
        Ok(receipt_path) => {
            println!(
                "macos_tcc_canary_receipt={} request={}",
                receipt_path.display(),
                archived_request_path.display()
            );
            Ok(true)
        }
        Err(error) => Err(error),
    }
}

#[cfg(feature = "screen-capture")]
fn await_live_identity_witness(receipt_dir: &Path, receipt: &MacosTccCanaryReceipt) -> Result<u64> {
    const WITNESS_DEADLINE: Duration = Duration::from_secs(25);
    let witness_path = receipt_dir.join(format!(
        "{}.witness.json",
        receipt.system_settings_identity_witness_id
    ));
    let deadline = Instant::now() + WITNESS_DEADLINE;
    loop {
        if witness_path.exists() {
            let witness =
                read_json_bounded::<MacosTccCanaryWitness>(&witness_path, MAX_WITNESS_BYTES)?;
            anyhow::ensure!(
                witness_evidence_matches(receipt_dir, &witness)?,
                "macOS TCC identity witness evidence hash does not match"
            );
            let live_identity_valid = live_identity_witness_is_valid(receipt, &witness);
            let mut verified_receipt = receipt.clone();
            if let Some(parent_signing) = verified_receipt.launcher.parent_signing.as_mut() {
                parent_signing.audit_token_bound_valid = live_identity_valid;
            }
            let identity_validated_unix_ms = unix_time_ms()?;
            verified_receipt.operation_finished_unix_ms = identity_validated_unix_ms;
            let witnesses = BTreeMap::from([(witness.witness_id.as_str(), &witness)]);
            anyhow::ensure!(
                validate_witness_structure(&witness)
                    && live_identity_valid
                    && receipt_identity_valid(&verified_receipt, &witnesses),
                "macOS TCC identity witness is not bound to the live signed process"
            );
            return Ok(identity_validated_unix_ms);
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "timed out waiting for the current-row System Settings identity witness"
        );
        thread::park_timeout(Duration::from_millis(25));
    }
}

#[cfg(feature = "screen-capture")]
fn live_identity_witness_is_valid(
    receipt: &MacosTccCanaryReceipt,
    witness: &MacosTccCanaryWitness,
) -> bool {
    let daemon_valid = witness
        .observed_signing_audit_token_identity
        .as_deref()
        .is_some_and(|audit_token| {
            live_signing_identity_is_valid(audit_token, &receipt.executable_path, &receipt.signing)
        });
    if receipt.topology != MacosDaemonOwner::AppSidecar {
        return daemon_valid;
    }
    let Some((parent_path, parent_signing, parent_audit_token)) = receipt
        .launcher
        .parent_executable_path
        .as_deref()
        .zip(receipt.launcher.parent_signing.as_ref())
        .zip(witness.parent_signing_audit_token_identity.as_deref())
        .map(|((path, signing), token)| (path, signing, token))
    else {
        return false;
    };
    daemon_valid && live_signing_identity_is_valid(parent_audit_token, parent_path, parent_signing)
}

#[cfg(feature = "screen-capture")]
fn live_signing_identity_is_valid(
    audit_token: &str,
    expected_path: &Path,
    signing: &MacosTccCanarySigningEvidence,
) -> bool {
    let Some(bytes) = audit_token_bytes(audit_token) else {
        return false;
    };
    if audit_token_identity(audit_token).map(|identity| identity.pid)
        != Some(signing.process_bound_pid)
    {
        return false;
    }
    let token_data = CFData::from_buffer(&bytes);
    let mut attributes = GuestAttributes::new();
    attributes.set_audit_token(token_data.as_concrete_TypeRef());
    let Ok(code) = SecCode::copy_guest_with_attribues(None, &attributes, CodeSigningFlags::NONE)
    else {
        return false;
    };
    let Some(path) = code
        .path(CodeSigningFlags::NONE)
        .ok()
        .and_then(|url| url.to_path())
    else {
        return false;
    };
    let Ok(requirement) = signing.designated_requirement.parse::<SecRequirement>() else {
        return false;
    };
    let Ok(cdhash_requirement) =
        format!("cdhash H\"{}\"", signing.cdhash).parse::<SecRequirement>()
    else {
        return false;
    };
    path == expected_path
        && code
            .check_validity(CodeSigningFlags::STRICT_VALIDATE, &requirement)
            .is_ok()
        && code
            .check_validity(CodeSigningFlags::STRICT_VALIDATE, &cdhash_requirement)
            .is_ok()
}

#[cfg(feature = "screen-capture")]
fn claim_request(
    data_dir: &Path,
    actual_topology: MacosDaemonOwner,
) -> Result<Option<(MacosTccCanaryRequest, PathBuf)>> {
    ensure_real_directory(data_dir, false)?;
    ensure_real_directory(&macos_tcc_canary_directory(data_dir), false)?;
    ensure_existing_real_directory(&macos_tcc_canary_directory(data_dir).join("requests"))?;
    ensure_existing_real_directory(&macos_tcc_canary_directory(data_dir).join("receipts"))?;
    let request_path = macos_tcc_canary_request_path(data_dir);
    if !request_path.exists() {
        return Ok(None);
    }
    let request = read_json_bounded::<MacosTccCanaryRequest>(&request_path, MAX_REQUEST_BYTES)?;
    request.validate()?;
    if request.expected_topology != actual_topology {
        return Ok(None);
    }
    let archive_dir = macos_tcc_canary_directory(data_dir)
        .join("requests")
        .join(&request.run_id);
    ensure_canary_descendant_directory(&macos_tcc_canary_directory(data_dir), &archive_dir)?;
    let archived = archive_dir.join(format!("{}.json", request.row_id));
    anyhow::ensure!(
        !archived.exists(),
        "macOS TCC canary row {} is already archived",
        request.row_id
    );
    fs::rename(&request_path, &archived).with_context(|| {
        format!(
            "failed to claim macOS TCC canary request {}",
            request_path.display()
        )
    })?;
    sync_parent(&macos_tcc_canary_directory(data_dir))?;
    sync_parent(&archive_dir)?;
    Ok(Some((request, archived)))
}

#[cfg(feature = "screen-capture")]
fn execute_request(
    request: MacosTccCanaryRequest,
    actual_topology: MacosDaemonOwner,
) -> Result<MacosTccCanaryReceipt> {
    let process_started_unix_ms = unix_time_ms()?;
    let executable_path = std::env::current_exe().context("failed to resolve canary executable")?;
    let pid = std::process::id();
    let audit_token_identity =
        current_process_audit_token_identity().map_err(anyhow::Error::from)?;
    let process_fingerprint = process_fingerprint(pid)?;
    let signing = inspect_signing(
        &executable_path,
        pid,
        &process_fingerprint,
        Some(&audit_token_identity),
    )?;
    let launcher = inspect_launcher(actual_topology, &signing)?;
    let host_architecture = host_architecture()?;
    let translated_process = sysctl_flag("sysctl.proc_translated")?;
    let os_version = bounded_command_text("/usr/bin/sw_vers", &["-productVersion"])?;
    let capabilities = execute_capabilities(&request);
    let operation_finished_unix_ms = unix_time_ms()?;
    Ok(MacosTccCanaryReceipt {
        schema_version: MACOS_TCC_CANARY_SCHEMA_VERSION,
        run_id: request.run_id,
        row_id: request.row_id,
        scenario_id: request.scenario_id,
        installation_scenario: request.installation_scenario,
        login_iteration: request.login_iteration,
        topology: actual_topology,
        lifecycle_phase: request.lifecycle_phase,
        predecessor_row_id: request.predecessor_row_id,
        process_replacement_witness_id: request.process_replacement_witness_id,
        lifecycle_action_witness_id: request.lifecycle_action_witness_id,
        login_arbitration_witness_id: request.login_arbitration_witness_id,
        scored_capability: request.scored_capability,
        fresh_tcc_reset_witness_id: request.fresh_tcc_reset_witness_id,
        system_settings_identity_witness_id: request.system_settings_identity_witness_id,
        expected_prompt_text: request.expected_prompt_text,
        expected_system_settings_entry: request.expected_system_settings_entry,
        host_architecture,
        executable_slice: std::env::consts::ARCH.to_owned(),
        translated_process,
        os_version,
        binary_version: env!("CARGO_PKG_VERSION").to_owned(),
        pid,
        process_fingerprint,
        audit_token_identity,
        executable_path,
        process_started_unix_ms,
        operation_finished_unix_ms,
        launcher,
        signing,
        capabilities,
        acceptance_claim: "evidence_only".to_owned(),
    })
}

#[cfg(feature = "screen-capture")]
fn execute_capabilities(request: &MacosTccCanaryRequest) -> Vec<MacosTccCanaryCapabilityEvidence> {
    let mut evidence = Vec::with_capacity(request.capabilities.len());
    for capability in &request.capabilities {
        match capability {
            MacosTccCanaryCapability::Keyboard => {
                evidence.push(execute_input_capability(request, true));
            }
            MacosTccCanaryCapability::Pointer => {
                evidence.push(execute_input_capability(request, false));
            }
            MacosTccCanaryCapability::Picker => {}
            MacosTccCanaryCapability::Stream => {}
        }
    }
    if request
        .capabilities
        .contains(&MacosTccCanaryCapability::Picker)
    {
        let (picker, stream) = execute_screen_capabilities(request);
        evidence.push(picker);
        if let Some(stream) = stream {
            evidence.push(stream);
        }
    }
    evidence
}

#[cfg(feature = "screen-capture")]
fn execute_input_capability(
    request: &MacosTccCanaryRequest,
    keyboard: bool,
) -> MacosTccCanaryCapabilityEvidence {
    let capability = if keyboard {
        MacosTccCanaryCapability::Keyboard
    } else {
        MacosTccCanaryCapability::Pointer
    };
    let preflight_before = keyboard.then(input_monitoring_granted);
    let request_result = (keyboard && request.allow_input_prompt).then(request_input_monitoring);
    let event_count = Arc::new(AtomicU64::new(0));
    let callback_count = Arc::clone(&event_count);
    let clock_started = Instant::now();
    let session = MacosInputSession::start(
        MacosInputConfig {
            keyboard,
            pointer: !keyboard,
            epoch: 1,
            clock: Arc::new(move || {
                u64::try_from(clock_started.elapsed().as_millis()).unwrap_or(u64::MAX)
            }),
        },
        move |batch| {
            callback_count.fetch_add(
                u64::try_from(batch.events.len()).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
            MacosInputPublicationOutcome::Published
        },
    );
    let mut session = match session {
        Ok(session) => session,
        Err(error) => {
            let outcome = match error {
                MacosInputError::PermissionDenied if request_result == Some(true) => {
                    MacosTccCanaryOutcome::NeedsProcessRestart
                }
                MacosInputError::PermissionDenied => MacosTccCanaryOutcome::Denied,
                _ => MacosTccCanaryOutcome::Failed,
            };
            return MacosTccCanaryCapabilityEvidence {
                capability,
                outcome,
                resulting_api_state: resulting_api_state(capability, outcome).to_owned(),
                typed_error: Some(input_error_code(&error).to_owned()),
                tcc_preflight_before: preflight_before,
                tcc_request_result: request_result,
                tcc_preflight_after: keyboard.then(input_monitoring_granted),
                requested_tap_mask: None,
                tap_mask: None,
                tap_created: Some(false),
                tap_enabled: Some(false),
                run_loop_started: Some(false),
                redacted_event_count: Some(0),
                picker_presented: None,
                picker_selected: None,
                stream_started: None,
                first_complete_frame: None,
                first_frame_monotonic_ns: None,
                resource_live_before_revocation: None,
                resource_failed_after_revocation: None,
            };
        }
    };
    let requested_masks = session.effective_masks();
    let requested_tap_mask = if keyboard {
        requested_masks.keyboard
    } else {
        requested_masks.pointer
    };
    let installed_masks = match session.installed_masks() {
        Ok(masks) => masks,
        Err(error) => {
            session.stop();
            return MacosTccCanaryCapabilityEvidence {
                capability,
                outcome: MacosTccCanaryOutcome::Failed,
                resulting_api_state: resulting_api_state(capability, MacosTccCanaryOutcome::Failed)
                    .to_owned(),
                typed_error: Some(input_error_code(&error).to_owned()),
                tcc_preflight_before: preflight_before,
                tcc_request_result: request_result,
                tcc_preflight_after: keyboard.then(input_monitoring_granted),
                requested_tap_mask: Some(requested_tap_mask),
                tap_mask: None,
                tap_created: Some(true),
                tap_enabled: Some(true),
                run_loop_started: Some(true),
                redacted_event_count: Some(0),
                picker_presented: None,
                picker_selected: None,
                stream_started: None,
                first_complete_frame: None,
                first_frame_monotonic_ns: None,
                resource_live_before_revocation: None,
                resource_failed_after_revocation: None,
            };
        }
    };
    let tap_mask = if keyboard {
        installed_masks.keyboard
    } else {
        installed_masks.pointer
    };
    if tap_mask != requested_tap_mask {
        session.stop();
        let outcome = if keyboard && (request_result == Some(true) || input_monitoring_granted()) {
            MacosTccCanaryOutcome::NeedsProcessRestart
        } else {
            MacosTccCanaryOutcome::Failed
        };
        return MacosTccCanaryCapabilityEvidence {
            capability,
            outcome,
            resulting_api_state: resulting_api_state(capability, outcome).to_owned(),
            typed_error: Some("installed_tap_mask_incomplete".to_owned()),
            tcc_preflight_before: preflight_before,
            tcc_request_result: request_result,
            tcc_preflight_after: keyboard.then(input_monitoring_granted),
            requested_tap_mask: Some(requested_tap_mask),
            tap_mask: Some(tap_mask),
            tap_created: Some(true),
            tap_enabled: Some(true),
            run_loop_started: Some(true),
            redacted_event_count: Some(0),
            picker_presented: None,
            picker_selected: None,
            stream_started: None,
            first_complete_frame: None,
            first_frame_monotonic_ns: None,
            resource_live_before_revocation: None,
            resource_failed_after_revocation: None,
        };
    }
    let deadline = Instant::now() + request.timeout();
    let mut live_before_revocation = false;
    let outcome = loop {
        let count = event_count.load(Ordering::Relaxed);
        live_before_revocation |= count > 0;
        match session.worker_state() {
            MacosWorkerState::PermissionRevoked => break MacosTccCanaryOutcome::Revoked,
            MacosWorkerState::Failed(_) => break MacosTccCanaryOutcome::Failed,
            MacosWorkerState::Running | MacosWorkerState::Degraded(_) => {}
        }
        if request.lifecycle_phase != MacosTccCanaryLifecyclePhase::RevokeWhileLive && count > 0 {
            break MacosTccCanaryOutcome::Passed;
        }
        if Instant::now() >= deadline {
            break MacosTccCanaryOutcome::TimedOut;
        }
        thread::park_timeout(Duration::from_millis(10));
    };
    session.stop();
    let final_count = event_count.load(Ordering::Relaxed);
    MacosTccCanaryCapabilityEvidence {
        capability,
        outcome,
        resulting_api_state: resulting_api_state(capability, outcome).to_owned(),
        typed_error: (outcome == MacosTccCanaryOutcome::Failed)
            .then(|| "input_worker_failed".to_owned()),
        tcc_preflight_before: preflight_before,
        tcc_request_result: request_result,
        tcc_preflight_after: keyboard.then(input_monitoring_granted),
        requested_tap_mask: Some(requested_tap_mask),
        tap_mask: Some(tap_mask),
        tap_created: Some(true),
        tap_enabled: Some(true),
        run_loop_started: Some(true),
        redacted_event_count: Some(final_count),
        picker_presented: None,
        picker_selected: None,
        stream_started: None,
        first_complete_frame: None,
        first_frame_monotonic_ns: None,
        resource_live_before_revocation: (request.lifecycle_phase
            == MacosTccCanaryLifecyclePhase::RevokeWhileLive)
            .then_some(live_before_revocation),
        resource_failed_after_revocation: (request.lifecycle_phase
            == MacosTccCanaryLifecyclePhase::RevokeWhileLive)
            .then_some(outcome == MacosTccCanaryOutcome::Revoked),
    }
}

#[cfg(feature = "screen-capture")]
fn execute_screen_capabilities(
    request: &MacosTccCanaryRequest,
) -> (
    MacosTccCanaryCapabilityEvidence,
    Option<MacosTccCanaryCapabilityEvidence>,
) {
    let deadline = Instant::now() + request.timeout();
    let stream_requested = request
        .capabilities
        .contains(&MacosTccCanaryCapability::Stream);
    let preflight_before = MacosScreenCaptureSession::screen_authorized();
    let stream_request = MacosStreamRequest::new(MacosCaptureCadence::NativeRefresh, true)
        .expect("native refresh is a valid canary cadence");
    let authorization_session =
        MacosScreenCaptureSession::new(stream_request, MacosCaptureSelector::SessionScoped);
    let Ok(authorization_session) = authorization_session else {
        let picker = failed_screen_evidence(
            MacosTccCanaryCapability::Picker,
            preflight_before,
            "capture_session_start_failed",
        );
        let stream = stream_requested.then(|| {
            failed_screen_evidence(
                MacosTccCanaryCapability::Stream,
                preflight_before,
                "capture_session_start_failed",
            )
        });
        return (picker, stream);
    };
    let request_result = request
        .allow_screen_prompt
        .then(|| authorization_session.request_authorization())
        .map(|state| {
            !matches!(
                state,
                MacosProtectedSourceState::PermissionDenied
                    | MacosProtectedSourceState::NeedsUserAction
            )
        });
    let preflight_after_request = MacosScreenCaptureSession::screen_authorized();
    if stream_requested && request_result == Some(true) && preflight_after_request {
        let diagnostic = authorization_session.begin_post_authorization_stream_diagnostic();
        let outcome = diagnostic
            .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
            .and_then(|receiver| {
                receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            });
        match outcome {
            Ok(MacosProtectedSourceState::ReadyIdle) => {}
            Ok(MacosProtectedSourceState::NeedsProcessRestart) => {
                authorization_session.stop();
                return post_authorization_restart_evidence(
                    preflight_before,
                    request_result,
                    preflight_after_request,
                );
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                authorization_session.stop();
                return post_authorization_failure_evidence(
                    preflight_before,
                    request_result,
                    preflight_after_request,
                    MacosTccCanaryOutcome::Failed,
                    "post_authorization_stream_diagnostic_failed",
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                authorization_session.stop();
                return post_authorization_failure_evidence(
                    preflight_before,
                    request_result,
                    preflight_after_request,
                    MacosTccCanaryOutcome::TimedOut,
                    "post_authorization_stream_diagnostic_timed_out",
                );
            }
        }
    }
    authorization_session.stop();
    drop(authorization_session);
    let session =
        MacosScreenCaptureSession::new(stream_request, MacosCaptureSelector::SessionScoped);
    let Ok(session) = session else {
        let picker = failed_screen_evidence(
            MacosTccCanaryCapability::Picker,
            preflight_after_request,
            "capture_session_restart_failed",
        );
        let stream = stream_requested.then(|| {
            failed_screen_evidence(
                MacosTccCanaryCapability::Stream,
                preflight_after_request,
                "capture_session_restart_failed",
            )
        });
        return (picker, stream);
    };
    if stream_requested {
        session.set_capture_active(true);
    }
    let present_result = session.present_picker();
    if present_result.is_err() {
        session.stop();
        let outcome = if !preflight_after_request && request_result != Some(true) {
            MacosTccCanaryOutcome::Denied
        } else {
            MacosTccCanaryOutcome::Failed
        };
        let picker = MacosTccCanaryCapabilityEvidence {
            capability: MacosTccCanaryCapability::Picker,
            outcome,
            resulting_api_state: resulting_api_state(MacosTccCanaryCapability::Picker, outcome)
                .to_owned(),
            typed_error: Some("picker_presentation_failed".to_owned()),
            tcc_preflight_before: Some(preflight_before),
            tcc_request_result: request_result,
            tcc_preflight_after: Some(preflight_after_request),
            requested_tap_mask: None,
            tap_mask: None,
            tap_created: None,
            tap_enabled: None,
            run_loop_started: None,
            redacted_event_count: None,
            picker_presented: Some(false),
            picker_selected: Some(false),
            stream_started: None,
            first_complete_frame: None,
            first_frame_monotonic_ns: None,
            resource_live_before_revocation: None,
            resource_failed_after_revocation: None,
        };
        let stream = stream_requested.then(|| MacosTccCanaryCapabilityEvidence {
            capability: MacosTccCanaryCapability::Stream,
            resulting_api_state: resulting_api_state(MacosTccCanaryCapability::Stream, outcome)
                .to_owned(),
            ..picker.clone()
        });
        return (picker, stream);
    }

    let started = Instant::now();
    let mailbox = session.mailbox();
    let mut selected = false;
    let mut first_frame_monotonic_ns = None;
    let mut live_before_revocation = false;
    let mut revocation_preflight_observed = false;
    let mut resource_failed_after_revocation = false;
    loop {
        selected |= !matches!(session.selection(), MacosCaptureSelection::None);
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(50));
        if stream_requested && let Some(delivery) = mailbox.wait_latest(wait) {
            match delivery {
                Ok(MacosFrameEvent::Frame(_)) => {
                    selected = true;
                    live_before_revocation = true;
                    first_frame_monotonic_ns =
                        Some(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
                    if request.lifecycle_phase != MacosTccCanaryLifecyclePhase::RevokeWhileLive {
                        break;
                    }
                }
                Ok(MacosFrameEvent::Lifecycle(_)) | Ok(MacosFrameEvent::RecoverableError(_)) => {}
                Err(_) if revocation_preflight_observed => {
                    resource_failed_after_revocation = true;
                    break;
                }
                Err(_) => {}
            }
        } else if !stream_requested {
            thread::park_timeout(wait);
        }
        if live_before_revocation && !MacosScreenCaptureSession::screen_authorized() {
            revocation_preflight_observed = true;
            resource_failed_after_revocation |= matches!(
                session.status(),
                MacosProtectedSourceState::PermissionDenied
                    | MacosProtectedSourceState::Revoked
                    | MacosProtectedSourceState::Interrupted
                    | MacosProtectedSourceState::Failed
            );
            if resource_failed_after_revocation {
                break;
            }
        }
        if Instant::now() >= deadline || (!stream_requested && selected) {
            break;
        }
    }
    session.stop();
    let preflight_after = MacosScreenCaptureSession::screen_authorized();
    let picker_outcome = if selected {
        MacosTccCanaryOutcome::Passed
    } else if session.status() == MacosProtectedSourceState::NeedsSelection {
        MacosTccCanaryOutcome::Cancelled
    } else {
        MacosTccCanaryOutcome::TimedOut
    };
    let picker = MacosTccCanaryCapabilityEvidence {
        capability: MacosTccCanaryCapability::Picker,
        outcome: picker_outcome,
        resulting_api_state: resulting_api_state(MacosTccCanaryCapability::Picker, picker_outcome)
            .to_owned(),
        typed_error: (picker_outcome != MacosTccCanaryOutcome::Passed)
            .then(|| "picker_did_not_select".to_owned()),
        tcc_preflight_before: Some(preflight_before),
        tcc_request_result: request_result,
        tcc_preflight_after: Some(preflight_after),
        requested_tap_mask: None,
        tap_mask: None,
        tap_created: None,
        tap_enabled: None,
        run_loop_started: None,
        redacted_event_count: None,
        picker_presented: Some(true),
        picker_selected: Some(selected),
        stream_started: stream_requested.then_some(selected),
        first_complete_frame: None,
        first_frame_monotonic_ns: None,
        resource_live_before_revocation: None,
        resource_failed_after_revocation: None,
    };
    let stream = stream_requested.then(|| {
        let revoked = live_before_revocation
            && revocation_preflight_observed
            && resource_failed_after_revocation;
        let outcome = if revoked {
            MacosTccCanaryOutcome::Revoked
        } else if first_frame_monotonic_ns.is_some() {
            MacosTccCanaryOutcome::Passed
        } else {
            MacosTccCanaryOutcome::TimedOut
        };
        MacosTccCanaryCapabilityEvidence {
            capability: MacosTccCanaryCapability::Stream,
            outcome,
            resulting_api_state: resulting_api_state(MacosTccCanaryCapability::Stream, outcome)
                .to_owned(),
            typed_error: (outcome != MacosTccCanaryOutcome::Passed
                && outcome != MacosTccCanaryOutcome::Revoked)
                .then(|| "first_complete_frame_missing".to_owned()),
            tcc_preflight_before: Some(preflight_before),
            tcc_request_result: request_result,
            tcc_preflight_after: Some(preflight_after),
            requested_tap_mask: None,
            tap_mask: None,
            tap_created: None,
            tap_enabled: None,
            run_loop_started: None,
            redacted_event_count: None,
            picker_presented: Some(true),
            picker_selected: Some(selected),
            stream_started: Some(selected),
            first_complete_frame: Some(first_frame_monotonic_ns.is_some()),
            first_frame_monotonic_ns,
            resource_live_before_revocation: (request.lifecycle_phase
                == MacosTccCanaryLifecyclePhase::RevokeWhileLive)
                .then_some(live_before_revocation),
            resource_failed_after_revocation: (request.lifecycle_phase
                == MacosTccCanaryLifecyclePhase::RevokeWhileLive)
                .then_some(resource_failed_after_revocation),
        }
    });
    (picker, stream)
}

#[cfg(feature = "screen-capture")]
fn post_authorization_restart_evidence(
    preflight_before: bool,
    request_result: Option<bool>,
    preflight_after: bool,
) -> (
    MacosTccCanaryCapabilityEvidence,
    Option<MacosTccCanaryCapabilityEvidence>,
) {
    let picker = post_authorization_evidence(
        MacosTccCanaryCapability::Picker,
        MacosTccCanaryOutcome::Failed,
        preflight_before,
        request_result,
        preflight_after,
        "stream_restart_required_before_picker",
    );
    let stream = post_authorization_evidence(
        MacosTccCanaryCapability::Stream,
        MacosTccCanaryOutcome::NeedsProcessRestart,
        preflight_before,
        request_result,
        preflight_after,
        "post_authorization_stream_requires_restart",
    );
    (picker, Some(stream))
}

#[cfg(feature = "screen-capture")]
fn post_authorization_failure_evidence(
    preflight_before: bool,
    request_result: Option<bool>,
    preflight_after: bool,
    outcome: MacosTccCanaryOutcome,
    typed_error: &str,
) -> (
    MacosTccCanaryCapabilityEvidence,
    Option<MacosTccCanaryCapabilityEvidence>,
) {
    let picker = post_authorization_evidence(
        MacosTccCanaryCapability::Picker,
        MacosTccCanaryOutcome::Failed,
        preflight_before,
        request_result,
        preflight_after,
        typed_error,
    );
    let stream = post_authorization_evidence(
        MacosTccCanaryCapability::Stream,
        outcome,
        preflight_before,
        request_result,
        preflight_after,
        typed_error,
    );
    (picker, Some(stream))
}

#[cfg(feature = "screen-capture")]
fn post_authorization_evidence(
    capability: MacosTccCanaryCapability,
    outcome: MacosTccCanaryOutcome,
    preflight_before: bool,
    request_result: Option<bool>,
    preflight_after: bool,
    typed_error: &str,
) -> MacosTccCanaryCapabilityEvidence {
    MacosTccCanaryCapabilityEvidence {
        capability,
        outcome,
        resulting_api_state: resulting_api_state(capability, outcome).to_owned(),
        typed_error: Some(typed_error.to_owned()),
        tcc_preflight_before: Some(preflight_before),
        tcc_request_result: request_result,
        tcc_preflight_after: Some(preflight_after),
        requested_tap_mask: None,
        tap_mask: None,
        tap_created: None,
        tap_enabled: None,
        run_loop_started: None,
        redacted_event_count: None,
        picker_presented: Some(false),
        picker_selected: Some(false),
        stream_started: (capability == MacosTccCanaryCapability::Stream).then_some(false),
        first_complete_frame: (capability == MacosTccCanaryCapability::Stream).then_some(false),
        first_frame_monotonic_ns: None,
        resource_live_before_revocation: None,
        resource_failed_after_revocation: None,
    }
}

#[cfg(feature = "screen-capture")]
fn failed_screen_evidence(
    capability: MacosTccCanaryCapability,
    preflight: bool,
    typed_error: &str,
) -> MacosTccCanaryCapabilityEvidence {
    MacosTccCanaryCapabilityEvidence {
        capability,
        outcome: if preflight {
            MacosTccCanaryOutcome::Failed
        } else {
            MacosTccCanaryOutcome::Denied
        },
        resulting_api_state: resulting_api_state(
            capability,
            if preflight {
                MacosTccCanaryOutcome::Failed
            } else {
                MacosTccCanaryOutcome::Denied
            },
        )
        .to_owned(),
        typed_error: Some(typed_error.to_owned()),
        tcc_preflight_before: Some(preflight),
        tcc_request_result: None,
        tcc_preflight_after: Some(MacosScreenCaptureSession::screen_authorized()),
        requested_tap_mask: None,
        tap_mask: None,
        tap_created: None,
        tap_enabled: None,
        run_loop_started: None,
        redacted_event_count: None,
        picker_presented: Some(false),
        picker_selected: Some(false),
        stream_started: (capability == MacosTccCanaryCapability::Stream).then_some(false),
        first_complete_frame: (capability == MacosTccCanaryCapability::Stream).then_some(false),
        first_frame_monotonic_ns: None,
        resource_live_before_revocation: None,
        resource_failed_after_revocation: None,
    }
}

#[cfg(feature = "screen-capture")]
fn input_error_code(error: &MacosInputError) -> &'static str {
    match error {
        MacosInputError::UnsupportedPlatform => "unsupported_platform",
        MacosInputError::NothingToCapture => "nothing_to_capture",
        MacosInputError::PermissionDenied => "permission_denied",
        MacosInputError::InvalidVirtualDesktop => "invalid_virtual_desktop",
        MacosInputError::DisplayTopology(_) => "display_topology_failed",
        MacosInputError::NoActiveDisplays => "no_active_displays",
        MacosInputError::WorkerSpawn(_) => "worker_spawn_failed",
        MacosInputError::WorkerReadyTimeout => "worker_ready_timeout",
        MacosInputError::TapCreation(_) => "tap_creation_failed",
        MacosInputError::RunLoopSource(_) => "run_loop_source_failed",
        MacosInputError::TapInspection(_) => "tap_inspection_failed",
        MacosInputError::AuditToken(_) => "audit_token_failed",
    }
}

#[cfg(feature = "screen-capture")]
fn inspect_signing(
    executable: &Path,
    pid: u32,
    process_fingerprint: &str,
    audit_token: Option<&str>,
) -> Result<MacosTccCanarySigningEvidence> {
    let static_details = bounded_command(
        "/usr/bin/codesign",
        &["-d", "--verbose=4", path_arg(executable)?],
    )?;
    let dynamic_target = format!("+{pid}");
    let dynamic_details =
        bounded_command("/usr/bin/codesign", &["-d", "--verbose=4", &dynamic_target])?;
    let requirement = bounded_command("/usr/bin/codesign", &["-d", "-r-", path_arg(executable)?])?;
    let static_verification = bounded_command(
        "/usr/bin/codesign",
        &["--verify", "--strict", "--verbose=4", path_arg(executable)?],
    )?;
    let dynamic_verification = bounded_command(
        "/usr/bin/codesign",
        &dynamic_codesign_verification_args(&dynamic_target),
    )?;
    let entitlements = bounded_command(
        "/usr/bin/codesign",
        &["-d", "--entitlements", ":-", path_arg(executable)?],
    )?;
    let spctl = bounded_command(
        "/usr/sbin/spctl",
        &[
            "--assess",
            "--type",
            "execute",
            "--verbose=4",
            path_arg(executable)?,
        ],
    )?;
    let static_detail_text = bounded_utf8(&static_details.stderr, "static codesign details")?;
    let dynamic_detail_text = bounded_utf8(&dynamic_details.stderr, "dynamic codesign details")?;
    let static_cdhash = details_value(static_detail_text, "CDHash=")?.to_ascii_lowercase();
    let dynamic_cdhash = details_value(dynamic_detail_text, "CDHash=")?.to_ascii_lowercase();
    let requirement_text = bounded_utf8(&requirement.stdout, "codesign requirement")?;
    let designated_requirement = requirement_text
        .lines()
        .find_map(|line| {
            line.strip_prefix("designated => ")
                .or_else(|| line.strip_prefix("# designated => "))
        })
        .context("codesign omitted designated requirement")?
        .to_owned();
    let requirement_digest = hex_digest(designated_requirement.as_bytes());
    let entitlement_text = bounded_utf8(&entitlements.stdout, "codesign entitlements")?;
    let mut evidence = MacosTccCanarySigningEvidence {
        bundle_identifier: details_value(dynamic_detail_text, "Identifier=")?.to_owned(),
        team_identifier: details_value(dynamic_detail_text, "TeamIdentifier=")?.to_owned(),
        designated_requirement,
        designated_requirement_sha256: requirement_digest,
        cdhash: dynamic_cdhash.clone(),
        process_bound_pid: pid,
        process_bound_fingerprint: process_fingerprint.to_owned(),
        process_bound_valid: dynamic_details.success
            && dynamic_verification.success
            && static_cdhash == dynamic_cdhash,
        audit_token_bound_valid: false,
        authorities: dynamic_detail_text
            .lines()
            .filter_map(|line| line.strip_prefix("Authority="))
            .map(str::to_owned)
            .collect(),
        entitlement_keys: plist_true_keys(entitlement_text)?,
        codesign_strict_valid: static_details.success
            && requirement.success
            && static_verification.success
            && entitlements.success,
        hardened_runtime: dynamic_detail_text
            .lines()
            .find(|line| line.starts_with("flags="))
            .is_some_and(|line| line.contains("runtime")),
        secure_timestamp: dynamic_detail_text
            .lines()
            .any(|line| line.starts_with("Timestamp=")),
        spctl_accepted: spctl.success,
    };
    evidence.audit_token_bound_valid = audit_token
        .is_some_and(|token| live_signing_identity_is_valid(token, executable, &evidence));
    Ok(evidence)
}

#[cfg(feature = "screen-capture")]
fn plist_true_keys(xml: &str) -> Result<Vec<String>> {
    let mut keys = Vec::new();
    let mut remaining = xml;
    while let Some(key_start) = remaining.find("<key>") {
        remaining = &remaining[key_start + "<key>".len()..];
        let key_end = remaining
            .find("</key>")
            .context("entitlement key is not terminated")?;
        let key = &remaining[..key_end];
        remaining = &remaining[key_end + "</key>".len()..];
        let value = remaining.trim_start();
        anyhow::ensure!(
            value.starts_with("<true/>") || value.starts_with("<true />"),
            "entitlement {key} is not true"
        );
        keys.push(key.to_owned());
    }
    anyhow::ensure!(
        !keys.is_empty(),
        "codesign returned no Boolean entitlements"
    );
    keys.sort();
    Ok(keys)
}

#[cfg(feature = "screen-capture")]
fn inspect_launcher(
    topology: MacosDaemonOwner,
    daemon_signing: &MacosTccCanarySigningEvidence,
) -> Result<MacosTccCanaryLauncherEvidence> {
    let pid = std::process::id();
    let mut system = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
    );
    system.refresh_processes(ProcessesToUpdate::All, true);
    let current = system
        .process(Pid::from_u32(pid))
        .context("current process is missing from the process table")?;
    let parent_pid = current.parent().map(Pid::as_u32);
    let parent_executable_path = current
        .parent()
        .and_then(|parent| system.process(parent))
        .and_then(|parent| parent.exe())
        .map(Path::to_path_buf);
    let mut parent_signing = None;
    let (actual_launcher, expected_label, launchctl_pid_matches, verified) = match topology {
        MacosDaemonOwner::AppSidecar => {
            parent_signing =
                parent_pid
                    .zip(parent_executable_path.as_deref())
                    .and_then(|(parent_pid, path)| {
                        process_fingerprint(parent_pid)
                            .and_then(|fingerprint| {
                                inspect_signing(path, parent_pid, &fingerprint, None)
                            })
                            .ok()
                    });
            let parent_verified = parent_signing.as_ref().is_some_and(|signing| {
                signing.bundle_identifier == "tech.hyperbliss.hypercolor"
                    && signing.team_identifier == daemon_signing.team_identifier
                    && signing.codesign_strict_valid
                    && signing.hardened_runtime
                    && signing.secure_timestamp
                    && signing.spctl_accepted
            });
            (
                "packaged_app_supervisor".to_owned(),
                None,
                None,
                parent_verified,
            )
        }
        MacosDaemonOwner::DirectLaunchd => {
            let label = "tech.hyperbliss.hypercolor";
            let matches = launchctl_service_pid(label)? == Some(pid);
            (
                "direct_launchd".to_owned(),
                Some(label.to_owned()),
                Some(matches),
                matches,
            )
        }
        MacosDaemonOwner::Homebrew => {
            let label = "homebrew.mxcl.hypercolor";
            let matches = launchctl_service_pid(label)? == Some(pid);
            (
                "homebrew_services".to_owned(),
                Some(label.to_owned()),
                Some(matches),
                matches,
            )
        }
        MacosDaemonOwner::Standalone => {
            let direct = launchctl_service_pid("tech.hyperbliss.hypercolor")? == Some(pid);
            let homebrew = launchctl_service_pid("homebrew.mxcl.hypercolor")? == Some(pid);
            let terminal_parent = parent_executable_path
                .as_deref()
                .is_some_and(terminal_parent_is_valid);
            (
                "terminal_parent".to_owned(),
                None,
                None,
                terminal_parent && !direct && !homebrew,
            )
        }
    };
    Ok(MacosTccCanaryLauncherEvidence {
        actual_launcher,
        expected_label,
        parent_pid,
        parent_executable_path,
        parent_signing,
        launchctl_pid_matches,
        verified,
    })
}

#[cfg(feature = "screen-capture")]
fn launchctl_service_pid(label: &str) -> Result<Option<u32>> {
    let uid = bounded_command_text("/usr/bin/id", &["-u"])?;
    let target = format!("gui/{uid}/{label}");
    let output = bounded_command("/bin/launchctl", &["print", &target])?;
    if !output.success {
        return Ok(None);
    }
    let output = bounded_utf8(&output.stdout, "launchctl output")?;
    output
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("pid = "))
        .map(str::parse)
        .transpose()
        .context("launchctl returned an invalid service pid")
}

#[cfg(feature = "screen-capture")]
fn host_architecture() -> Result<String> {
    if sysctl_flag("hw.optional.arm64")? || sysctl_flag("sysctl.proc_translated")? {
        Ok("apple_silicon".to_owned())
    } else {
        Ok("intel".to_owned())
    }
}

#[cfg(feature = "screen-capture")]
fn process_fingerprint(pid: u32) -> Result<String> {
    let pid = pid.to_string();
    let identity =
        bounded_command_text("/bin/ps", &["-p", &pid, "-o", "lstart=", "-o", "command="])?;
    let identity = identity.split_whitespace().collect::<Vec<_>>().join(" ");
    anyhow::ensure!(!identity.is_empty(), "process identity is empty");
    Ok(hex_digest(identity.as_bytes()))
}

#[cfg(feature = "screen-capture")]
fn sysctl_flag(name: &str) -> Result<bool> {
    let output = bounded_command("/usr/sbin/sysctl", &["-in", name])?;
    if !output.success {
        return Ok(false);
    }
    match bounded_utf8(&output.stdout, "sysctl output")?.trim() {
        "" | "0" => Ok(false),
        "1" => Ok(true),
        value => anyhow::bail!("sysctl {name} returned unexpected value {value:?}"),
    }
}

#[cfg(feature = "screen-capture")]
struct BoundedCommandOutput {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[cfg(feature = "screen-capture")]
fn dynamic_codesign_verification_args(dynamic_target: &str) -> [&str; 2] {
    ["--verify", dynamic_target]
}

#[cfg(feature = "screen-capture")]
fn bounded_command(program: &str, args: &[&str]) -> Result<BoundedCommandOutput> {
    const MAX_OUTPUT: usize = 64 * 1024;
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    anyhow::ensure!(
        output.stdout.len() <= MAX_OUTPUT && output.stderr.len() <= MAX_OUTPUT,
        "{program} output exceeds 64 KiB"
    );
    Ok(BoundedCommandOutput {
        success: output.status.success(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

#[cfg(feature = "screen-capture")]
fn bounded_command_text(program: &str, args: &[&str]) -> Result<String> {
    let output = bounded_command(program, args)?;
    anyhow::ensure!(output.success, "{program} exited unsuccessfully");
    Ok(bounded_utf8(&output.stdout, "command output")?
        .trim()
        .to_owned())
}

#[cfg(feature = "screen-capture")]
fn bounded_utf8<'a>(bytes: &'a [u8], label: &str) -> Result<&'a str> {
    std::str::from_utf8(bytes).with_context(|| format!("{label} is not UTF-8"))
}

#[cfg(feature = "screen-capture")]
fn details_value<'a>(details: &'a str, prefix: &str) -> Result<&'a str> {
    details
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .filter(|value| !value.is_empty())
        .with_context(|| format!("codesign omitted {prefix}"))
}

#[cfg(feature = "screen-capture")]
fn path_arg(path: &Path) -> Result<&str> {
    path.to_str().context("process path is not valid UTF-8")
}

#[cfg(feature = "screen-capture")]
fn hex_digest(bytes: &[u8]) -> String {
    hex_bytes(&Sha256::digest(bytes))
}

fn hex_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(
        String::with_capacity(bytes.len().saturating_mul(2)),
        |mut digest, byte| {
            write!(&mut digest, "{byte:02x}").expect("writing into a String cannot fail");
            digest
        },
    )
}

#[cfg(feature = "screen-capture")]
fn unix_time_ms() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock predates the Unix epoch")?
        .as_millis()
        .try_into()
        .context("system time exceeds u64 milliseconds")
}

fn validate_receipt_set(
    receipts: &[MacosTccCanaryReceipt],
    witnesses: &[MacosTccCanaryWitness],
) -> MacosTccCanaryValidation {
    let mut missing = BTreeSet::new();
    let mut by_row = BTreeMap::new();
    let mut witness_by_id = BTreeMap::new();
    let mut structure_valid = !receipts.is_empty();
    let run_id = receipts.first().map(|receipt| receipt.run_id.as_str());
    for receipt in receipts {
        structure_valid &= receipt.schema_version == MACOS_TCC_CANARY_SCHEMA_VERSION;
        structure_valid &= validate_identifier(&receipt.run_id, "run_id").is_ok();
        structure_valid &= validate_identifier(&receipt.row_id, "row_id").is_ok();
        structure_valid &= validate_identifier(&receipt.scenario_id, "scenario_id").is_ok();
        structure_valid &= validate_identifier(
            &receipt.system_settings_identity_witness_id,
            "system_settings_identity_witness_id",
        )
        .is_ok();
        structure_valid &= receipt
            .predecessor_row_id
            .as_deref()
            .is_none_or(|value| validate_identifier(value, "predecessor_row_id").is_ok());
        structure_valid &= receipt
            .process_replacement_witness_id
            .as_deref()
            .is_none_or(|value| {
                validate_identifier(value, "process_replacement_witness_id").is_ok()
            });
        structure_valid &= receipt
            .lifecycle_action_witness_id
            .as_deref()
            .is_none_or(|value| validate_identifier(value, "lifecycle_action_witness_id").is_ok());
        structure_valid &= receipt
            .login_arbitration_witness_id
            .as_deref()
            .is_none_or(|value| validate_identifier(value, "login_arbitration_witness_id").is_ok());
        structure_valid &= receipt
            .fresh_tcc_reset_witness_id
            .as_deref()
            .is_none_or(|value| validate_identifier(value, "fresh_tcc_reset_witness_id").is_ok());
        structure_valid &= receipt.installation_scenario.permits(receipt.topology);
        structure_valid &=
            receipt.lifecycle_phase.needs_predecessor() == receipt.predecessor_row_id.is_some();
        structure_valid &= receipt.lifecycle_phase.replaces_process()
            == receipt.process_replacement_witness_id.is_some();
        structure_valid &= receipt.lifecycle_phase.needs_lifecycle_action_witness()
            == receipt.lifecycle_action_witness_id.is_some();
        structure_valid &= receipt.installation_scenario.needs_repeated_login_proof()
            == receipt.login_arbitration_witness_id.is_some();
        structure_valid &= receipt.login_iteration > 0;
        structure_valid &= receipt.acceptance_claim == "evidence_only";
        structure_valid &= Some(receipt.run_id.as_str()) == run_id;
        structure_valid &= receipt.operation_finished_unix_ms >= receipt.process_started_unix_ms;
        structure_valid &= validate_observed_text(&receipt.expected_prompt_text, "prompt").is_ok();
        structure_valid &= validate_observed_text(
            &receipt.expected_system_settings_entry,
            "system settings entry",
        )
        .is_ok();
        structure_valid &= is_sha256(&receipt.process_fingerprint);
        structure_valid &= architecture_evidence_is_coherent(receipt);
        structure_valid &= !receipt.capabilities.is_empty();
        structure_valid &= scored_capability_shape_is_valid(
            receipt.scored_capability,
            &receipt
                .capabilities
                .iter()
                .map(|evidence| evidence.capability)
                .collect(),
        );
        structure_valid &= by_row.insert(receipt.row_id.as_str(), receipt).is_none();
    }
    for witness in witnesses {
        structure_valid &= validate_witness_structure(witness);
        structure_valid &= by_row
            .get(witness.row_id.as_str())
            .is_some_and(|receipt| receipt.run_id == witness.run_id);
        structure_valid &= witness_by_id
            .insert(witness.witness_id.as_str(), witness)
            .is_none();
    }
    if receipts.is_empty() {
        missing.insert("no_receipts".to_owned());
    }
    if !structure_valid {
        missing.insert("receipt_structure".to_owned());
    }

    let identity_consistent = structure_valid
        && receipts
            .iter()
            .all(|receipt| receipt_identity_valid(receipt, &witness_by_id))
        && matrix_identity_is_stable(receipts);
    if !identity_consistent {
        missing.insert("signed_launcher_identity".to_owned());
    }

    for receipt in receipts {
        validate_capability_evidence(receipt, &mut missing);
        validate_lifecycle_link(receipt, &by_row, &witness_by_id, &mut missing);
        validate_login_arbitration(receipt, &witness_by_id, &mut missing);
    }
    for topology in [
        MacosDaemonOwner::AppSidecar,
        MacosDaemonOwner::DirectLaunchd,
        MacosDaemonOwner::Homebrew,
        MacosDaemonOwner::Standalone,
    ] {
        for capability in [
            MacosTccCanaryCapability::Keyboard,
            MacosTccCanaryCapability::Pointer,
            MacosTccCanaryCapability::Picker,
            MacosTccCanaryCapability::Stream,
        ] {
            for (architecture, os_family) in platform_cells() {
                for phase in capability_phases(capability)
                    .iter()
                    .chain(topology_phases(topology))
                    .copied()
                {
                    if !receipts.iter().any(|receipt| {
                        receipt.topology == topology
                            && receipt.scored_capability == capability
                            && receipt.lifecycle_phase == phase
                            && platform_cell_matches(receipt, architecture)
                            && macos_os_family(&receipt.os_version) == Some(os_family)
                            && receipt.capabilities.iter().any(|evidence| {
                                evidence.capability == capability
                                    && evidence.outcome == expected_outcome(phase)
                            })
                    }) {
                        missing.insert(
                            format!(
                                "{topology:?}_{capability:?}_{architecture}_{os_family}_{phase:?}"
                            )
                            .to_ascii_lowercase(),
                        );
                    }
                }
                if !receipts.iter().any(|receipt| {
                    receipt.topology == topology
                        && receipt.scored_capability == capability
                        && platform_cell_matches(receipt, architecture)
                        && macos_os_family(&receipt.os_version) == Some(os_family)
                        && receipt
                            .fresh_tcc_reset_witness_id
                            .as_deref()
                            .and_then(|witness_id| {
                                matching_witness(
                                    receipt,
                                    witness_id,
                                    MacosTccCanaryWitnessKind::FreshTccReset,
                                    &witness_by_id,
                                )
                            })
                            .is_some_and(|witness| {
                                witness.fresh_tcc_database_observed == Some(true)
                            })
                        && receipt.capabilities.iter().any(|evidence| {
                            evidence.capability == capability
                                && evidence.outcome == MacosTccCanaryOutcome::Passed
                        })
                }) {
                    missing.insert(
                        format!(
                            "{topology:?}_{capability:?}_{architecture}_{os_family}_fresh_database"
                        )
                        .to_ascii_lowercase(),
                    );
                }
            }
        }
    }
    for scenario in [
        MacosTccCanaryInstallationScenario::AppOnly,
        MacosTccCanaryInstallationScenario::DirectLaunchdOnly,
        MacosTccCanaryInstallationScenario::HomebrewOnly,
        MacosTccCanaryInstallationScenario::StandaloneOnly,
        MacosTccCanaryInstallationScenario::AppDirectAppEnabledFirst,
        MacosTccCanaryInstallationScenario::AppDirectDirectEnabledFirst,
        MacosTccCanaryInstallationScenario::AppHomebrew,
        MacosTccCanaryInstallationScenario::DirectHomebrew,
        MacosTccCanaryInstallationScenario::AppDirectHomebrew,
    ] {
        let scenario_receipts = receipts
            .iter()
            .filter(|receipt| receipt.installation_scenario == scenario)
            .collect::<Vec<_>>();
        if scenario_receipts.is_empty() {
            missing.insert(format!("installation_{scenario:?}").to_ascii_lowercase());
            continue;
        }
        if scenario.needs_repeated_login_proof() {
            for (architecture, os_family) in platform_cells() {
                let mut scenario_groups: BTreeMap<
                    &str,
                    (MacosDaemonOwner, BTreeMap<u32, &str>, bool),
                > = BTreeMap::new();
                for receipt in scenario_receipts.iter().copied().filter(|receipt| {
                    platform_cell_matches(receipt, architecture)
                        && macos_os_family(&receipt.os_version) == Some(os_family)
                }) {
                    let Some(witness) = login_arbitration_witness(receipt, &witness_by_id)
                        .filter(|witness| login_arbitration_witness_is_valid(receipt, witness))
                    else {
                        continue;
                    };
                    let group = scenario_groups
                        .entry(receipt.scenario_id.as_str())
                        .or_insert((receipt.topology, BTreeMap::new(), true));
                    group.2 &= group.0 == receipt.topology;
                    let Some(session_id) = witness.login_session_id.as_deref() else {
                        continue;
                    };
                    group.2 &= group
                        .1
                        .insert(receipt.login_iteration, session_id)
                        .is_none();
                }
                if !scenario_groups
                    .values()
                    .any(|(_, sessions, topology_stable)| {
                        *topology_stable
                            && sessions.len() >= 2
                            && sessions.values().copied().collect::<BTreeSet<_>>().len() >= 2
                    })
                {
                    missing.insert(
                        format!(
                            "installation_{scenario:?}_{architecture}_{os_family}_repeated_login"
                        )
                        .to_ascii_lowercase(),
                    );
                }
            }
        }
    }

    let capability_qualifications = [
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryCapability::Pointer,
        MacosTccCanaryCapability::Picker,
        MacosTccCanaryCapability::Stream,
    ]
    .into_iter()
    .map(|capability| {
        let qualified_topologies = [
            MacosDaemonOwner::AppSidecar,
            MacosDaemonOwner::DirectLaunchd,
            MacosDaemonOwner::Homebrew,
            MacosDaemonOwner::Standalone,
        ]
        .into_iter()
        .filter(|topology| {
            topology_capability_qualifies(receipts, &witness_by_id, *topology, capability)
        })
        .collect::<Vec<_>>();
        let preferred_topology = qualified_topologies
            .contains(&MacosDaemonOwner::AppSidecar)
            .then_some(MacosDaemonOwner::AppSidecar);
        MacosTccCanaryCapabilityQualification {
            capability,
            preferred_topology,
            qualified_topologies,
            app_broker_required: preferred_topology.is_none(),
        }
    })
    .collect::<Vec<_>>();
    let preferred_topology_eligible = structure_valid
        && identity_consistent
        && missing.is_empty()
        && capability_qualifications
            .iter()
            .all(|qualification| qualification.preferred_topology.is_some());
    MacosTccCanaryValidation {
        schema_version: MACOS_TCC_CANARY_SCHEMA_VERSION,
        receipt_structure_valid: structure_valid,
        identity_consistent,
        preferred_topology_eligible,
        physical_acceptance_claimed: false,
        receipt_count: receipts.len(),
        capability_qualifications,
        missing_requirements: missing.into_iter().collect(),
    }
}

fn launcher_identity_valid(receipt: &MacosTccCanaryReceipt) -> bool {
    if !receipt.launcher.verified {
        return false;
    }
    match receipt.topology {
        MacosDaemonOwner::AppSidecar => {
            receipt.launcher.actual_launcher == "packaged_app_supervisor"
                && receipt.launcher.expected_label.is_none()
                && receipt
                    .launcher
                    .parent_signing
                    .as_ref()
                    .is_some_and(|signing| {
                        signing.bundle_identifier == "tech.hyperbliss.hypercolor"
                            && signing.team_identifier == receipt.signing.team_identifier
                            && receipt.launcher.parent_pid == Some(signing.process_bound_pid)
                            && signing_identity_is_valid(signing)
                    })
        }
        MacosDaemonOwner::DirectLaunchd => {
            receipt.launcher.actual_launcher == "direct_launchd"
                && receipt.launcher.expected_label.as_deref() == Some("tech.hyperbliss.hypercolor")
                && receipt.launcher.launchctl_pid_matches == Some(true)
        }
        MacosDaemonOwner::Homebrew => {
            receipt.launcher.actual_launcher == "homebrew_services"
                && receipt.launcher.expected_label.as_deref() == Some("homebrew.mxcl.hypercolor")
                && receipt.launcher.launchctl_pid_matches == Some(true)
        }
        MacosDaemonOwner::Standalone => {
            receipt.launcher.actual_launcher == "terminal_parent"
                && receipt.launcher.expected_label.is_none()
                && receipt.launcher.parent_pid.is_some()
                && receipt
                    .launcher
                    .parent_executable_path
                    .as_deref()
                    .is_some_and(terminal_parent_is_valid)
        }
    }
}

fn receipt_identity_valid(
    receipt: &MacosTccCanaryReceipt,
    witnesses: &BTreeMap<&str, &MacosTccCanaryWitness>,
) -> bool {
    let expected_bundle_identifier = match receipt.topology {
        MacosDaemonOwner::AppSidecar => "tech.hyperbliss.hypercolor.sidecar",
        MacosDaemonOwner::DirectLaunchd
        | MacosDaemonOwner::Homebrew
        | MacosDaemonOwner::Standalone => "tech.hyperbliss.hypercolor.daemon",
    };
    launcher_identity_valid(receipt)
        && receipt.signing.bundle_identifier == expected_bundle_identifier
        && signing_identity_is_valid(&receipt.signing)
        && receipt.signing.process_bound_pid == receipt.pid
        && receipt.signing.process_bound_fingerprint == receipt.process_fingerprint
        && audit_token_identity(&receipt.audit_token_identity)
            .is_some_and(|identity| identity.pid == receipt.pid)
        && receipt.executable_path.is_absolute()
        && matching_witness(
            receipt,
            &receipt.system_settings_identity_witness_id,
            MacosTccCanaryWitnessKind::SystemSettingsIdentity,
            witnesses,
        )
        .is_some_and(|witness| {
            witness.observed_unix_ms >= receipt.process_started_unix_ms
                && witness.prompt_text.as_deref() == Some(receipt.expected_prompt_text.as_str())
                && witness.system_settings_entry.as_deref()
                    == Some(receipt.expected_system_settings_entry.as_str())
                && witness.observed_pid == Some(receipt.pid)
                && witness.observed_audit_token_identity.as_deref()
                    == Some(receipt.audit_token_identity.as_str())
                && witness.observed_signing_audit_token_identity.as_deref()
                    == Some(receipt.audit_token_identity.as_str())
                && witness.observed_cdhash.as_deref() == Some(receipt.signing.cdhash.as_str())
                && witness.observed_designated_requirement_sha256.as_deref()
                    == Some(receipt.signing.designated_requirement_sha256.as_str())
                && witness.observed_process_fingerprint.as_deref()
                    == Some(receipt.process_fingerprint.as_str())
                && app_parent_witness_is_valid(receipt, witness)
        })
}

fn app_parent_witness_is_valid(
    receipt: &MacosTccCanaryReceipt,
    witness: &MacosTccCanaryWitness,
) -> bool {
    if receipt.topology != MacosDaemonOwner::AppSidecar {
        return witness.parent_pid.is_none()
            && witness.parent_audit_token_identity.is_none()
            && witness.parent_signing_audit_token_identity.is_none()
            && witness.parent_cdhash.is_none()
            && witness.parent_designated_requirement_sha256.is_none()
            && witness.parent_process_fingerprint.is_none();
    }
    let Some(parent_signing) = receipt.launcher.parent_signing.as_ref() else {
        return false;
    };
    witness.parent_pid == receipt.launcher.parent_pid
        && witness.parent_pid == Some(parent_signing.process_bound_pid)
        && witness
            .parent_audit_token_identity
            .as_deref()
            .and_then(audit_token_identity)
            .is_some_and(|identity| Some(identity.pid) == witness.parent_pid)
        && witness.parent_signing_audit_token_identity == witness.parent_audit_token_identity
        && witness.parent_cdhash.as_deref() == Some(parent_signing.cdhash.as_str())
        && witness.parent_designated_requirement_sha256.as_deref()
            == Some(parent_signing.designated_requirement_sha256.as_str())
        && witness.parent_process_fingerprint.as_deref()
            == Some(parent_signing.process_bound_fingerprint.as_str())
}

fn signing_identity_is_valid(signing: &MacosTccCanarySigningEvidence) -> bool {
    signing.process_bound_valid
        && signing.audit_token_bound_valid
        && signing.process_bound_pid > 0
        && is_sha256(&signing.process_bound_fingerprint)
        && signing.codesign_strict_valid
        && signing.hardened_runtime
        && signing.secure_timestamp
        && signing.spctl_accepted
        && !signing.bundle_identifier.is_empty()
        && !signing.team_identifier.is_empty()
        && signing.authorities.first().is_some_and(|authority| {
            authority.starts_with("Developer ID Application:")
                && authority.contains(&format!("({})", signing.team_identifier))
        })
        && signing.entitlement_keys.len() == REQUIRED_ENTITLEMENTS.len()
        && signing
            .entitlement_keys
            .iter()
            .map(String::as_str)
            .eq(REQUIRED_ENTITLEMENTS)
        && !signing.designated_requirement.is_empty()
        && signing
            .designated_requirement
            .contains(&signing.bundle_identifier)
        && signing.designated_requirement_sha256
            == hex_digest(signing.designated_requirement.as_bytes())
        && is_hex_with_length(&signing.cdhash, &[40, 64])
}

fn matrix_identity_is_stable(receipts: &[MacosTccCanaryReceipt]) -> bool {
    let mut identities = BTreeMap::new();
    let team_identifier = receipts
        .first()
        .map(|receipt| receipt.signing.team_identifier.as_str());
    receipts.iter().all(|receipt| {
        let identity = (
            receipt.signing.bundle_identifier.as_str(),
            receipt.signing.team_identifier.as_str(),
            receipt.signing.designated_requirement.as_str(),
            receipt.signing.authorities.as_slice(),
            receipt.signing.entitlement_keys.as_slice(),
            receipt.launcher.parent_signing.as_ref().map(|signing| {
                (
                    signing.bundle_identifier.as_str(),
                    signing.team_identifier.as_str(),
                    signing.designated_requirement.as_str(),
                    signing.authorities.as_slice(),
                    signing.entitlement_keys.as_slice(),
                )
            }),
        );
        Some(receipt.signing.team_identifier.as_str()) == team_identifier
            && identities
                .entry(topology_key(receipt.topology))
                .or_insert(identity)
                == &identity
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AuditTokenIdentity {
    pid: u32,
    pidversion: u32,
}

fn audit_token_identity(identity: &str) -> Option<AuditTokenIdentity> {
    let words = identity.split(':').collect::<Vec<_>>();
    if words.len() != 8
        || words
            .iter()
            .any(|word| word.len() != 8 || !word.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return None;
    }
    Some(AuditTokenIdentity {
        pid: u32::from_str_radix(words[5], 16).ok()?,
        pidversion: u32::from_str_radix(words[7], 16).ok()?,
    })
}

#[cfg(feature = "screen-capture")]
fn audit_token_bytes(identity: &str) -> Option<[u8; 32]> {
    let words = identity.split(':').collect::<Vec<_>>();
    if words.len() != 8 {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (index, word) in words.into_iter().enumerate() {
        let parsed = u32::from_str_radix(word, 16).ok()?;
        bytes[index * 4..index * 4 + 4].copy_from_slice(&parsed.to_ne_bytes());
    }
    Some(bytes)
}

const fn topology_key(topology: MacosDaemonOwner) -> u8 {
    match topology {
        MacosDaemonOwner::AppSidecar => 0,
        MacosDaemonOwner::DirectLaunchd => 1,
        MacosDaemonOwner::Homebrew => 2,
        MacosDaemonOwner::Standalone => 3,
    }
}

fn terminal_parent_is_valid(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                "bash" | "dash" | "fish" | "nu" | "sh" | "tcsh" | "zsh"
            )
        })
}

fn validate_capability_evidence(receipt: &MacosTccCanaryReceipt, missing: &mut BTreeSet<String>) {
    let capabilities = receipt
        .capabilities
        .iter()
        .map(|evidence| evidence.capability)
        .collect::<BTreeSet<_>>();
    if capabilities.len() != receipt.capabilities.len() {
        missing.insert(format!("{}_duplicate_capability", receipt.row_id));
    }
    let keyboard_mask = receipt
        .capabilities
        .iter()
        .find(|evidence| evidence.capability == MacosTccCanaryCapability::Keyboard)
        .and_then(|evidence| evidence.tap_mask);
    let pointer_mask = receipt
        .capabilities
        .iter()
        .find(|evidence| evidence.capability == MacosTccCanaryCapability::Pointer)
        .and_then(|evidence| evidence.tap_mask);
    if keyboard_mask
        .zip(pointer_mask)
        .is_some_and(|(keyboard, pointer)| keyboard & pointer != 0)
    {
        missing.insert(format!("{}_input_masks_overlap", receipt.row_id));
    }
    let scored = receipt
        .capabilities
        .iter()
        .find(|evidence| evidence.capability == receipt.scored_capability);
    if scored.is_none() {
        missing.insert(format!("{}_scored_capability", receipt.row_id));
    }
    if let Some(scored) = scored {
        let tcc_protected = receipt.scored_capability != MacosTccCanaryCapability::Pointer;
        match receipt.lifecycle_phase {
            MacosTccCanaryLifecyclePhase::Deny => {
                if scored.outcome == MacosTccCanaryOutcome::Denied
                    && tcc_protected
                    && scored.tcc_preflight_after != Some(false)
                {
                    missing.insert(format!("{}_denial_evidence", receipt.row_id));
                }
            }
            MacosTccCanaryLifecyclePhase::LaterGrant => {
                if scored.outcome == MacosTccCanaryOutcome::Passed
                    && tcc_protected
                    && (scored.tcc_request_result != Some(true)
                        || scored.tcc_preflight_after != Some(true))
                {
                    missing.insert(format!("{}_later_grant_evidence", receipt.row_id));
                }
            }
            MacosTccCanaryLifecyclePhase::OwnerRestart
            | MacosTccCanaryLifecyclePhase::AppRelaunch
            | MacosTccCanaryLifecyclePhase::ServiceRestart
            | MacosTccCanaryLifecyclePhase::SignedUpdate => {
                if scored.outcome == MacosTccCanaryOutcome::Passed
                    && tcc_protected
                    && (scored.tcc_preflight_before != Some(true)
                        || scored.tcc_preflight_after != Some(true))
                {
                    missing.insert(format!("{}_persistent_grant_evidence", receipt.row_id));
                }
            }
            MacosTccCanaryLifecyclePhase::Grant
            | MacosTccCanaryLifecyclePhase::GrantAfterRevocation
            | MacosTccCanaryLifecyclePhase::AppLaunch
            | MacosTccCanaryLifecyclePhase::ServiceInstall
            | MacosTccCanaryLifecyclePhase::LoginStart => {
                if scored.outcome == MacosTccCanaryOutcome::Passed
                    && tcc_protected
                    && scored.tcc_preflight_after != Some(true)
                {
                    missing.insert(format!("{}_grant_evidence", receipt.row_id));
                }
            }
            MacosTccCanaryLifecyclePhase::RevokeWhileLive => {}
        }
    }
    for evidence in &receipt.capabilities {
        if evidence.resulting_api_state
            != resulting_api_state(evidence.capability, evidence.outcome)
        {
            missing.insert(format!("{}_api_state", receipt.row_id));
        }
        if evidence.outcome == MacosTccCanaryOutcome::NeedsProcessRestart
            && !process_restart_evidence_is_valid(receipt, evidence)
        {
            missing.insert(format!("{}_process_restart_evidence", receipt.row_id));
        }
        if evidence.outcome != MacosTccCanaryOutcome::Passed {
            continue;
        }
        match evidence.capability {
            MacosTccCanaryCapability::Keyboard => {
                if evidence.tap_created != Some(true)
                    || evidence.tap_enabled != Some(true)
                    || evidence.run_loop_started != Some(true)
                    || !evidence
                        .requested_tap_mask
                        .zip(evidence.tap_mask)
                        .is_some_and(|(requested, installed)| {
                            requested != 0 && installed == requested
                        })
                    || evidence.redacted_event_count.is_none_or(|count| count == 0)
                {
                    missing.insert(format!("{}_keyboard_operation", receipt.row_id));
                }
            }
            MacosTccCanaryCapability::Pointer => {
                if evidence.tap_created != Some(true)
                    || evidence.tap_enabled != Some(true)
                    || evidence.run_loop_started != Some(true)
                    || !evidence
                        .requested_tap_mask
                        .zip(evidence.tap_mask)
                        .is_some_and(|(requested, installed)| {
                            requested != 0 && installed == requested
                        })
                    || evidence.redacted_event_count.is_none_or(|count| count == 0)
                {
                    missing.insert(format!("{}_pointer_operation", receipt.row_id));
                }
            }
            MacosTccCanaryCapability::Picker => {
                if evidence.picker_presented != Some(true) || evidence.picker_selected != Some(true)
                {
                    missing.insert(format!("{}_picker_operation", receipt.row_id));
                }
            }
            MacosTccCanaryCapability::Stream => {
                let picker_passed = receipt.capabilities.iter().any(|candidate| {
                    candidate.capability == MacosTccCanaryCapability::Picker
                        && candidate.outcome == MacosTccCanaryOutcome::Passed
                        && candidate.picker_selected == Some(true)
                });
                if !picker_passed
                    || evidence.stream_started != Some(true)
                    || evidence.first_complete_frame != Some(true)
                    || evidence.first_frame_monotonic_ns.is_none()
                {
                    missing.insert(format!("{}_stream_operation", receipt.row_id));
                }
            }
        }
    }
    if receipt.lifecycle_phase == MacosTccCanaryLifecyclePhase::RevokeWhileLive
        && receipt
            .capabilities
            .iter()
            .find(|evidence| evidence.capability == receipt.scored_capability)
            .is_some_and(|evidence| evidence.outcome == MacosTccCanaryOutcome::Revoked)
        && !receipt.capabilities.iter().any(|evidence| {
            evidence.capability == receipt.scored_capability
                && evidence.resource_live_before_revocation == Some(true)
                && evidence.resource_failed_after_revocation == Some(true)
                && evidence.tcc_preflight_after == Some(false)
        })
    {
        missing.insert(format!("{}_live_revocation", receipt.row_id));
    }
}

fn process_restart_evidence_is_valid(
    receipt: &MacosTccCanaryReceipt,
    evidence: &MacosTccCanaryCapabilityEvidence,
) -> bool {
    match evidence.capability {
        MacosTccCanaryCapability::Keyboard => {
            (evidence.tcc_request_result == Some(true)
                || evidence.tcc_preflight_after == Some(true))
                && (evidence
                    .requested_tap_mask
                    .zip(evidence.tap_mask)
                    .is_some_and(|(requested, installed)| requested != 0 && installed != requested)
                    || (evidence.tap_created == Some(false)
                        && evidence.typed_error.as_deref() == Some("permission_denied")))
        }
        MacosTccCanaryCapability::Stream => {
            let picker = receipt
                .capabilities
                .iter()
                .find(|candidate| candidate.capability == MacosTccCanaryCapability::Picker);
            evidence.tcc_request_result == Some(true)
                && evidence.tcc_preflight_after == Some(true)
                && evidence.typed_error.as_deref()
                    == Some("post_authorization_stream_requires_restart")
                && evidence.picker_presented == Some(false)
                && evidence.picker_selected == Some(false)
                && evidence.stream_started == Some(false)
                && evidence.first_complete_frame == Some(false)
                && evidence.first_frame_monotonic_ns.is_none()
                && picker.is_some_and(|picker| {
                    picker.outcome == MacosTccCanaryOutcome::Failed
                        && picker.typed_error.as_deref()
                            == Some("stream_restart_required_before_picker")
                        && picker.picker_presented == Some(false)
                        && picker.picker_selected == Some(false)
                })
        }
        MacosTccCanaryCapability::Pointer | MacosTccCanaryCapability::Picker => false,
    }
}

const fn resulting_api_state(
    capability: MacosTccCanaryCapability,
    outcome: MacosTccCanaryOutcome,
) -> &'static str {
    match outcome {
        MacosTccCanaryOutcome::Passed => match capability {
            MacosTccCanaryCapability::Picker => "ready_idle",
            MacosTccCanaryCapability::Keyboard
            | MacosTccCanaryCapability::Pointer
            | MacosTccCanaryCapability::Stream => "live",
        },
        MacosTccCanaryOutcome::Denied => "permission_denied",
        MacosTccCanaryOutcome::Revoked => "revoked",
        MacosTccCanaryOutcome::NeedsProcessRestart => "needs_process_restart",
        MacosTccCanaryOutcome::Cancelled => "needs_selection",
        MacosTccCanaryOutcome::TimedOut => "interrupted",
        MacosTccCanaryOutcome::Failed => "failed",
    }
}

fn validate_witness_structure(witness: &MacosTccCanaryWitness) -> bool {
    witness.schema_version == MACOS_TCC_CANARY_SCHEMA_VERSION
        && validate_identifier(&witness.run_id, "run_id").is_ok()
        && validate_identifier(&witness.row_id, "row_id").is_ok()
        && validate_identifier(&witness.witness_id, "witness_id").is_ok()
        && !witness.observer.is_empty()
        && witness.observer.len() <= 256
        && witness.observed_unix_ms > 0
        && is_sha256(&witness.evidence_sha256)
        && witness_kind_shape_is_valid(witness)
}

fn witness_kind_shape_is_valid(witness: &MacosTccCanaryWitness) -> bool {
    let has_login_fields = witness.installed_topologies.is_some()
        || witness.enable_order.is_some()
        || witness.selected_topology.is_some()
        || witness.losing_topologies.is_some()
        || witness.owner_conflict_observed.is_some()
        || witness.login_iteration.is_some()
        || witness.login_session_id.is_some();
    let has_observed_process_fields = witness.observed_pid.is_some()
        || witness.observed_audit_token_identity.is_some()
        || witness.observed_signing_audit_token_identity.is_some()
        || witness.observed_cdhash.is_some()
        || witness.observed_designated_requirement_sha256.is_some()
        || witness.observed_process_fingerprint.is_some()
        || witness.parent_pid.is_some()
        || witness.parent_audit_token_identity.is_some()
        || witness.parent_signing_audit_token_identity.is_some()
        || witness.parent_cdhash.is_some()
        || witness.parent_designated_requirement_sha256.is_some()
        || witness.parent_process_fingerprint.is_some();
    let has_replacement_fields = witness.predecessor_pid.is_some()
        || witness.predecessor_audit_token_identity.is_some()
        || witness.predecessor_process_fingerprint.is_some()
        || witness.predecessor_exit_observed.is_some()
        || witness.predecessor_parent_pid.is_some()
        || witness.predecessor_parent_audit_token_identity.is_some()
        || witness.predecessor_parent_process_fingerprint.is_some()
        || witness.predecessor_parent_exit_observed.is_some();
    match witness.kind {
        MacosTccCanaryWitnessKind::FreshTccReset => {
            witness.fresh_tcc_database_observed == Some(true)
                && witness.prompt_text.is_none()
                && witness.system_settings_entry.is_none()
                && !has_observed_process_fields
                && !has_replacement_fields
                && witness.launcher_action.is_none()
                && !has_login_fields
        }
        MacosTccCanaryWitnessKind::SystemSettingsIdentity => {
            witness
                .prompt_text
                .as_deref()
                .is_some_and(|text| !text.is_empty())
                && witness
                    .system_settings_entry
                    .as_deref()
                    .is_some_and(|entry| !entry.is_empty())
                && witness.observed_pid.is_some()
                && witness
                    .observed_audit_token_identity
                    .as_deref()
                    .and_then(audit_token_identity)
                    .is_some_and(|identity| Some(identity.pid) == witness.observed_pid)
                && witness.observed_signing_audit_token_identity
                    == witness.observed_audit_token_identity
                && witness
                    .observed_cdhash
                    .as_deref()
                    .is_some_and(|cdhash| is_hex_with_length(cdhash, &[40, 64]))
                && witness
                    .observed_designated_requirement_sha256
                    .as_deref()
                    .is_some_and(is_sha256)
                && witness
                    .observed_process_fingerprint
                    .as_deref()
                    .is_some_and(is_sha256)
                && ((witness.parent_pid.is_none()
                    && witness.parent_audit_token_identity.is_none()
                    && witness.parent_signing_audit_token_identity.is_none()
                    && witness.parent_cdhash.is_none()
                    && witness.parent_designated_requirement_sha256.is_none()
                    && witness.parent_process_fingerprint.is_none())
                    || (witness
                        .parent_audit_token_identity
                        .as_deref()
                        .and_then(audit_token_identity)
                        .is_some_and(|identity| Some(identity.pid) == witness.parent_pid)
                        && witness.parent_signing_audit_token_identity
                            == witness.parent_audit_token_identity
                        && witness
                            .parent_cdhash
                            .as_deref()
                            .is_some_and(|cdhash| is_hex_with_length(cdhash, &[40, 64]))
                        && witness
                            .parent_designated_requirement_sha256
                            .as_deref()
                            .is_some_and(is_sha256)
                        && witness
                            .parent_process_fingerprint
                            .as_deref()
                            .is_some_and(is_sha256)))
                && witness.fresh_tcc_database_observed.is_none()
                && !has_replacement_fields
                && witness.launcher_action.is_none()
                && !has_login_fields
        }
        MacosTccCanaryWitnessKind::ProcessReplacement => {
            witness.prompt_text.is_none()
                && witness.system_settings_entry.is_none()
                && !has_observed_process_fields
                && witness.fresh_tcc_database_observed.is_none()
                && witness.predecessor_pid.is_some()
                && witness
                    .predecessor_audit_token_identity
                    .as_deref()
                    .and_then(audit_token_identity)
                    .is_some_and(|identity| Some(identity.pid) == witness.predecessor_pid)
                && witness
                    .predecessor_process_fingerprint
                    .as_deref()
                    .is_some_and(is_sha256)
                && witness.predecessor_exit_observed == Some(true)
                && witness
                    .launcher_action
                    .as_deref()
                    .is_some_and(|action| !action.is_empty())
                && !has_login_fields
        }
        MacosTccCanaryWitnessKind::LifecycleAction => {
            witness.prompt_text.is_none()
                && witness.system_settings_entry.is_none()
                && !has_observed_process_fields
                && witness.fresh_tcc_database_observed.is_none()
                && !has_replacement_fields
                && witness
                    .launcher_action
                    .as_deref()
                    .is_some_and(|action| !action.is_empty())
                && !has_login_fields
        }
        MacosTccCanaryWitnessKind::LoginArbitration => {
            witness.prompt_text.is_none()
                && witness.system_settings_entry.is_none()
                && !has_observed_process_fields
                && witness.fresh_tcc_database_observed.is_none()
                && !has_replacement_fields
                && witness.launcher_action.is_none()
                && has_login_fields
        }
    }
}

fn matching_witness<'a>(
    receipt: &MacosTccCanaryReceipt,
    witness_id: &str,
    kind: MacosTccCanaryWitnessKind,
    witnesses: &BTreeMap<&'a str, &'a MacosTccCanaryWitness>,
) -> Option<&'a MacosTccCanaryWitness> {
    witnesses.get(witness_id).copied().filter(|witness| {
        witness.run_id == receipt.run_id
            && witness.row_id == receipt.row_id
            && witness.kind == kind
            && witness.observed_unix_ms <= receipt.operation_finished_unix_ms
    })
}

fn validate_lifecycle_link<'a>(
    receipt: &MacosTccCanaryReceipt,
    by_row: &BTreeMap<&'a str, &'a MacosTccCanaryReceipt>,
    witnesses: &BTreeMap<&'a str, &'a MacosTccCanaryWitness>,
    missing: &mut BTreeSet<String>,
) {
    if !receipt.lifecycle_phase.needs_predecessor() {
        if receipt.predecessor_row_id.is_some() {
            missing.insert(format!("{}_unexpected_predecessor", receipt.row_id));
        }
        if receipt.lifecycle_phase.needs_lifecycle_action_witness() {
            let action_witness =
                receipt
                    .lifecycle_action_witness_id
                    .as_deref()
                    .and_then(|witness_id| {
                        matching_witness(
                            receipt,
                            witness_id,
                            MacosTccCanaryWitnessKind::LifecycleAction,
                            witnesses,
                        )
                    });
            if !action_witness.is_some_and(|witness| {
                witness.launcher_action.as_deref()
                    == expected_launcher_action(receipt.topology, receipt.lifecycle_phase)
                    && witness.observed_unix_ms <= receipt.process_started_unix_ms
            }) {
                missing.insert(format!("{}_lifecycle_action_witness", receipt.row_id));
            }
        }
        return;
    }
    let Some(predecessor) = receipt
        .predecessor_row_id
        .as_deref()
        .and_then(|row| by_row.get(row).copied())
    else {
        missing.insert(format!("{}_predecessor", receipt.row_id));
        return;
    };
    if receipt.lifecycle_phase.required_predecessor() != Some(predecessor.lifecycle_phase) {
        missing.insert(format!("{}_predecessor_phase", receipt.row_id));
    }
    if predecessor.run_id != receipt.run_id
        || predecessor.scenario_id != receipt.scenario_id
        || predecessor.installation_scenario != receipt.installation_scenario
        || predecessor.login_iteration != receipt.login_iteration
        || predecessor.topology != receipt.topology
        || predecessor.scored_capability != receipt.scored_capability
        || predecessor.host_architecture != receipt.host_architecture
        || predecessor.executable_slice != receipt.executable_slice
        || predecessor.translated_process != receipt.translated_process
        || predecessor.os_version != receipt.os_version
    {
        missing.insert(format!("{}_predecessor_context", receipt.row_id));
    }
    if predecessor.operation_finished_unix_ms > receipt.process_started_unix_ms {
        missing.insert(format!("{}_predecessor_chronology", receipt.row_id));
    }
    if receipt.lifecycle_phase.replaces_process() {
        let predecessor_identity = audit_token_identity(&predecessor.audit_token_identity);
        let successor_identity = audit_token_identity(&receipt.audit_token_identity);
        if predecessor_identity.is_none()
            || successor_identity.is_none()
            || predecessor_identity == successor_identity
        {
            missing.insert(format!("{}_process_replacement", receipt.row_id));
        }
        let replacement_witness =
            receipt
                .process_replacement_witness_id
                .as_deref()
                .and_then(|witness_id| {
                    matching_witness(
                        receipt,
                        witness_id,
                        MacosTccCanaryWitnessKind::ProcessReplacement,
                        witnesses,
                    )
                });
        if !replacement_witness.is_some_and(|witness| {
            witness.predecessor_pid == Some(predecessor.pid)
                && witness.predecessor_audit_token_identity.as_deref()
                    == Some(predecessor.audit_token_identity.as_str())
                && witness.predecessor_process_fingerprint.as_deref()
                    == Some(predecessor.process_fingerprint.as_str())
                && witness.predecessor_exit_observed == Some(true)
                && witness.launcher_action.as_deref()
                    == expected_launcher_action(receipt.topology, receipt.lifecycle_phase)
                && witness.observed_unix_ms >= predecessor.operation_finished_unix_ms
                && witness.observed_unix_ms <= receipt.process_started_unix_ms
                && predecessor_parent_replacement_is_valid(receipt, predecessor, witness, witnesses)
        }) {
            missing.insert(format!("{}_process_replacement_witness", receipt.row_id));
        }
        if predecessor.signing.bundle_identifier != receipt.signing.bundle_identifier
            || predecessor.signing.team_identifier != receipt.signing.team_identifier
            || predecessor.signing.designated_requirement != receipt.signing.designated_requirement
        {
            missing.insert(format!("{}_stable_process_identity", receipt.row_id));
        }
    }
    if receipt.lifecycle_phase == MacosTccCanaryLifecyclePhase::SignedUpdate {
        let stable_identity = predecessor.signing.bundle_identifier
            == receipt.signing.bundle_identifier
            && predecessor.signing.team_identifier == receipt.signing.team_identifier
            && predecessor.signing.designated_requirement == receipt.signing.designated_requirement;
        let changed_artifact = predecessor.binary_version != receipt.binary_version
            && predecessor.signing.cdhash != receipt.signing.cdhash;
        if !stable_identity || !changed_artifact {
            missing.insert(format!("{}_signed_update_identity", receipt.row_id));
        }
    }
}

fn predecessor_parent_replacement_is_valid(
    receipt: &MacosTccCanaryReceipt,
    predecessor: &MacosTccCanaryReceipt,
    witness: &MacosTccCanaryWitness,
    witnesses: &BTreeMap<&str, &MacosTccCanaryWitness>,
) -> bool {
    let replaces_app_parent = receipt.topology == MacosDaemonOwner::AppSidecar
        && matches!(
            receipt.lifecycle_phase,
            MacosTccCanaryLifecyclePhase::AppRelaunch | MacosTccCanaryLifecyclePhase::SignedUpdate
        );
    if !replaces_app_parent {
        return witness.predecessor_parent_pid.is_none()
            && witness.predecessor_parent_audit_token_identity.is_none()
            && witness.predecessor_parent_process_fingerprint.is_none()
            && witness.predecessor_parent_exit_observed.is_none();
    }
    let Some(parent_signing) = predecessor.launcher.parent_signing.as_ref() else {
        return false;
    };
    let predecessor_settings = matching_witness(
        predecessor,
        &predecessor.system_settings_identity_witness_id,
        MacosTccCanaryWitnessKind::SystemSettingsIdentity,
        witnesses,
    );
    witness.predecessor_parent_pid == predecessor.launcher.parent_pid
        && witness.predecessor_parent_pid == Some(parent_signing.process_bound_pid)
        && witness.predecessor_parent_audit_token_identity.as_deref()
            == predecessor_settings
                .and_then(|settings| settings.parent_audit_token_identity.as_deref())
        && witness
            .predecessor_parent_audit_token_identity
            .as_deref()
            .and_then(audit_token_identity)
            .is_some_and(|identity| Some(identity.pid) == witness.predecessor_parent_pid)
        && witness.predecessor_parent_process_fingerprint.as_deref()
            == Some(parent_signing.process_bound_fingerprint.as_str())
        && witness.predecessor_parent_exit_observed == Some(true)
}

fn validate_login_arbitration(
    receipt: &MacosTccCanaryReceipt,
    witnesses: &BTreeMap<&str, &MacosTccCanaryWitness>,
    missing: &mut BTreeSet<String>,
) {
    if receipt.installation_scenario.needs_repeated_login_proof() {
        if !login_arbitration_witness(receipt, witnesses)
            .is_some_and(|witness| login_arbitration_witness_is_valid(receipt, witness))
        {
            missing.insert(format!("{}_login_arbitration_witness", receipt.row_id));
        }
    } else if receipt.login_arbitration_witness_id.is_some() {
        missing.insert(format!("{}_unexpected_login_arbitration", receipt.row_id));
    }
}

fn login_arbitration_witness<'a>(
    receipt: &MacosTccCanaryReceipt,
    witnesses: &BTreeMap<&'a str, &'a MacosTccCanaryWitness>,
) -> Option<&'a MacosTccCanaryWitness> {
    receipt
        .login_arbitration_witness_id
        .as_deref()
        .and_then(|witness_id| {
            matching_witness(
                receipt,
                witness_id,
                MacosTccCanaryWitnessKind::LoginArbitration,
                witnesses,
            )
        })
}

fn login_arbitration_witness_is_valid(
    receipt: &MacosTccCanaryReceipt,
    witness: &MacosTccCanaryWitness,
) -> bool {
    let scenario = receipt.installation_scenario;
    let expected_installed = scenario.installed_topologies();
    let expected_losers = expected_installed
        .iter()
        .copied()
        .filter(|owner| *owner != receipt.topology)
        .collect::<Vec<_>>();
    witness
        .installed_topologies
        .as_deref()
        .is_some_and(|installed| topology_sets_equal(installed, expected_installed))
        && witness
            .enable_order
            .as_deref()
            .is_some_and(|order| scenario.enable_order_is_valid(order))
        && witness.selected_topology == Some(receipt.topology)
        && witness
            .losing_topologies
            .as_deref()
            .is_some_and(|losers| topology_sets_equal(losers, &expected_losers))
        && witness.owner_conflict_observed == Some(true)
        && witness.login_iteration == Some(receipt.login_iteration)
        && witness
            .login_session_id
            .as_deref()
            .is_some_and(|id| validate_identifier(id, "login_session_id").is_ok())
        && witness.observed_unix_ms <= receipt.process_started_unix_ms
}

fn topology_sets_equal(left: &[MacosDaemonOwner], right: &[MacosDaemonOwner]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .map(|owner| topology_key(*owner))
            .collect::<BTreeSet<_>>()
            == right
                .iter()
                .map(|owner| topology_key(*owner))
                .collect::<BTreeSet<_>>()
}

fn scored_capability_shape_is_valid(
    scored: MacosTccCanaryCapability,
    evidence: &BTreeSet<MacosTccCanaryCapability>,
) -> bool {
    if scored == MacosTccCanaryCapability::Stream {
        *evidence
            == BTreeSet::from([
                MacosTccCanaryCapability::Picker,
                MacosTccCanaryCapability::Stream,
            ])
    } else {
        *evidence == BTreeSet::from([scored])
    }
}

fn topology_capability_qualifies(
    receipts: &[MacosTccCanaryReceipt],
    witnesses: &BTreeMap<&str, &MacosTccCanaryWitness>,
    topology: MacosDaemonOwner,
    capability: MacosTccCanaryCapability,
) -> bool {
    platform_cells()
        .into_iter()
        .all(|(architecture, os_family)| {
            let phases_qualify = capability_phases(capability)
                .iter()
                .chain(topology_phases(topology))
                .copied()
                .all(|phase| {
                    receipts.iter().any(|receipt| {
                        receipt.topology == topology
                            && receipt.scored_capability == capability
                            && receipt.lifecycle_phase == phase
                            && platform_cell_matches(receipt, architecture)
                            && macos_os_family(&receipt.os_version) == Some(os_family)
                            && receipt.capabilities.iter().any(|evidence| {
                                evidence.capability == capability
                                    && evidence.outcome == expected_outcome(phase)
                            })
                    })
                });
            let fresh_qualifies = receipts.iter().any(|receipt| {
                receipt.topology == topology
                    && receipt.scored_capability == capability
                    && platform_cell_matches(receipt, architecture)
                    && macos_os_family(&receipt.os_version) == Some(os_family)
                    && receipt.capabilities.iter().any(|evidence| {
                        evidence.capability == capability
                            && evidence.outcome == MacosTccCanaryOutcome::Passed
                    })
                    && receipt
                        .fresh_tcc_reset_witness_id
                        .as_deref()
                        .and_then(|witness_id| {
                            matching_witness(
                                receipt,
                                witness_id,
                                MacosTccCanaryWitnessKind::FreshTccReset,
                                witnesses,
                            )
                        })
                        .is_some_and(|witness| witness.fresh_tcc_database_observed == Some(true))
            });
            phases_qualify && fresh_qualifies
        })
}

const fn platform_cells() -> [(&'static str, &'static str); 4] {
    [
        ("apple_silicon", "sequoia_15_2"),
        ("apple_silicon", "tahoe_26"),
        ("intel", "sequoia_15_2"),
        ("intel", "tahoe_26"),
    ]
}

fn architecture_evidence_is_coherent(receipt: &MacosTccCanaryReceipt) -> bool {
    matches!(
        (
            receipt.host_architecture.as_str(),
            receipt.executable_slice.as_str(),
            receipt.translated_process,
        ),
        ("apple_silicon", "aarch64", false)
            | ("apple_silicon", "x86_64", true)
            | ("intel", "x86_64", false)
    )
}

fn platform_cell_matches(receipt: &MacosTccCanaryReceipt, architecture: &str) -> bool {
    match architecture {
        "apple_silicon" => {
            receipt.host_architecture == "apple_silicon"
                && receipt.executable_slice == "aarch64"
                && !receipt.translated_process
        }
        "intel" => {
            receipt.host_architecture == "intel"
                && receipt.executable_slice == "x86_64"
                && !receipt.translated_process
        }
        _ => false,
    }
}

fn capability_phases(
    capability: MacosTccCanaryCapability,
) -> &'static [MacosTccCanaryLifecyclePhase] {
    use MacosTccCanaryLifecyclePhase::{
        Deny, Grant, GrantAfterRevocation, LaterGrant, RevokeWhileLive,
    };

    match capability {
        MacosTccCanaryCapability::Keyboard | MacosTccCanaryCapability::Stream => &[
            Grant,
            Deny,
            LaterGrant,
            RevokeWhileLive,
            GrantAfterRevocation,
        ],
        MacosTccCanaryCapability::Pointer => &[Grant],
        MacosTccCanaryCapability::Picker => &[Grant, Deny, LaterGrant],
    }
}

fn topology_phases(topology: MacosDaemonOwner) -> &'static [MacosTccCanaryLifecyclePhase] {
    use MacosTccCanaryLifecyclePhase::{
        AppLaunch, AppRelaunch, LoginStart, OwnerRestart, ServiceInstall, ServiceRestart,
        SignedUpdate,
    };

    match topology {
        MacosDaemonOwner::AppSidecar => &[AppLaunch, OwnerRestart, AppRelaunch, SignedUpdate],
        MacosDaemonOwner::DirectLaunchd | MacosDaemonOwner::Homebrew => {
            &[ServiceInstall, LoginStart, ServiceRestart, SignedUpdate]
        }
        MacosDaemonOwner::Standalone => &[SignedUpdate],
    }
}

const fn expected_launcher_action(
    topology: MacosDaemonOwner,
    phase: MacosTccCanaryLifecyclePhase,
) -> Option<&'static str> {
    use MacosTccCanaryLifecyclePhase::{
        AppLaunch, AppRelaunch, GrantAfterRevocation, LaterGrant, LoginStart, OwnerRestart,
        ServiceInstall, ServiceRestart, SignedUpdate,
    };

    match (topology, phase) {
        (MacosDaemonOwner::AppSidecar, AppLaunch) => Some("app_minimized_launch"),
        (MacosDaemonOwner::AppSidecar, OwnerRestart) => Some("app_supervisor_daemon_restart"),
        (MacosDaemonOwner::AppSidecar, AppRelaunch) => Some("app_quit_then_minimized_launch"),
        (MacosDaemonOwner::AppSidecar, LaterGrant | GrantAfterRevocation) => {
            Some("app_supervisor_daemon_restart_after_authorization")
        }
        (MacosDaemonOwner::AppSidecar, SignedUpdate) => Some("signed_app_update_then_app_relaunch"),
        (MacosDaemonOwner::DirectLaunchd, ServiceInstall) => Some("hypercolor_service_enable"),
        (MacosDaemonOwner::DirectLaunchd, LoginStart) => Some("launchd_login_start"),
        (MacosDaemonOwner::DirectLaunchd, ServiceRestart) => Some("hypercolor_service_restart"),
        (MacosDaemonOwner::DirectLaunchd, LaterGrant | GrantAfterRevocation) => {
            Some("hypercolor_service_restart_after_authorization")
        }
        (MacosDaemonOwner::DirectLaunchd, SignedUpdate) => {
            Some("signed_daemon_update_then_hypercolor_service_restart")
        }
        (MacosDaemonOwner::Homebrew, ServiceInstall) => Some("brew_services_start"),
        (MacosDaemonOwner::Homebrew, LoginStart) => Some("brew_services_login_start"),
        (MacosDaemonOwner::Homebrew, ServiceRestart) => Some("brew_services_restart"),
        (MacosDaemonOwner::Homebrew, LaterGrant | GrantAfterRevocation) => {
            Some("brew_services_restart_after_authorization")
        }
        (MacosDaemonOwner::Homebrew, SignedUpdate) => {
            Some("signed_daemon_update_then_brew_services_restart")
        }
        (MacosDaemonOwner::Standalone, LaterGrant | GrantAfterRevocation) => {
            Some("terminal_successor_launch_after_authorization")
        }
        (MacosDaemonOwner::Standalone, SignedUpdate) => {
            Some("signed_daemon_update_then_terminal_launch")
        }
        _ => None,
    }
}

const fn expected_outcome(phase: MacosTccCanaryLifecyclePhase) -> MacosTccCanaryOutcome {
    match phase {
        MacosTccCanaryLifecyclePhase::Deny => MacosTccCanaryOutcome::Denied,
        MacosTccCanaryLifecyclePhase::RevokeWhileLive => MacosTccCanaryOutcome::Revoked,
        MacosTccCanaryLifecyclePhase::Grant
        | MacosTccCanaryLifecyclePhase::LaterGrant
        | MacosTccCanaryLifecyclePhase::GrantAfterRevocation
        | MacosTccCanaryLifecyclePhase::AppLaunch
        | MacosTccCanaryLifecyclePhase::OwnerRestart
        | MacosTccCanaryLifecyclePhase::AppRelaunch
        | MacosTccCanaryLifecyclePhase::ServiceInstall
        | MacosTccCanaryLifecyclePhase::LoginStart
        | MacosTccCanaryLifecyclePhase::ServiceRestart
        | MacosTccCanaryLifecyclePhase::SignedUpdate => MacosTccCanaryOutcome::Passed,
    }
}

fn macos_os_family(version: &str) -> Option<&'static str> {
    let mut components = version.split('.');
    let major = components.next()?.parse::<u32>().ok()?;
    let minor = components.next().unwrap_or("0").parse::<u32>().ok()?;
    match (major, minor) {
        (15, minor) if minor >= 2 => Some("sequoia_15_2"),
        (26, _) => Some("tahoe_26"),
        _ => None,
    }
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    anyhow::ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value != "."
            && value != ".."
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')),
        "{label} must be 1 through 128 ASCII identifier characters"
    );
    Ok(())
}

fn validate_observed_text(value: &str, label: &str) -> Result<()> {
    anyhow::ensure!(
        !value.is_empty() && value.len() <= 1_024 && !value.contains('\0'),
        "{label} must be 1 through 1024 non-NUL bytes"
    );
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    is_hex_with_length(value, &[64])
}

fn is_hex_with_length(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn open_regular_file(path: &Path) -> Result<(File, fs::Metadata)> {
    let path_metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    anyhow::ensure!(
        path_metadata.file_type().is_file() && !path_metadata.file_type().is_symlink(),
        "{} must be a regular non-symlink file",
        path.display()
    );
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let file_metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect opened file {}", path.display()))?;
    anyhow::ensure!(
        file_metadata.file_type().is_file()
            && path_metadata.dev() == file_metadata.dev()
            && path_metadata.ino() == file_metadata.ino(),
        "{} changed while it was being opened",
        path.display()
    );
    Ok((file, file_metadata))
}

fn witness_evidence_matches(receipt_dir: &Path, witness: &MacosTccCanaryWitness) -> Result<bool> {
    anyhow::ensure!(
        is_sha256(&witness.evidence_sha256),
        "witness evidence hash is not lowercase SHA-256"
    );
    let path = receipt_dir
        .join("evidence")
        .join(format!("{}.bin", witness.evidence_sha256));
    let (mut file, metadata) = open_regular_file(&path)?;
    anyhow::ensure!(
        metadata.len() <= MAX_WITNESS_EVIDENCE_BYTES,
        "witness evidence exceeds {MAX_WITNESS_EVIDENCE_BYTES} bytes"
    );
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut remaining = MAX_WITNESS_EVIDENCE_BYTES.saturating_add(1);
    while remaining > 0 {
        let read_limit = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded evidence buffer length fits usize");
        let read = file
            .read(&mut buffer[..read_limit])
            .with_context(|| format!("failed to read witness evidence {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        remaining = remaining.saturating_sub(u64::try_from(read).unwrap_or(u64::MAX));
    }
    anyhow::ensure!(remaining > 0, "witness evidence exceeds the read bound");
    Ok(hex_bytes(&hasher.finalize()) == witness.evidence_sha256)
}

fn read_json_bounded<T>(path: &Path, maximum: u64) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let (file, metadata) = open_regular_file(path)?;
    anyhow::ensure!(metadata.len() <= maximum, "{} is too large", path.display());
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {}", path.display()))?;
    anyhow::ensure!(
        bytes.len() as u64 <= maximum,
        "{} is too large",
        path.display()
    );
    serde_json::from_slice(&bytes).with_context(|| format!("failed to parse {}", path.display()))
}

fn ensure_real_directory(path: &Path, create: bool) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(
            metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
            "macOS TCC canary directory must be a real directory: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
            fs::create_dir(path).with_context(|| format!("failed to create {}", path.display()))?;
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }
    Ok(())
}

fn ensure_existing_real_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(
            metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
            "macOS TCC canary directory must be a real directory: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }
    Ok(())
}

fn ensure_canary_descendant_directory(root: &Path, directory: &Path) -> Result<()> {
    ensure_real_directory(root, false)?;
    let relative = directory.strip_prefix(root).with_context(|| {
        format!(
            "macOS TCC canary directory {} escapes {}",
            directory.display(),
            root.display()
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            anyhow::bail!(
                "macOS TCC canary descendant contains traversal: {}",
                directory.display()
            );
        };
        current.push(component);
        ensure_real_directory(&current, true)?;
    }
    Ok(())
}

fn write_json_new<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize + ?Sized,
{
    let bytes =
        serde_json::to_vec_pretty(value).context("failed to encode macOS TCC canary JSON")?;
    write_bytes_new(path, &[bytes.as_slice(), b"\n"].concat())
}

fn write_bytes_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("macOS TCC canary JSON path has no parent")?;
    ensure_real_directory(parent, false)?;
    anyhow::ensure!(!path.exists(), "refusing to overwrite {}", path.display());
    let mut temporary = tempfile::Builder::new()
        .prefix(".macos-tcc-canary-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .with_context(|| format!("failed to create temporary file in {}", parent.display()))?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .with_context(|| format!("failed to write temporary JSON for {}", path.display()))?;
    anyhow::ensure!(!path.exists(), "refusing to overwrite {}", path.display());
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to atomically publish {}", path.display()))?;
    sync_parent(parent)
}

fn sync_parent(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("failed to sync {}", path.display()))
}

#[cfg(all(test, feature = "screen-capture"))]
mod tests {
    use super::*;

    #[test]
    fn compact_entitlement_plist_preserves_exact_true_keys() {
        let xml = concat!(
            "<plist><dict>",
            "<key>com.apple.security.device.usb</key><true/>",
            "<key>com.apple.security.cs.allow-jit</key><true />",
            "</dict></plist>"
        );

        assert_eq!(
            plist_true_keys(xml).expect("compact entitlement plist should parse"),
            [
                "com.apple.security.cs.allow-jit".to_owned(),
                "com.apple.security.device.usb".to_owned(),
            ]
        );
    }

    #[test]
    fn entitlement_plist_rejects_non_true_values() {
        assert!(plist_true_keys("<plist><dict><key>unsafe</key><false/></dict></plist>").is_err());
    }

    #[test]
    fn dynamic_codesign_verification_uses_the_nonverbose_live_pid_form() {
        assert_eq!(
            dynamic_codesign_verification_args("+42"),
            ["--verify", "+42"]
        );
    }
}
