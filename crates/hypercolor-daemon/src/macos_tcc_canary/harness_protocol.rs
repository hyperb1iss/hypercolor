#![cfg(feature = "screen-capture")]

#[cfg(feature = "screen-capture")]
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[cfg(feature = "screen-capture")]
use anyhow::{Context, Result};
#[cfg(feature = "screen-capture")]
use hypercolor_macos_input::current_process_audit_token_identity;
#[cfg(feature = "screen-capture")]
use hypercolor_macos_owner::MacosDaemonOwner;

#[cfg(feature = "screen-capture")]
use super::{
    artifacts::{
        claim_request, ensure_canary_descendant_directory, macos_tcc_canary_directory,
        read_json_bounded, sync_parent, witness_evidence_matches, write_json_new,
    },
    identity::{
        bounded_command_text, host_architecture, inspect_launcher, inspect_signing,
        live_signing_identity_is_valid, process_fingerprint, sysctl_flag, unix_time_ms,
    },
    model::{MACOS_TCC_CANARY_SCHEMA_VERSION, MAX_WITNESS_BYTES, MacosTccCanaryRequest},
    receipts::{MacosTccCanaryReceipt, MacosTccCanaryWitness},
    rows::execute_capabilities,
    validation::{receipt_identity_valid, validate_witness_structure},
};

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
