use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;

use sha2::{Digest as _, Sha256};

use super::super::{InstallPlatformError, PlatformCheckpoint, PlatformState, UnitRecord};
use super::LinuxInstallPlatform;
use super::executor::LinuxInstallExecutor;
use super::model::{
    DAEMON_RELATIVE_PATH, LAUNCHER_MODE, LinuxExactEntry, LinuxHttpResponse, LinuxLauncher,
    LinuxOwnerReceipt, LinuxRecord, LinuxServiceIdentity, LinuxServicePhase, LinuxServiceWatch,
    LinuxSystemdObservation, LinuxUnitBinding, MAX_HTTP_RESPONSE_BYTES, MAX_SYSTEMD_SHOW_BYTES,
    error, parse_systemd_show,
};
use super::systemd::canonical_launcher_exec;

impl<E: LinuxInstallExecutor> LinuxInstallPlatform<E> {
    pub(super) fn unit_binding(
        &self,
        unit: &UnitRecord,
    ) -> Result<LinuxUnitBinding, InstallPlatformError> {
        self.unit_binding_at(unit, &self.config.immutable_units_root)
    }

    pub(super) fn unit_binding_at(
        &self,
        unit: &UnitRecord,
        units_root: &Path,
    ) -> Result<LinuxUnitBinding, InstallPlatformError> {
        retained_unit_binding(unit, units_root)
    }

    pub(super) fn candidate_launcher(&self) -> Result<LinuxLauncher, InstallPlatformError> {
        render_launcher(&self.config.active_root, self.config.managed.as_ref())
    }

    pub(super) fn layout_target(
        &self,
        candidate: &super::super::UnitId,
        item: super::model::LinuxLayoutItem,
    ) -> String {
        let _ = candidate;
        layout_target_for(&self.config.active_root, item)
    }
}

/// Render the exact generated service unit.
///
/// The historical root's unit runs the daemon straight through `active`.
/// A managed installation's unit runs it through the installation's stable
/// launcher, which selects one release and derives the daemon's UI and
/// effects directories from that same release, inside a sandbox that makes
/// everything read-only except the recorded configuration, data and daemon
/// state roots and the coordinator's directory, with the release root and
/// the update state root read-only beneath them. systemd refuses to build
/// that sandbox around a writable root that does not exist, so the launcher
/// first runs unsandboxed (`ExecStartPre=+`) to create any recorded root a
/// user deleted. The text depends only on the recorded location, so it is
/// the same for every release of launcher contract 1.
///
/// # Errors
/// Refuses a non-UTF-8 root or a launcher that exceeds its byte bound.
pub(super) fn render_launcher(
    active_root: &Path,
    managed: Option<&super::LinuxInstallLocation>,
) -> Result<LinuxLauncher, InstallPlatformError> {
    let text = |path: &Path| {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| error("Linux install roots must be exact UTF-8"))
    };
    let (prepare, launcher_exec, sandbox) = match managed {
        None => {
            let active = text(active_root)?;
            (
                String::new(),
                format!(
                    "{active}/bin/hypercolor-daemon --ui-dir {active}/share/hypercolor/ui --effects-dir {active}/share/hypercolor/effects/bundled"
                ),
                String::new(),
            )
        }
        Some(location) => {
            if active_root != location.release_root().join("active") {
                return Err(error(
                    "the managed service must select the recorded release root",
                ));
            }
            let launcher = text(&super::bootstrap::linux_launcher_path(location))?;
            let state = text(location.state_root())?;
            let command = super::bootstrap::LINUX_LAUNCH_COMMAND;
            (
                format!(
                    "ExecStartPre=+{launcher} {command} --role {}\n",
                    super::launch::LinuxLaunchRole::PrepareRoots.as_str()
                ),
                format!(
                    "{launcher} {command} --role {}",
                    super::launch::LinuxLaunchRole::Daemon.as_str()
                ),
                format!(
                    "ProtectSystem=strict\nProtectHome=read-only\nPrivateTmp=true\nNoNewPrivileges=true\nReadWritePaths={config} {data} {daemon_state} -{state}/coordinator\nReadOnlyPaths={releases} {state}\n",
                    config = text(location.config_root())?,
                    data = text(location.data_root())?,
                    daemon_state = text(location.daemon_state_root())?,
                    releases = text(location.release_root())?,
                ),
            )
        }
    };
    let bytes = format!(
        "[Unit]\nDescription=Hypercolor RGB Lighting Daemon\nAfter=graphical-session.target dbus.socket\nWants=graphical-session.target\n\n[Service]\nType=notify\n{prepare}ExecStart={launcher_exec}\nWatchdogSec=30\nRestart=on-failure\nRestartSec=3\nEnvironment=HYPERCOLOR_LOG=info\nEnvironment=RUST_BACKTRACE=1\nEnvironment=HYPERCOLOR_SERVICE_IDENTITY=user_service:systemd:hypercolor.service\n{sandbox}\n[Install]\nWantedBy=default.target\n"
    )
    .into_bytes();
    if bytes.len() > super::model::MAX_LAUNCHER_BYTES {
        return Err(error("rendered Linux launcher exceeds its byte bound"));
    }
    Ok(LinuxLauncher {
        mode: LAUNCHER_MODE,
        bytes,
        exec_start: canonical_launcher_exec(&launcher_exec)?,
    })
}

