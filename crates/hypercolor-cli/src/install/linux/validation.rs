use super::super::{
    InstallPlatformError, PlatformOwnerReceipt, PlatformTransactionRecord, UnitRecord,
};
use super::LinuxInstallPlatform;
use super::executor::LinuxInstallExecutor;
use super::model::{
    LINUX_DIRECTORY_ITEMS, LINUX_LAYOUT_ITEMS, LinuxDirectoryState, LinuxExactEntry,
    LinuxLayoutEffect, LinuxRecord, error, hex_digest,
};
use super::proof::{
    open_unit_file, read_unit_file, require_notify_launcher, validate_prior_launcher_entry,
};
use super::record::{decode_receipt, decode_record};
use super::systemd::canonical_executable;

impl<E: LinuxInstallExecutor> LinuxInstallPlatform<E> {
    pub(super) fn validated_record(
        &self,
        encoded: &PlatformTransactionRecord,
    ) -> Result<LinuxRecord, InstallPlatformError> {
        let record = decode_record(encoded)?;
        self.validate_record(&record)?;
        Ok(record)
    }

    pub(super) fn validated_receipt(
        &self,
        record: &LinuxRecord,
        encoded: Option<&PlatformOwnerReceipt>,
    ) -> Result<Option<super::model::LinuxOwnerReceipt>, InstallPlatformError> {
        let receipt = decode_receipt(encoded)?;
        if let Some(receipt) = &receipt
            && (receipt.unit != record.candidate.unit
                || receipt.main_pid == 0
                || receipt.invocation_id == record.baseline_systemd.invocation_id
                || receipt.invocation_id.len() != 32
                || !receipt
                    .invocation_id
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(error(
                "candidate owner receipt is not bound to the candidate record",
            ));
        }
        Ok(receipt)
    }

    fn validate_record(&self, record: &LinuxRecord) -> Result<(), InstallPlatformError> {
        record.baseline_systemd.validate()?;
        validate_prior_launcher_entry(&record.prior_launcher, &record.prior_launcher_bytes)?;
        if let Some(launcher) = &record.candidate_launcher
            && launcher != &self.candidate_launcher()?
        {
            return Err(error("candidate launcher binding is not exact"));
        }
        if record.baseline_systemd.load_state == "loaded" {
            if record.baseline_systemd.fragment_path != self.config.direct_fragment_path
                || matches!(record.prior_launcher, LinuxExactEntry::Absent)
                || record.baseline_systemd.exec_start
                    != require_notify_launcher(&record.prior_launcher_bytes)?
            {
                return Err(error(
                    "baseline systemd identity is not bound to the launcher",
                ));
            }
        } else if !matches!(record.prior_launcher, LinuxExactEntry::Absent) {
            return Err(error("absent baseline systemd unit has a prior launcher"));
        }
        self.validate_binding(&record.candidate, false, record)?;
        if let Some(prior) = &record.prior {
            self.validate_binding(prior, true, record)?;
        }
        let first_conversion = record
            .prior
            .as_ref()
            .is_some_and(|prior| prior.unit.as_str().starts_with("legacy-"));
        if record.first_conversion != first_conversion {
            return Err(error(
                "first-conversion marker disagrees with the prior binding",
            ));
        }
        self.validate_layout_plan(record)
    }

    fn validate_binding(
        &self,
        binding: &super::model::LinuxUnitBinding,
        prior: bool,
        record: &LinuxRecord,
    ) -> Result<(), InstallPlatformError> {
        let unit = self.retained_unit(&binding.unit)?;
        let mut expected = self.unit_binding(unit)?;
        if prior && binding.unit.as_str().starts_with("legacy-") {
            let launcher = require_notify_launcher(&record.prior_launcher_bytes)?;
            expected.daemon_path = canonical_executable(&launcher)?;
        }
        if &expected != binding {
            return Err(error(
                "Linux unit binding does not match retained authority",
            ));
        }
        Ok(())
    }

    fn validate_layout_plan(&self, record: &LinuxRecord) -> Result<(), InstallPlatformError> {
        if record.prior_directories.len() != LINUX_DIRECTORY_ITEMS.len()
            || LINUX_DIRECTORY_ITEMS
                .iter()
                .any(|item| !record.prior_directories.contains_key(item))
        {
            return Err(error(
                "Linux record lacks the complete prior directory state",
            ));
        }
        let mut operation_index = 0;
        for expected_item in LINUX_DIRECTORY_ITEMS
            .into_iter()
            .filter(|item| record.prior_directories[item] == LinuxDirectoryState::Absent)
        {
            let operation = record
                .layout
                .get(operation_index)
                .ok_or_else(|| error("Linux layout plan omits an absent directory"))?;
            if !matches!(
                operation.effect,
                LinuxLayoutEffect::Directory { item } if item == expected_item
            ) {
                return Err(error("Linux directory effects are missing or reordered"));
            }
            operation_index += 1;
        }
        for expected_item in LINUX_LAYOUT_ITEMS {
            let operation = record
                .layout
                .get(operation_index)
                .ok_or_else(|| error("Linux layout plan is missing a fixed entry"))?;
            let LinuxLayoutEffect::Entry {
                item,
                prior,
                candidate_target,
            } = &operation.effect
            else {
                return Err(error("Linux directory effect follows a file effect"));
            };
            if *item != expected_item
                || *candidate_target != self.layout_target(&record.candidate.unit, expected_item)
            {
                return Err(error(
                    "Linux layout effect order or target is not canonical",
                ));
            }
            self.validate_prior_entry(record, expected_item, prior)?;
            operation_index += 1;
        }
        if operation_index != record.layout.len() {
            return Err(error("Linux layout plan has extra effects"));
        }
        Ok(())
    }

    fn validate_prior_entry(
        &self,
        record: &LinuxRecord,
        item: super::model::LinuxLayoutItem,
        prior: &LinuxExactEntry,
    ) -> Result<(), InstallPlatformError> {
        let LinuxExactEntry::RegularFile {
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
            .ok_or_else(|| error("prior regular entry lacks an immutable snapshot unit"))?;
        let snapshot_path = snapshot_path
            .as_deref()
            .filter(|path| *path == item.unit_path())
            .ok_or_else(|| error("prior regular entry has a foreign snapshot path"))?;
        if record
            .prior
            .as_ref()
            .is_none_or(|binding| &binding.unit != snapshot_unit)
        {
            return Err(error(
                "prior regular entry snapshot unit is not the prior binding",
            ));
        }
        let unit = self.retained_unit(snapshot_unit)?;
        let opened = open_unit_file(unit, snapshot_path)?;
        if opened.metadata().mode() & 0o7777 != *mode {
            return Err(error("prior regular entry snapshot mode changed"));
        }
        let contents = read_unit_file(
            unit,
            snapshot_path,
            hypercolor_platform_fs::MAX_EXACT_ENTRY_BYTES,
        )?;
        if hex_digest(&contents) != *sha256 {
            return Err(error("prior regular entry snapshot digest changed"));
        }
        Ok(())
    }

    fn retained_unit(
        &self,
        id: &super::super::UnitId,
    ) -> Result<&UnitRecord, InstallPlatformError> {
        self.known_units
            .iter()
            .find(|known| known.id() == id)
            .ok_or_else(|| error("Linux record references an unretained unit"))
    }
}
