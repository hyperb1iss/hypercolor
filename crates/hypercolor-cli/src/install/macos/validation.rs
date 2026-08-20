use std::collections::BTreeSet;

use super::super::{
    InstallPlatformError, PlatformOwnerReceipt, PlatformTransactionRecord, UnitRecord,
};
use super::MacosInstallPlatform;
use super::executor::MacosInstallExecutor;
use super::model::{
    MAX_LAUNCHER_BYTES, MacosDirectoryState, MacosExactEntry, MacosFilePublication,
    MacosLauncherSnapshot, MacosLayoutEffect, MacosOwnerReceipt, MacosRecord,
    compare_directory_paths, error, hex_digest, is_sha256, launcher_snapshot_id,
    validate_candidate_layout, validate_public_path,
};
use super::record::{decode_receipt, decode_record};

impl<E: MacosInstallExecutor> MacosInstallPlatform<E> {
    pub(super) fn validated_record(
        &mut self,
        encoded: &PlatformTransactionRecord,
    ) -> Result<MacosRecord, InstallPlatformError> {
        let record = decode_record(encoded)?;
        self.validate_record(&record)?;
        Ok(record)
    }

    pub(super) fn validated_receipt(
        &self,
        record: &MacosRecord,
        encoded: Option<&PlatformOwnerReceipt>,
    ) -> Result<Option<MacosOwnerReceipt>, InstallPlatformError> {
        let receipt = decode_receipt(encoded)?;
        if let Some(receipt) = &receipt
            && (receipt.unit != record.candidate.unit
                || receipt.owner_epoch <= record.baseline_owner_epoch.unwrap_or_default()
                || receipt.pid == 0
                || receipt.audit_token_identity.is_empty()
                || receipt.audit_token_identity.len() > 256
                || receipt.executable_path != std::path::Path::new(&record.candidate.daemon_path)
                || receipt.designated_requirement_hash
                    != record.candidate.designated_requirement_sha256)
        {
            return Err(error("macOS owner receipt is not bound to the candidate"));
        }
        Ok(receipt)
    }

    fn validate_record(&mut self, record: &MacosRecord) -> Result<(), InstallPlatformError> {
        record.baseline_launchd.validate()?;
        validate_prior_launcher(
            &record.prior_launcher,
            record.prior_launcher_bytes.as_bytes(),
        )?;
        if let Some(launcher) = &record.candidate_launcher
            && launcher != &self.candidate_launcher()?
        {
            return Err(error("candidate macOS launcher binding is not exact"));
        }
        self.validate_binding(&record.candidate, false, record)?;
        if let Some(prior) = &record.prior {
            self.validate_binding(prior, true, record)?;
        }
        if record.first_conversion
            != record
                .prior
                .as_ref()
                .is_some_and(|prior| prior.synthetic_legacy)
        {
            return Err(error("macOS first-conversion marker is inconsistent"));
        }
        if record.baseline_launchd.pid.is_some() != record.baseline_owner_epoch.is_some() {
            return Err(error(
                "macOS baseline owner epoch is inconsistent with runtime",
            ));
        }
        self.validate_stop_authority(record)?;
        self.validate_launcher_snapshots(record)?;
        self.validate_layout_plan(record)
    }

    fn validate_stop_authority(&self, record: &MacosRecord) -> Result<(), InstallPlatformError> {
        let Some(authority) = &record.baseline_stop_authority else {
            if record.baseline_launchd.pid.is_some() {
                return Err(error("running macOS baseline lacks stop authority"));
            }
            return Ok(());
        };
        let prior = record
            .prior
            .as_ref()
            .ok_or_else(|| error("macOS baseline stop authority lacks a prior binding"))?;
        if record.baseline_launchd.pid != Some(authority.pid)
            || record.baseline_owner_epoch != Some(authority.owner_epoch)
            || authority.pid == 0
            || authority.owner_epoch == 0
            || authority.audit_token_identity.is_empty()
            || authority.audit_token_identity.len() > 256
            || authority.unit != prior.unit
            || authority.executable_path != std::path::Path::new(&prior.daemon_path)
            || authority.designated_requirement_hash != prior.designated_requirement_sha256
        {
            return Err(error("macOS baseline stop authority is not exact"));
        }
        Ok(())
    }