/// The exact public symlink target one layout item names beneath a root.
pub(super) fn layout_target_for(active_root: &Path, item: super::model::LinuxLayoutItem) -> String {
    active_root
        .join(item.unit_path())
        .to_str()
        .expect("Linux install roots were validated as exact UTF-8")
        .to_owned()
}

impl<E: LinuxInstallExecutor> LinuxInstallPlatform<E> {
    pub(super) fn prove_owner(
        &mut self,
        checkpoint: PlatformCheckpoint,
        expected: &PlatformState,
        record: &LinuxRecord,
        candidate_receipt: Option<&LinuxOwnerReceipt>,
    ) -> Result<Option<LinuxOwnerReceipt>, InstallPlatformError> {
        let before = parse_systemd_show(&self.executor.systemd_show(MAX_SYSTEMD_SHOW_BYTES)?)?;
        if expected.running_unit.is_none() {
            if before.phase() != LinuxServicePhase::Stopped {
                return Err(error("inactive proof observed a running systemd service"));
            }
            return Ok(None);
        }
        let unit = expected
            .running_unit
            .as_ref()
            .expect("checked running unit");
        let candidate_owner = match checkpoint {
            PlatformCheckpoint::CandidateRuntime => true,
            PlatformCheckpoint::PriorRestored => false,
            _ => return Err(error("owner proof received an invalid checkpoint")),
        };
        let binding = if candidate_owner {
            if unit != &record.candidate.unit {
                return Err(error("candidate owner proof requested an unknown unit"));
            }
            &record.candidate
        } else {
            record
                .prior
                .as_ref()
                .filter(|prior| &prior.unit == unit)
                .ok_or_else(|| error("prior owner proof requested an unknown unit"))?
        };
        let expected_exec = if candidate_owner {
            &record
                .candidate_launcher
                .as_ref()
                .ok_or_else(|| error("running candidate lacks launcher metadata"))?
                .exec_start
        } else {
            &require_notify_launcher(&record.prior_launcher_bytes)?
        };
        require_running_observation(&before, &self.config.direct_fragment_path, expected_exec)?;
        if before.invocation_id == record.baseline_systemd.invocation_id {
            return Err(error("systemd owner invocation is not fresh"));
        }
        if candidate_owner {
            let receipt = candidate_receipt
                .filter(|receipt| receipt.unit == record.candidate.unit)
                .ok_or_else(|| error("candidate owner proof lacks its stop authority receipt"))?;
            if receipt.invocation_id != before.invocation_id || receipt.main_pid != before.main_pid
            {
                return Err(error(
                    "candidate systemd owner no longer matches its receipt",
                ));
            }
        } else if candidate_receipt
            .is_some_and(|receipt| receipt.invocation_id == before.invocation_id)
        {
            return Err(error(
                "rollback systemd invocation reused the candidate identity",
            ));
        }
        let process = self
            .executor
            .process_executable(before.main_pid, binding.daemon_size)?;
        if process.path != binding.daemon_path
            || process.sha256 != binding.daemon_sha256
            || (candidate_owner || !unit.as_str().starts_with("legacy-"))
                && (process.device != binding.daemon_device
                    || process.inode != binding.daemon_inode)
        {
            return Err(error(
                "/proc executable identity does not match the immutable unit",
            ));
        }
        self.require_daemon_arguments(&process, &before.exec_start, binding)?;
        verify_http(
            self.executor.http_get("/health", MAX_HTTP_RESPONSE_BYTES)?,
            self.executor
                .http_get("/api/v1/system", MAX_HTTP_RESPONSE_BYTES)?,
            &binding.version,
        )?;
        let after = parse_systemd_show(&self.executor.systemd_show(MAX_SYSTEMD_SHOW_BYTES)?)?;
        if after != before {
            return Err(error("systemd owner changed during process and HTTP proof"));
        }
        Ok(Some(LinuxOwnerReceipt {
            invocation_id: before.invocation_id,
            main_pid: before.main_pid,
            unit: unit.clone(),
        }))
    }

