use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;
use std::time::Duration;

use hypercolor_macos_owner::{
    MacosDaemonOwner, MacosDirectLaunchdExecutableExpectation,
    MacosDirectLaunchdPublicationExpectation, MacosOwnerRecord,
};
use hypercolor_platform_fs::{DirectoryEntryMetadata, OpenedRegularFile};

use super::super::{InstallPlatformError, PlatformCheckpoint, PlatformState, UnitRecord};
use super::MacosInstallPlatform;
use super::executor::MacosInstallExecutor;
use super::model::{
    DAEMON_RELATIVE_PATH, LAUNCHER_MODE, MANIFEST_RELATIVE_PATH, MAX_LAUNCHER_BYTES, MacosLauncher,
    MacosLegacyExecutable, MacosOwnerReceipt, MacosRecord, MacosUnitBinding, error,
};

const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
const OWNER_PUBLICATION_TIMEOUT: Duration = Duration::from_secs(10);

impl<E: MacosInstallExecutor> MacosInstallPlatform<E> {
    pub(super) fn candidate_launcher(&self) -> Result<MacosLauncher, InstallPlatformError> {
        let active = exact_path(&self.config.active_root)?;
        let log_directory = exact_path(&self.config.log_directory)?;
        let daemon = xml_escape(&format!("{active}/{DAEMON_RELATIVE_PATH}"));
        let ui = xml_escape(&format!("{active}/share/hypercolor/ui"));
        let log = xml_escape(&format!("{log_directory}/hypercolor.log"));
        let binary_directory = xml_escape(&format!("{active}/bin"));
        let bytes = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\"><dict>\n\
<key>Label</key><string>tech.hyperbliss.hypercolor</string>\n\
<key>ProgramArguments</key><array><string>{daemon}</string><string>--macos-owner</string><string>direct-launchd</string><string>--ui-dir</string><string>{ui}</string></array>\n\
<key>RunAtLoad</key><true/>\n\
<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>\n\
<key>ThrottleInterval</key><integer>3</integer>\n\
<key>StandardOutPath</key><string>{log}</string>\n\
<key>StandardErrorPath</key><string>{log}</string>\n\
<key>EnvironmentVariables</key><dict><key>HYPERCOLOR_MACOS_OWNER</key><string>direct-launchd</string><key>HYPERCOLOR_SERVICE_IDENTITY</key><string>user_service:launchd:tech.hyperbliss.hypercolor</string><key>HYPERCOLOR_LOG</key><string>info</string><key>PATH</key><string>/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:{binary_directory}</string></dict>\n\
<key>ProcessType</key><string>Standard</string>\n\
<key>LowPriorityBackgroundIO</key><true/>\n\
</dict></plist>\n"
        );
        if bytes.len() > MAX_LAUNCHER_BYTES {
            return Err(error("rendered macOS launcher exceeds its byte bound"));
        }
        Ok(MacosLauncher {
            mode: LAUNCHER_MODE,
            bytes,
        })
    }

    pub(super) fn unit_binding(
        &mut self,
        unit: &UnitRecord,
    ) -> Result<MacosUnitBinding, InstallPlatformError> {
        let provenance = super::super::payload::bind_macos_release_provenance(unit)
            .map_err(|source| error(source.to_string()))?;
        let version = retained_manifest_version(unit)?;
        let daemon_path = self
            .config
            .immutable_units_root
            .join(unit.id().as_str())
            .join(DAEMON_RELATIVE_PATH)
            .to_str()
            .ok_or_else(|| error("retained macOS daemon path is not exact UTF-8"))?
            .to_owned();
        let binding = MacosUnitBinding {
            unit: unit.id().clone(),
            daemon_path,
            daemon_sha256: provenance.daemon_sha256().to_owned(),
            daemon_size: provenance.daemon_size(),
            daemon_mode: provenance.daemon_mode(),
            daemon_device: provenance.daemon_device(),
            daemon_inode: provenance.daemon_inode(),
            designated_requirement: provenance.designated_requirement().to_owned(),
            designated_requirement_sha256: provenance.designated_requirement_sha256().to_owned(),
            cdhash: provenance.cdhash().to_owned(),
            version,
            synthetic_legacy: false,
        };
        self.executor
            .validate_unit_executable(unit, &runtime_executable(&binding))?;
        Ok(binding)
    }

    pub(super) fn legacy_binding(
        &self,
        unit: &UnitRecord,
        executable: &MacosLegacyExecutable,
    ) -> MacosUnitBinding {
        MacosUnitBinding {
            unit: unit.id().clone(),
            daemon_path: executable.path.clone(),
            daemon_sha256: executable.sha256.clone(),
            daemon_size: executable.size,
            daemon_mode: executable.mode,
            daemon_device: executable.device,
            daemon_inode: executable.inode,
            designated_requirement: executable.designated_requirement.clone(),
            designated_requirement_sha256: executable.designated_requirement_sha256.clone(),
            cdhash: executable.cdhash.clone(),
            version: executable.version.clone(),
            synthetic_legacy: true,
        }
    }

    pub(super) fn prove_owner(
        &mut self,
        checkpoint: PlatformCheckpoint,
        expected: &PlatformState,
        record: &MacosRecord,
        candidate_receipt: Option<&MacosOwnerReceipt>,
    ) -> Result<Option<MacosOwnerReceipt>, InstallPlatformError> {
        let observation = self.executor.launchd_observation()?;
        observation.validate()?;
        let Some(running_unit) = expected.running_unit.as_ref() else {
            if observation.pid.is_some() {
                return Err(error(
                    "inactive macOS proof observed a loaded launchd owner",
                ));
            }
            return Ok(None);
        };
        let (binding, after_epoch) = match checkpoint {
            PlatformCheckpoint::CandidateRuntime if running_unit == &record.candidate.unit => (
                &record.candidate,
                record.baseline_owner_epoch.unwrap_or_default(),
            ),
            PlatformCheckpoint::PriorRestored => (
                record
                    .prior
                    .as_ref()
                    .filter(|prior| &prior.unit == running_unit)
                    .ok_or_else(|| error("prior macOS owner proof requested an unknown unit"))?,
                candidate_receipt
                    .map_or(record.baseline_owner_epoch.unwrap_or_default(), |receipt| {
                        receipt.owner_epoch
                    }),
            ),
            _ => return Err(error("macOS owner proof received an invalid checkpoint")),
        };
        let published = self.wait_for_binding_publication(binding, after_epoch)?;
        if observation.pid != Some(published.active_identity.pid) {
            return Err(error(
                "launchd owner changed around exact publication proof",
            ));
        }
        Ok(Some(owner_receipt(&published, running_unit.clone())))
    }

    pub(super) fn prove_baseline_owner(
        &mut self,
        expected: &PlatformState,
        record: &MacosRecord,
    ) -> Result<(), InstallPlatformError> {
        if expected.running_unit.is_none() {
            if record.baseline_launchd.pid.is_some() || record.baseline_owner_epoch.is_some() {
                return Err(error("inactive macOS baseline contains owner identity"));
            }
            return Ok(());
        }
        let binding = record
            .prior
            .as_ref()
            .filter(|prior| Some(&prior.unit) == expected.running_unit.as_ref())
            .ok_or_else(|| error("running macOS baseline lacks its prior unit binding"))?;
        let owner = self
            .executor
            .owner_record()?
            .ok_or_else(|| error("running macOS baseline lacks its durable owner record"))?;
        self.executor.corroborate_owner(&owner)?;
        if owner.active_owner != MacosDaemonOwner::DirectLaunchd
            || Some(owner.active_identity.pid) != record.baseline_launchd.pid
            || Some(owner.owner_epoch) != record.baseline_owner_epoch
            || owner.active_identity.executable_path != Path::new(&binding.daemon_path)
            || owner.active_identity.designated_requirement_hash
                != binding.designated_requirement_sha256
        {
            return Err(error(
                "macOS baseline owner is not bound to the retained prior",
            ));
        }
        if binding.synthetic_legacy {
            let before = self.executor.launchd_observation()?;
            let executable = self
                .executor
                .inspect_legacy_executable(Some(&owner))?
                .ok_or_else(|| error("running legacy baseline lacks exact executable identity"))?;
            let after = self.executor.launchd_observation()?;
            if before != after
                || executable.path != binding.daemon_path
                || executable.sha256 != binding.daemon_sha256
                || executable.size != binding.daemon_size
                || executable.mode != binding.daemon_mode
                || executable.device != binding.daemon_device
                || executable.inode != binding.daemon_inode
                || executable.designated_requirement != binding.designated_requirement
                || executable.designated_requirement_sha256 != binding.designated_requirement_sha256
                || executable.cdhash != binding.cdhash
            {
                return Err(error(
                    "legacy macOS baseline executable changed during proof",
                ));
            }
        } else {
            let expectation = MacosDirectLaunchdPublicationExpectation::new(
                owner.owner_epoch.saturating_sub(1),
                self.publication_expectation(binding)?,
            )
            .map_err(|source| error(source.to_string()))?;
            let published = self
                .executor
                .wait_for_exact_publication(&expectation, Duration::ZERO)?
                .ok_or_else(|| error("ordinary macOS baseline lacks exact retained publication"))?;
            if published != owner {
                return Err(error("ordinary macOS baseline publication changed"));
            }
        }
        Ok(())
    }

    pub(super) fn capture_stop_authority(
        &mut self,
        expected: &PlatformState,
        record: &MacosRecord,
    ) -> Result<MacosOwnerReceipt, InstallPlatformError> {
        let running = expected
            .running_unit
            .as_ref()
            .filter(|unit| *unit == &record.candidate.unit)
            .ok_or_else(|| error("candidate receipt requires the running macOS candidate"))?;
        let published = self.wait_for_binding_publication(
            &record.candidate,
            record.baseline_owner_epoch.unwrap_or_default(),
        )?;
        Ok(owner_receipt(&published, running.clone()))
    }

    fn wait_for_binding_publication(
        &mut self,
        binding: &MacosUnitBinding,
        after_epoch: u64,
    ) -> Result<MacosOwnerRecord, InstallPlatformError> {
        let observation = self.executor.launchd_observation()?;
        let pid = observation
            .pid
            .ok_or_else(|| error("macOS owner publication proof requires a loaded service"))?;
        let published = if binding.synthetic_legacy {
            let executable = legacy_executable(binding);
            self.executor.wait_for_legacy_publication(
                &executable,
                after_epoch,
                OWNER_PUBLICATION_TIMEOUT,
            )?
        } else {
            let executable = self.publication_expectation(binding)?;
            let expectation =
                MacosDirectLaunchdPublicationExpectation::new(after_epoch, executable)
                    .map_err(|source| error(source.to_string()))?;
            self.executor
                .wait_for_exact_publication(&expectation, OWNER_PUBLICATION_TIMEOUT)?
        }
        .ok_or_else(|| error("exact macOS owner publication did not arrive before deadline"))?;
        if published.active_owner != MacosDaemonOwner::DirectLaunchd
            || published.active_identity.pid != pid
            || published.active_identity.executable_path != Path::new(&binding.daemon_path)
        {
            return Err(error("exact macOS owner publication changed during proof"));
        }
        Ok(published)
    }

    fn publication_expectation(
        &mut self,
        binding: &MacosUnitBinding,
    ) -> Result<MacosDirectLaunchdExecutableExpectation, InstallPlatformError> {
        let binding = if binding.synthetic_legacy {
            let observed = self
                .executor
                .inspect_legacy_executable(None)?
                .ok_or_else(|| error("restored legacy daemon is not exactly observable"))?;
            if observed.path != binding.daemon_path
                || observed.sha256 != binding.daemon_sha256
                || observed.size != binding.daemon_size
                || observed.mode != binding.daemon_mode
                || observed.designated_requirement != binding.designated_requirement
                || observed.designated_requirement_sha256 != binding.designated_requirement_sha256
                || observed.cdhash != binding.cdhash
            {
                return Err(error(
                    "restored legacy daemon identity drifted from its snapshot",
                ));
            }
            MacosUnitBinding {
                daemon_device: observed.device,
                daemon_inode: observed.inode,
                ..binding.clone()
            }
        } else {
            binding.clone()
        };
        MacosDirectLaunchdExecutableExpectation::new(
            binding.daemon_path,
            binding.designated_requirement,
            binding.designated_requirement_sha256,
            binding.cdhash,
            binding.daemon_sha256,
            binding.daemon_mode,
            binding.daemon_size,
            binding.daemon_device,
            binding.daemon_inode,
        )
        .map_err(|source| error(source.to_string()))
    }
}

