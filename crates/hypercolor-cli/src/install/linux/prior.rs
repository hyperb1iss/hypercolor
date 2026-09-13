use std::path::PathBuf;

use hypercolor_platform_fs::{DirectoryAuthority, PublicDirectoryAuthority};

use super::super::{InstallPlatformError, UnitId, UnitRecord};
use super::LinuxInstallPlatform;
use super::executor::{LinuxInstallExecutor, LinuxNativeExecutor, retained_unit};
use super::model::{LinuxUnitBinding, error};

pub(super) struct PriorUnitAuthority {
    unit: UnitRecord,
    units_root: PathBuf,
}

#[derive(Debug)]
pub(super) struct NativePriorUnits {
    ancestry: PublicDirectoryAuthority,
    directory: DirectoryAuthority,
    path: PathBuf,
}

impl LinuxNativeExecutor {
    /// Retain the historical units directory through the existing HOME authority.
    ///
    /// No second install lock is acquired. The caller's retained transaction
    /// authority must already be the elected authority for the installation.
    ///
    /// # Errors
    ///
    /// Refuses missing, replaced, aliased, or already bound historical roots.
    pub fn retain_prior_units(&mut self) -> Result<(), InstallPlatformError> {
        if self.prior_units.is_some() {
            return Err(error("historical units authority is already bound"));
        }
        let (path, ancestry) = self.public_tree.historical_units()?;
        if path == self.units_root_hint {
            return Err(error("historical units root is the current units root"));
        }
        let (_, fresh) = self.public_tree.historical_units()?;
        let directory = fresh
            .into_directory_authority()
            .map_err(|source| error(source.to_string()))?;
        let original = ancestry
            .metadata()
            .map_err(|source| error(source.to_string()))?;
        if !original.is_owned_by_current_user() || original.mode() & 0o022 != 0 {
            return Err(error(
                "historical units root has unsafe ownership or permissions",
            ));
        }
        let retained = directory
            .metadata()
            .map_err(|source| error(source.to_string()))?;
        if (original.device(), original.inode()) != (retained.device(), retained.inode()) {
            return Err(error("historical units root changed during retention"));
        }
        let current = self
            .units
            .metadata()
            .map_err(|source| error(source.to_string()))?;
        if (original.device(), original.inode()) == (current.device(), current.inode()) {
            return Err(error(
                "historical units root aliases the current units authority",
            ));
        }
        self.prior_units = Some(NativePriorUnits {
            ancestry,
            directory,
            path,
        });
        Ok(())
    }

    /// Retain a historical prior only when the persisted path selects it.
    ///
    /// Current-store and synthetic legacy bindings remain with their existing
    /// validators. A historical binding must match its original path, inode,
    /// digest, size and version before returning any authority.
    ///
    /// # Errors
    /// Refuses unknown records, foreign paths and changed original releases.
    pub fn retain_recorded_prior(
        &mut self,
        encoded: &super::super::PlatformTransactionRecord,
    ) -> Result<Option<UnitRecord>, InstallPlatformError> {
        encoded
            .validate()
            .map_err(|source| error(source.to_string()))?;
        let record = super::record::decode_record(encoded)?;
        let Some(binding) = record.prior else {
            return Ok(None);
        };
        if binding.unit.as_str().starts_with("legacy-") {
            return Ok(None);
        }
        let current_path = self
            .units_root_hint
            .join(binding.unit.as_str())
            .join(super::model::DAEMON_RELATIVE_PATH);
        if current_path.to_str() == Some(binding.daemon_path.as_str()) {
            return Ok(None);
        }
        if self.prior_units.is_none() {
            self.retain_prior_units()?;
        }
        let prior = self
            .prior_units
            .as_ref()
            .ok_or_else(|| error("missing prior authority"))?;
        let expected_path = prior
            .path
            .join(binding.unit.as_str())
            .join(super::model::DAEMON_RELATIVE_PATH);
        if expected_path.to_str() != Some(binding.daemon_path.as_str()) {
            return Err(error(
                "recorded prior path does not select an authorized store",
            ));
        }
        let unit = retained_unit(&prior.directory, &prior.path, binding.unit.clone())?;
        self.validate_prior_unit(&unit)?;
        super::super::payload::validate_installed_release_record(&unit)
            .map_err(|source| error(source.to_string()))?;
        if super::proof::retained_unit_binding(&unit, &prior.path)? != binding {
            return Err(error(
                "recorded historical prior changed its executable identity",
            ));
        }
        self.validate_prior_unit(&unit)?;
        Ok(Some(unit))
    }