    fn validate_launcher_snapshots(
        &mut self,
        record: &MacosRecord,
    ) -> Result<(), InstallPlatformError> {
        match (
            &record.candidate_launcher,
            &record.candidate_launcher_snapshot,
        ) {
            (Some(launcher), Some(snapshot)) => {
                let publication = MacosFilePublication {
                    mode: launcher.mode,
                    contents: launcher.bytes.as_bytes().to_vec(),
                };
                validate_launcher_snapshot(&publication, snapshot)?;
                self.executor
                    .validate_launcher_snapshot(&publication, snapshot)?;
            }
            (None, None) => {}
            _ => return Err(error("candidate macOS launcher snapshot is inconsistent")),
        }
        let prior_publication = match &record.prior_launcher {
            MacosExactEntry::RegularFile { mode, .. } => Some(MacosFilePublication {
                mode: *mode,
                contents: record.prior_launcher_bytes.as_bytes().to_vec(),
            }),
            MacosExactEntry::Absent | MacosExactEntry::Symlink { .. } => None,
        };
        match (prior_publication, &record.prior_launcher_snapshot) {
            (Some(publication), Some(snapshot)) => {
                validate_launcher_snapshot(&publication, snapshot)?;
                let prior = record
                    .prior
                    .as_ref()
                    .ok_or_else(|| error("prior launcher snapshot lacks a retained unit"))?;
                self.retained_unit(&prior.unit)?;
                self.executor
                    .validate_launcher_snapshot(&publication, snapshot)?;
            }
            (None, None) => {}
            _ => return Err(error("prior macOS launcher snapshot is inconsistent")),
        }
        Ok(())
    }

    fn validate_binding(
        &mut self,
        binding: &super::model::MacosUnitBinding,
        prior: bool,
        record: &MacosRecord,
    ) -> Result<(), InstallPlatformError> {
        if !is_sha256(&binding.daemon_sha256)
            || !is_sha256(&binding.designated_requirement_sha256)
            || binding.daemon_size == 0
            || binding.daemon_mode > 0o777
            || binding.daemon_device == 0
            || binding.daemon_inode == 0
            || binding.version.is_empty()
            || binding.version.len() > 128
        {
            return Err(error("macOS unit binding is malformed"));
        }
        if (binding.synthetic_legacy
            && (binding.daemon_mode & 0o100 == 0 || binding.daemon_mode & 0o022 != 0))
            || (!binding.synthetic_legacy && binding.daemon_mode & 0o222 != 0)
        {
            return Err(error("macOS unit binding has an unsafe executable mode"));
        }
        let unit = self.retained_unit(&binding.unit)?.clone();
        if binding.synthetic_legacy {
            if !prior || !binding.unit.as_str().starts_with("legacy-") {
                return Err(error("macOS synthetic binding is not an exact prior unit"));
            }
            super::legacy::validate_legacy_unit_id(&unit)?;
            self.executor.validate_legacy_snapshot(
                &unit,
                &super::model::MacosLegacyExecutable {
                    path: binding.daemon_path.clone(),
                    sha256: binding.daemon_sha256.clone(),
                    size: binding.daemon_size,
                    mode: binding.daemon_mode,
                    device: binding.daemon_device,
                    inode: binding.daemon_inode,
                    designated_requirement: binding.designated_requirement.clone(),
                    designated_requirement_sha256: binding.designated_requirement_sha256.clone(),
                    version: binding.version.clone(),
                },
                &record.prior_launcher,
                record.prior_launcher_bytes.as_bytes(),
                &record.prior_entries,
            )?;
        } else if &self.unit_binding(&unit)? != binding {
            return Err(error("macOS unit binding changed from retained authority"));
        }
        Ok(())
    }

