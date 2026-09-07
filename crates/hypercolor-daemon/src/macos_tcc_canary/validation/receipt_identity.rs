use std::collections::BTreeMap;

use hypercolor_macos_owner::MacosDaemonOwner;

use super::{
    super::{
        identity::{audit_token_identity, hex_digest, terminal_parent_is_valid, topology_key},
        model::{REQUIRED_ENTITLEMENTS, is_hex_with_length, is_sha256},
        receipts::{
            MacosTccCanaryReceipt, MacosTccCanarySigningEvidence, MacosTccCanaryWitness,
            MacosTccCanaryWitnessKind,
        },
    },
    lifecycle::matching_witness,
};

pub(super) fn launcher_identity_valid(receipt: &MacosTccCanaryReceipt) -> bool {
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

pub(in crate::macos_tcc_canary) fn receipt_identity_valid(
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

pub(super) fn app_parent_witness_is_valid(
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

pub(super) fn signing_identity_is_valid(signing: &MacosTccCanarySigningEvidence) -> bool {
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

pub(super) fn matrix_identity_is_stable(receipts: &[MacosTccCanaryReceipt]) -> bool {
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