fn legacy_executable(binding: &MacosUnitBinding) -> MacosLegacyExecutable {
    MacosLegacyExecutable {
        path: binding.daemon_path.clone(),
        sha256: binding.daemon_sha256.clone(),
        size: binding.daemon_size,
        mode: binding.daemon_mode,
        device: binding.daemon_device,
        inode: binding.daemon_inode,
        designated_requirement: binding.designated_requirement.clone(),
        designated_requirement_sha256: binding.designated_requirement_sha256.clone(),
        cdhash: binding.cdhash.clone(),
        version: binding.version.clone(),
    }
}

fn runtime_executable(binding: &MacosUnitBinding) -> super::model::MacosRuntimeExecutable {
    super::model::MacosRuntimeExecutable {
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

fn owner_receipt(record: &MacosOwnerRecord, unit: super::super::UnitId) -> MacosOwnerReceipt {
    MacosOwnerReceipt {
        owner_epoch: record.owner_epoch,
        audit_token_identity: record.active_identity.audit_token_identity.clone(),
        executable_path: record.active_identity.executable_path.clone(),
        designated_requirement_hash: record.active_identity.designated_requirement_hash.clone(),
        pid: record.active_identity.pid,
        unit,
    }
}

fn retained_manifest_version(unit: &UnitRecord) -> Result<String, InstallPlatformError> {
    let mut opened = open_retained_file(unit, MANIFEST_RELATIVE_PATH)?;
    let size = opened.metadata().size();
    if size > MAX_MANIFEST_BYTES {
        return Err(error("retained macOS manifest exceeds its byte bound"));
    }
    let bytes = read_opened(&mut opened, size)?;
    let manifest: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|source| error(format!("invalid retained macOS manifest: {source}")))?;
    manifest
        .get("version")
        .and_then(serde_json::Value::as_str)
        .filter(|version| !version.is_empty() && version.len() <= 128)
        .map(str::to_owned)
        .ok_or_else(|| error("retained macOS manifest has no bounded version"))
}

pub(super) fn open_retained_file(
    unit: &UnitRecord,
    relative: &str,
) -> Result<OpenedRegularFile, InstallPlatformError> {
    let path = Path::new(relative);
    let name = path
        .file_name()
        .ok_or_else(|| error("retained macOS file path has no name"))?;
    let mut directory: Option<hypercolor_platform_fs::ReadOnlyDirectoryAuthority> = None;
    if let Some(parent) = path.parent() {
        for component in parent.components() {
            let std::path::Component::Normal(component) = component else {
                return Err(error("retained macOS file path is not canonical"));
            };
            let child = match directory.as_ref() {
                Some(directory) => directory.open_child_directory(Path::new(component)),
                None => unit.directory().open_child_directory(Path::new(component)),
            }
            .map_err(|source| error(source.to_string()))?;
            directory = Some(child);
        }
    }
    directory
        .as_ref()
        .unwrap_or_else(|| unit.directory())
        .open_regular_file(Path::new(name))
        .map_err(|source| error(source.to_string()))
}

pub(super) fn read_opened(
    opened: &mut OpenedRegularFile,
    size: u64,
) -> Result<Vec<u8>, InstallPlatformError> {
    opened
        .file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|source| error(source.to_string()))?;
    let capacity = usize::try_from(size).map_err(|_| error("retained file does not fit memory"))?;
    let before: DirectoryEntryMetadata = opened.metadata();
    let mut bytes = Vec::with_capacity(capacity);
    opened
        .file_mut()
        .take(size + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| error(source.to_string()))?;
    let after = opened
        .file()
        .metadata()
        .map_err(|source| error(source.to_string()))?;
    if bytes.len() != capacity
        || after.len() != before.size()
        || std::os::unix::fs::MetadataExt::dev(&after) != before.device()
        || std::os::unix::fs::MetadataExt::ino(&after) != before.inode()
    {
        return Err(error("retained macOS file changed during exact read"));
    }
    Ok(bytes)
}

fn exact_path(path: &Path) -> Result<&str, InstallPlatformError> {
    path.to_str()
        .ok_or_else(|| error("macOS launcher path is not exact UTF-8"))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
