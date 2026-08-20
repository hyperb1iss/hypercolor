mod effects;
mod executor;
mod legacy;
mod model;
mod platform;
mod proof;
mod record;
mod runtime;
mod state;
mod validation;

use super::{InstallPlatformError, UnitId, UnitRecord};

pub use executor::MacosInstallExecutor;
pub use model::{
    MacosCandidateLayout, MacosDirectoryState, MacosEntryPublication, MacosExactEntry,
    MacosFilePublication, MacosInstallConfig, MacosLaunchdObservation, MacosLauncherSnapshot,
    MacosLegacyExecutable, MacosLegacyFile, MacosLegacySnapshot, MacosMutationOutcome,
    MacosPublicSnapshot, MacosRuntimeExecutable, MacosRuntimeTransition, MacosStopAuthority,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MacosInspection {
    active_unit: Option<UnitId>,
    launchd: MacosLaunchdObservation,
    owner_record: Option<hypercolor_macos_owner::MacosOwnerRecord>,
    launcher: MacosExactEntry,
    launcher_bytes: Vec<u8>,
    public: MacosPublicSnapshot,
    legacy_executable: Option<MacosLegacyExecutable>,
}

pub struct MacosInstallPlatform<E> {
    pub(super) executor: E,
    pub(super) config: MacosInstallConfig,
    pub(super) known_units: Vec<UnitRecord>,
    pub(super) last_inspection: Option<MacosInspection>,
    pub(super) legacy_unit: Option<UnitId>,
    pub(super) projections: Vec<(UnitId, MacosCandidateLayout)>,
    pub(super) pending_effect_checkpoint: Option<super::PlatformCheckpoint>,
}

impl<E: MacosInstallExecutor> MacosInstallPlatform<E> {
    pub fn new(
        mut executor: E,
        config: MacosInstallConfig,
        known_units: impl IntoIterator<Item = UnitRecord>,
    ) -> Result<Self, InstallPlatformError> {
        model::validate_public_path(&config.direct_plist_path)?;
        for path in [
            &config.immutable_units_root,
            &config.active_root,
            &config.log_directory,
        ] {
            model::validate_public_path(
                path.to_str()
                    .ok_or_else(|| model::error("macOS install paths must be exact UTF-8"))?,
            )?;
        }
        let units_parent = config
            .immutable_units_root
            .parent()
            .ok_or_else(|| model::error("macOS immutable units root has no parent"))?;
        if config.immutable_units_root.file_name() != Some(std::ffi::OsStr::new("units"))
            || config.active_root != units_parent.join("active")
        {
            return Err(model::error(
                "macOS install roots do not share one exact topology",
            ));
        }
        executor.validate_topology(&config)?;
        let mut units = Vec::new();
        let mut projections = Vec::new();
        let mut legacy_unit = None;
        for unit in known_units {
            executor.validate_unit_authority(&unit)?;
            if units
                .iter()
                .any(|known: &UnitRecord| known.id() == unit.id())
            {
                return Err(model::error("duplicate retained macOS unit authority"));
            }
            if unit.id().as_str().starts_with("legacy-") {
                if legacy_unit.replace(unit.id().clone()).is_some() {
                    return Err(model::error("multiple retained macOS legacy units"));
                }
            } else {
                let projection = executor.candidate_layout(&unit)?;
                model::validate_candidate_layout(&projection)?;
                projections.push((unit.id().clone(), projection));
            }
            units.push(unit);
        }
        Ok(Self {
            executor,
            config,
            known_units: units,
            last_inspection: None,
            legacy_unit,
            projections,
            pending_effect_checkpoint: None,
        })
    }

    pub fn into_executor(self) -> E {
        self.executor
    }

    pub(super) fn projection(
        &self,
        unit: &UnitId,
    ) -> Result<&MacosCandidateLayout, InstallPlatformError> {
        self.projections
            .iter()
            .find_map(|(known, projection)| (known == unit).then_some(projection))
            .ok_or_else(|| model::error("macOS candidate projection is not retained"))
    }
}

pub fn bind_macos_retained_legacy_unit(
    id: UnitId,
    root_hint: impl Into<std::path::PathBuf>,
    directory: hypercolor_platform_fs::ReadOnlyDirectoryAuthority,
) -> Result<UnitRecord, InstallPlatformError> {
    if !id.as_str().starts_with("legacy-") {
        return Err(model::error(
            "macOS legacy binder requires a synthetic legacy unit ID",
        ));
    }
    UnitRecord::new(id, root_hint, directory).map_err(|source| model::error(source.to_string()))
}

pub fn retain_macos_unit(
    store: &super::InstallStore,
    lock: &super::InstallLock,
    id: &UnitId,
) -> Result<UnitRecord, InstallPlatformError> {
    super::payload::retain_installed_release_unit(store, lock, id)
        .map_err(|source| model::error(source.to_string()))
}
