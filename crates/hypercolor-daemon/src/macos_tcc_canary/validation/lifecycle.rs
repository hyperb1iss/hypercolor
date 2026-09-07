use std::collections::{BTreeMap, BTreeSet};

use hypercolor_macos_owner::MacosDaemonOwner;

use super::super::{
    identity::{audit_token_identity, topology_key},
    model::{
        MACOS_TCC_CANARY_SCHEMA_VERSION, MacosTccCanaryLifecyclePhase, is_hex_with_length,
        is_sha256, validate_identifier,
    },
    receipts::{MacosTccCanaryReceipt, MacosTccCanaryWitness, MacosTccCanaryWitnessKind},
};

pub(in crate::macos_tcc_canary) fn validate_witness_structure(
    witness: &MacosTccCanaryWitness,
) -> bool {
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

pub(super) fn matching_witness<'a>(
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

pub(super) fn validate_lifecycle_link<'a>(
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

pub(super) fn validate_login_arbitration(
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

pub(super) fn login_arbitration_witness<'a>(
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

pub(super) fn login_arbitration_witness_is_valid(
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