    /// Hold a proven candidate through its probation window.
    ///
    /// The service must keep the identity the proof just established, with
    /// no change to its state, invocation or main process, for the whole
    /// window; then the whole proof runs again, which is what catches a
    /// daemon whose API hung while its watchdog kept pinging. A change ends
    /// the window at once as a candidate failure, so the transaction rolls
    /// back. An installer that dies mid-window leaves the journal at
    /// `ProveCandidate` with the same receipt, and the next run holds the
    /// candidate through a whole window again.
    pub(super) fn hold_probation(
        &mut self,
        expected: &PlatformState,
        record: &LinuxRecord,
        candidate_receipt: Option<&LinuxOwnerReceipt>,
        owner: &LinuxOwnerReceipt,
    ) -> Result<(), InstallPlatformError> {
        let window = self.config.probation;
        let identity = LinuxServiceIdentity {
            invocation_id: owner.invocation_id.clone(),
            main_pid: owner.main_pid,
        };
        if let LinuxServiceWatch::Changed { after, observed } =
            self.executor.watch_service(&identity, window)?
        {
            return Err(error(format!(
                "candidate failed probation {:.1} s into its {} s window: the service went {observed}",
                after.as_secs_f64(),
                window.as_secs()
            )));
        }
        self.prove_owner(
            PlatformCheckpoint::CandidateRuntime,
            expected,
            record,
            candidate_receipt,
        )
        .map_err(|source| {
            error(format!(
                "candidate failed the proof at the end of its {} s probation window: {source}",
                window.as_secs()
            ))
        })
        .map(drop)
    }

    pub(super) fn capture_stop_authority(
        &mut self,
        expected: &PlatformState,
        record: &LinuxRecord,
    ) -> Result<LinuxOwnerReceipt, InstallPlatformError> {
        let unit = expected
            .running_unit
            .as_ref()
            .filter(|unit| *unit == &record.candidate.unit)
            .ok_or_else(|| error("candidate receipt requires the running candidate unit"))?;
        let launcher = record
            .candidate_launcher
            .as_ref()
            .ok_or_else(|| error("running candidate lacks launcher metadata"))?;
        let before = parse_systemd_show(&self.executor.systemd_show(MAX_SYSTEMD_SHOW_BYTES)?)?;
        require_running_observation(
            &before,
            &self.config.direct_fragment_path,
            &launcher.exec_start,
        )?;
        if before.invocation_id == record.baseline_systemd.invocation_id {
            return Err(error("candidate systemd owner invocation is not fresh"));
        }
        let after = parse_systemd_show(&self.executor.systemd_show(MAX_SYSTEMD_SHOW_BYTES)?)?;
        if after != before {
            return Err(error(
                "candidate systemd owner changed during receipt capture",
            ));
        }
        Ok(LinuxOwnerReceipt {
            invocation_id: before.invocation_id,
            main_pid: before.main_pid,
            unit: unit.clone(),
        })
    }

