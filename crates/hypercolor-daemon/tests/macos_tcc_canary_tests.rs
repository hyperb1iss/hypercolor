#![cfg(all(target_os = "macos", feature = "macos-tcc-canary"))]

use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use hypercolor_daemon::macos_tcc_canary::{
    MACOS_TCC_CANARY_SCHEMA_VERSION, MacosTccCanaryCapability, MacosTccCanaryCapabilityEvidence,
    MacosTccCanaryInstallationScenario, MacosTccCanaryLauncherEvidence,
    MacosTccCanaryLifecyclePhase, MacosTccCanaryOutcome, MacosTccCanaryReceipt,
    MacosTccCanaryRequest, MacosTccCanarySigningEvidence, MacosTccCanaryValidation,
    MacosTccCanaryWitness, MacosTccCanaryWitnessKind, arm_macos_tcc_canary,
    publish_macos_tcc_canary_artifact, validate_macos_tcc_canary_receipts,
};
use hypercolor_macos_owner::MacosDaemonOwner;
use sha2::{Digest, Sha256};

const RUN_ID: &str = "signed-acceptance-run";

fn base_request() -> MacosTccCanaryRequest {
    MacosTccCanaryRequest {
        schema_version: MACOS_TCC_CANARY_SCHEMA_VERSION,
        run_id: RUN_ID.to_owned(),
        row_id: "app-keyboard-grant".to_owned(),
        scenario_id: "app-only".to_owned(),
        installation_scenario: MacosTccCanaryInstallationScenario::AppOnly,
        login_iteration: 1,
        expected_topology: MacosDaemonOwner::AppSidecar,
        lifecycle_phase: MacosTccCanaryLifecyclePhase::Grant,
        predecessor_row_id: None,
        process_replacement_witness_id: None,
        lifecycle_action_witness_id: None,
        login_arbitration_witness_id: None,
        scored_capability: MacosTccCanaryCapability::Keyboard,
        capabilities: vec![MacosTccCanaryCapability::Keyboard],
        allow_input_prompt: true,
        allow_screen_prompt: false,
        allow_picker: false,
        operation_timeout_ms: 30_000,
        fresh_tcc_reset_witness_id: Some("fresh-app-keyboard".to_owned()),
        system_settings_identity_witness_id: "settings-app-keyboard".to_owned(),
        expected_prompt_text: "Hypercolor requests access".to_owned(),
        expected_system_settings_entry: "Hypercolor".to_owned(),
    }
}

#[test]
fn canary_request_closes_capability_and_lifecycle_shapes() {
    base_request()
        .validate()
        .expect("baseline request should pass");

    let mut stream_without_picker = base_request();
    stream_without_picker.scored_capability = MacosTccCanaryCapability::Stream;
    stream_without_picker.capabilities = vec![MacosTccCanaryCapability::Stream];
    stream_without_picker.allow_picker = true;
    assert!(stream_without_picker.validate().is_err());

    let mut restart_without_links = base_request();
    restart_without_links.lifecycle_phase = MacosTccCanaryLifecyclePhase::OwnerRestart;
    assert!(restart_without_links.validate().is_err());

    let mut wrong_topology_phase = base_request();
    wrong_topology_phase.expected_topology = MacosDaemonOwner::Homebrew;
    wrong_topology_phase.installation_scenario = MacosTccCanaryInstallationScenario::HomebrewOnly;
    wrong_topology_phase.lifecycle_phase = MacosTccCanaryLifecyclePhase::AppLaunch;
    assert!(wrong_topology_phase.validate().is_err());

    let mut mixed_without_login_witness = base_request();
    mixed_without_login_witness.installation_scenario =
        MacosTccCanaryInstallationScenario::AppHomebrew;
    assert!(mixed_without_login_witness.validate().is_err());

    let mut later_grant = base_request();
    later_grant.lifecycle_phase = MacosTccCanaryLifecyclePhase::LaterGrant;
    later_grant.predecessor_row_id = Some("denied-row".to_owned());
    later_grant.process_replacement_witness_id = Some("replacement-row".to_owned());
    later_grant
        .validate()
        .expect("later grant runs in a replacement process");

    let mut grant_with_predecessor = base_request();
    grant_with_predecessor.predecessor_row_id = Some("unexpected-row".to_owned());
    assert!(grant_with_predecessor.validate().is_err());

    let mut app_launch = base_request();
    app_launch.lifecycle_phase = MacosTccCanaryLifecyclePhase::AppLaunch;
    assert!(app_launch.validate().is_err());
    app_launch.lifecycle_action_witness_id = Some("app-launch-action".to_owned());
    app_launch
        .validate()
        .expect("app launch with an exact action witness should pass");

    for invalid in [".", ".."] {
        let mut invalid_request = base_request();
        invalid_request.run_id = invalid.to_owned();
        assert!(invalid_request.validate().is_err());

        let mut invalid_request = base_request();
        invalid_request.row_id = invalid.to_owned();
        assert!(invalid_request.validate().is_err());
    }
}

#[test]
fn arming_uses_private_modes_and_never_overwrites() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let request_path = directory.path().join("row.json");
    fs::write(
        &request_path,
        serde_json::to_vec(&base_request()).expect("request should encode"),
    )
    .expect("request should write");

    let armed =
        arm_macos_tcc_canary(directory.path(), &request_path).expect("valid request should arm");
    let mode = fs::metadata(&armed)
        .expect("armed request should exist")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let root_mode = fs::metadata(
        armed
            .parent()
            .expect("armed request should have a canary root"),
    )
    .expect("canary root should exist")
    .permissions()
    .mode()
        & 0o777;
    assert_eq!(root_mode, 0o700);
    assert!(
        fs::read_dir(
            armed
                .parent()
                .expect("armed request should have a canary root")
        )
        .expect("canary root should read")
        .all(|entry| {
            !entry
                .expect("canary entry should read")
                .file_name()
                .to_string_lossy()
                .contains(".tmp")
        })
    );
    assert!(arm_macos_tcc_canary(directory.path(), &request_path).is_err());
}

#[test]
fn artifact_publication_is_atomic_synced_and_never_overwrites() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let canary_root = directory.path().join("macos-tcc-canary");
    let receipt_dir = canary_root.join("receipts/run");
    fs::create_dir_all(&receipt_dir).expect("receipt directory should create");
    let source = directory.path().join("witness.json");
    let destination = receipt_dir.join("witness.json");
    fs::write(&source, b"first").expect("source should write");

    publish_macos_tcc_canary_artifact(&canary_root, &source, &destination)
        .expect("artifact should publish");
    assert_eq!(
        fs::read(&destination).expect("artifact should read"),
        b"first"
    );
    assert!(
        fs::read_dir(&receipt_dir)
            .expect("receipt directory should read")
            .all(|entry| !entry
                .expect("receipt entry should read")
                .file_name()
                .to_string_lossy()
                .contains(".tmp"))
    );

    fs::write(&source, b"second").expect("replacement source should write");
    assert!(publish_macos_tcc_canary_artifact(&canary_root, &source, &destination).is_err());
    assert_eq!(
        fs::read(&destination).expect("artifact should read"),
        b"first"
    );
}

#[test]
fn arming_rejects_symlinked_request_files() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let request_path = directory.path().join("row.json");
    let request_link = directory.path().join("row-link.json");
    fs::write(
        &request_path,
        serde_json::to_vec(&base_request()).expect("request should encode"),
    )
    .expect("request should write");
    std::os::unix::fs::symlink(&request_path, &request_link)
        .expect("request symlink should create");

    assert!(arm_macos_tcc_canary(directory.path(), &request_link).is_err());
}

#[test]
fn arming_rejects_a_symlinked_canary_root() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let redirected = tempfile::tempdir().expect("redirect directory should exist");
    let request_path = directory.path().join("row.json");
    fs::write(
        &request_path,
        serde_json::to_vec(&base_request()).expect("request should encode"),
    )
    .expect("request should write");
    std::os::unix::fs::symlink(redirected.path(), directory.path().join("macos-tcc-canary"))
        .expect("canary root symlink should create");

    assert!(arm_macos_tcc_canary(directory.path(), &request_path).is_err());
    assert!(!redirected.path().join("request.json").exists());
}

