use std::fmt::Write as _;
use std::fs::File;
use std::io::Read;
use std::net::SocketAddr;
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use hypercolor_platform_fs::{
    DirectoryAuthority, DirectoryEntryKind, EntryReplacement, ExactEntry, PublicDirectoryAuthority,
};
use sha2::{Digest as _, Sha256};

use super::super::{InstallLock, InstallPlatformError, InstallStore, UnitId, UnitRecord};
use super::directory::{LinuxPublicTree, read_opened_public_bytes};
use super::effects::autostart_operation;
use super::http::http_get;
use super::legacy::{collect_public_legacy_inventory, prepare_legacy_files};
use super::legacy_validation::{
    populate_legacy_stage, validate_legacy_snapshot_binding, validate_legacy_unit,
};
use super::model::{
    LinuxDirectoryItem, LinuxDirectoryState, LinuxExactEntry, LinuxFilePublication,
    LinuxHttpResponse, LinuxLayoutItem, LinuxLayoutPublication, LinuxLegacySnapshot,
    LinuxProcessExecutable, MAX_SYSTEMD_SHOW_BYTES, error, parse_systemd_show,
};
use super::runtime::{LinuxRuntimeManager, LinuxSystemdConnection, RuntimeJobOutcome};

const SYSTEMCTL: &str = "/usr/bin/systemctl";
const TIMEOUT: &str = "/usr/bin/timeout";
const COMMAND_TIMEOUT: &str = "10s";
const MAX_COMMAND_STDERR_BYTES: usize = 16 * 1024;
const MAX_COMMAND_OUTPUT_BYTES: usize = 64 * 1024;
pub(super) const SERVICE: &str = "hypercolor.service";
static LEGACY_STAGE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub trait LinuxInstallExecutor {
    fn validate_topology(
        &mut self,
        config: &super::model::LinuxInstallConfig,
    ) -> Result<(), InstallPlatformError>;
    fn validate_unit_authority(&mut self, unit: &UnitRecord) -> Result<(), InstallPlatformError>;
    fn active_unit(&mut self) -> Result<Option<UnitId>, InstallPlatformError>;
    fn systemd_show(&mut self, max_bytes: usize) -> Result<Vec<u8>, InstallPlatformError>;
    fn launcher_entry(
        &mut self,
        max_bytes: usize,
    ) -> Result<(LinuxExactEntry, Vec<u8>), InstallPlatformError>;
    fn layout_entry(
        &mut self,
        item: LinuxLayoutItem,
    ) -> Result<LinuxExactEntry, InstallPlatformError>;
    fn directory_state(
        &mut self,
        item: LinuxDirectoryItem,
    ) -> Result<LinuxDirectoryState, InstallPlatformError>;
    fn legacy_inventory(
        &mut self,
    ) -> Result<Vec<super::model::LinuxLegacyFile>, InstallPlatformError>;
    fn replace_launcher(
        &mut self,
        expected: &LinuxExactEntry,
        replacement: Option<&LinuxFilePublication>,
    ) -> Result<(), InstallPlatformError>;
    fn replace_layout(
        &mut self,
        item: LinuxLayoutItem,
        expected: &LinuxExactEntry,
        replacement: Option<&LinuxLayoutPublication>,
    ) -> Result<(), InstallPlatformError>;
    fn replace_directory(
        &mut self,
        item: LinuxDirectoryItem,
        expected: LinuxDirectoryState,
        create: bool,
    ) -> Result<(), InstallPlatformError>;
    fn reload_manager(&mut self) -> Result<(), InstallPlatformError>;
    fn set_autostart(&mut self, enabled: bool) -> Result<(), InstallPlatformError>;
    fn set_runtime(&mut self, running: bool) -> Result<(), InstallPlatformError>;
    fn process_executable(
        &mut self,
        pid: u32,
        max_bytes: u64,
    ) -> Result<LinuxProcessExecutable, InstallPlatformError>;
    fn http_get(
        &mut self,
        path: &'static str,
        max_bytes: usize,
    ) -> Result<LinuxHttpResponse, InstallPlatformError>;
    fn snapshot_legacy_unit(
        &mut self,
        snapshot: &LinuxLegacySnapshot,
    ) -> Result<super::super::UnitRecord, InstallPlatformError>;
}