    pub(super) fn prove_baseline_owner(
        &mut self,
        prior: &PlatformState,
        record: &LinuxRecord,
    ) -> Result<(), InstallPlatformError> {
        let Some(unit) = &prior.running_unit else {
            return Ok(());
        };
        let binding = record
            .prior
            .as_ref()
            .filter(|binding| &binding.unit == unit)
            .ok_or_else(|| error("running prior owner lacks an exact immutable binding"))?;
        let expected_exec = require_notify_launcher(&record.prior_launcher_bytes)?;
        let before = parse_systemd_show(&self.executor.systemd_show(MAX_SYSTEMD_SHOW_BYTES)?)?;
        if before != record.baseline_systemd {
            return Err(error("prior systemd owner changed before baseline proof"));
        }
        require_running_observation(&before, &self.config.direct_fragment_path, &expected_exec)?;
        let process = self
            .executor
            .process_executable(before.main_pid, binding.daemon_size)?;
        if process.path != binding.daemon_path
            || process.sha256 != binding.daemon_sha256
            || !unit.as_str().starts_with("legacy-")
                && (process.device != binding.daemon_device
                    || process.inode != binding.daemon_inode)
        {
            return Err(error(
                "prior /proc executable identity does not match its retained unit",
            ));
        }
        self.require_daemon_arguments(&process, &before.exec_start, binding)?;
        verify_http(
            self.executor.http_get("/health", MAX_HTTP_RESPONSE_BYTES)?,
            self.executor
                .http_get("/api/v1/system", MAX_HTTP_RESPONSE_BYTES)?,
            &binding.version,
        )?;
        let after = parse_systemd_show(&self.executor.systemd_show(MAX_SYSTEMD_SHOW_BYTES)?)?;
        if after != before {
            return Err(error("prior systemd owner changed during baseline proof"));
        }
        Ok(())
    }
}

impl<E: LinuxInstallExecutor> LinuxInstallPlatform<E> {
    /// Prove the daemon runs with the UI and effects directories of the
    /// same release its executable belongs to.
    ///
    /// Under the launcher, every path comes from the one release it
    /// selected, so the arguments name exactly that release's directories.
    /// Under a direct unit they are the unit's own `ExecStart` arguments.
    /// Legacy snapshots, whose historical units this installer did not
    /// write, keep only the executable proof.
    pub(super) fn require_daemon_arguments(
        &self,
        process: &super::model::LinuxProcessExecutable,
        exec_start: &str,
        binding: &LinuxUnitBinding,
    ) -> Result<(), InstallPlatformError> {
        if binding.unit.as_str().starts_with("legacy-") {
            return Ok(());
        }
        let expected =
            expected_daemon_arguments(exec_start, self.config.managed.as_ref(), binding)?;
        if process.arguments != expected {
            return Err(error(format!(
                "the daemon of {} runs with arguments {:?}, not {:?}",
                binding.unit.as_str(),
                process.arguments,
                expected
            )));
        }
        Ok(())
    }
}

/// The argument vector the daemon of `binding` runs with when its service's
/// canonical `ExecStart` is `exec_start`.
pub(super) fn expected_daemon_arguments(
    exec_start: &str,
    managed: Option<&super::LinuxInstallLocation>,
    binding: &LinuxUnitBinding,
) -> Result<Vec<String>, InstallPlatformError> {
    let words: Vec<String> = serde_json::from_str(exec_start)
        .map_err(|_| error("canonical ExecStart argument vector is malformed"))?;
    let launched = managed.is_some_and(|location| {
        super::bootstrap::linux_launcher_path(location).to_str()
            == words.first().map(String::as_str)
            && words[1..]
                == [
                    super::bootstrap::LINUX_LAUNCH_COMMAND,
                    "--role",
                    super::launch::LinuxLaunchRole::Daemon.as_str(),
                ]
    });
    if !launched {
        return Ok(words);
    }
    let release = Path::new(&binding.daemon_path)
        .parent()
        .and_then(Path::parent)
        .and_then(Path::to_str)
        .ok_or_else(|| error("the daemon path names no release directory"))?;
    Ok(vec![
        binding.daemon_path.clone(),
        "--ui-dir".to_owned(),
        format!("{release}/share/hypercolor/ui"),
        "--effects-dir".to_owned(),
        format!("{release}/share/hypercolor/effects/bundled"),
    ])
}