#[test]
fn arming_rejects_symlinked_reserved_descendants() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let redirected = tempfile::tempdir().expect("redirect directory should exist");
    let canary_root = directory.path().join("macos-tcc-canary");
    fs::create_dir(&canary_root).expect("canary root should create");
    std::os::unix::fs::symlink(redirected.path(), canary_root.join("receipts"))
        .expect("reserved descendant symlink should create");
    let request_path = directory.path().join("row.json");
    write_json(&request_path, &base_request());

    assert!(arm_macos_tcc_canary(directory.path(), &request_path).is_err());
    assert!(!canary_root.join("request.json").exists());
}

#[derive(Default)]
struct MatrixFixture {
    next_row: u32,
    failed_row: Option<(
        MacosDaemonOwner,
        MacosTccCanaryCapability,
        MacosTccCanaryLifecyclePhase,
    )>,
}

impl MatrixFixture {
    fn add_receipt(
        &mut self,
        directory: &Path,
        topology: MacosDaemonOwner,
        capability: MacosTccCanaryCapability,
        phase: MacosTccCanaryLifecyclePhase,
        architecture: &str,
        os_version: &str,
        scenario: MacosTccCanaryInstallationScenario,
        scenario_id: &str,
        login_iteration: u32,
        predecessor: Option<&MacosTccCanaryReceipt>,
    ) -> MacosTccCanaryReceipt {
        self.next_row += 1;
        let row_id = format!("row-{:04}", self.next_row);
        let pid = 10_000 + self.next_row;
        let process_started_unix_ms = 1_000_000 + u64::from(self.next_row) * 100;
        let expected_outcome = if self.failed_row == Some((topology, capability, phase)) {
            MacosTccCanaryOutcome::Failed
        } else {
            expected_outcome(phase)
        };
        let capabilities = evidence_for(capability, expected_outcome, phase);
        let settings_witness_id = format!("settings-{row_id}");
        let fresh_witness_id =
            (phase == MacosTccCanaryLifecyclePhase::Grant).then(|| format!("fresh-{row_id}"));
        let replacement_witness_id = (predecessor.is_some() && phase_replaces_process(phase))
            .then(|| format!("replacement-{row_id}"));
        let lifecycle_action_witness_id =
            phase_needs_lifecycle_action_witness(phase).then(|| format!("lifecycle-{row_id}"));
        let login_witness_id =
            scenario_needs_login_witness(scenario).then(|| format!("login-{row_id}"));
        let signed_update = phase == MacosTccCanaryLifecyclePhase::SignedUpdate;
        let receipt = MacosTccCanaryReceipt {
            schema_version: MACOS_TCC_CANARY_SCHEMA_VERSION,
            run_id: RUN_ID.to_owned(),
            row_id: row_id.clone(),
            scenario_id: scenario_id.to_owned(),
            installation_scenario: scenario,
            login_iteration,
            topology,
            lifecycle_phase: phase,
            predecessor_row_id: predecessor.map(|receipt| receipt.row_id.clone()),
            process_replacement_witness_id: replacement_witness_id.clone(),
            lifecycle_action_witness_id: lifecycle_action_witness_id.clone(),
            login_arbitration_witness_id: login_witness_id.clone(),
            scored_capability: capability,
            fresh_tcc_reset_witness_id: fresh_witness_id.clone(),
            system_settings_identity_witness_id: settings_witness_id.clone(),
            expected_prompt_text: "Hypercolor requests access".to_owned(),
            expected_system_settings_entry: "Hypercolor".to_owned(),
            host_architecture: architecture.to_owned(),
            executable_slice: if architecture == "intel" {
                "x86_64"
            } else {
                "aarch64"
            }
            .to_owned(),
            translated_process: false,
            os_version: os_version.to_owned(),
            binary_version: if signed_update { "2.0.0" } else { "1.0.0" }.to_owned(),
            pid,
            process_fingerprint: format!("{pid:064x}"),
            audit_token_identity: format!(
                "00000000:00000000:00000000:00000000:00000000:{pid:08x}:00000000:{pid:08x}"
            ),
            executable_path: PathBuf::from(
                "/Applications/Hypercolor.app/Contents/MacOS/hypercolor-daemon",
            ),
            process_started_unix_ms,
            operation_finished_unix_ms: process_started_unix_ms + 50,
            launcher: launcher(topology),
            signing: signing(topology, signed_update, pid, &format!("{pid:064x}")),
            capabilities,
            acceptance_claim: "evidence_only".to_owned(),
        };
        write_witness(
            directory,
            witness(
                &receipt,
                settings_witness_id,
                MacosTccCanaryWitnessKind::SystemSettingsIdentity,
                None,
            ),
        );
        if let Some(witness_id) = fresh_witness_id {
            write_witness(
                directory,
                witness(
                    &receipt,
                    witness_id,
                    MacosTccCanaryWitnessKind::FreshTccReset,
                    None,
                ),
            );
        }
        if let (Some(witness_id), Some(predecessor)) = (replacement_witness_id, predecessor) {
            write_witness(
                directory,
                witness(
                    &receipt,
                    witness_id,
                    MacosTccCanaryWitnessKind::ProcessReplacement,
                    Some(predecessor),
                ),
            );
        }
        if let Some(witness_id) = lifecycle_action_witness_id {
            write_witness(
                directory,
                witness(
                    &receipt,
                    witness_id,
                    MacosTccCanaryWitnessKind::LifecycleAction,
                    None,
                ),
            );
        }
        if let Some(witness_id) = login_witness_id {
            write_witness(
                directory,
                witness(
                    &receipt,
                    witness_id,
                    MacosTccCanaryWitnessKind::LoginArbitration,
                    None,
                ),
            );
        }
        write_json(&directory.join(format!("{row_id}.receipt.json")), &receipt);
        receipt
    }
}

fn validate_full_signed_matrix(
    failed_row: Option<(
        MacosDaemonOwner,
        MacosTccCanaryCapability,
        MacosTccCanaryLifecyclePhase,
    )>,
) -> MacosTccCanaryValidation {
    let directory = full_signed_matrix_directory(failed_row);
    validate_macos_tcc_canary_receipts(directory.path()).expect("complete fixture should validate")
}

fn full_signed_matrix_directory(
    failed_row: Option<(
        MacosDaemonOwner,
        MacosTccCanaryCapability,
        MacosTccCanaryLifecyclePhase,
    )>,
) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let mut fixture = MatrixFixture {
        failed_row,
        ..MatrixFixture::default()
    };

    for topology in topologies() {
        for capability in capabilities() {
            let scenario = single_scenario(topology);
            for (architecture, os_version) in platform_cells() {
                let scenario_id =
                    format!("single-{topology:?}-{capability:?}-{architecture}-{os_version}");
                let grant = fixture.add_receipt(
                    directory.path(),
                    topology,
                    capability,
                    MacosTccCanaryLifecyclePhase::Grant,
                    architecture,
                    os_version,
                    scenario,
                    &scenario_id,
                    1,
                    None,
                );
                let deny = if capability == MacosTccCanaryCapability::Pointer {
                    None
                } else {
                    Some(fixture.add_receipt(
                        directory.path(),
                        topology,
                        capability,
                        MacosTccCanaryLifecyclePhase::Deny,
                        architecture,
                        os_version,
                        scenario,
                        &scenario_id,
                        1,
                        None,
                    ))
                };
                let revoke = matches!(
                    capability,
                    MacosTccCanaryCapability::Keyboard | MacosTccCanaryCapability::Stream
                )
                .then(|| {
                    fixture.add_receipt(
                        directory.path(),
                        topology,
                        capability,
                        MacosTccCanaryLifecyclePhase::RevokeWhileLive,
                        architecture,
                        os_version,
                        scenario,
                        &scenario_id,
                        1,
                        None,
                    )
                });
                if let Some(deny) = deny.as_ref() {
                    fixture.add_receipt(
                        directory.path(),
                        topology,
                        capability,
                        MacosTccCanaryLifecyclePhase::LaterGrant,
                        architecture,
                        os_version,
                        scenario,
                        &scenario_id,
                        1,
                        Some(deny),
                    );
                }
                if let Some(revoke) = revoke.as_ref() {
                    fixture.add_receipt(
                        directory.path(),
                        topology,
                        capability,
                        MacosTccCanaryLifecyclePhase::GrantAfterRevocation,
                        architecture,
                        os_version,
                        scenario,
                        &scenario_id,
                        1,
                        Some(revoke),
                    );
                }
                for phase in topology_phases(topology) {
                    let predecessor = phase_needs_predecessor(phase).then_some(&grant);
                    fixture.add_receipt(
                        directory.path(),
                        topology,
                        capability,
                        phase,
                        architecture,
                        os_version,
                        scenario,
                        &scenario_id,
                        1,
                        predecessor,
                    );
                }
            }
        }
    }
    for (scenario, topology) in mixed_scenarios() {
        for (architecture, os_version) in platform_cells() {
            let scenario_id = format!("mixed-{scenario:?}-{architecture}-{os_version}");
            for iteration in [1, 2] {
                fixture.add_receipt(
                    directory.path(),
                    topology,
                    MacosTccCanaryCapability::Pointer,
                    MacosTccCanaryLifecyclePhase::Grant,
                    architecture,
                    os_version,
                    scenario,
                    &scenario_id,
                    iteration,
                    None,
                );
            }
        }
    }

    directory
}

