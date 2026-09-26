mod adoption;
mod adoption_roots;
mod command;
mod directory;
mod effects;
mod executor;
#[cfg(test)]
mod executor_tests;
mod http;
mod legacy;
#[cfg(test)]
mod legacy_tests;
mod legacy_validation;
mod location;
#[cfg(test)]
mod location_tests;
mod locator;
mod locator_receipt;
mod manager_bus;
mod model;
mod platform;
mod prior;
mod proof;
mod record;
mod runtime;
mod state;
mod systemd;
mod uninstall;
mod validation;

use std::collections::BTreeMap;

use super::{InstallLock, InstallPlatformError, InstallStore, UnitId, UnitRecord};

pub use adoption::{LinuxAdoption, LinuxAdoptionError};
pub use command::{
    LinuxInstallCheckpoint, LinuxInstallCommandError, LinuxInstallHost, LinuxInstallRequest,
    LinuxInstallRun, LinuxPlatformInputs, bind_linux_platform, run_linux_install,
};
pub use directory::LinuxPublicTree;
pub use executor::{LinuxInstallExecutor, LinuxNativeExecutor, LinuxPublicEntry};
pub use location::{InstallLocationError, LinuxInstallLocation, RetainedLinuxInstallLocation};
pub use locator::{
    LinuxInstallAuthority, LinuxInstallElection, LinuxInstallLocator, LinuxLocatorError,
    LinuxManagedAuthority, elect_linux_installation, elect_linux_installation_with,
};
pub use model::{
    DEFAULT_PROBATION_WINDOW, LINUX_DIRECTORY_ITEMS, LINUX_LAYOUT_ITEMS, LinuxDirectoryItem,
    LinuxDirectoryState, LinuxExactEntry, LinuxFilePublication, LinuxHttpResponse,
    LinuxInstallConfig, LinuxLayoutItem, LinuxLayoutPublication, LinuxLegacyFile,
    LinuxLegacySnapshot, LinuxProcessExecutable, LinuxServiceIdentity, LinuxServicePhase,
    LinuxServiceWatch, LinuxSystemdObservation, MAX_PROBATION_WINDOW, parse_systemd_show,
};
pub use runtime::{LinuxRuntimeSettlement, LinuxSystemdConnection};
pub use uninstall::{
    LinuxUninstallCheckpoint, LinuxUninstallHost, LinuxUninstallRun, run_linux_uninstall,
};

/// Retain and validate one installed Linux unit through the transaction lock.
///
/// Ordinary digest units are checked against their exact release manifest and
/// installed tree. Synthetic legacy units are checked against their exact
/// self-bound snapshot manifest and inventory.
///
/// # Errors
///
/// Returns an error when the unit is absent, malformed, belongs to another
/// store, or no longer matches its immutable identity.
pub fn retain_linux_unit(
    store: &InstallStore,
    lock: &InstallLock,
    id: &UnitId,
) -> Result<UnitRecord, InstallPlatformError> {
    if !id.as_str().starts_with("legacy-") {
        return super::payload::retain_installed_release_unit(store, lock, id)
            .map_err(|source| model::error(source.to_string()));
    }
    let directory = store
        .open_unit_directory(lock, id)
        .map_err(|source| model::error(source.to_string()))?;
    bind_linux_retained_unit(id.clone(), store.unit_path(id), directory)
}

#[cfg(unix)]
pub fn bind_linux_retained_unit(
    id: super::UnitId,
    root_hint: impl Into<std::path::PathBuf>,
    directory: hypercolor_platform_fs::DirectoryAuthority,
) -> Result<UnitRecord, InstallPlatformError> {
    legacy_validation::validate_legacy_snapshot_binding(&directory, &id)?;
    let read_only = directory
        .read_only()
        .map_err(|source| model::error(source.to_string()))?;
    UnitRecord::new(id, root_hint, read_only).map_err(|source| model::error(source.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LinuxInspection {
    active_unit: Option<super::UnitId>,
    systemd: LinuxSystemdObservation,
    launcher: LinuxExactEntry,
    launcher_bytes: Vec<u8>,
    layout: BTreeMap<LinuxLayoutItem, LinuxExactEntry>,
    directories: BTreeMap<LinuxDirectoryItem, LinuxDirectoryState>,
    legacy_inventory: Vec<model::LinuxLegacyFile>,
}

pub struct LinuxInstallPlatform<E> {
    pub(super) executor: E,
    pub(super) config: LinuxInstallConfig,
    pub(super) known_units: Vec<UnitRecord>,
    prior_unit: Option<prior::PriorUnitAuthority>,
    pub(super) last_inspection: Option<LinuxInspection>,
    pub(super) legacy_unit: Option<super::UnitId>,
    /// The prior the last `ProvePrior` proof found running, until taken.
    pub(super) restored: Option<super::RestoredRelease>,
}

impl<E: LinuxInstallExecutor> LinuxInstallPlatform<E> {
    pub fn new(
        mut executor: E,
        config: LinuxInstallConfig,
        known_units: impl IntoIterator<Item = UnitRecord>,
    ) -> Result<Self, InstallPlatformError> {
        model::require_absolute(&config.direct_fragment_path, "direct fragment")?;
        if !config.immutable_units_root.is_absolute() || !config.active_root.is_absolute() {
            return Err(model::error("Linux install roots must be absolute"));
        }
        require_systemd_safe_root(&config.immutable_units_root)?;
        require_systemd_safe_root(&config.active_root)?;
        let units_parent = config
            .immutable_units_root
            .parent()
            .ok_or_else(|| model::error("Linux immutable units root has no parent"))?;
        if config.probation > model::MAX_PROBATION_WINDOW {
            return Err(model::error(format!(
                "the probation window may be at most {} seconds",
                model::MAX_PROBATION_WINDOW.as_secs()
            )));
        }
        if config.immutable_units_root.file_name() != Some(std::ffi::OsStr::new("units"))
            || config.active_root != units_parent.join("active")
            || config.immutable_units_root.components().any(|component| {
                !matches!(
                    component,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
        {
            return Err(model::error(
                "Linux install roots do not share one exact topology",
            ));
        }
        executor.validate_topology(&config)?;
        let mut units = Vec::new();
        for unit in known_units {
            executor.validate_unit_authority(&unit)?;
            if units
                .iter()
                .any(|known: &UnitRecord| known.id() == unit.id())
            {
                return Err(model::error("duplicate retained immutable unit authority"));
            }
            units.push(unit);
        }
        let mut legacy_units = units
            .iter()
            .filter(|unit| unit.id().as_str().starts_with("legacy-"));
        let legacy_unit = legacy_units.next().map(|unit| unit.id().clone());
        if legacy_units.next().is_some() {
            return Err(model::error(
                "multiple retained legacy units require explicit transaction authority",
            ));
        }
        Ok(Self {
            executor,
            config,
            known_units: units,
            prior_unit: None,
            last_inspection: None,
            legacy_unit,
            restored: None,
        })
    }

    pub fn into_executor(self) -> E {
        self.executor
    }
}

fn require_systemd_safe_root(root: &std::path::Path) -> Result<(), InstallPlatformError> {
    let text = root
        .to_str()
        .ok_or_else(|| model::error("Linux install roots must be exact UTF-8"))?;
    if !text
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte))
    {
        return Err(model::error(
            "Linux install roots are not safely representable in systemd ExecStart",
        ));
    }
    Ok(())
}