#[derive(Debug)]
pub struct LinuxPublicEntry {
    directory: PublicDirectoryAuthority,
    name: PathBuf,
}

impl LinuxPublicEntry {
    #[must_use]
    pub fn new(directory: PublicDirectoryAuthority, name: impl Into<PathBuf>) -> Self {
        Self {
            directory,
            name: name.into(),
        }
    }
}

#[derive(Debug)]
pub struct LinuxNativeExecutor {
    active: LinuxPublicEntry,
    public_tree: LinuxPublicTree,
    units: DirectoryAuthority,
    units_root_hint: PathBuf,
    http_address: SocketAddr,
    systemd_connection: LinuxSystemdConnection,
    runtime_manager: LinuxRuntimeManager,
}

impl LinuxNativeExecutor {
    pub fn new(
        store: &InstallStore,
        lock: &InstallLock,
        public_tree: LinuxPublicTree,
        http_address: SocketAddr,
    ) -> Result<Self, InstallPlatformError> {
        let connection = LinuxSystemdConnection::from_environment()?;
        Self::new_with_connection(store, lock, public_tree, http_address, connection)
    }

    pub fn new_with_connection(
        store: &InstallStore,
        lock: &InstallLock,
        public_tree: LinuxPublicTree,
        http_address: SocketAddr,
        systemd_connection: LinuxSystemdConnection,
    ) -> Result<Self, InstallPlatformError> {
        if !http_address.ip().is_loopback() {
            return Err(error("Linux owner proof HTTP address must be loopback"));
        }
        let active = LinuxPublicEntry::new(
            lock.open_store_public_directory()
                .map_err(|source| error(source.to_string()))?,
            "active",
        );
        let units = store
            .units_authority(lock)
            .map_err(|source| error(source.to_string()))?;
        let units_root_hint = store.root().join("units");
        let runtime_manager = LinuxRuntimeManager::new(systemd_connection.clone());
        Ok(Self {
            active,
            public_tree,
            units,
            units_root_hint,
            http_address,
            systemd_connection,
            runtime_manager,
        })
    }

    fn layout_entry_authority(
        &self,
        item: LinuxLayoutItem,
    ) -> Result<(PublicDirectoryAuthority, &'static Path), InstallPlatformError> {
        let directory = self.public_tree.open_directory(public_parent(item))?;
        Ok((directory, Path::new(public_name(item))))
    }

    fn launcher_authority(
        &self,
    ) -> Result<(PublicDirectoryAuthority, &'static Path), InstallPlatformError> {
        let directory = self
            .public_tree
            .open_directory(LinuxDirectoryItem::SystemdUser)?;
        Ok((directory, Path::new(SERVICE)))
    }
}

impl LinuxInstallExecutor for LinuxNativeExecutor {
    fn validate_topology(
        &mut self,
        config: &super::model::LinuxInstallConfig,
    ) -> Result<(), InstallPlatformError> {
        let active_root = self
            .units_root_hint
            .parent()
            .ok_or_else(|| error("native Linux units authority has no parent"))?
            .join("active");
        if config.immutable_units_root != self.units_root_hint
            || config.active_root != active_root
            || config.direct_fragment_path != self.public_tree.direct_fragment_path()
        {
            return Err(error(
                "Linux platform config does not match the native store authority",
            ));
        }
        Ok(())
    }

    fn validate_unit_authority(&mut self, unit: &UnitRecord) -> Result<(), InstallPlatformError> {
        let retained = retained_unit(&self.units, &self.units_root_hint, unit.id().clone())?;
        if &retained != unit {
            return Err(error(
                "retained unit does not belong to the native units authority",
            ));
        }
        Ok(())
    }

    fn active_unit(&mut self) -> Result<Option<UnitId>, InstallPlatformError> {
        let exact = self
            .active
            .directory
            .observe_entry(&self.active.name)
            .map_err(io_error)?;
        let ExactEntry::Symlink { target, .. } = exact else {
            return if matches!(exact, ExactEntry::Absent) {
                Ok(None)
            } else {
                Err(error("active unit entry is not an exact symbolic link"))
            };
        };
        parse_active_target(&target).map(Some)
    }