#[test]
fn full_signed_matrix_qualifies_preferred_sidecar_without_minting_acceptance() {
    let validation = validate_full_signed_matrix(None);
    assert!(validation.receipt_structure_valid);
    assert!(validation.identity_consistent);
    assert!(validation.preferred_topology_eligible);
    assert!(!validation.physical_acceptance_claimed);
    assert!(validation.missing_requirements.is_empty());
    assert_eq!(validation.capability_qualifications.len(), 4);
    assert!(
        validation
            .capability_qualifications
            .iter()
            .all(|qualification| {
                qualification.preferred_topology == Some(MacosDaemonOwner::AppSidecar)
                    && !qualification.app_broker_required
            })
    );
}

#[test]
fn every_lifecycle_phase_must_pass_in_every_native_platform_cell() {
    let directory = full_signed_matrix_directory(None);
    let (path, mut receipt) = receipt_matching(directory.path(), |receipt| {
        receipt.topology == MacosDaemonOwner::AppSidecar
            && receipt.scored_capability == MacosTccCanaryCapability::Keyboard
            && receipt.lifecycle_phase == MacosTccCanaryLifecyclePhase::OwnerRestart
            && receipt.host_architecture == "intel"
            && receipt.os_version == "26.0"
    });
    receipt.capabilities[0].outcome = MacosTccCanaryOutcome::Failed;
    receipt.capabilities[0].resulting_api_state = "failed".to_owned();
    write_json(&path, &receipt);

    let validation = validate_macos_tcc_canary_receipts(directory.path())
        .expect("mutated matrix should remain bounded");
    assert!(
        validation
            .missing_requirements
            .contains(&"appsidecar_keyboard_intel_tahoe_26_ownerrestart".to_owned())
    );
    assert!(!validation.preferred_topology_eligible);
}

#[test]
fn system_settings_witness_is_exact_current_row_process_evidence() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let receipt = MatrixFixture::default().add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Grant,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "settings-identity",
        1,
        None,
    );
    let witness_path = directory.path().join(format!(
        "{}.witness.json",
        receipt.system_settings_identity_witness_id
    ));
    let mut witness: MacosTccCanaryWitness =
        serde_json::from_slice(&fs::read(&witness_path).expect("settings witness should read"))
            .expect("settings witness should decode");
    witness.observed_unix_ms = receipt.process_started_unix_ms.saturating_sub(1);
    witness.system_settings_entry = Some("another process".to_owned());
    witness.observed_audit_token_identity =
        Some("00000000:00000000:00000000:00000000:00000000:ffffffff:00000000:00000001".to_owned());
    write_json(&witness_path, &witness);

    let validation = validate_macos_tcc_canary_receipts(directory.path())
        .expect("mutated witness should remain bounded");
    assert!(!validation.identity_consistent);
    assert!(
        validation
            .missing_requirements
            .contains(&"signed_launcher_identity".to_owned())
    );
}

#[test]
fn signing_identity_must_be_bound_to_the_observed_live_process() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let mut receipt = MatrixFixture::default().add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Grant,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "process-bound-signing",
        1,
        None,
    );
    receipt.signing.process_bound_pid = receipt.pid + 1;
    write_json(
        &directory
            .path()
            .join(format!("{}.receipt.json", receipt.row_id)),
        &receipt,
    );

    let validation = validate_macos_tcc_canary_receipts(directory.path())
        .expect("mutated receipt should remain bounded");
    assert!(!validation.identity_consistent);
}

#[test]
fn signing_identity_requires_a_successful_live_audit_token_check() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let mut receipt = MatrixFixture::default().add_receipt(
        directory.path(),
        MacosDaemonOwner::DirectLaunchd,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Grant,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::DirectLaunchdOnly,
        "audit-token-bound-signing",
        1,
        None,
    );
    receipt.signing.audit_token_bound_valid = false;
    write_json(
        &directory
            .path()
            .join(format!("{}.receipt.json", receipt.row_id)),
        &receipt,
    );

    let validation = validate_macos_tcc_canary_receipts(directory.path())
        .expect("mutated receipt should remain bounded");
    assert!(!validation.identity_consistent);
}

#[test]
fn app_parent_signing_must_be_bound_to_its_audit_token_process() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let mut receipt = MatrixFixture::default().add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Grant,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "parent-process-bound-signing",
        1,
        None,
    );
    receipt
        .launcher
        .parent_signing
        .as_mut()
        .expect("app parent signing should exist")
        .process_bound_pid = 2;
    write_json(
        &directory
            .path()
            .join(format!("{}.receipt.json", receipt.row_id)),
        &receipt,
    );

    let validation = validate_macos_tcc_canary_receipts(directory.path())
        .expect("mutated receipt should remain bounded");
    assert!(!validation.identity_consistent);
}

#[test]
fn app_parent_signing_requires_a_successful_live_audit_token_check() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let mut receipt = MatrixFixture::default().add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Grant,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "parent-audit-token-check",
        1,
        None,
    );
    receipt
        .launcher
        .parent_signing
        .as_mut()
        .expect("app parent signing should exist")
        .audit_token_bound_valid = false;
    write_json(
        &directory
            .path()
            .join(format!("{}.receipt.json", receipt.row_id)),
        &receipt,
    );

    let validation = validate_macos_tcc_canary_receipts(directory.path())
        .expect("mutated receipt should remain bounded");
    assert!(!validation.identity_consistent);
}

#[test]
fn app_parent_signing_observation_rejects_a_different_pidversion() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let receipt = MatrixFixture::default().add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Grant,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "parent-audit-token-bound-signing",
        1,
        None,
    );
    let witness_path = directory.path().join(format!(
        "{}.witness.json",
        receipt.system_settings_identity_witness_id
    ));
    let mut witness: MacosTccCanaryWitness =
        serde_json::from_slice(&fs::read(&witness_path).expect("settings witness should read"))
            .expect("settings witness should decode");
    witness.parent_signing_audit_token_identity =
        Some("00000000:00000000:00000000:00000000:00000000:00000001:00000000:00000002".to_owned());
    write_json(&witness_path, &witness);

    let validation = validate_macos_tcc_canary_receipts(directory.path())
        .expect("mutated witness should remain bounded");
    assert!(!validation.identity_consistent);
}

#[test]
fn failing_direct_grant_does_not_erase_sidecar_qualification() {
    let validation = validate_full_signed_matrix(Some((
        MacosDaemonOwner::DirectLaunchd,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Grant,
    )));
    let keyboard = validation
        .capability_qualifications
        .iter()
        .find(|qualification| qualification.capability == MacosTccCanaryCapability::Keyboard)
        .expect("keyboard qualification should exist");
    assert_eq!(
        keyboard.preferred_topology,
        Some(MacosDaemonOwner::AppSidecar)
    );
    assert!(
        keyboard
            .qualified_topologies
            .contains(&MacosDaemonOwner::AppSidecar)
    );
    assert!(
        !keyboard
            .qualified_topologies
            .contains(&MacosDaemonOwner::DirectLaunchd)
    );
    assert!(!keyboard.app_broker_required);
}