pub(super) fn retained_unit_binding(
    unit: &UnitRecord,
    units_root: &Path,
) -> Result<LinuxUnitBinding, InstallPlatformError> {
    let daemon = open_unit_file(unit, DAEMON_RELATIVE_PATH)?;
    let daemon_size = daemon.metadata().size();
    let daemon_device = daemon.metadata().device();
    let daemon_inode = daemon.metadata().inode();
    let daemon_sha256 = hash_opened(daemon, daemon_size)?;
    let manifest = read_unit_file(
        unit,
        "manifest.json",
        super::model::MAX_MANIFEST_BYTES as u64,
    )?;
    let manifest: serde_json::Value = serde_json::from_slice(&manifest)
        .map_err(|source| error(format!("invalid retained unit manifest: {source}")))?;
    let version = manifest
        .get("version")
        .and_then(serde_json::Value::as_str)
        .filter(|version| !version.is_empty() && version.len() <= 128)
        .ok_or_else(|| error("retained unit manifest has no bounded version"))?
        .to_owned();
    let daemon_path = units_root
        .join(unit.id().as_str())
        .join(DAEMON_RELATIVE_PATH)
        .to_str()
        .expect("Linux install roots were validated as exact UTF-8")
        .to_owned();
    Ok(LinuxUnitBinding {
        unit: unit.id().clone(),
        daemon_path,
        daemon_sha256,
        daemon_size,
        daemon_device,
        daemon_inode,
        version,
    })
}

pub(super) fn validate_prior_launcher_entry(
    entry: &LinuxExactEntry,
    bytes: &[u8],
) -> Result<(), InstallPlatformError> {
    match entry {
        LinuxExactEntry::Absent if bytes.is_empty() => Ok(()),
        LinuxExactEntry::RegularFile { mode, sha256, .. }
            if matches!(*mode, 0o400 | 0o440 | 0o444 | 0o600 | 0o640 | LAUNCHER_MODE)
                && bytes.len() <= super::model::MAX_LAUNCHER_BYTES
                && *sha256 == super::model::hex_digest(bytes) =>
        {
            Ok(())
        }
        LinuxExactEntry::Absent
        | LinuxExactEntry::RegularFile { .. }
        | LinuxExactEntry::Symlink { .. } => Err(error(
            "prior launcher entry bytes, type, mode, or digest are invalid",
        )),
    }
}

pub(super) fn require_notify_launcher(bytes: &[u8]) -> Result<String, InstallPlatformError> {
    let text = std::str::from_utf8(bytes).map_err(|_| error("launcher is not UTF-8"))?;
    let mut service = false;
    let mut unit_type = None;
    let mut exec_start = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            service = line == "[Service]";
            continue;
        }
        if !service || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(value) = line.strip_prefix("Type=")
            && unit_type.replace(value).is_some()
        {
            return Err(error("launcher has duplicate Type"));
        }
        if let Some(value) = line.strip_prefix("ExecStart=")
            && exec_start.replace(value.to_owned()).is_some()
        {
            return Err(error("launcher has duplicate ExecStart"));
        }
    }
    if unit_type != Some("notify") {
        return Err(error("raw-direct systemd launcher must use Type=notify"));
    }
    canonical_launcher_exec(
        &exec_start.ok_or_else(|| error("raw-direct systemd launcher lacks ExecStart"))?,
    )
}

pub(super) fn candidate_systemd(
    record: &LinuxRecord,
    fragment_path: &str,
    running: bool,
    autostart_enabled: bool,
) -> LinuxSystemdObservation {
    let Some(launcher) = &record.candidate_launcher else {
        return absent_systemd();
    };
    LinuxSystemdObservation {
        load_state: "loaded".to_owned(),
        active_state: if running { "active" } else { "inactive" }.to_owned(),
        sub_state: if running { "running" } else { "dead" }.to_owned(),
        unit_file_state: if autostart_enabled {
            "enabled"
        } else {
            "disabled"
        }
        .to_owned(),
        fragment_path: fragment_path.to_owned(),
        exec_start: launcher.exec_start.clone(),
        main_pid: u32::from(running),
        invocation_id: String::new(),
    }
}

