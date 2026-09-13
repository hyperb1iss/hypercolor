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
