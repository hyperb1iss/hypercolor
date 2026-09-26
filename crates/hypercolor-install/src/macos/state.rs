use super::super::{InstallPlatformError, PlatformState, UnitId};
use super::executor::MacosInstallExecutor;
use super::legacy::legacy_identity;
use super::model::{MacosExactEntry, error};
use super::{MacosInspection, MacosInstallPlatform};

impl<E: MacosInstallExecutor> MacosInstallPlatform<E> {
    pub(super) fn inspect_exact(&mut self) -> Result<MacosInspection, InstallPlatformError> {
        let active_unit = self.executor.active_unit()?;
        let launchd = self.executor.launchd_observation()?;
        launchd.validate()?;
        let owner_record = self.executor.owner_record()?;
        let (launcher, launcher_bytes) = self
            .executor
            .launcher_entry(super::model::MAX_LAUNCHER_BYTES)?;
        super::validation::validate_prior_launcher(&launcher, &launcher_bytes)?;
        let layouts = self
            .projections
            .iter()
            .map(|(_, projection)| projection.clone())
            .collect::<Vec<_>>();
        let public = self.executor.public_snapshot(&layouts)?;
        let legacy_executable = self
            .executor
            .inspect_legacy_executable(owner_record.as_ref())?;
        let inspection = MacosInspection {
            active_unit,
            launchd,
            owner_record,
            launcher,
            launcher_bytes,
            public,
            legacy_executable,
        };
        self.validate_inspection(&inspection)?;
        Ok(inspection)
    }

    fn validate_inspection(
        &mut self,
        inspection: &MacosInspection,
    ) -> Result<(), InstallPlatformError> {
        if let Some(pid) = inspection.launchd.pid {
            if matches!(inspection.launcher, MacosExactEntry::Absent) {
                return Err(error("loaded direct launchd owner has no restorable plist"));
            }
            let record = inspection
                .owner_record
                .as_ref()
                .filter(|record| {
                    record.active_owner == hypercolor_macos_owner::MacosDaemonOwner::DirectLaunchd
                        && record.active_identity.pid == pid
                })
                .ok_or_else(|| error("loaded direct launchd owner lacks its exact publication"))?;
            self.executor.corroborate_owner(record)?;
        }
        for (path, entry) in &inspection.public.entries {
            super::model::validate_public_path(path)?;
            if let MacosExactEntry::RegularFile { .. } = entry
                && !inspection.public.regular_bytes.contains_key(path)
            {
                return Err(error("macOS public snapshot omits regular entry bytes"));
            }
        }
        for path in inspection.public.directories.keys() {
            super::model::validate_public_path(path)?;
        }
        for (_, projection) in &self.projections {
            if projection
                .directories
                .iter()
                .any(|path| !inspection.public.directories.contains_key(path))
                || projection
                    .entries
                    .iter()
                    .any(|(path, _)| !inspection.public.entries.contains_key(path))
            {
                return Err(error("macOS public snapshot omits a projected path"));
            }
        }
        Ok(())
    }

    pub(super) fn state_from(
        &self,
        inspection: &MacosInspection,
    ) -> Result<PlatformState, InstallPlatformError> {
        let layout_present = inspection
            .public
            .entries
            .values()
            .any(|entry| !matches!(entry, MacosExactEntry::Absent));
        let launcher_present = !matches!(inspection.launcher, MacosExactEntry::Absent);
        let logical = inspection
            .active_unit
            .clone()
            .or(self.legacy_unit_id(inspection)?);
        if inspection.launchd.pid.is_some() && logical.is_none() {
            return Err(error(
                "running direct launchd owner has no exact logical unit",
            ));
        }
        let legacy_layout = inspection.active_unit.is_none()
            && inspection.public.entries.values().any(|entry| {
                matches!(entry, MacosExactEntry::RegularFile { .. })
                    || matches!(entry, MacosExactEntry::Symlink { target } if !self.is_active_target(target))
            });
        let legacy_launcher = inspection.active_unit.is_none()
            && launcher_present
            && !self.candidate_launcher_matches(inspection);
        let layout_unit = if layout_present {
            inspection
                .active_unit
                .clone()
                .or_else(|| legacy_layout.then(|| logical.clone()).flatten())
        } else {
            None
        };
        let launcher_unit = if launcher_present {
            inspection
                .active_unit
                .clone()
                .or_else(|| legacy_launcher.then(|| logical.clone()).flatten())
        } else {
            None
        };
        Ok(PlatformState {
            layout_unit,
            launcher_unit,
            loaded: inspection.launchd.pid.is_some(),
            running_unit: inspection.launchd.pid.and(logical),
            autostart_enabled: launcher_present && inspection.launchd.autostart_enabled,
        })
    }

    fn legacy_unit_id(
        &self,
        inspection: &MacosInspection,
    ) -> Result<Option<UnitId>, InstallPlatformError> {
        if inspection.active_unit.is_some() || self.is_pre_switch_candidate(inspection) {
            return Ok(None);
        }
        let has_raw = !matches!(inspection.launcher, MacosExactEntry::Absent)
            || inspection
                .public
                .entries
                .values()
                .any(|entry| !matches!(entry, MacosExactEntry::Absent));
        if !has_raw {
            return Ok(None);
        }
        if let Some(unit) = &self.legacy_unit {
            return Ok(Some(unit.clone()));
        }
        UnitId::new(format!("legacy-{}", legacy_identity(inspection)?))
            .map(Some)
            .map_err(|source| error(source.to_string()))
    }

    fn is_pre_switch_candidate(&self, inspection: &MacosInspection) -> bool {
        self.candidate_launcher_matches(inspection)
            && inspection.public.entries.iter().all(|(path, entry)| {
                matches!(entry, MacosExactEntry::Absent)
                    || matches!(entry, MacosExactEntry::Symlink { target }
                        if self.projections.iter().any(|(_, projection)| projection.entries.iter().any(
                            |(candidate_path, candidate_target)| candidate_path == path && candidate_target == target
                        )))
            })
    }

    fn candidate_launcher_matches(&self, inspection: &MacosInspection) -> bool {
        let Ok(launcher) = self.candidate_launcher() else {
            return false;
        };
        match &inspection.launcher {
            MacosExactEntry::Absent => true,
            MacosExactEntry::RegularFile { mode, sha256, .. } => {
                *mode == launcher.mode
                    && *sha256 == super::model::hex_digest(launcher.bytes.as_bytes())
                    && inspection.launcher_bytes == launcher.bytes.as_bytes()
            }
            MacosExactEntry::Symlink { .. } => false,
        }
    }

    fn is_active_target(&self, target: &str) -> bool {
        self.projections.iter().any(|(_, projection)| {
            projection
                .entries
                .iter()
                .any(|(_, candidate_target)| candidate_target == target)
        })
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
        return Err(error("macOS platform state has inconsistent logical units"));
    }
    Ok(first)
}