#[test]
fn a_single_signed_receipt_never_claims_physical_acceptance() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    MatrixFixture::default().add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Grant,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "one-row",
        1,
        None,
    );

    let validation =
        validate_macos_tcc_canary_receipts(directory.path()).expect("bounded receipt should parse");
    assert!(validation.receipt_structure_valid);
    assert!(!validation.preferred_topology_eligible);
    assert!(!validation.physical_acceptance_claimed);
    assert!(!validation.missing_requirements.is_empty());
}

#[test]
fn corrupted_witness_evidence_is_rejected_before_validation() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let evidence_dir = directory.path().join("evidence");
    fs::create_dir(&evidence_dir).expect("evidence directory should create");
    MatrixFixture::default().add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Grant,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "one-row",
        1,
        None,
    );
    let evidence_path = fs::read_dir(&evidence_dir)
        .expect("evidence directory should read")
        .next()
        .expect("evidence file should exist")
        .expect("evidence entry should read")
        .path();
    fs::write(evidence_path, b"corrupted witness evidence")
        .expect("evidence corruption should write");

    assert!(validate_macos_tcc_canary_receipts(directory.path()).is_err());
}

#[test]
fn receipt_identity_rejects_wrong_bundle_and_requirement_hash() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let mut receipt = MatrixFixture::default().add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Grant,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "identity-row",
        1,
        None,
    );
    receipt.signing.bundle_identifier = "tech.hyperbliss.hypercolor.daemon".to_owned();
    receipt.signing.designated_requirement_sha256 = "f".repeat(64);
    receipt.signing.authorities.clear();
    receipt.signing.entitlement_keys.pop();
    receipt.audit_token_identity =
        "00000000:00000000:00000000:00000000:00000000:ffffffff:00000000:00000000".to_owned();
    write_json(
        &directory
            .path()
            .join(format!("{}.receipt.json", receipt.row_id)),
        &receipt,
    );

    let validation =
        validate_macos_tcc_canary_receipts(directory.path()).expect("bounded receipt should parse");
    assert!(!validation.identity_consistent);
    assert!(!validation.preferred_topology_eligible);
    assert!(
        validation
            .missing_requirements
            .contains(&"signed_launcher_identity".to_owned())
    );
}

#[test]
fn lifecycle_links_reject_cross_context_predecessors_and_null_input_proof() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let mut fixture = MatrixFixture::default();
    let mut denied = fixture.add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Deny,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "denial-scenario",
        1,
        None,
    );
    let mut later = fixture.add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::LaterGrant,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "denial-scenario",
        1,
        Some(&denied),
    );
    later.scenario_id = "another-scenario".to_owned();
    later.capabilities[0].tap_mask = None;
    later.capabilities[0].redacted_event_count = None;
    denied.process_fingerprint = "e".repeat(64);
    write_json(
        &directory
            .path()
            .join(format!("{}.receipt.json", denied.row_id)),
        &denied,
    );
    write_json(
        &directory
            .path()
            .join(format!("{}.receipt.json", later.row_id)),
        &later,
    );

    let validation =
        validate_macos_tcc_canary_receipts(directory.path()).expect("receipts should parse");
    assert!(
        validation
            .missing_requirements
            .contains(&format!("{}_predecessor_context", later.row_id))
    );
    assert!(
        validation
            .missing_requirements
            .contains(&format!("{}_keyboard_operation", later.row_id))
    );
    assert!(
        validation
            .missing_requirements
            .contains(&format!("{}_process_replacement_witness", later.row_id))
    );
}

#[test]
fn lifecycle_requires_predecessor_completion_and_the_exact_launcher_action() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let mut fixture = MatrixFixture::default();
    let mut denied = fixture.add_receipt(
        directory.path(),
        MacosDaemonOwner::DirectLaunchd,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Deny,
        "intel",
        "26.0",
        MacosTccCanaryInstallationScenario::DirectLaunchdOnly,
        "ordered-replacement",
        1,
        None,
    );
    let later = fixture.add_receipt(
        directory.path(),
        MacosDaemonOwner::DirectLaunchd,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::LaterGrant,
        "intel",
        "26.0",
        MacosTccCanaryInstallationScenario::DirectLaunchdOnly,
        "ordered-replacement",
        1,
        Some(&denied),
    );
    denied.operation_finished_unix_ms = later.process_started_unix_ms + 1;
    write_json(
        &directory
            .path()
            .join(format!("{}.receipt.json", denied.row_id)),
        &denied,
    );
    let witness_path = directory.path().join(format!(
        "{}.witness.json",
        later
            .process_replacement_witness_id
            .as_deref()
            .expect("replacement witness should exist")
    ));
    let mut replacement: MacosTccCanaryWitness =
        serde_json::from_slice(&fs::read(&witness_path).expect("replacement witness should read"))
            .expect("replacement witness should decode");
    replacement.launcher_action = Some("launchctl_kickstart".to_owned());
    write_json(&witness_path, &replacement);

    let validation = validate_macos_tcc_canary_receipts(directory.path())
        .expect("mutated lifecycle should remain bounded");
    assert!(
        validation
            .missing_requirements
            .contains(&format!("{}_predecessor_chronology", later.row_id))
    );
    assert!(
        validation
            .missing_requirements
            .contains(&format!("{}_process_replacement_witness", later.row_id))
    );
}

#[test]
fn full_app_relaunch_requires_the_predecessor_app_to_exit() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let mut fixture = MatrixFixture::default();
    let grant = fixture.add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Grant,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "app-relaunch-parent",
        1,
        None,
    );
    let relaunch = fixture.add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::AppRelaunch,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "app-relaunch-parent",
        1,
        Some(&grant),
    );
    let witness_path = directory.path().join(format!(
        "{}.witness.json",
        relaunch
            .process_replacement_witness_id
            .as_deref()
            .expect("replacement witness should exist")
    ));
    let mut replacement: MacosTccCanaryWitness =
        serde_json::from_slice(&fs::read(&witness_path).expect("replacement witness should read"))
            .expect("replacement witness should decode");
    replacement.predecessor_parent_exit_observed = Some(false);
    write_json(&witness_path, &replacement);

    let validation = validate_macos_tcc_canary_receipts(directory.path())
        .expect("mutated lifecycle should remain bounded");
    assert!(
        validation
            .missing_requirements
            .contains(&format!("{}_process_replacement_witness", relaunch.row_id))
    );
}

#[test]
fn replacement_identity_allows_pid_reuse_when_pidversion_changes() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let mut fixture = MatrixFixture::default();
    let denied = fixture.add_receipt(
        directory.path(),
        MacosDaemonOwner::DirectLaunchd,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Deny,
        "intel",
        "26.0",
        MacosTccCanaryInstallationScenario::DirectLaunchdOnly,
        "pid-reuse",
        1,
        None,
    );
    let mut later = fixture.add_receipt(
        directory.path(),
        MacosDaemonOwner::DirectLaunchd,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::LaterGrant,
        "intel",
        "26.0",
        MacosTccCanaryInstallationScenario::DirectLaunchdOnly,
        "pid-reuse",
        1,
        Some(&denied),
    );
    later.pid = denied.pid;
    later.audit_token_identity = format!(
        "00000000:00000000:00000000:00000000:00000000:{:08x}:00000000:ffffffff",
        denied.pid
    );
    later.signing.process_bound_pid = denied.pid;
    write_json(
        &directory
            .path()
            .join(format!("{}.receipt.json", later.row_id)),
        &later,
    );
    let settings_path = directory.path().join(format!(
        "{}.witness.json",
        later.system_settings_identity_witness_id
    ));
    let mut settings: MacosTccCanaryWitness =
        serde_json::from_slice(&fs::read(&settings_path).expect("settings witness should read"))
            .expect("settings witness should decode");
    settings.observed_pid = Some(later.pid);
    settings.observed_audit_token_identity = Some(later.audit_token_identity.clone());
    write_json(&settings_path, &settings);

    let validation = validate_macos_tcc_canary_receipts(directory.path())
        .expect("PID-reuse receipts should remain bounded");
    assert!(
        !validation
            .missing_requirements
            .contains(&format!("{}_process_replacement", later.row_id))
    );
}

