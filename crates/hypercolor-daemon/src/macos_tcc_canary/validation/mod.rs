mod evidence;
mod lifecycle;
mod receipt_identity;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result};
use hypercolor_macos_owner::MacosDaemonOwner;

use self::{
    evidence::validate_capability_evidence,
    lifecycle::{
        login_arbitration_witness, login_arbitration_witness_is_valid, matching_witness,
        validate_lifecycle_link, validate_login_arbitration,
    },
    receipt_identity::matrix_identity_is_stable,
};
pub(super) use self::{
    lifecycle::validate_witness_structure, receipt_identity::receipt_identity_valid,
};
use super::{
    artifacts::{ensure_real_directory, read_json_bounded, witness_evidence_matches},
    model::{
        MACOS_TCC_CANARY_SCHEMA_VERSION, MAX_EVIDENCE_ARTIFACTS, MAX_RECEIPT_BYTES,
        MAX_WITNESS_BYTES, MacosTccCanaryCapability, MacosTccCanaryInstallationScenario,
        MacosTccCanaryLifecyclePhase, MacosTccCanaryOutcome, capability_phases, is_sha256,
        scored_capability_shape_is_valid, topology_phases, validate_identifier,
        validate_observed_text,
    },
    receipts::{
        MacosTccCanaryCapabilityQualification, MacosTccCanaryReceipt, MacosTccCanaryValidation,
        MacosTccCanaryWitness, MacosTccCanaryWitnessKind,
    },
};

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