    fn validate_layout_plan(&mut self, record: &MacosRecord) -> Result<(), InstallPlatformError> {
        let projection = self.projection(&record.candidate.unit)?.clone();
        validate_candidate_layout(&projection)?;
        let mut expected_directories = record
            .prior_directories
            .iter()
            .filter_map(|(path, state)| (*state == MacosDirectoryState::Absent).then_some(path))
            .cloned()
            .collect::<Vec<_>>();
        expected_directories.sort_by(|left, right| compare_directory_paths(left, right));
        let projected_targets = projection
            .entries
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut entry_paths = projected_targets.keys().cloned().collect::<BTreeSet<_>>();
        for path in record.prior_entries.keys() {
            entry_paths.insert(path.clone());
        }
        let mut index = 0;
        for path in expected_directories {
            if !matches!(
                record.layout.get(index).map(|operation| &operation.effect),
                Some(MacosLayoutEffect::Directory { path: actual }) if actual == &path
            ) {
                return Err(error("macOS directory effects are missing or reordered"));
            }
            index += 1;
        }
        for path in entry_paths {
            let operation = record
                .layout
                .get(index)
                .ok_or_else(|| error("macOS layout plan omits a public entry"))?;
            let MacosLayoutEffect::Entry {
                path: actual,
                prior,
                candidate_target,
            } = &operation.effect
            else {
                return Err(error("macOS directory effect follows an entry effect"));
            };
            if actual != &path || candidate_target.as_ref() != projected_targets.get(&path) {
                return Err(error("macOS layout entry plan is not canonical"));
            }
            if record.prior_entries.get(&path) != Some(prior) {
                return Err(error("macOS layout entry prior state is not canonical"));
            }
            validate_public_path(actual)?;
            self.validate_prior_entry(record, actual, prior)?;
            index += 1;
        }
        if index != record.layout.len() {
            return Err(error("macOS layout plan has extra effects"));
        }
        Ok(())
    }

    fn validate_prior_entry(
        &mut self,
        record: &MacosRecord,
        path: &str,
        prior: &MacosExactEntry,
    ) -> Result<(), InstallPlatformError> {
        let MacosExactEntry::RegularFile {
            mode,
            sha256,
            snapshot_unit,
            snapshot_path,
        } = prior
        else {
            return Ok(());
        };
        let snapshot_unit = snapshot_unit
            .as_ref()
            .ok_or_else(|| error("prior macOS file lacks a snapshot unit"))?;
        let snapshot_path = snapshot_path
            .as_deref()
            .ok_or_else(|| error("prior macOS file lacks a snapshot path"))?;
        if record
            .prior
            .as_ref()
            .is_none_or(|prior| &prior.unit != snapshot_unit)
        {
            return Err(error("prior macOS file snapshot has a foreign unit"));
        }
        if snapshot_path != path {
            return Err(error("prior macOS file snapshot path is not canonical"));
        }
        let unit = self.retained_unit(snapshot_unit)?.clone();
        let contents = self.executor.read_snapshot_file(
            &unit,
            snapshot_path,
            super::model::MAX_LEGACY_FILE_BYTES,
        )?;
        if *mode > 0o777 || !is_sha256(sha256) || hex_digest(&contents) != *sha256 {
            return Err(error("prior macOS file snapshot changed"));
        }
        Ok(())
    }

    pub(super) fn retained_unit(
        &self,
        id: &super::super::UnitId,
    ) -> Result<&UnitRecord, InstallPlatformError> {
        self.known_units
            .iter()
            .find(|unit| unit.id() == id)
            .ok_or_else(|| error("macOS record references an unretained unit"))
    }
}

fn validate_launcher_snapshot(
    launcher: &MacosFilePublication,
    snapshot: &MacosLauncherSnapshot,
) -> Result<(), InstallPlatformError> {
    let snapshot_id = launcher_snapshot_id(launcher.mode, &launcher.contents);
    let expected_path = format!("launchd/{snapshot_id}.plist");
    if snapshot.snapshot_id != snapshot_id
        || snapshot.relative_path != expected_path
        || snapshot.content_sha256 != hex_digest(&launcher.contents)
        || snapshot.mode != launcher.mode
        || snapshot.size != launcher.contents.len() as u64
        || snapshot.device == 0
        || snapshot.inode == 0
    {
        return Err(error("private macOS launcher snapshot is not exact"));
    }
    Ok(())
}

pub(super) fn validate_prior_launcher(
    entry: &MacosExactEntry,
    bytes: &[u8],
) -> Result<(), InstallPlatformError> {
    match entry {
        MacosExactEntry::Absent if bytes.is_empty() => Ok(()),
        MacosExactEntry::RegularFile {
            mode,
            sha256,
            snapshot_unit,
            snapshot_path,
        } if *mode <= 0o777
            && *mode & 0o111 == 0
            && bytes.len() <= MAX_LAUNCHER_BYTES
            && is_sha256(sha256)
            && *sha256 == hex_digest(bytes) =>
        {
            if snapshot_unit.is_some() || snapshot_path.is_some() {
                return Err(error("prior macOS launcher has foreign snapshot authority"));
            }
            Ok(())
        }
        MacosExactEntry::Absent
        | MacosExactEntry::RegularFile { .. }
        | MacosExactEntry::Symlink { .. } => Err(error("prior macOS launcher is not exact")),
    }
}