#[test]
fn rosetta_receipt_never_qualifies_an_apple_silicon_cell() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let mut receipt = MatrixFixture::default().add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Grant,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "rosetta-row",
        1,
        None,
    );
    receipt.executable_slice = "x86_64".to_owned();
    receipt.translated_process = true;
    write_json(
        &directory
            .path()
            .join(format!("{}.receipt.json", receipt.row_id)),
        &receipt,
    );

    let validation =
        validate_macos_tcc_canary_receipts(directory.path()).expect("receipt should parse");
    assert!(validation.receipt_structure_valid);
    assert!(
        validation
            .missing_requirements
            .contains(&"appsidecar_keyboard_apple_silicon_tahoe_26_grant".to_owned())
    );
}

#[test]
fn a_future_macos_major_does_not_substitute_for_tahoe_26() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    MatrixFixture::default().add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Grant,
        "apple_silicon",
        "27.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "future-major",
        1,
        None,
    );

    let validation = validate_macos_tcc_canary_receipts(directory.path())
        .expect("future-major receipt should remain bounded");
    assert!(
        validation
            .missing_requirements
            .contains(&"appsidecar_keyboard_apple_silicon_tahoe_26_grant".to_owned())
    );
}

#[test]
fn keyboard_receipt_requires_the_complete_requested_tap_mask() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let mut receipt = MatrixFixture::default().add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryLifecyclePhase::Grant,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "incomplete-mask",
        1,
        None,
    );
    receipt.capabilities[0].requested_tap_mask = Some(0b111);
    receipt.capabilities[0].tap_mask = Some(0b001);
    write_json(
        &directory
            .path()
            .join(format!("{}.receipt.json", receipt.row_id)),
        &receipt,
    );

    let validation =
        validate_macos_tcc_canary_receipts(directory.path()).expect("receipt should parse");
    assert!(
        validation
            .missing_requirements
            .contains(&format!("{}_keyboard_operation", receipt.row_id))
    );
    assert!(!validation.preferred_topology_eligible);
}

#[test]
fn stream_restart_receipt_requires_the_post_authorization_probe_shape() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let mut receipt = MatrixFixture::default().add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Stream,
        MacosTccCanaryLifecyclePhase::Grant,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppOnly,
        "stream-restart-row",
        1,
        None,
    );
    let stream = receipt
        .capabilities
        .iter_mut()
        .find(|evidence| evidence.capability == MacosTccCanaryCapability::Stream)
        .expect("stream evidence should exist");
    stream.outcome = MacosTccCanaryOutcome::NeedsProcessRestart;
    stream.resulting_api_state = "needs_process_restart".to_owned();
    stream.typed_error = Some("post_authorization_stream_requires_restart".to_owned());
    stream.tcc_request_result = Some(true);
    stream.tcc_preflight_after = Some(true);
    stream.picker_presented = Some(false);
    stream.picker_selected = Some(false);
    stream.stream_started = Some(false);
    stream.first_complete_frame = Some(false);
    stream.first_frame_monotonic_ns = None;
    let picker = receipt
        .capabilities
        .iter_mut()
        .find(|evidence| evidence.capability == MacosTccCanaryCapability::Picker)
        .expect("picker evidence should exist");
    picker.outcome = MacosTccCanaryOutcome::Failed;
    picker.resulting_api_state = "failed".to_owned();
    picker.typed_error = Some("stream_restart_required_before_picker".to_owned());
    picker.picker_presented = Some(false);
    picker.picker_selected = Some(false);
    write_json(
        &directory
            .path()
            .join(format!("{}.receipt.json", receipt.row_id)),
        &receipt,
    );

    let validation =
        validate_macos_tcc_canary_receipts(directory.path()).expect("receipt should parse");
    assert!(
        !validation
            .missing_requirements
            .contains(&format!("{}_process_restart_evidence", receipt.row_id))
    );

    stream_restart_receipt_mutation_is_rejected(directory.path(), receipt);
}

fn stream_restart_receipt_mutation_is_rejected(
    directory: &Path,
    mut receipt: MacosTccCanaryReceipt,
) {
    let path = directory.join(format!("{}.receipt.json", receipt.row_id));
    fs::remove_file(&path).expect("original receipt should remove");
    let picker = receipt
        .capabilities
        .iter_mut()
        .find(|evidence| evidence.capability == MacosTccCanaryCapability::Picker)
        .expect("picker evidence should exist");
    picker.outcome = MacosTccCanaryOutcome::Passed;
    "ready_idle".clone_into(&mut picker.resulting_api_state);
    picker.typed_error = None;
    picker.picker_presented = Some(true);
    picker.picker_selected = Some(true);
    write_json(&path, &receipt);

    let validation = validate_macos_tcc_canary_receipts(directory).expect("receipt should parse");
    assert!(
        validation
            .missing_requirements
            .contains(&format!("{}_process_restart_evidence", receipt.row_id))
    );
}

#[test]
fn mixed_installation_requires_login_bound_arbitration_evidence() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    fs::create_dir(directory.path().join("evidence")).expect("evidence directory should create");
    let receipt = MatrixFixture::default().add_receipt(
        directory.path(),
        MacosDaemonOwner::AppSidecar,
        MacosTccCanaryCapability::Pointer,
        MacosTccCanaryLifecyclePhase::Grant,
        "apple_silicon",
        "26.0",
        MacosTccCanaryInstallationScenario::AppDirectAppEnabledFirst,
        "mixed-login",
        1,
        None,
    );
    let witness_id = receipt
        .login_arbitration_witness_id
        .as_deref()
        .expect("mixed row should name a login witness");
    let witness_path = directory.path().join(format!("{witness_id}.witness.json"));
    let mut witness: MacosTccCanaryWitness =
        serde_json::from_slice(&fs::read(&witness_path).expect("witness should read"))
            .expect("witness should decode");
    witness.selected_topology = Some(MacosDaemonOwner::DirectLaunchd);
    write_json(&witness_path, &witness);

    let validation =
        validate_macos_tcc_canary_receipts(directory.path()).expect("receipt should parse");
    assert!(
        validation
            .missing_requirements
            .contains(&format!("{}_login_arbitration_witness", receipt.row_id))
    );
    assert!(validation.missing_requirements.contains(
        &"installation_appdirectappenabledfirst_apple_silicon_tahoe_26_repeated_login".to_owned()
    ));
}

fn evidence_for(
    capability: MacosTccCanaryCapability,
    outcome: MacosTccCanaryOutcome,
    phase: MacosTccCanaryLifecyclePhase,
) -> Vec<MacosTccCanaryCapabilityEvidence> {
    if capability == MacosTccCanaryCapability::Stream {
        let picker_outcome = if outcome == MacosTccCanaryOutcome::Denied {
            MacosTccCanaryOutcome::Denied
        } else {
            MacosTccCanaryOutcome::Passed
        };
        vec![
            evidence(MacosTccCanaryCapability::Picker, picker_outcome, phase),
            evidence(capability, outcome, phase),
        ]
    } else {
        vec![evidence(capability, outcome, phase)]
    }
}

