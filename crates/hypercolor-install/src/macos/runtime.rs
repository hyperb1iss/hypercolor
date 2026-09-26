use super::super::{InstallPlatformError, PlatformCheckpoint, PlatformState};
use super::model::{
    MacosOwnerReceipt, MacosRecord, MacosRuntimeExecutable, MacosRuntimeTransition,
    MacosStopAuthority, MacosUnitBinding, error,
};
use hypercolor_macos_owner::MacosOwnerRecord;

pub(super) fn effect_destination(checkpoint: PlatformCheckpoint) -> Option<PlatformCheckpoint> {
    match checkpoint {
        PlatformCheckpoint::PriorOriginal => Some(PlatformCheckpoint::PriorUnloaded),
        PlatformCheckpoint::CandidateAutostart => Some(PlatformCheckpoint::CandidateRuntime),
        PlatformCheckpoint::CandidateRuntime => Some(PlatformCheckpoint::CandidateAutostart),
        PlatformCheckpoint::PriorAutostart => Some(PlatformCheckpoint::PriorRestored),
        _ => None,
    }
}

pub(super) fn transition(
    checkpoint: PlatformCheckpoint,
    expected: &PlatformState,
    record: &MacosRecord,
    receipt: Option<&MacosOwnerReceipt>,
) -> Result<Option<MacosRuntimeTransition>, InstallPlatformError> {
    match checkpoint {
        PlatformCheckpoint::PriorUnloaded => {
            require_inactive(expected)?;
            Ok(record
                .baseline_stop_authority
                .clone()
                .map(|authority| MacosRuntimeTransition::Stop { authority }))
        }
        PlatformCheckpoint::CandidateRuntime => {
            require_running(expected, &record.candidate.unit)?;
            let snapshot = record
                .candidate_launcher_snapshot
                .clone()
                .ok_or_else(|| error("candidate runtime lacks its private launcher snapshot"))?;
            Ok(Some(MacosRuntimeTransition::Start {
                executable: runtime_executable(&record.candidate),
                launcher_snapshot: snapshot,
                after_epoch: record.baseline_owner_epoch.unwrap_or_default(),
            }))
        }
        PlatformCheckpoint::CandidateAutostart => {
            require_inactive(expected)?;
            let authority = receipt
                .map(stop_authority)
                .ok_or_else(|| error("candidate stop lacks its durable owner receipt"))?;
            Ok(Some(MacosRuntimeTransition::Stop { authority }))
        }
        PlatformCheckpoint::PriorRestored => {
            let Some(prior) = &record.prior else {
                require_inactive(expected)?;
                return Ok(None);
            };
            require_running(expected, &prior.unit)?;
            let snapshot = record
                .prior_launcher_snapshot
                .clone()
                .ok_or_else(|| error("prior runtime lacks its private launcher snapshot"))?;
            Ok(Some(MacosRuntimeTransition::Start {
                executable: runtime_executable(prior),
                launcher_snapshot: snapshot,
                after_epoch: receipt
                    .map_or(record.baseline_owner_epoch.unwrap_or_default(), |receipt| {
                        receipt.owner_epoch
                    }),
            }))
        }
        _ => Err(error(
            "macOS runtime effect received an invalid destination checkpoint",
        )),
    }
}

pub(super) fn owner_matches_stop_authority(
    owner: &MacosOwnerRecord,
    authority: &MacosStopAuthority,
) -> bool {
    owner.owner_epoch == authority.owner_epoch
        && owner.active_identity.audit_token_identity == authority.audit_token_identity
        && owner.active_identity.executable_path == authority.executable_path
        && owner.active_identity.designated_requirement_hash
            == authority.designated_requirement_hash
        && owner.active_identity.pid == authority.pid
}

pub(super) fn owner_matches_binding(owner: &MacosOwnerRecord, binding: &MacosUnitBinding) -> bool {
    owner.active_identity.executable_path == std::path::Path::new(&binding.daemon_path)
        && owner.active_identity.designated_requirement_hash
            == binding.designated_requirement_sha256
}

pub(super) fn owner_matches_receipt(owner: &MacosOwnerRecord, receipt: &MacosOwnerReceipt) -> bool {
    owner.owner_epoch == receipt.owner_epoch
        && owner.active_identity.audit_token_identity == receipt.audit_token_identity
        && owner.active_identity.executable_path == receipt.executable_path
        && owner.active_identity.designated_requirement_hash == receipt.designated_requirement_hash
        && owner.active_identity.pid == receipt.pid
}

fn runtime_executable(binding: &MacosUnitBinding) -> MacosRuntimeExecutable {
    MacosRuntimeExecutable {
        unit: binding.unit.clone(),
        path: binding.daemon_path.clone(),
        sha256: binding.daemon_sha256.clone(),
        size: binding.daemon_size,
        mode: binding.daemon_mode,
        device: binding.daemon_device,
        inode: binding.daemon_inode,
        designated_requirement: binding.designated_requirement.clone(),
        designated_requirement_sha256: binding.designated_requirement_sha256.clone(),
        cdhash: binding.cdhash.clone(),
        synthetic_legacy: binding.synthetic_legacy,
    }
}

fn stop_authority(receipt: &MacosOwnerReceipt) -> MacosStopAuthority {
    MacosStopAuthority {
        owner_epoch: receipt.owner_epoch,
        audit_token_identity: receipt.audit_token_identity.clone(),
        executable_path: receipt.executable_path.clone(),
        designated_requirement_hash: receipt.designated_requirement_hash.clone(),
        pid: receipt.pid,
        unit: receipt.unit.clone(),
    }
}

fn require_inactive(expected: &PlatformState) -> Result<(), InstallPlatformError> {
    if expected.loaded || expected.running_unit.is_some() {
        return Err(error("macOS stop destination is not quiescent"));
    }
    Ok(())
}

fn require_running(
    expected: &PlatformState,
    unit: &super::super::UnitId,
) -> Result<(), InstallPlatformError> {
    if !expected.loaded || expected.running_unit.as_ref() != Some(unit) {
        return Err(error(
            "macOS start destination is not the exact retained unit",
        ));
    }
    Ok(())
}
