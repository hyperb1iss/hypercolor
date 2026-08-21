use std::path::PathBuf;

use hypercolor_macos_owner::MacosDaemonOwner;
use serde::{Deserialize, Serialize};

use super::model::{
    MacosTccCanaryCapability, MacosTccCanaryInstallationScenario, MacosTccCanaryLifecyclePhase,
    MacosTccCanaryOutcome,
};

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