fn evidence(
    capability: MacosTccCanaryCapability,
    outcome: MacosTccCanaryOutcome,
    phase: MacosTccCanaryLifecyclePhase,
) -> MacosTccCanaryCapabilityEvidence {
    let passed = outcome == MacosTccCanaryOutcome::Passed;
    let denied = outcome == MacosTccCanaryOutcome::Denied;
    let revoked = outcome == MacosTccCanaryOutcome::Revoked;
    let tcc_protected = capability != MacosTccCanaryCapability::Pointer;
    let persistent = matches!(
        phase,
        MacosTccCanaryLifecyclePhase::OwnerRestart
            | MacosTccCanaryLifecyclePhase::AppRelaunch
            | MacosTccCanaryLifecyclePhase::ServiceRestart
            | MacosTccCanaryLifecyclePhase::SignedUpdate
    );
    MacosTccCanaryCapabilityEvidence {
        capability,
        outcome,
        resulting_api_state: api_state(capability, outcome).to_owned(),
        typed_error: None,
        tcc_preflight_before: tcc_protected.then_some(persistent || revoked),
        tcc_request_result: (tcc_protected && phase == MacosTccCanaryLifecyclePhase::LaterGrant)
            .then_some(true),
        tcc_preflight_after: tcc_protected.then_some(passed),
        requested_tap_mask: matches!(
            capability,
            MacosTccCanaryCapability::Keyboard | MacosTccCanaryCapability::Pointer
        )
        .then_some(if capability == MacosTccCanaryCapability::Keyboard {
            1
        } else {
            2
        }),
        tap_mask: matches!(
            capability,
            MacosTccCanaryCapability::Keyboard | MacosTccCanaryCapability::Pointer
        )
        .then_some(if capability == MacosTccCanaryCapability::Keyboard {
            1
        } else {
            2
        }),
        tap_created: matches!(
            capability,
            MacosTccCanaryCapability::Keyboard | MacosTccCanaryCapability::Pointer
        )
        .then_some(passed),
        tap_enabled: matches!(
            capability,
            MacosTccCanaryCapability::Keyboard | MacosTccCanaryCapability::Pointer
        )
        .then_some(passed),
        run_loop_started: matches!(
            capability,
            MacosTccCanaryCapability::Keyboard | MacosTccCanaryCapability::Pointer
        )
        .then_some(passed),
        redacted_event_count: matches!(
            capability,
            MacosTccCanaryCapability::Keyboard | MacosTccCanaryCapability::Pointer
        )
        .then_some(u64::from(passed)),
        picker_presented: matches!(
            capability,
            MacosTccCanaryCapability::Picker | MacosTccCanaryCapability::Stream
        )
        .then_some(!denied),
        picker_selected: matches!(
            capability,
            MacosTccCanaryCapability::Picker | MacosTccCanaryCapability::Stream
        )
        .then_some(!denied),
        stream_started: (capability == MacosTccCanaryCapability::Stream)
            .then_some(passed || revoked),
        first_complete_frame: (capability == MacosTccCanaryCapability::Stream)
            .then_some(passed || revoked),
        first_frame_monotonic_ns: (capability == MacosTccCanaryCapability::Stream
            && (passed || revoked))
            .then_some(1_000_000),
        resource_live_before_revocation: (phase == MacosTccCanaryLifecyclePhase::RevokeWhileLive)
            .then_some(revoked),
        resource_failed_after_revocation: (phase == MacosTccCanaryLifecyclePhase::RevokeWhileLive)
            .then_some(revoked),
    }
}

fn witness(
    receipt: &MacosTccCanaryReceipt,
    witness_id: String,
    kind: MacosTccCanaryWitnessKind,
    predecessor: Option<&MacosTccCanaryReceipt>,
) -> MacosTccCanaryWitness {
    let evidence = format!("{RUN_ID}:{}:{witness_id}", receipt.row_id);
    let login_witness = kind == MacosTccCanaryWitnessKind::LoginArbitration;
    let settings_witness = kind == MacosTccCanaryWitnessKind::SystemSettingsIdentity;
    let replacement_witness = kind == MacosTccCanaryWitnessKind::ProcessReplacement;
    let lifecycle_witness = kind == MacosTccCanaryWitnessKind::LifecycleAction;
    let replaces_app_parent = replacement_witness
        && receipt.topology == MacosDaemonOwner::AppSidecar
        && matches!(
            receipt.lifecycle_phase,
            MacosTccCanaryLifecyclePhase::AppRelaunch | MacosTccCanaryLifecyclePhase::SignedUpdate
        );
    let installed_topologies =
        login_witness.then(|| scenario_topologies(receipt.installation_scenario).to_vec());
    let enable_order = login_witness.then(|| scenario_enable_order(receipt.installation_scenario));
    let losing_topologies = login_witness.then(|| {
        scenario_topologies(receipt.installation_scenario)
            .iter()
            .copied()
            .filter(|topology| *topology != receipt.topology)
            .collect()
    });
    MacosTccCanaryWitness {
        schema_version: MACOS_TCC_CANARY_SCHEMA_VERSION,
        run_id: RUN_ID.to_owned(),
        row_id: receipt.row_id.clone(),
        witness_id,
        kind,
        observer: "fixture-observer".to_owned(),
        observed_unix_ms: if settings_witness {
            receipt.process_started_unix_ms + 1
        } else if let Some(predecessor) = predecessor {
            predecessor.operation_finished_unix_ms + 1
        } else {
            receipt.process_started_unix_ms.saturating_sub(1)
        },
        evidence_sha256: hex_digest(evidence.as_bytes()),
        prompt_text: settings_witness.then(|| receipt.expected_prompt_text.clone()),
        system_settings_entry: settings_witness
            .then(|| receipt.expected_system_settings_entry.clone()),
        observed_pid: settings_witness.then_some(receipt.pid),
        observed_audit_token_identity: settings_witness
            .then(|| receipt.audit_token_identity.clone()),
        observed_signing_audit_token_identity: settings_witness
            .then(|| receipt.audit_token_identity.clone()),
        observed_cdhash: settings_witness.then(|| receipt.signing.cdhash.clone()),
        observed_designated_requirement_sha256: settings_witness
            .then(|| receipt.signing.designated_requirement_sha256.clone()),
        observed_process_fingerprint: settings_witness.then(|| receipt.process_fingerprint.clone()),
        parent_pid: (settings_witness && receipt.topology == MacosDaemonOwner::AppSidecar)
            .then_some(1),
        parent_audit_token_identity: (settings_witness
            && receipt.topology == MacosDaemonOwner::AppSidecar)
            .then(|| {
                "00000000:00000000:00000000:00000000:00000000:00000001:00000000:00000001".to_owned()
            }),
        parent_signing_audit_token_identity: (settings_witness
            && receipt.topology == MacosDaemonOwner::AppSidecar)
            .then(|| {
                "00000000:00000000:00000000:00000000:00000000:00000001:00000000:00000001".to_owned()
            }),
        parent_cdhash: (settings_witness && receipt.topology == MacosDaemonOwner::AppSidecar)
            .then(|| "c".repeat(40)),
        parent_designated_requirement_sha256: (settings_witness
            && receipt.topology == MacosDaemonOwner::AppSidecar)
            .then(|| {
                receipt
                    .launcher
                    .parent_signing
                    .as_ref()
                    .expect("app parent signing should exist")
                    .designated_requirement_sha256
                    .clone()
            }),
        parent_process_fingerprint: (settings_witness
            && receipt.topology == MacosDaemonOwner::AppSidecar)
            .then(|| "d".repeat(64)),
        fresh_tcc_database_observed: (kind == MacosTccCanaryWitnessKind::FreshTccReset)
            .then_some(true),
        predecessor_pid: predecessor.map(|receipt| receipt.pid),
        predecessor_audit_token_identity: predecessor
            .map(|receipt| receipt.audit_token_identity.clone()),
        predecessor_process_fingerprint: predecessor
            .map(|receipt| receipt.process_fingerprint.clone()),
        predecessor_exit_observed: predecessor.map(|_| true),
        predecessor_parent_pid: if replaces_app_parent {
            predecessor.and_then(|receipt| receipt.launcher.parent_pid)
        } else {
            None
        },
        predecessor_parent_audit_token_identity: replaces_app_parent.then(|| {
            "00000000:00000000:00000000:00000000:00000000:00000001:00000000:00000001".to_owned()
        }),
        predecessor_parent_process_fingerprint: replaces_app_parent.then(|| {
            predecessor
                .and_then(|receipt| receipt.launcher.parent_signing.as_ref())
                .expect("app predecessor parent signing should exist")
                .process_bound_fingerprint
                .clone()
        }),
        predecessor_parent_exit_observed: replaces_app_parent.then_some(true),
        launcher_action: (replacement_witness || lifecycle_witness).then(|| {
            expected_launcher_action(receipt.topology, receipt.lifecycle_phase).to_owned()
        }),
        installed_topologies,
        enable_order,
        selected_topology: login_witness.then_some(receipt.topology),
        losing_topologies,
        owner_conflict_observed: login_witness.then_some(true),
        login_iteration: login_witness.then_some(receipt.login_iteration),
        login_session_id: login_witness.then(|| {
            format!(
                "login-session-{}-{}",
                receipt.scenario_id, receipt.login_iteration
            )
        }),
    }
}

fn write_witness(directory: &Path, witness: MacosTccCanaryWitness) {
    let evidence = format!("{RUN_ID}:{}:{}", witness.row_id, witness.witness_id);
    assert_eq!(hex_digest(evidence.as_bytes()), witness.evidence_sha256);
    let evidence_path = directory
        .join("evidence")
        .join(format!("{}.bin", witness.evidence_sha256));
    if !evidence_path.exists() {
        fs::write(&evidence_path, evidence).expect("witness evidence should write");
    }
    write_json(
        &directory.join(format!("{}.witness.json", witness.witness_id)),
        &witness,
    );
}

