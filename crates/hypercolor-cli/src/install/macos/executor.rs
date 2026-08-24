use std::collections::BTreeMap;
use std::time::Duration;

use hypercolor_macos_owner::{MacosDirectLaunchdPublicationExpectation, MacosOwnerRecord};

use super::super::{InstallPlatformError, UnitId, UnitRecord};
use super::model::{
    MacosCandidateLayout, MacosDirectoryState, MacosEntryPublication, MacosExactEntry,
    MacosFilePublication, MacosInstallConfig, MacosLaunchdObservation, MacosLauncherSnapshot,
    MacosLegacyExecutable, MacosLegacySnapshot, MacosMutationOutcome, MacosPublicSnapshot,
    MacosRuntimeTransition,
};

pub trait MacosInstallExecutor {
    fn validate_topology(
        &mut self,
        config: &MacosInstallConfig,
    ) -> Result<(), InstallPlatformError>;

    fn validate_unit_authority(&mut self, unit: &UnitRecord) -> Result<(), InstallPlatformError>;

    fn validate_unit_executable(
        &mut self,
        unit: &UnitRecord,
        executable: &super::model::MacosRuntimeExecutable,
    ) -> Result<(), InstallPlatformError>;

    fn active_unit(&mut self) -> Result<Option<UnitId>, InstallPlatformError>;

    fn launchd_observation(&mut self) -> Result<MacosLaunchdObservation, InstallPlatformError>;

    fn owner_record(&mut self) -> Result<Option<MacosOwnerRecord>, InstallPlatformError>;

    fn launcher_entry(
        &mut self,
        max_bytes: usize,
    ) -> Result<(MacosExactEntry, Vec<u8>), InstallPlatformError>;

    fn public_snapshot(
        &mut self,
        layouts: &[MacosCandidateLayout],
    ) -> Result<MacosPublicSnapshot, InstallPlatformError>;

    fn bind_public_inventory(
        &mut self,
        directories: &[String],
        entries: &[String],
    ) -> Result<(), InstallPlatformError>;

    fn candidate_layout(
        &mut self,
        unit: &UnitRecord,
    ) -> Result<MacosCandidateLayout, InstallPlatformError>;

    fn inspect_legacy_executable(
        &mut self,
        owner: Option<&MacosOwnerRecord>,
    ) -> Result<Option<MacosLegacyExecutable>, InstallPlatformError>;

    fn replace_launcher(
        &mut self,
        expected: &MacosExactEntry,
        replacement: Option<&MacosFilePublication>,
    ) -> Result<(), InstallPlatformError>;

    fn replace_layout(
        &mut self,
        path: &str,
        expected: &MacosExactEntry,
        replacement: Option<&MacosEntryPublication>,
    ) -> Result<(), InstallPlatformError>;

    fn replace_directory(
        &mut self,
        path: &str,
        expected: MacosDirectoryState,
        create: bool,
    ) -> Result<(), InstallPlatformError>;

    fn set_autostart(
        &mut self,
        enabled: bool,
    ) -> Result<MacosMutationOutcome, InstallPlatformError>;

    fn persist_launcher_snapshot(
        &mut self,
        launcher: &MacosFilePublication,
    ) -> Result<MacosLauncherSnapshot, InstallPlatformError>;

    fn validate_launcher_snapshot(
        &mut self,
        launcher: &MacosFilePublication,
        snapshot: &MacosLauncherSnapshot,
    ) -> Result<(), InstallPlatformError>;

    fn transition_runtime(
        &mut self,
        transition: &MacosRuntimeTransition,
    ) -> Result<MacosMutationOutcome, InstallPlatformError>;

    fn snapshot_legacy_unit(
        &mut self,
        snapshot: &MacosLegacySnapshot,
    ) -> Result<UnitRecord, InstallPlatformError>;

    fn validate_legacy_snapshot(
        &mut self,
        unit: &UnitRecord,
        executable: &MacosLegacyExecutable,
        launcher: &MacosExactEntry,
        launcher_bytes: &[u8],
        entries: &BTreeMap<String, MacosExactEntry>,
    ) -> Result<(), InstallPlatformError>;

    fn read_snapshot_file(
        &mut self,
        unit: &UnitRecord,
        path: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, InstallPlatformError>;

    fn corroborate_owner(&mut self, record: &MacosOwnerRecord) -> Result<(), InstallPlatformError>;

    fn wait_for_exact_publication(
        &mut self,
        expectation: &MacosDirectLaunchdPublicationExpectation,
        timeout: Duration,
    ) -> Result<Option<MacosOwnerRecord>, InstallPlatformError>;

    fn wait_for_legacy_publication(
        &mut self,
        executable: &MacosLegacyExecutable,
        after_epoch: u64,
        timeout: Duration,
    ) -> Result<Option<MacosOwnerRecord>, InstallPlatformError>;

    fn wait_for_guard_release(&mut self, timeout: Duration) -> Result<bool, InstallPlatformError>;
}