    fn systemd_show(&mut self, max_bytes: usize) -> Result<Vec<u8>, InstallPlatformError> {
        run_systemctl(
            &self.systemd_connection,
            &[
                "show",
                "--no-pager",
                "--property=LoadState,ActiveState,SubState,UnitFileState,FragmentPath,ExecStart,MainPID,InvocationID",
                SERVICE,
            ],
            max_bytes.min(MAX_COMMAND_OUTPUT_BYTES),
        )
    }

    fn launcher_entry(
        &mut self,
        max_bytes: usize,
    ) -> Result<(LinuxExactEntry, Vec<u8>), InstallPlatformError> {
        if self.public_tree.state(LinuxDirectoryItem::SystemdUser)? == LinuxDirectoryState::Absent {
            return Ok((LinuxExactEntry::Absent, Vec::new()));
        }
        let (directory, name) = self.launcher_authority()?;
        read_exact_entry(&directory, name, max_bytes)
    }

    fn layout_entry(
        &mut self,
        item: LinuxLayoutItem,
    ) -> Result<LinuxExactEntry, InstallPlatformError> {
        if self.public_tree.state(public_parent(item))? == LinuxDirectoryState::Absent {
            return Ok(LinuxExactEntry::Absent);
        }
        let (directory, name) = self.layout_entry_authority(item)?;
        directory
            .observe_entry(name)
            .map_err(io_error)
            .and_then(exact_entry)
    }

    fn directory_state(
        &mut self,
        item: LinuxDirectoryItem,
    ) -> Result<LinuxDirectoryState, InstallPlatformError> {
        self.public_tree.state(item)
    }

    fn legacy_inventory(
        &mut self,
    ) -> Result<Vec<super::model::LinuxLegacyFile>, InstallPlatformError> {
        collect_public_legacy_inventory(&self.public_tree)
    }

    fn replace_launcher(
        &mut self,
        expected: &LinuxExactEntry,
        replacement: Option<&LinuxFilePublication>,
    ) -> Result<(), InstallPlatformError> {
        let (directory, name) = self.launcher_authority()?;
        replace_entry(
            &directory,
            name,
            expected,
            replacement.map(|file| LinuxLayoutPublication::RegularFile(file.clone())),
        )
    }

    fn replace_layout(
        &mut self,
        item: LinuxLayoutItem,
        expected: &LinuxExactEntry,
        replacement: Option<&LinuxLayoutPublication>,
    ) -> Result<(), InstallPlatformError> {
        let (directory, name) = self.layout_entry_authority(item)?;
        replace_entry(&directory, name, expected, replacement.cloned())
    }

    fn replace_directory(
        &mut self,
        item: LinuxDirectoryItem,
        expected: LinuxDirectoryState,
        create: bool,
    ) -> Result<(), InstallPlatformError> {
        self.public_tree.replace(item, expected, create)
    }

    fn reload_manager(&mut self) -> Result<(), InstallPlatformError> {
        run_systemctl(
            &self.systemd_connection,
            &["daemon-reload"],
            MAX_COMMAND_OUTPUT_BYTES,
        )
        .map(|_| ())
    }

    fn set_autostart(&mut self, enabled: bool) -> Result<(), InstallPlatformError> {
        let observation = parse_systemd_show(&self.systemd_show(MAX_SYSTEMD_SHOW_BYTES)?)?;
        let Some(operation) = autostart_operation(enabled, &observation) else {
            return Ok(());
        };
        run_systemctl(
            &self.systemd_connection,
            &[operation, "--no-reload", SERVICE],
            MAX_COMMAND_OUTPUT_BYTES,
        )
        .map(|_| ())
    }

