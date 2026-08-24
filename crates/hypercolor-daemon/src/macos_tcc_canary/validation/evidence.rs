use std::collections::BTreeSet;

use super::super::{
    model::{MacosTccCanaryCapability, MacosTccCanaryLifecyclePhase, MacosTccCanaryOutcome},
    receipts::{MacosTccCanaryCapabilityEvidence, MacosTccCanaryReceipt},
    rows::resulting_api_state,
};

pub(super) fn validate_capability_evidence(
    receipt: &MacosTccCanaryReceipt,
    missing: &mut BTreeSet<String>,
) {
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