fn write_json(path: &Path, value: &impl serde::Serialize) {
    fs::write(
        path,
        serde_json::to_vec_pretty(value).expect("JSON should encode"),
    )
    .expect("JSON should write");
}

fn receipt_matching(
    directory: &Path,
    predicate: impl Fn(&MacosTccCanaryReceipt) -> bool,
) -> (PathBuf, MacosTccCanaryReceipt) {
    fs::read_dir(directory)
        .expect("fixture directory should read")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".receipt.json"))
        })
        .find_map(|path| {
            let receipt = serde_json::from_slice::<MacosTccCanaryReceipt>(
                &fs::read(&path).expect("receipt should read"),
            )
            .expect("receipt should decode");
            predicate(&receipt).then_some((path, receipt))
        })
        .expect("matching fixture receipt should exist")
}

fn launcher(topology: MacosDaemonOwner) -> MacosTccCanaryLauncherEvidence {
    let (actual_launcher, expected_label, parent_signing) = match topology {
        MacosDaemonOwner::AppSidecar => (
            "packaged_app_supervisor",
            None,
            Some(app_signing(1, &"d".repeat(64))),
        ),
        MacosDaemonOwner::DirectLaunchd => (
            "direct_launchd",
            Some("tech.hyperbliss.hypercolor".to_owned()),
            None,
        ),
        MacosDaemonOwner::Homebrew => (
            "homebrew_services",
            Some("homebrew.mxcl.hypercolor".to_owned()),
            None,
        ),
        MacosDaemonOwner::Standalone => ("terminal_parent", None, None),
    };
    MacosTccCanaryLauncherEvidence {
        actual_launcher: actual_launcher.to_owned(),
        expected_label,
        parent_pid: Some(1),
        parent_executable_path: Some(if topology == MacosDaemonOwner::Standalone {
            PathBuf::from("/bin/zsh")
        } else {
            PathBuf::from("/Applications/Hypercolor.app/Contents/MacOS/Hypercolor")
        }),
        parent_signing,
        launchctl_pid_matches: Some(true),
        verified: true,
    }
}

fn app_signing(pid: u32, process_fingerprint: &str) -> MacosTccCanarySigningEvidence {
    let designated_requirement = "identifier tech.hyperbliss.hypercolor and anchor apple generic";
    MacosTccCanarySigningEvidence {
        bundle_identifier: "tech.hyperbliss.hypercolor".to_owned(),
        team_identifier: "TEAMID1234".to_owned(),
        designated_requirement: designated_requirement.to_owned(),
        designated_requirement_sha256: hex_digest(designated_requirement.as_bytes()),
        cdhash: "c".repeat(40),
        process_bound_pid: pid,
        process_bound_fingerprint: process_fingerprint.to_owned(),
        process_bound_valid: true,
        audit_token_bound_valid: true,
        authorities: vec!["Developer ID Application: Hypercolor (TEAMID1234)".to_owned()],
        entitlement_keys: required_entitlements(),
        codesign_strict_valid: true,
        hardened_runtime: true,
        secure_timestamp: true,
        spctl_accepted: true,
    }
}

fn signing(
    topology: MacosDaemonOwner,
    signed_update: bool,
    pid: u32,
    process_fingerprint: &str,
) -> MacosTccCanarySigningEvidence {
    let designated_requirement = match topology {
        MacosDaemonOwner::AppSidecar => {
            "identifier tech.hyperbliss.hypercolor.sidecar and anchor apple generic"
        }
        MacosDaemonOwner::DirectLaunchd
        | MacosDaemonOwner::Homebrew
        | MacosDaemonOwner::Standalone => {
            "identifier tech.hyperbliss.hypercolor.daemon and anchor apple generic"
        }
    };
    MacosTccCanarySigningEvidence {
        bundle_identifier: match topology {
            MacosDaemonOwner::AppSidecar => "tech.hyperbliss.hypercolor.sidecar",
            MacosDaemonOwner::DirectLaunchd
            | MacosDaemonOwner::Homebrew
            | MacosDaemonOwner::Standalone => "tech.hyperbliss.hypercolor.daemon",
        }
        .to_owned(),
        team_identifier: "TEAMID1234".to_owned(),
        designated_requirement: designated_requirement.to_owned(),
        designated_requirement_sha256: hex_digest(designated_requirement.as_bytes()),
        cdhash: if signed_update { "b" } else { "a" }.repeat(40),
        process_bound_pid: pid,
        process_bound_fingerprint: process_fingerprint.to_owned(),
        process_bound_valid: true,
        audit_token_bound_valid: true,
        authorities: vec!["Developer ID Application: Hypercolor (TEAMID1234)".to_owned()],
        entitlement_keys: required_entitlements(),
        codesign_strict_valid: true,
        hardened_runtime: true,
        secure_timestamp: true,
        spctl_accepted: true,
    }
}

fn expected_outcome(phase: MacosTccCanaryLifecyclePhase) -> MacosTccCanaryOutcome {
    match phase {
        MacosTccCanaryLifecyclePhase::Deny => MacosTccCanaryOutcome::Denied,
        MacosTccCanaryLifecyclePhase::RevokeWhileLive => MacosTccCanaryOutcome::Revoked,
        _ => MacosTccCanaryOutcome::Passed,
    }
}

fn api_state(capability: MacosTccCanaryCapability, outcome: MacosTccCanaryOutcome) -> &'static str {
    match outcome {
        MacosTccCanaryOutcome::Passed if capability == MacosTccCanaryCapability::Picker => {
            "ready_idle"
        }
        MacosTccCanaryOutcome::Passed => "live",
        MacosTccCanaryOutcome::Denied => "permission_denied",
        MacosTccCanaryOutcome::Revoked => "revoked",
        MacosTccCanaryOutcome::NeedsProcessRestart => "needs_process_restart",
        MacosTccCanaryOutcome::Cancelled => "needs_selection",
        MacosTccCanaryOutcome::TimedOut => "interrupted",
        MacosTccCanaryOutcome::Failed => "failed",
    }
}

fn topologies() -> [MacosDaemonOwner; 4] {
    [
        MacosDaemonOwner::AppSidecar,
        MacosDaemonOwner::DirectLaunchd,
        MacosDaemonOwner::Homebrew,
        MacosDaemonOwner::Standalone,
    ]
}

fn capabilities() -> [MacosTccCanaryCapability; 4] {
    [
        MacosTccCanaryCapability::Keyboard,
        MacosTccCanaryCapability::Pointer,
        MacosTccCanaryCapability::Picker,
        MacosTccCanaryCapability::Stream,
    ]
}

fn single_scenario(topology: MacosDaemonOwner) -> MacosTccCanaryInstallationScenario {
    match topology {
        MacosDaemonOwner::AppSidecar => MacosTccCanaryInstallationScenario::AppOnly,
        MacosDaemonOwner::DirectLaunchd => MacosTccCanaryInstallationScenario::DirectLaunchdOnly,
        MacosDaemonOwner::Homebrew => MacosTccCanaryInstallationScenario::HomebrewOnly,
        MacosDaemonOwner::Standalone => MacosTccCanaryInstallationScenario::StandaloneOnly,
    }
}

fn topology_phases(topology: MacosDaemonOwner) -> Vec<MacosTccCanaryLifecyclePhase> {
    match topology {
        MacosDaemonOwner::AppSidecar => vec![
            MacosTccCanaryLifecyclePhase::AppLaunch,
            MacosTccCanaryLifecyclePhase::OwnerRestart,
            MacosTccCanaryLifecyclePhase::AppRelaunch,
            MacosTccCanaryLifecyclePhase::SignedUpdate,
        ],
        MacosDaemonOwner::DirectLaunchd | MacosDaemonOwner::Homebrew => vec![
            MacosTccCanaryLifecyclePhase::ServiceInstall,
            MacosTccCanaryLifecyclePhase::LoginStart,
            MacosTccCanaryLifecyclePhase::ServiceRestart,
            MacosTccCanaryLifecyclePhase::SignedUpdate,
        ],
        MacosDaemonOwner::Standalone => vec![MacosTccCanaryLifecyclePhase::SignedUpdate],
    }
}

