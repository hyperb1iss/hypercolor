use super::super::{InstallPlatformError, PlatformState, UnitId};
use super::executor::LinuxInstallExecutor;
use super::legacy::legacy_identity_digest;
use super::model::{
    LINUX_DIRECTORY_ITEMS, LINUX_LAYOUT_ITEMS, LinuxExactEntry, MAX_LAUNCHER_BYTES,
    MAX_SYSTEMD_SHOW_BYTES, error, hex_digest, parse_systemd_show,
};
use super::proof::{require_notify_launcher, validate_prior_launcher_entry};
use super::{LinuxInspection, LinuxInstallPlatform};

impl<E: LinuxInstallExecutor> LinuxInstallPlatform<E> {
    pub(super) fn inspect_exact(&mut self) -> Result<LinuxInspection, InstallPlatformError> {
        let active_unit = self.executor.active_unit()?;
        let systemd = parse_systemd_show(&self.executor.systemd_show(MAX_SYSTEMD_SHOW_BYTES)?)?;
        let (launcher, launcher_bytes) = self.executor.launcher_entry(MAX_LAUNCHER_BYTES)?;
        validate_prior_launcher_entry(&launcher, &launcher_bytes)?;
        let mut layout = std::collections::BTreeMap::new();
        for item in LINUX_LAYOUT_ITEMS {
            layout.insert(item, self.executor.layout_entry(item)?);
        }
        let mut directories = std::collections::BTreeMap::new();
        for item in LINUX_DIRECTORY_ITEMS {
            directories.insert(item, self.executor.directory_state(item)?);
        }
        let legacy_inventory = self.executor.legacy_inventory()?;
        let inspection = LinuxInspection {
            active_unit,
            systemd,
            launcher,
            launcher_bytes,
            layout,
            directories,
            legacy_inventory,
        };
        self.validate_direct_inspection(&inspection)?;
        Ok(inspection)
    }

    fn validate_direct_inspection(
        &self,
        inspection: &LinuxInspection,
    ) -> Result<(), InstallPlatformError> {
        let launcher_present = !matches!(inspection.launcher, LinuxExactEntry::Absent);
        if inspection.systemd.load_state == "loaded" {
            if inspection.systemd.fragment_path != self.config.direct_fragment_path {
                return Err(error("loaded service is not the exact raw-direct fragment"));
            }
            if launcher_present {
                require_notify_launcher(&inspection.launcher_bytes)?;
            }
        }
        if inspection.systemd.load_state == "not-found" && inspection.systemd.main_pid != 0 {
            return Err(error("absent systemd service reported a process"));
        }
        Ok(())
    }

    pub(super) fn state_from(
        &self,
        inspection: &LinuxInspection,
    ) -> Result<PlatformState, InstallPlatformError> {
        let launcher_present = !matches!(inspection.launcher, LinuxExactEntry::Absent);
        let layout_present = inspection
            .layout
            .values()
            .any(|entry| !matches!(entry, LinuxExactEntry::Absent));
        let running = inspection.systemd.active_state == "active";
        let logical_unit = inspection
            .active_unit
            .clone()
            .or(self.legacy_unit_id(inspection)?);
        if running && logical_unit.is_none() {
            return Err(error("running direct service has no exact logical unit"));
        }
        let (layout_unit, launcher_unit) = if let Some(active) = &inspection.active_unit {
            (
                layout_present.then(|| active.clone()),
                launcher_present.then(|| active.clone()),
            )
        } else {
            let legacy_layout_present = inspection.layout.iter().any(|(item, entry)| {
                !matches!(entry, LinuxExactEntry::Absent)
                    && !self.is_candidate_layout_entry(*item, entry)
            });
            let legacy_launcher_present =
                launcher_present && !self.candidate_launcher_matches(inspection).unwrap_or(false);
            (
                legacy_layout_present
                    .then(|| logical_unit.clone())
                    .flatten(),
                legacy_launcher_present
                    .then(|| logical_unit.clone())
                    .flatten(),
            )
        };
        Ok(PlatformState {
            layout_unit,
            launcher_unit,
            loaded: running,
            running_unit: running.then_some(logical_unit).flatten(),
            autostart_enabled: launcher_present && inspection.systemd.unit_file_state == "enabled",
        })
    }

    fn legacy_unit_id(
        &self,
        inspection: &LinuxInspection,
    ) -> Result<Option<UnitId>, InstallPlatformError> {
        if inspection.active_unit.is_some() || self.is_pre_switch_candidate(inspection)? {
            return Ok(None);
        }
        if matches!(inspection.launcher, LinuxExactEntry::Absent)
            && inspection
                .layout
                .values()
                .all(|entry| !matches!(entry, LinuxExactEntry::RegularFile { .. }))
        {
            return Ok(None);
        }
        if let Some(unit) = &self.legacy_unit {
            return Ok(Some(unit.clone()));
        }
        let identity = legacy_identity_digest(
            &inspection.launcher,
            &inspection.launcher_bytes,
            inspection.layout.iter(),
            &inspection.legacy_inventory,
        )?;
        UnitId::new(format!("legacy-{identity}"))
            .map(Some)
            .map_err(|source| error(source.to_string()))
    }

    fn is_pre_switch_candidate(
        &self,
        inspection: &LinuxInspection,
    ) -> Result<bool, InstallPlatformError> {
        let launcher_matches = self.candidate_launcher_matches(inspection)?;
        if !launcher_matches {
            return Ok(false);
        }
        Ok(self.known_units.iter().any(|unit| {
            let mut artifact_present = !matches!(inspection.launcher, LinuxExactEntry::Absent);
            let layout_matches = inspection.layout.iter().all(|(item, entry)| match entry {
                LinuxExactEntry::Absent => true,
                LinuxExactEntry::Symlink { target }
                    if *target == self.layout_target(unit.id(), *item) =>
                {
                    artifact_present = true;
                    true
                }
                LinuxExactEntry::RegularFile { .. } | LinuxExactEntry::Symlink { .. } => false,
            });
            layout_matches && artifact_present
        }))
    }

    fn candidate_launcher_matches(
        &self,
        inspection: &LinuxInspection,
    ) -> Result<bool, InstallPlatformError> {
        let launcher = self.candidate_launcher()?;
        Ok(match &inspection.launcher {
            LinuxExactEntry::Absent => true,
            LinuxExactEntry::RegularFile { mode, sha256, .. } => {
                *mode == launcher.mode
                    && *sha256 == hex_digest(&launcher.bytes)
                    && inspection.launcher_bytes == launcher.bytes
            }
            LinuxExactEntry::Symlink { .. } => false,
        })
    }

    fn is_candidate_layout_entry(
        &self,
        item: super::model::LinuxLayoutItem,
        entry: &LinuxExactEntry,
    ) -> bool {
        let LinuxExactEntry::Symlink { target } = entry else {
            return false;
        };
        self.known_units
            .iter()
            .any(|unit| *target == self.layout_target(unit.id(), item))
    }
}

pub(super) fn consistent_platform_unit(
    state: &PlatformState,
) -> Result<Option<UnitId>, InstallPlatformError> {
    let mut units = [
        state.layout_unit.as_ref(),
        state.launcher_unit.as_ref(),
        state.running_unit.as_ref(),
    ]
    .into_iter()
    .flatten();
    let first = units.next().cloned();
    if units.any(|unit| Some(unit) != first.as_ref()) {
        return Err(error("Linux platform state has inconsistent logical units"));
    }
    Ok(first)
}