pub(super) fn prior_systemd(
    record: &LinuxRecord,
    running: bool,
    autostart_enabled: bool,
) -> Result<LinuxSystemdObservation, InstallPlatformError> {
    if matches!(record.prior_launcher, LinuxExactEntry::Absent) {
        return Ok(absent_systemd());
    }
    let exec_start = require_notify_launcher(&record.prior_launcher_bytes)?;
    Ok(LinuxSystemdObservation {
        load_state: record.baseline_systemd.load_state.clone(),
        active_state: if running { "active" } else { "inactive" }.to_owned(),
        sub_state: if running { "running" } else { "dead" }.to_owned(),
        unit_file_state: if autostart_enabled {
            "enabled"
        } else {
            "disabled"
        }
        .to_owned(),
        fragment_path: record.baseline_systemd.fragment_path.clone(),
        exec_start,
        main_pid: u32::from(running),
        invocation_id: String::new(),
    })
}

fn absent_systemd() -> LinuxSystemdObservation {
    LinuxSystemdObservation {
        load_state: "not-found".to_owned(),
        active_state: "inactive".to_owned(),
        sub_state: "dead".to_owned(),
        unit_file_state: String::new(),
        fragment_path: String::new(),
        exec_start: String::new(),
        main_pid: 0,
        invocation_id: String::new(),
    }
}

/// Whether an observation is the expected definition in the expected phase.
///
/// `failed` and `inactive` are the same stopped phase: a service that ended
/// in failure is not running, and starting it resets the failure first.
pub(super) fn systemd_equivalent(
    actual: &LinuxSystemdObservation,
    expected: &LinuxSystemdObservation,
) -> bool {
    actual.load_state == expected.load_state
        && actual.phase() == expected.phase()
        && actual.unit_file_state == expected.unit_file_state
        && actual.fragment_path == expected.fragment_path
        && actual.exec_start == expected.exec_start
        && (actual.phase() == LinuxServicePhase::Running || actual.main_pid == 0)
}

pub(super) fn require_running_observation(
    observation: &LinuxSystemdObservation,
    fragment: &str,
    exec_start: &str,
) -> Result<(), InstallPlatformError> {
    if observation.active_state != "active"
        || observation.sub_state != "running"
        || observation.main_pid == 0
        || observation.invocation_id.is_empty()
        || observation.fragment_path != fragment
        || observation.exec_start != exec_start
    {
        return Err(error(
            "systemd observation does not identify the expected running direct unit",
        ));
    }
    Ok(())
}

pub(super) fn verify_http(
    health: LinuxHttpResponse,
    system: LinuxHttpResponse,
    version: &str,
) -> Result<(), InstallPlatformError> {
    if health.status != 200
        || system.status != 200
        || health.body.len() > super::model::MAX_HTTP_RESPONSE_BYTES
        || system.body.len() > super::model::MAX_HTTP_RESPONSE_BYTES
    {
        return Err(error("bounded daemon HTTP proof failed"));
    }
    let health: serde_json::Value = serde_json::from_slice(&health.body)
        .map_err(|_| error("invalid /health identity response"))?;
    let system: serde_json::Value = serde_json::from_slice(&system.body)
        .map_err(|_| error("invalid /api/v1/system identity response"))?;
    if health.get("status").and_then(serde_json::Value::as_str) != Some("healthy")
        || health.get("version").and_then(serde_json::Value::as_str) != Some(version)
        || system
            .pointer("/data/identity/version")
            .and_then(serde_json::Value::as_str)
            != Some(version)
        || system
            .pointer("/data/identity/instance_id")
            .and_then(serde_json::Value::as_str)
            .is_none_or(str::is_empty)
        || system
            .pointer("/data/identity/instance_name")
            .and_then(serde_json::Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(error(
            "daemon HTTP identity does not match the immutable unit",
        ));
    }
    Ok(())
}

pub(super) fn read_health_version(
    health: LinuxHttpResponse,
) -> Result<String, InstallPlatformError> {
    if health.status != 200 || health.body.len() > super::model::MAX_HTTP_RESPONSE_BYTES {
        return Err(error("bounded legacy health proof failed"));
    }
    let health: serde_json::Value = serde_json::from_slice(&health.body)
        .map_err(|_| error("invalid legacy /health identity response"))?;
    if health.get("status").and_then(serde_json::Value::as_str) != Some("healthy") {
        return Err(error("legacy /health response is not healthy"));
    }
    health
        .get("version")
        .and_then(serde_json::Value::as_str)
        .filter(|version| !version.is_empty() && version.len() <= 128)
        .map(str::to_owned)
        .ok_or_else(|| error("legacy /health response has no bounded version"))
}