    fn set_runtime(&mut self, running: bool) -> Result<(), InstallPlatformError> {
        let outcome = self.runtime_manager.set_runtime(running)?;
        let observation = parse_systemd_show(&run_systemctl(
            &self.systemd_connection,
            &[
                "show",
                "--no-pager",
                "--property=LoadState,ActiveState,SubState,UnitFileState,FragmentPath,ExecStart,MainPID,InvocationID",
                SERVICE,
            ],
            MAX_SYSTEMD_SHOW_BYTES,
        )?)?;
        let stable = if running {
            observation.active_state == "active"
                && observation.sub_state == "running"
                && observation.main_pid != 0
        } else {
            observation.active_state == "inactive"
                && observation.sub_state == "dead"
                && observation.main_pid == 0
        };
        if outcome == RuntimeJobOutcome::Done && stable {
            Ok(())
        } else if outcome == RuntimeJobOutcome::Cancelled {
            Err(error("systemd runtime job was cancelled at its deadline"))
        } else {
            Err(error("systemd runtime job reached the wrong stable state"))
        }
    }

    fn process_executable(
        &mut self,
        pid: u32,
        max_bytes: u64,
    ) -> Result<LinuxProcessExecutable, InstallPlatformError> {
        let proc_exe = PathBuf::from(format!("/proc/{pid}/exe"));
        let mut executable = File::open(&proc_exe).map_err(io_error)?;
        let metadata = executable.metadata().map_err(io_error)?;
        if !metadata.is_file() || metadata.len() != max_bytes {
            return Err(error(
                "opened /proc executable size is not the immutable size",
            ));
        }
        let descriptor = PathBuf::from(format!("/proc/self/fd/{}", executable.as_raw_fd()));
        let path = std::fs::read_link(descriptor).map_err(io_error)?;
        let path = path
            .to_str()
            .filter(|path| path.len() <= 4096)
            .ok_or_else(|| error("opened /proc executable path is not bounded UTF-8"))?
            .to_owned();
        let mut hasher = Sha256::new();
        let copied = std::io::copy(
            &mut std::io::Read::by_ref(&mut executable).take(max_bytes + 1),
            &mut hasher,
        )
        .map_err(io_error)?;
        if copied != max_bytes {
            return Err(error("opened /proc executable changed size while hashing"));
        }
        Ok(LinuxProcessExecutable {
            path,
            sha256: format!("{:x}", hasher.finalize()),
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn http_get(
        &mut self,
        path: &'static str,
        max_bytes: usize,
    ) -> Result<LinuxHttpResponse, InstallPlatformError> {
        if !matches!(path, "/health" | "/api/v1/system") {
            return Err(error("Linux owner proof requested an unapproved HTTP path"));
        }
        http_get(self.http_address, path, max_bytes)
    }

    fn snapshot_legacy_unit(
        &mut self,
        snapshot: &LinuxLegacySnapshot,
    ) -> Result<super::super::UnitRecord, InstallPlatformError> {
        let files = prepare_legacy_files(snapshot, &self.public_tree)?;
        if let Some(metadata) = self
            .units
            .entry_metadata(Path::new(snapshot.unit.as_str()))
            .map_err(io_error)?
        {
            if metadata.kind() != DirectoryEntryKind::Directory {
                return Err(error("legacy snapshot destination is not a directory"));
            }
            let existing = self
                .units
                .open_child_directory(Path::new(snapshot.unit.as_str()))
                .map_err(io_error)?;
            validate_legacy_unit(&existing, &files)?;
            validate_legacy_snapshot_binding(&existing, &snapshot.unit)?;
            return retained_unit(&self.units, &self.units_root_hint, snapshot.unit.clone());
        }
        let sequence = LEGACY_STAGE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let stage_name = PathBuf::from(format!(
            ".hypercolor-stage-legacy-{}-{sequence}",
            std::process::id()
        ));
        let stage = self
            .units
            .create_private_staging_directory(&stage_name)
            .map_err(io_error)?;
        if let Err(source) = populate_legacy_stage(stage.directory(), &files) {
            return match stage.remove() {
                Ok(()) => Err(source),
                Err(cleanup) => Err(error(format!(
                    "{source}; legacy snapshot cleanup failed: {cleanup}"
                ))),
            };
        }
        let published = stage
            .publish_or_remove(Path::new(snapshot.unit.as_str()))
            .map_err(io_error)?;
        validate_legacy_snapshot_binding(&published, &snapshot.unit)?;
        let readonly = published.read_only().map_err(io_error)?;
        super::super::UnitRecord::new(
            snapshot.unit.clone(),
            self.units_root_hint.join(snapshot.unit.as_str()),
            readonly,
        )
        .map_err(io_error)
    }
}

fn parse_active_target(target: &Path) -> Result<UnitId, InstallPlatformError> {
    let mut components = target.components();
    let units = components.next().and_then(|part| part.as_os_str().to_str());
    let unit = components.next().and_then(|part| part.as_os_str().to_str());
    if units != Some("units") || components.next().is_some() {
        return Err(error("active unit link has an invalid exact target"));
    }
    UnitId::new(unit.unwrap_or_default()).map_err(|source| error(source.to_string()))
}

pub(super) fn public_parent(item: LinuxLayoutItem) -> LinuxDirectoryItem {
    match item {
        LinuxLayoutItem::Hypercolor
        | LinuxLayoutItem::HypercolorDaemon
        | LinuxLayoutItem::HypercolorApp
        | LinuxLayoutItem::HypercolorTui
        | LinuxLayoutItem::HypercolorOpen => LinuxDirectoryItem::LocalBin,
        LinuxLayoutItem::DesktopEntry => LinuxDirectoryItem::Applications,
        LinuxLayoutItem::BashCompletion => LinuxDirectoryItem::BashCompletions,
        LinuxLayoutItem::ZshCompletion => LinuxDirectoryItem::ZshSiteFunctions,
        LinuxLayoutItem::FishCompletion => LinuxDirectoryItem::FishVendorCompletions,
        LinuxLayoutItem::Icon48 => LinuxDirectoryItem::Icon48Apps,
        LinuxLayoutItem::Icon128 => LinuxDirectoryItem::Icon128Apps,
        LinuxLayoutItem::Icon256 => LinuxDirectoryItem::Icon256Apps,
    }
}

pub(super) fn public_name(item: LinuxLayoutItem) -> &'static str {
    match item {
        LinuxLayoutItem::Hypercolor => "hypercolor",
        LinuxLayoutItem::HypercolorDaemon => "hypercolor-daemon",
        LinuxLayoutItem::HypercolorApp => "hypercolor-app",
        LinuxLayoutItem::HypercolorTui => "hypercolor-tui",
        LinuxLayoutItem::HypercolorOpen => "hypercolor-open",
        LinuxLayoutItem::DesktopEntry => "hypercolor.desktop",
        LinuxLayoutItem::BashCompletion => "hypercolor",
        LinuxLayoutItem::ZshCompletion => "_hypercolor",
        LinuxLayoutItem::FishCompletion => "hypercolor.fish",
        LinuxLayoutItem::Icon48 | LinuxLayoutItem::Icon128 | LinuxLayoutItem::Icon256 => {
            "hypercolor.png"
        }
    }
}

pub(super) fn read_exact_entry(
    directory: &PublicDirectoryAuthority,
    name: &Path,
    max_bytes: usize,
) -> Result<(LinuxExactEntry, Vec<u8>), InstallPlatformError> {
    let observed = directory.observe_entry(name).map_err(io_error)?;
    let ExactEntry::RegularFile { .. } = observed else {
        return exact_entry(observed).map(|exact| (exact, Vec::new()));
    };
    let mut opened = directory.open_regular_file(name).map_err(io_error)?;
    let initial_size = opened.metadata().size();
    let bytes = read_opened_public_bytes(&mut opened, initial_size, max_bytes)?;
    let exact = LinuxExactEntry::RegularFile {
        mode: opened.metadata().mode(),
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        snapshot_unit: None,
        snapshot_path: None,
    };
    if exact_entry(observed)? != exact {
        return Err(error("public regular entry changed during exact read"));
    }
    Ok((exact, bytes))
}

fn exact_entry(entry: ExactEntry) -> Result<LinuxExactEntry, InstallPlatformError> {
    match entry {
        ExactEntry::Absent => Ok(LinuxExactEntry::Absent),
        ExactEntry::RegularFile { mode, sha256, .. } => Ok(LinuxExactEntry::RegularFile {
            mode,
            sha256: hex_array(sha256),
            snapshot_unit: None,
            snapshot_path: None,
        }),
        ExactEntry::Symlink { target, .. } => Ok(LinuxExactEntry::Symlink {
            target: target
                .to_str()
                .filter(|target| target.len() <= 4096)
                .ok_or_else(|| error("public symbolic link target is not bounded UTF-8"))?
                .to_owned(),
        }),
    }
}

fn replace_entry(
    directory: &PublicDirectoryAuthority,
    name: &Path,
    expected: &LinuxExactEntry,
    replacement: Option<LinuxLayoutPublication>,
) -> Result<(), InstallPlatformError> {
    let observed = directory.observe_entry(name).map_err(io_error)?;
    if !super::model::entries_match(&exact_entry(observed.clone())?, expected) {
        return Err(error("public entry drifted before exact replacement"));
    }
    match replacement {
        Some(LinuxLayoutPublication::RegularFile(file)) => directory
            .durable_replace_entry(
                name,
                &observed,
                EntryReplacement::RegularFile {
                    mode: file.mode,
                    contents: &file.contents,
                },
            )
            .map(|_| ())
            .map_err(io_error),
        Some(LinuxLayoutPublication::Symlink(target)) => directory
            .durable_replace_entry(
                name,
                &observed,
                EntryReplacement::Symlink {
                    target: Path::new(&target),
                },
            )
            .map(|_| ())
            .map_err(io_error),
        None if matches!(observed, ExactEntry::Absent) => Ok(()),
        None => directory
            .durable_remove_entry(name, &observed)
            .map_err(io_error),
    }
}

fn retained_unit(
    units: &DirectoryAuthority,
    units_root_hint: &Path,
    unit: UnitId,
) -> Result<super::super::UnitRecord, InstallPlatformError> {
    let directory = units
        .open_child_directory(Path::new(unit.as_str()))
        .map_err(io_error)?;
    let readonly = directory.read_only().map_err(io_error)?;
    super::super::UnitRecord::new(unit.clone(), units_root_hint.join(unit.as_str()), readonly)
        .map_err(io_error)
}

fn run_systemctl(
    connection: &LinuxSystemdConnection,
    args: &[&str],
    max_stdout: usize,
) -> Result<Vec<u8>, InstallPlatformError> {
    let mut child = systemctl_command(connection, args)
        .spawn()
        .map_err(io_error)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| error("missing command stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| error("missing command stderr"))?;
    let stdout_thread = std::thread::spawn(move || read_bounded(stdout, max_stdout));
    let stderr_thread = std::thread::spawn(move || read_bounded(stderr, MAX_COMMAND_STDERR_BYTES));
    let status = child.wait().map_err(io_error)?;
    let stdout = stdout_thread
        .join()
        .map_err(|_| error("systemctl stdout reader panicked"))??;
    let stderr = stderr_thread
        .join()
        .map_err(|_| error("systemctl stderr reader panicked"))??;
    if !status.success() {
        let detail = String::from_utf8_lossy(&stderr);
        return Err(error(format!(
            "bounded noninteractive systemctl failed with {status}: {}",
            detail.trim()
        )));
    }
    Ok(stdout)
}

pub(super) fn systemctl_command(connection: &LinuxSystemdConnection, args: &[&str]) -> Command {
    let (connection_name, connection_value) = connection.command_environment();
    let mut command = Command::new(TIMEOUT);
    command
        .args([
            "--signal=KILL",
            COMMAND_TIMEOUT,
            SYSTEMCTL,
            "--user",
            "--no-ask-password",
        ])
        .args(args)
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env(connection_name, connection_value)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn read_bounded(mut reader: impl Read, max_bytes: usize) -> Result<Vec<u8>, InstallPlatformError> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > max_bytes {
        return Err(error("command output exceeds its byte bound"));
    }
    Ok(bytes)
}

fn io_error(source: std::io::Error) -> InstallPlatformError {
    error(source.to_string())
}

fn hex_array(bytes: [u8; 32]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}
