use std::collections::BTreeSet;

#[cfg(feature = "screen-capture")]
use std::time::Duration;

use anyhow::Result;
use hypercolor_macos_owner::MacosDaemonOwner;
use serde::{Deserialize, Serialize};

pub const MACOS_TCC_CANARY_SCHEMA_VERSION: u32 = 2;
pub(super) const REQUEST_FILE_NAME: &str = "request.json";
pub(super) const MAX_REQUEST_BYTES: u64 = 64 * 1024;
pub(super) const MAX_RECEIPT_BYTES: u64 = 128 * 1024;
pub(super) const MAX_WITNESS_BYTES: u64 = 64 * 1024;
pub(super) const MAX_WITNESS_EVIDENCE_BYTES: u64 = 16 * 1024 * 1024;
pub(super) const MAX_EVIDENCE_ARTIFACTS: usize = 2_048;
const MIN_OPERATION_TIMEOUT_MS: u64 = 1_000;
const MAX_OPERATION_TIMEOUT_MS: u64 = 300_000;
pub(super) const REQUIRED_ENTITLEMENTS: [&str; 6] = [
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
    pub(super) const fn permits(self, topology: MacosDaemonOwner) -> bool {
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

    pub(super) const fn needs_repeated_login_proof(self) -> bool {
        !matches!(
            self,
            Self::AppOnly | Self::DirectLaunchdOnly | Self::HomebrewOnly | Self::StandaloneOnly
        )
    }

    pub(super) const fn installed_topologies(self) -> &'static [MacosDaemonOwner] {
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

    pub(super) fn enable_order_is_valid(self, order: &[MacosDaemonOwner]) -> bool {
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
    pub(super) const fn needs_predecessor(self) -> bool {
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

    pub(super) const fn replaces_process(self) -> bool {
        self.needs_predecessor()
    }

    pub(super) const fn needs_lifecycle_action_witness(self) -> bool {
        matches!(
            self,
            Self::AppLaunch | Self::ServiceInstall | Self::LoginStart
        )
    }

    pub(super) const fn required_predecessor(self) -> Option<Self> {
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
    pub(super) fn timeout(&self) -> Duration {
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

pub(super) fn scored_capability_shape_is_valid(
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

pub(super) fn capability_phases(
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

pub(super) fn topology_phases(
    topology: MacosDaemonOwner,
) -> &'static [MacosTccCanaryLifecyclePhase] {
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

pub(super) fn validate_identifier(value: &str, label: &str) -> Result<()> {
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

pub(super) fn validate_observed_text(value: &str, label: &str) -> Result<()> {
    anyhow::ensure!(
        !value.is_empty() && value.len() <= 1_024 && !value.contains('\0'),
        "{label} must be 1 through 1024 non-NUL bytes"
    );
    Ok(())
}

pub(super) fn is_sha256(value: &str) -> bool {
    is_hex_with_length(value, &[64])
}

pub(super) fn is_hex_with_length(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