pub(super) fn open_unit_file(
    unit: &UnitRecord,
    path: &str,
) -> Result<hypercolor_platform_fs::OpenedRegularFile, InstallPlatformError> {
    let mut components = path.split('/');
    let first = components
        .next()
        .ok_or_else(|| error("empty unit-relative path"))?;
    let mut directory = unit
        .directory()
        .open_child_directory(Path::new(first))
        .map_err(|source| error(source.to_string()))?;
    let rest = components.collect::<Vec<_>>();
    for component in &rest[..rest.len().saturating_sub(1)] {
        directory = directory
            .open_child_directory(Path::new(component))
            .map_err(|source| error(source.to_string()))?;
    }
    directory
        .open_regular_file(Path::new(rest.last().copied().unwrap_or(first)))
        .map_err(|source| error(source.to_string()))
}

pub(super) fn read_unit_file(
    unit: &UnitRecord,
    path: &str,
    max: u64,
) -> Result<Vec<u8>, InstallPlatformError> {
    let mut opened = if path.contains('/') {
        open_unit_file(unit, path)?
    } else {
        unit.directory()
            .open_regular_file(Path::new(path))
            .map_err(|source| error(source.to_string()))?
    };
    let size = opened.metadata().size();
    if size > max {
        return Err(error("retained unit file exceeds its byte bound"));
    }
    let capacity =
        usize::try_from(size).map_err(|_| error("retained unit file does not fit in memory"))?;
    let mut bytes = Vec::with_capacity(capacity);
    opened
        .file_mut()
        .take(size + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| error(source.to_string()))?;
    if bytes.len() != capacity {
        return Err(error("retained unit file changed size while reading"));
    }
    Ok(bytes)
}

fn hash_opened(
    mut opened: hypercolor_platform_fs::OpenedRegularFile,
    size: u64,
) -> Result<String, InstallPlatformError> {
    opened
        .file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|source| error(source.to_string()))?;
    let mut hasher = Sha256::new();
    let copied = std::io::copy(
        &mut opened.file_mut().take(size + 1),
        &mut super::model::Sha256Writer(&mut hasher),
    )
    .map_err(|source| error(source.to_string()))?;
    if copied != size {
        return Err(error("retained daemon size changed while hashing"));
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod identity_tests {
    use super::{LinuxHttpResponse, verify_http};
    use serde_json::json;

    fn response(value: serde_json::Value) -> LinuxHttpResponse {
        LinuxHttpResponse {
            status: 200,
            body: serde_json::to_vec(&value).expect("serialize identity response fixture"),
        }
    }

    #[test]
    fn owner_proof_uses_the_system_resources_nested_public_identity() {
        let health = response(json!({"status":"healthy", "version":"0.4.0"}));
        let system = response(json!({"data":{"identity":{
            "version":"0.4.0", "instance_id":"daemon-id", "instance_name":"lights"
        },"status":null}}));
        verify_http(health, system, "0.4.0")
            .expect("public system identity without privileged status");
    }

    #[test]
    fn owner_proof_rejects_retired_flat_missing_and_mismatched_system_identity() {
        for system in [
            json!({"data":{"version":"0.4.0", "instance_id":"id", "instance_name":"name"}}),
            json!({"data":{"status":{}}}),
            json!({"data":{"identity":{"version":"0.3.0", "instance_id":"id", "instance_name":"name"}}}),
            json!({"data":{"identity":{"version":"0.4.0", "instance_id":"", "instance_name":"name"}}}),
            json!({"data":{"identity":{"version":"0.4.0", "instance_id":"id", "instance_name":""}}}),
        ] {
            assert!(
                verify_http(
                    response(json!({"status":"healthy", "version":"0.4.0"})),
                    response(system),
                    "0.4.0"
                )
                .is_err()
            );
        }
    }
}