    pub(super) fn validate_prior_unit(
        &self,
        unit: &UnitRecord,
    ) -> Result<PathBuf, InstallPlatformError> {
        let prior = self
            .prior_units
            .as_ref()
            .ok_or_else(|| error("historical units authority has not been retained"))?;
        prior
            .ancestry
            .validate_ancestry()
            .map_err(|source| error(source.to_string()))?;
        let retained = retained_unit(&prior.directory, &prior.path, unit.id().clone())?;
        prior
            .ancestry
            .validate_ancestry()
            .map_err(|source| error(source.to_string()))?;
        if &retained != unit {
            return Err(error(
                "prior unit does not belong to the retained historical authority",
            ));
        }
        Ok(prior.path.clone())
    }
}

impl<E: LinuxInstallExecutor> LinuxInstallPlatform<E> {
    /// Bind the original prior unit independently of the candidate's store.
    ///
    /// Identical release digests may name distinct copied inodes. The executor
    /// supplies the authoritative prior path; UnitRecord diagnostics never do.
    ///
    /// # Errors
    ///
    /// Refuses rebinding, binding after inspection, or an unrecognized authority.
    pub fn with_prior_unit(mut self, unit: UnitRecord) -> Result<Self, InstallPlatformError> {
        if self.prior_unit.is_some() || self.last_inspection.is_some() {
            return Err(error(
                "prior unit authority must be bound once before inspection",
            ));
        }
        let units_root = self.executor.prior_units_root(&unit)?;
        if !units_root.is_absolute()
            || units_root.components().any(|part| {
                !matches!(
                    part,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
        {
            return Err(error("prior units root must be absolute and normalized"));
        }
        super::require_systemd_safe_root(&units_root)?;
        self.prior_unit = Some(PriorUnitAuthority { unit, units_root });
        Ok(self)
    }

    pub(super) fn prior_retained_unit(
        &self,
        id: &UnitId,
    ) -> Result<&UnitRecord, InstallPlatformError> {
        if let Some(prior) = &self.prior_unit {
            if prior.unit.id() != id
                || self.executor.prior_units_root(&prior.unit)? != prior.units_root
            {
                return Err(error("prior unit role does not match retained authority"));
            }
            return Ok(&prior.unit);
        }
        self.known_units
            .iter()
            .find(|known| known.id() == id)
            .ok_or_else(|| error("prior unit lacks retained authority"))
    }

    pub(super) fn prior_layout_units(&self) -> Vec<UnitRecord> {
        self.prior_unit
            .iter()
            .map(|prior| prior.unit.clone())
            .chain(self.known_units.iter().cloned())
            .collect()
    }

    pub(super) fn prior_unit_binding(
        &self,
        id: &UnitId,
    ) -> Result<LinuxUnitBinding, InstallPlatformError> {
        let unit = self.prior_retained_unit(id)?;
        match &self.prior_unit {
            Some(prior) => self.unit_binding_at(unit, &prior.units_root),
            None => self.unit_binding(unit),
        }
    }
}

impl LinuxInstallPlatform<LinuxNativeExecutor> {
    /// Restore prior-role selection and validate a cold transaction record.
    ///
    /// # Errors
    /// Refuses any prior or candidate record inconsistent with retained authority.
    pub fn with_recorded_prior(
        mut self,
        record: &super::super::PlatformTransactionRecord,
    ) -> Result<Self, InstallPlatformError> {
        if let Some(unit) = self.executor.retain_recorded_prior(record)? {
            self = self.with_prior_unit(unit)?;
        }
        self.validated_record(record)?;
        Ok(self)
    }
}