fn phase_needs_predecessor(phase: MacosTccCanaryLifecyclePhase) -> bool {
    matches!(
        phase,
        MacosTccCanaryLifecyclePhase::LaterGrant
            | MacosTccCanaryLifecyclePhase::GrantAfterRevocation
            | MacosTccCanaryLifecyclePhase::OwnerRestart
            | MacosTccCanaryLifecyclePhase::AppRelaunch
            | MacosTccCanaryLifecyclePhase::ServiceRestart
            | MacosTccCanaryLifecyclePhase::SignedUpdate
    )
}

fn phase_replaces_process(phase: MacosTccCanaryLifecyclePhase) -> bool {
    phase_needs_predecessor(phase)
}

fn phase_needs_lifecycle_action_witness(phase: MacosTccCanaryLifecyclePhase) -> bool {
    matches!(
        phase,
        MacosTccCanaryLifecyclePhase::AppLaunch
            | MacosTccCanaryLifecyclePhase::ServiceInstall
            | MacosTccCanaryLifecyclePhase::LoginStart
    )
}

fn expected_launcher_action(
    topology: MacosDaemonOwner,
    phase: MacosTccCanaryLifecyclePhase,
) -> &'static str {
    match (topology, phase) {
        (MacosDaemonOwner::AppSidecar, MacosTccCanaryLifecyclePhase::AppLaunch) => {
            "app_minimized_launch"
        }
        (MacosDaemonOwner::AppSidecar, MacosTccCanaryLifecyclePhase::OwnerRestart) => {
            "app_supervisor_daemon_restart"
        }
        (MacosDaemonOwner::AppSidecar, MacosTccCanaryLifecyclePhase::AppRelaunch) => {
            "app_quit_then_minimized_launch"
        }
        (
            MacosDaemonOwner::AppSidecar,
            MacosTccCanaryLifecyclePhase::LaterGrant
            | MacosTccCanaryLifecyclePhase::GrantAfterRevocation,
        ) => "app_supervisor_daemon_restart_after_authorization",
        (MacosDaemonOwner::AppSidecar, MacosTccCanaryLifecyclePhase::SignedUpdate) => {
            "signed_app_update_then_app_relaunch"
        }
        (MacosDaemonOwner::DirectLaunchd, MacosTccCanaryLifecyclePhase::ServiceInstall) => {
            "hypercolor_service_enable"
        }
        (MacosDaemonOwner::DirectLaunchd, MacosTccCanaryLifecyclePhase::LoginStart) => {
            "launchd_login_start"
        }
        (MacosDaemonOwner::DirectLaunchd, MacosTccCanaryLifecyclePhase::ServiceRestart) => {
            "hypercolor_service_restart"
        }
        (
            MacosDaemonOwner::DirectLaunchd,
            MacosTccCanaryLifecyclePhase::LaterGrant
            | MacosTccCanaryLifecyclePhase::GrantAfterRevocation,
        ) => "hypercolor_service_restart_after_authorization",
        (MacosDaemonOwner::DirectLaunchd, MacosTccCanaryLifecyclePhase::SignedUpdate) => {
            "signed_daemon_update_then_hypercolor_service_restart"
        }
        (MacosDaemonOwner::Homebrew, MacosTccCanaryLifecyclePhase::ServiceInstall) => {
            "brew_services_start"
        }
        (MacosDaemonOwner::Homebrew, MacosTccCanaryLifecyclePhase::LoginStart) => {
            "brew_services_login_start"
        }
        (MacosDaemonOwner::Homebrew, MacosTccCanaryLifecyclePhase::ServiceRestart) => {
            "brew_services_restart"
        }
        (
            MacosDaemonOwner::Homebrew,
            MacosTccCanaryLifecyclePhase::LaterGrant
            | MacosTccCanaryLifecyclePhase::GrantAfterRevocation,
        ) => "brew_services_restart_after_authorization",
        (MacosDaemonOwner::Homebrew, MacosTccCanaryLifecyclePhase::SignedUpdate) => {
            "signed_daemon_update_then_brew_services_restart"
        }
        (
            MacosDaemonOwner::Standalone,
            MacosTccCanaryLifecyclePhase::LaterGrant
            | MacosTccCanaryLifecyclePhase::GrantAfterRevocation,
        ) => "terminal_successor_launch_after_authorization",
        (MacosDaemonOwner::Standalone, MacosTccCanaryLifecyclePhase::SignedUpdate) => {
            "signed_daemon_update_then_terminal_launch"
        }
        _ => panic!("fixture requested an action for an inapplicable lifecycle phase"),
    }
}

fn platform_cells() -> [(&'static str, &'static str); 4] {
    [
        ("apple_silicon", "15.2"),
        ("apple_silicon", "26.0"),
        ("intel", "15.2"),
        ("intel", "26.0"),
    ]
}

fn scenario_needs_login_witness(scenario: MacosTccCanaryInstallationScenario) -> bool {
    !matches!(
        scenario,
        MacosTccCanaryInstallationScenario::AppOnly
            | MacosTccCanaryInstallationScenario::DirectLaunchdOnly
            | MacosTccCanaryInstallationScenario::HomebrewOnly
            | MacosTccCanaryInstallationScenario::StandaloneOnly
    )
}

fn scenario_topologies(
    scenario: MacosTccCanaryInstallationScenario,
) -> &'static [MacosDaemonOwner] {
    match scenario {
        MacosTccCanaryInstallationScenario::AppOnly => &[MacosDaemonOwner::AppSidecar],
        MacosTccCanaryInstallationScenario::DirectLaunchdOnly => &[MacosDaemonOwner::DirectLaunchd],
        MacosTccCanaryInstallationScenario::HomebrewOnly => &[MacosDaemonOwner::Homebrew],
        MacosTccCanaryInstallationScenario::StandaloneOnly => &[MacosDaemonOwner::Standalone],
        MacosTccCanaryInstallationScenario::AppDirectAppEnabledFirst
        | MacosTccCanaryInstallationScenario::AppDirectDirectEnabledFirst => &[
            MacosDaemonOwner::AppSidecar,
            MacosDaemonOwner::DirectLaunchd,
        ],
        MacosTccCanaryInstallationScenario::AppHomebrew => {
            &[MacosDaemonOwner::AppSidecar, MacosDaemonOwner::Homebrew]
        }
        MacosTccCanaryInstallationScenario::DirectHomebrew => {
            &[MacosDaemonOwner::DirectLaunchd, MacosDaemonOwner::Homebrew]
        }
        MacosTccCanaryInstallationScenario::AppDirectHomebrew => &[
            MacosDaemonOwner::AppSidecar,
            MacosDaemonOwner::DirectLaunchd,
            MacosDaemonOwner::Homebrew,
        ],
    }
}

fn scenario_enable_order(scenario: MacosTccCanaryInstallationScenario) -> Vec<MacosDaemonOwner> {
    match scenario {
        MacosTccCanaryInstallationScenario::AppDirectDirectEnabledFirst => vec![
            MacosDaemonOwner::DirectLaunchd,
            MacosDaemonOwner::AppSidecar,
        ],
        _ => scenario_topologies(scenario).to_vec(),
    }
}

fn required_entitlements() -> Vec<String> {
    [
        "com.apple.security.cs.allow-jit",
        "com.apple.security.cs.allow-unsigned-executable-memory",
        "com.apple.security.device.audio-input",
        "com.apple.security.device.usb",
        "com.apple.security.network.client",
        "com.apple.security.network.server",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn mixed_scenarios() -> [(MacosTccCanaryInstallationScenario, MacosDaemonOwner); 5] {
    [
        (
            MacosTccCanaryInstallationScenario::AppDirectAppEnabledFirst,
            MacosDaemonOwner::AppSidecar,
        ),
        (
            MacosTccCanaryInstallationScenario::AppDirectDirectEnabledFirst,
            MacosDaemonOwner::DirectLaunchd,
        ),
        (
            MacosTccCanaryInstallationScenario::AppHomebrew,
            MacosDaemonOwner::AppSidecar,
        ),
        (
            MacosTccCanaryInstallationScenario::DirectHomebrew,
            MacosDaemonOwner::DirectLaunchd,
        ),
        (
            MacosTccCanaryInstallationScenario::AppDirectHomebrew,
            MacosDaemonOwner::Homebrew,
        ),
    ]
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(&mut output, "{byte:02x}").expect("writing to a string should succeed");
            output
        })
}
