#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use hypercolor_cli::install::{
    InstallAction, InstallCoordinator, InstallDisposition, InstallPlatform, InstallRequest,
    InstallStore, InstallTargetPolicy, InstallTransactionId, InstallationState,
    LINUX_DIRECTORY_ITEMS, LINUX_LAYOUT_ITEMS, LinuxDirectoryItem, LinuxDirectoryState,
    LinuxExactEntry, LinuxFilePublication, LinuxHttpResponse, LinuxInstallConfig,
    LinuxInstallExecutor, LinuxInstallPlatform, LinuxLayoutItem, LinuxLayoutPublication,
    LinuxLegacyFile, LinuxProcessExecutable, LinuxPublicTree, PlatformCheckpoint,
    PlatformOwnerReceipt, PlatformState, PlatformTransactionRecord, UnitId,
    bind_linux_retained_unit, parse_systemd_show, retain_linux_unit, stage_release_payload,
};
use hypercolor_platform_fs::ExclusiveDirectory;
use serde_json::json;
use sha2::{Digest as _, Sha256};

const VERSION: &str = "9.8.7";
const UPGRADE_VERSION: &str = "9.8.8";
const FRAGMENT: &str = "/home/test/.config/systemd/user/hypercolor.service";
const ACTIVE_ROOT: &str = "/home/test/.local/lib/hypercolor/active";
const UNITS_ROOT: &str = "/home/test/.local/lib/hypercolor/units";

#[derive(Debug, Clone)]
struct FakeExecutor {
    launcher_discovery: Option<fn(&mut FakeSystemd)>,
    active_path: PathBuf,
    launcher: LinuxExactEntry,
    launcher_bytes: Vec<u8>,
    layout: BTreeMap<LinuxLayoutItem, LinuxExactEntry>,
    directories: BTreeMap<LinuxDirectoryItem, LinuxDirectoryState>,
    legacy_inventory: Vec<LinuxLegacyFile>,
    systemd: FakeSystemd,
    daemon_digest: String,
    daemon_digests: BTreeMap<String, String>,
    daemon_identities: BTreeMap<String, (u64, u64)>,
    expected_unit_authorities: Option<Vec<hypercolor_cli::install::UnitRecord>>,
    versions: BTreeMap<String, String>,
    invocation: u32,
    http_calls: usize,
    effects: Vec<String>,
    legacy_snapshot_root: Option<PathBuf>,
    process_override: Option<LinuxProcessExecutable>,
    process_calls: usize,
    process_inode_mismatch_for: Option<(String, usize)>,
    launcher_observations: usize,
    layout_observations: BTreeMap<LinuxLayoutItem, usize>,
    launcher_drift: Option<(usize, LinuxExactEntry, Vec<u8>)>,
    launcher_checkpoint_drift: Option<(LinuxExactEntry, Vec<u8>)>,
    launcher_ready_observations: usize,
    layout_drift: Option<(LinuxLayoutItem, usize, LinuxExactEntry)>,
    fault: Option<(String, FaultPoint)>,
    secondary_fault: Option<(String, FaultPoint)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FaultPoint {
    Before,
    After,
}

#[derive(Debug, Clone)]
struct FakeSystemd {
    load: &'static str,
    fragment: String,
    active: bool,
    enabled: bool,
    exec_start: String,
    pid: u32,
    invocation: String,
}

impl FakeExecutor {
    fn absent(active_path: PathBuf, daemon_digest: String) -> Self {
        Self {
            launcher_discovery: None,
            active_path,
            launcher: LinuxExactEntry::Absent,
            launcher_bytes: Vec::new(),
            layout: LINUX_LAYOUT_ITEMS
                .into_iter()
                .map(|item| (item, LinuxExactEntry::Absent))
                .collect(),
            directories: LINUX_DIRECTORY_ITEMS
                .into_iter()
                .map(|item| (item, LinuxDirectoryState::Present))
                .collect(),
            legacy_inventory: Vec::new(),
            systemd: FakeSystemd {
                load: "not-found",
                fragment: String::new(),
                active: false,
                enabled: false,
                exec_start: String::new(),
                pid: 0,
                invocation: String::new(),
            },
            daemon_digest,
            daemon_digests: BTreeMap::new(),
            daemon_identities: BTreeMap::new(),
            expected_unit_authorities: None,
            versions: BTreeMap::new(),
            invocation: 0,
            http_calls: 0,
            effects: Vec::new(),
            legacy_snapshot_root: None,
            process_override: None,
            process_calls: 0,
            process_inode_mismatch_for: None,
            launcher_observations: 0,
            layout_observations: BTreeMap::new(),
            launcher_drift: None,
            launcher_checkpoint_drift: None,
            launcher_ready_observations: 0,
            layout_drift: None,
            fault: None,
            secondary_fault: None,
        }
    }

    fn begin_effect(
        &mut self,
        name: &str,
    ) -> Result<bool, hypercolor_cli::install::InstallPlatformError> {
        self.effects.push(name.to_owned());
        let point = if self.fault.as_ref().is_some_and(|fault| fault.0 == name) {
            self.fault.take().map(|fault| fault.1)
        } else if self
            .secondary_fault
            .as_ref()
            .is_some_and(|fault| fault.0 == name)
        {
            self.secondary_fault.take().map(|fault| fault.1)
        } else {
            None
        };
        if point == Some(FaultPoint::Before) {
            return Err(hypercolor_cli::install::InstallPlatformError::new(
                "scripted before-effect failure",
            ));
        }
        Ok(point == Some(FaultPoint::After))
    }

    fn finish_effect(
        fail_after: bool,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        if fail_after {
            Err(hypercolor_cli::install::InstallPlatformError::new(
                "scripted after-effect failure",
            ))
        } else {
            Ok(())
        }
    }

    fn show(&self) -> Vec<u8> {
        let exec_start = if self.systemd.exec_start.is_empty() {
            String::new()
        } else {
            let executable = self
                .systemd
                .exec_start
                .split_ascii_whitespace()
                .next()
                .expect("scripted executable");
            format!(
                "{{ path={executable} ; argv[]={command} ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid={pid} ; code=(null) ; status=0/0 }}",
                command = self.systemd.exec_start,
                pid = self.systemd.pid,
            )
        };
        format!(
            "LoadState={}\nActiveState={}\nSubState={}\nUnitFileState={}\nFragmentPath={}\nExecStart={}\nMainPID={}\nInvocationID={}\n",
            self.systemd.load,
            if self.systemd.active { "active" } else { "inactive" },
            if self.systemd.active { "running" } else { "dead" },
            if self.systemd.load == "not-found" {
                ""
            } else if self.systemd.enabled {
                "enabled"
            } else {
                "disabled"
            },
            self.systemd.fragment,
            exec_start,
            self.systemd.pid,
            self.systemd.invocation,
        )
        .into_bytes()
    }

    fn active(&self) -> Option<UnitId> {
        fs::read_link(&self.active_path)
            .ok()
            .and_then(|target| target.file_name().map(ToOwned::to_owned))
            .and_then(|name| UnitId::new(name.to_string_lossy()).ok())
    }
}

impl LinuxInstallExecutor for FakeExecutor {
    fn validate_topology(
        &mut self,
        topology: &LinuxInstallConfig,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        if topology != &config() {
            return Err(hypercolor_cli::install::InstallPlatformError::new(
                "split install-store topology",
            ));
        }
        Ok(())
    }

    fn validate_unit_authority(
        &mut self,
        unit: &hypercolor_cli::install::UnitRecord,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        if self
            .expected_unit_authorities
            .as_ref()
            .is_some_and(|expected| !expected.iter().any(|known| known == unit))
        {
            return Err(hypercolor_cli::install::InstallPlatformError::new(
                "split retained unit authority",
            ));
        }
        let daemon = unit
            .directory()
            .open_child_directory(Path::new("bin"))
            .and_then(|directory| directory.open_regular_file(Path::new("hypercolor-daemon")))
            .map_err(|source| {
                hypercolor_cli::install::InstallPlatformError::new(source.to_string())
            })?;
        self.daemon_identities.insert(
            unit.id().as_str().to_owned(),
            (daemon.metadata().device(), daemon.metadata().inode()),
        );
        Ok(())
    }

    fn active_unit(
        &mut self,
    ) -> Result<Option<UnitId>, hypercolor_cli::install::InstallPlatformError> {
        Ok(self.active())
    }

    fn systemd_show(
        &mut self,
        max_bytes: usize,
    ) -> Result<Vec<u8>, hypercolor_cli::install::InstallPlatformError> {
        let output = self.show();
        assert!(output.len() <= max_bytes);
        Ok(output)
    }

    fn launcher_entry(
        &mut self,
        max_bytes: usize,
    ) -> Result<(LinuxExactEntry, Vec<u8>), hypercolor_cli::install::InstallPlatformError> {
        self.launcher_observations += 1;
        if self
            .launcher_drift
            .as_ref()
            .is_some_and(|drift| drift.0 == self.launcher_observations)
        {
            let (_, entry, bytes) = self.launcher_drift.take().expect("checked launcher drift");
            self.launcher = entry;
            self.launcher_bytes = bytes;
        }
        if self.launcher_checkpoint_drift.is_some()
            && self.effects.len() == LINUX_LAYOUT_ITEMS.len()
        {
            self.launcher_ready_observations += 1;
            if self.launcher_ready_observations == 3 {
                let (entry, bytes) = self
                    .launcher_checkpoint_drift
                    .take()
                    .expect("checked checkpoint drift");
                self.launcher = entry;
                self.launcher_bytes = bytes;
            }
        }
        assert!(self.launcher_bytes.len() <= max_bytes);
        Ok((self.launcher.clone(), self.launcher_bytes.clone()))
    }

    fn layout_entry(
        &mut self,
        item: LinuxLayoutItem,
    ) -> Result<LinuxExactEntry, hypercolor_cli::install::InstallPlatformError> {
        let observations = self.layout_observations.entry(item).or_default();
        *observations += 1;
        if self
            .layout_drift
            .as_ref()
            .is_some_and(|drift| drift.0 == item && drift.1 == *observations)
        {
            let (_, _, entry) = self.layout_drift.take().expect("checked layout drift");
            self.layout.insert(item, entry);
        }
        Ok(self.layout[&item].clone())
    }

    fn directory_state(
        &mut self,
        item: LinuxDirectoryItem,
    ) -> Result<LinuxDirectoryState, hypercolor_cli::install::InstallPlatformError> {
        Ok(self.directories[&item])
    }

    fn legacy_inventory(
        &mut self,
    ) -> Result<Vec<LinuxLegacyFile>, hypercolor_cli::install::InstallPlatformError> {
        Ok(self.legacy_inventory.clone())
    }

    fn replace_launcher(
        &mut self,
        expected: &LinuxExactEntry,
        replacement: Option<&LinuxFilePublication>,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        assert_eq!(&self.launcher, expected);
        let fail_after = self.begin_effect("launcher")?;
        if let Some(replacement) = replacement {
            self.launcher_bytes.clone_from(&replacement.contents);
            self.launcher = LinuxExactEntry::RegularFile {
                mode: replacement.mode,
                sha256: sha256(&replacement.contents),
                snapshot_unit: None,
                snapshot_path: None,
            };
        } else {
            self.launcher = LinuxExactEntry::Absent;
            self.launcher_bytes.clear();
        }
        if let Some(discover) = self.launcher_discovery
            && !matches!(self.launcher, LinuxExactEntry::Absent)
        {
            self.systemd.load = "loaded";
            FRAGMENT.clone_into(&mut self.systemd.fragment);
            self.systemd.exec_start = launcher_exec(&self.launcher_bytes);
            discover(&mut self.systemd);
        }
        Self::finish_effect(fail_after)
    }

    fn replace_layout(
        &mut self,
        item: LinuxLayoutItem,
        expected: &LinuxExactEntry,
        replacement: Option<&LinuxLayoutPublication>,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        assert!(exact_entries_match(&self.layout[&item], expected));
        let effect = format!("layout:{}", item_name(item));
        let fail_after = self.begin_effect(&effect)?;
        let next = match replacement {
            Some(LinuxLayoutPublication::Symlink(target)) => LinuxExactEntry::Symlink {
                target: target.clone(),
            },
            Some(LinuxLayoutPublication::RegularFile(file)) => LinuxExactEntry::RegularFile {
                mode: file.mode,
                sha256: sha256(&file.contents),
                snapshot_unit: None,
                snapshot_path: None,
            },
            None => LinuxExactEntry::Absent,
        };
        self.layout.insert(item, next);
        Self::finish_effect(fail_after)
    }

    fn replace_directory(
        &mut self,
        item: LinuxDirectoryItem,
        expected: LinuxDirectoryState,
        create: bool,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        assert_eq!(self.directories[&item], expected);
        assert!(create, "rollback must retain version-neutral scaffolding");
        let effect = format!("directory:{item:?}:create");
        let fail_after = self.begin_effect(&effect)?;
        self.directories.insert(item, LinuxDirectoryState::Present);
        Self::finish_effect(fail_after)
    }

    fn reload_manager(&mut self) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        let fail_after = self.begin_effect("manager")?;
        if matches!(self.launcher, LinuxExactEntry::Absent) {
            self.systemd.load = "not-found";
            self.systemd.fragment.clear();
            self.systemd.exec_start.clear();
        } else {
            self.systemd.load = "loaded";
            FRAGMENT.clone_into(&mut self.systemd.fragment);
            self.systemd.exec_start = launcher_exec(&self.launcher_bytes);
        }
        Self::finish_effect(fail_after)
    }

    fn set_autostart(
        &mut self,
        enabled: bool,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        let effect = format!("autostart:{enabled}");
        let fail_after = self.begin_effect(&effect)?;
        self.systemd.enabled = enabled;
        Self::finish_effect(fail_after)
    }

    fn set_runtime(
        &mut self,
        running: bool,
    ) -> Result<(), hypercolor_cli::install::InstallPlatformError> {
        let effect = format!("runtime:{running}");
        let fail_after = self.begin_effect(&effect)?;
        self.systemd.active = running;
        if running {
            if self.active().is_some() {
                self.process_override = None;
            }
            self.invocation += 1;
            self.systemd.pid = 4000 + self.invocation;
            self.systemd.invocation = format!("{:032x}", self.invocation);
        } else {
            self.systemd.pid = 0;
            self.systemd.invocation.clear();
        }
        Self::finish_effect(fail_after)
    }

    fn process_executable(
        &mut self,
        _pid: u32,
        _max_bytes: u64,
    ) -> Result<LinuxProcessExecutable, hypercolor_cli::install::InstallPlatformError> {
        self.process_calls += 1;
        if let Some(process) = &self.process_override {
            return Ok(process.clone());
        }
        let unit = self.active().expect("running unit");
        let (device, inode) = self.daemon_identities[unit.as_str()];
        Ok(LinuxProcessExecutable {
            path: format!("{UNITS_ROOT}/{}/bin/hypercolor-daemon", unit.as_str()),
            sha256: self
                .daemon_digests
                .get(unit.as_str())
                .unwrap_or(&self.daemon_digest)
                .clone(),
            device,
            inode: inode
                + u64::from(
                    self.process_inode_mismatch_for
                        .as_ref()
                        .is_some_and(|mismatch| {
                            mismatch.0 == unit.as_str() && mismatch.1 == self.process_calls
                        }),
                ),
        })
    }

    fn http_get(
        &mut self,
        path: &'static str,
        max_bytes: usize,
    ) -> Result<LinuxHttpResponse, hypercolor_cli::install::InstallPlatformError> {
        self.http_calls += 1;
        let version = self
            .active()
            .and_then(|unit| self.versions.get(unit.as_str()))
            .map_or(VERSION, String::as_str);
        let value = match path {
            "/health" => json!({"status":"healthy","version":version}),
            "/api/v1/system" => {
                json!({"data":{"identity":{"instance_id":"local","instance_name":"Hypercolor","version":version},"status":null}})
            }
            _ => unreachable!("fixed proof endpoint"),
        };
        let body = serde_json::to_vec(&value).expect("HTTP JSON");
        assert!(body.len() <= max_bytes);
        Ok(LinuxHttpResponse { status: 200, body })
    }

    fn snapshot_legacy_unit(
        &mut self,
        snapshot: &hypercolor_cli::install::LinuxLegacySnapshot,
    ) -> Result<hypercolor_cli::install::UnitRecord, hypercolor_cli::install::InstallPlatformError>
    {
        self.effects.push("snapshot".to_owned());
        assert_eq!(snapshot.inventory, self.legacy_inventory);
        let root = self.legacy_snapshot_root.take().ok_or_else(|| {
            hypercolor_cli::install::InstallPlatformError::new(
                "scripted legacy snapshot was not configured",
            )
        })?;
        let mut descriptors = BTreeMap::new();
        if let Some(launcher) = &snapshot.launcher {
            write_legacy_member(
                &root,
                "launcher/hypercolor.service",
                launcher.mode,
                &launcher.contents,
            );
            descriptors.insert(
                "launcher/hypercolor.service".to_owned(),
                (launcher.mode, sha256(&launcher.contents)),
            );
        }
        for (item, entry) in &snapshot.layout {
            let LinuxExactEntry::RegularFile {
                mode,
                sha256: digest,
                ..
            } = entry
            else {
                continue;
            };
            let path = legacy_item_path(*item);
            let contents = fs::read(root.join(path)).expect("legacy layout member");
            assert_eq!(sha256(&contents), *digest);
            descriptors.insert(path.to_owned(), (*mode, digest.clone()));
        }
        for file in &snapshot.inventory {
            write_legacy_member(&root, &file.path, file.mode, &file.contents);
            assert!(
                descriptors
                    .insert(file.path.clone(), (file.mode, sha256(&file.contents)))
                    .is_none()
            );
        }
        let manifest = serde_json::to_vec(&json!({
            "name": "hypercolor-legacy-snapshot",
            "version": snapshot.version,
            "files": descriptors,
        }))
        .expect("legacy manifest");
        write_legacy_member(&root, "manifest.json", 0o644, &manifest);
        let parent = root.parent().expect("legacy root parent");
        let exclusive = ExclusiveDirectory::try_acquire(parent, Path::new(".fake-legacy.lock"))
            .expect("legacy authority lock")
            .expect("exclusive legacy authority lock");
        let authority = exclusive
            .root_directory()
            .expect("legacy parent authority")
            .open_child_directory(Path::new("legacy-snapshot"))
            .expect("legacy snapshot authority");
        bind_linux_retained_unit(snapshot.unit.clone(), root, authority)
    }
}

#[test]
fn systemd_show_parser_is_strict_and_rejects_third_states() {
    let valid = b"LoadState=loaded\nActiveState=active\nSubState=running\nUnitFileState=enabled\nFragmentPath=/home/test/.config/systemd/user/hypercolor.service\nExecStart={ path=/daemon ; argv[]=/daemon --flag value ; ignore_errors=no ; start_time=[Thu 2026-08-20 12:00:00 PDT] ; stop_time=[n/a] ; pid=42 ; code=(null) ; status=0/0 }\nMainPID=42\nInvocationID=00112233445566778899aabbccddeeff\n";
    assert_eq!(
        parse_systemd_show(valid)
            .expect("strict observation")
            .main_pid,
        42
    );
    let unknown = [valid.as_slice(), b"Mystery=x\n"].concat();
    let duplicate = [valid.as_slice(), b"MainPID=43\n"].concat();
    let third = String::from_utf8(valid.to_vec())
        .expect("UTF-8")
        .replace("ActiveState=active", "ActiveState=failed")
        .into_bytes();
    let malformed = b"LoadState\n".to_vec();
    let missing = valid
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.starts_with(b"ExecStart="))
        .flat_map(|line| line.iter().copied().chain([b'\n']))
        .collect::<Vec<_>>();
    let ambiguous = String::from_utf8(valid.to_vec())
        .expect("UTF-8")
        .replace("SubState=running", "SubState=dead")
        .into_bytes();
    let multiple = String::from_utf8(valid.to_vec())
        .expect("UTF-8")
        .replace(
            " ; code=(null) ; status=0/0 }",
            " ; code=(null) ; status=0/0 } ; { path=/other ; argv[]=/other ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid=42 ; code=(null) ; status=0/0 }",
        )
        .into_bytes();
    let pid_mismatch = String::from_utf8(valid.to_vec())
        .expect("UTF-8")
        .replace(" ; pid=42 ;", " ; pid=43 ;")
        .into_bytes();
    let executable_mismatch = String::from_utf8(valid.to_vec())
        .expect("UTF-8")
        .replace("argv[]=/daemon", "argv[]=/other")
        .into_bytes();
    for invalid in [
        unknown,
        duplicate,
        third,
        malformed,
        missing,
        ambiguous,
        multiple,
        pid_mismatch,
        executable_mismatch,
    ] {
        assert!(parse_systemd_show(&invalid).is_err());
    }
    let absent = b"LoadState=not-found\nActiveState=inactive\nSubState=dead\nUnitFileState=\nFragmentPath=\nExecStart=\nMainPID=0\nInvocationID=\n";
    assert!(parse_systemd_show(absent).is_ok());
    let absent_disabled = String::from_utf8(absent.to_vec())
        .expect("UTF-8")
        .replace("UnitFileState=\n", "UnitFileState=disabled\n");
    assert!(parse_systemd_show(absent_disabled.as_bytes()).is_err());
    let loaded_empty = String::from_utf8(valid.to_vec())
        .expect("UTF-8")
        .replace("UnitFileState=enabled", "UnitFileState=");
    assert!(parse_systemd_show(loaded_empty.as_bytes()).is_err());
}

#[test]
fn loaded_service_requires_the_exact_direct_fragment_and_notify_launcher() {
    let fixture = Fixture::new();
    let mut executor =
        FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    executor.systemd.load = "loaded";
    executor.systemd.fragment = "/tmp/foreign.service".to_owned();
    let mut platform = LinuxInstallPlatform::new(executor, config(), []).expect("Linux platform");
    assert!(platform.inspect().is_err());

    let mut executor =
        FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    executor.systemd.load = "loaded";
    executor.systemd.fragment = FRAGMENT.to_owned();
    executor.launcher_bytes = b"[Service]\nType=simple\nExecStart=/daemon\n".to_vec();
    executor.launcher = LinuxExactEntry::RegularFile {
        mode: 0o644,
        sha256: sha256(&executor.launcher_bytes),
        snapshot_unit: None,
        snapshot_path: None,
    };
    let mut platform = LinuxInstallPlatform::new(executor, config(), []).expect("Linux platform");
    assert!(platform.inspect().is_err());
}

#[test]
fn first_install_enables_starts_and_proves_exact_owner() {
    let fixture = Fixture::new();
    let executor = FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    let mut platform = LinuxInstallPlatform::new(executor, config(), []).expect("Linux platform");
    let mut lock = fixture.store.acquire_lock().expect("install lock");
    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .install_with_lock(
            fixture.request(InstallTargetPolicy::EnableOnFirstInstall),
            &mut lock,
        )
        .expect("first install");
    assert!(format!("{outcome:?}").contains("Committed"));
    let executor = platform.into_executor();
    assert!(executor.systemd.active);
    assert!(executor.systemd.enabled);
    assert_eq!(executor.http_calls, 2);
    assert!(executor.effects.contains(&"manager".to_owned()));
    assert!(executor.effects.contains(&"autostart:true".to_owned()));
    assert!(executor.effects.contains(&"runtime:true".to_owned()));
    let mut expected_effects = LINUX_LAYOUT_ITEMS
        .into_iter()
        .map(|item| format!("layout:{}", item_name(item)))
        .collect::<Vec<_>>();
    expected_effects.extend([
        "launcher".to_owned(),
        "manager".to_owned(),
        "autostart:true".to_owned(),
        "runtime:true".to_owned(),
    ]);
    assert_eq!(executor.effects, expected_effects);
    for item in LINUX_LAYOUT_ITEMS {
        assert!(matches!(
            executor.layout[&item],
            LinuxExactEntry::Symlink { .. }
        ));
    }
}

#[test]
fn first_install_accepts_exact_lazy_launcher_discovery_and_still_reloads() {
    let fixture = Fixture::new();
    let mut executor =
        FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    executor.launcher_discovery = Some(|_| {});
    let mut platform =
        LinuxInstallPlatform::new(executor, config(), []).expect("lazy discovery platform");
    let mut lock = fixture.store.acquire_lock().expect("install lock");
    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .install_with_lock(
            fixture.request(InstallTargetPolicy::EnableOnFirstInstall),
            &mut lock,
        )
        .expect("query-discovered inactive candidate is an exact launcher checkpoint");
    assert!(format!("{outcome:?}").contains("Committed"));
    let executor = platform.into_executor();
    assert!(executor.effects.contains(&"manager".to_owned()));
    assert!(executor.systemd.active);
    assert!(executor.systemd.enabled);
    assert_eq!(executor.http_calls, 2);
}

#[test]
fn lazy_launcher_discovery_rejects_foreign_or_running_manager_state() {
    let mutations: [fn(&mut FakeSystemd); 5] = [
        |state| state.fragment = "/foreign.service".to_owned(),
        |state| state.exec_start = "/foreign-daemon".to_owned(),
        |state| {
            state.active = true;
            state.pid = 99;
        },
        |state| state.pid = 99,
        |state| state.enabled = true,
    ];
    for mutate in mutations {
        let fixture = Fixture::new();
        let mut executor =
            FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
        executor.launcher_discovery = Some(mutate);
        let mut platform =
            LinuxInstallPlatform::new(executor, config(), []).expect("foreign discovery platform");
        let mut lock = fixture.store.acquire_lock().expect("install lock");
        assert!(
            InstallCoordinator::new(&fixture.store, &mut platform)
                .install_with_lock(
                    fixture.request(InstallTargetPolicy::EnableOnFirstInstall),
                    &mut lock
                )
                .is_err()
        );
        let executor = platform.into_executor();
        assert!(!executor.effects.contains(&"manager".to_owned()));
        assert_eq!(executor.http_calls, 0);
    }
}

#[test]
fn no_service_first_install_stays_absent_inactive_and_skips_http() {
    let fixture = Fixture::new();
    let executor = FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    let mut platform = LinuxInstallPlatform::new(executor, config(), []).expect("Linux platform");
    let mut lock = fixture.store.acquire_lock().expect("install lock");
    InstallCoordinator::new(&fixture.store, &mut platform)
        .install_with_lock(fixture.request(InstallTargetPolicy::Preserve), &mut lock)
        .expect("no-service install");
    let executor = platform.into_executor();
    assert!(matches!(executor.launcher, LinuxExactEntry::Absent));
    assert!(!executor.systemd.active);
    assert!(!executor.systemd.enabled);
    assert_eq!(executor.http_calls, 0);
}

#[test]
fn launcher_effect_rejects_record_drift_in_both_directions() {
    for (rollback, observation) in [(false, 2), (true, 3)] {
        let fixture = Fixture::new();
        let sentinel_bytes = b"sentinel launcher".to_vec();
        let sentinel = LinuxExactEntry::RegularFile {
            mode: 0o600,
            sha256: sha256(&sentinel_bytes),
            snapshot_unit: None,
            snapshot_path: None,
        };
        let mut executor =
            FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
        executor.launcher_drift = Some((observation, sentinel.clone(), sentinel_bytes.clone()));
        let mut platform =
            LinuxInstallPlatform::new(executor, config(), []).expect("Linux platform");
        let prior = InstallationState {
            active_unit: None,
            platform: platform.inspect().expect("prior inspection"),
        };
        let target = PlatformState {
            layout_unit: Some(fixture.candidate.id().clone()),
            launcher_unit: Some(fixture.candidate.id().clone()),
            loaded: true,
            running_unit: Some(fixture.candidate.id().clone()),
            autostart_enabled: true,
        };
        let prepared = platform
            .prepare_transaction(&fixture.candidate, &prior, &target)
            .expect("prepared transaction");
        if rollback {
            platform
                .install_launcher(
                    PlatformCheckpoint::CandidateLauncher,
                    Some(fixture.candidate.id()),
                    &prepared.record,
                )
                .expect("publish candidate launcher");
        }

        let rollback_unit = (!rollback).then_some(fixture.candidate.id());
        let error = platform
            .install_launcher(
                if rollback {
                    PlatformCheckpoint::PriorLauncherRestored
                } else {
                    PlatformCheckpoint::CandidateLauncher
                },
                rollback_unit,
                &prepared.record,
            )
            .expect_err("namespace drift must block launcher mutation");

        assert!(error.to_string().contains("launcher drifted"));
        let executor = platform.into_executor();
        assert_eq!(executor.launcher, sentinel);
        assert_eq!(executor.launcher_bytes, sentinel_bytes);
        assert_eq!(
            executor
                .effects
                .iter()
                .filter(|effect| effect.as_str() == "launcher")
                .count(),
            usize::from(rollback)
        );
    }
}

#[test]
fn coordinator_preserves_launcher_sentinel_without_advancing_the_journal() {
    let fixture = Fixture::new();
    let sentinel_bytes = b"concurrent launcher sentinel".to_vec();
    let sentinel = LinuxExactEntry::RegularFile {
        mode: 0o600,
        sha256: sha256(&sentinel_bytes),
        snapshot_unit: None,
        snapshot_path: None,
    };
    let mut executor =
        FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    executor.launcher_checkpoint_drift = Some((sentinel.clone(), sentinel_bytes.clone()));
    let mut platform = LinuxInstallPlatform::new(executor, config(), []).expect("Linux platform");
    let mut lock = fixture.store.acquire_lock().expect("install lock");

    InstallCoordinator::new(&fixture.store, &mut platform)
        .install_with_lock(
            fixture.request(InstallTargetPolicy::EnableOnFirstInstall),
            &mut lock,
        )
        .expect_err("concurrent launcher drift must stop the forward effect");

    let journal = fixture
        .store
        .load_journal(&lock)
        .expect("load retained journal")
        .expect("pending forward journal");
    assert_eq!(journal.disposition, InstallDisposition::Forward);
    assert_eq!(
        journal.next_action,
        Some(InstallAction::InstallCandidateLauncher)
    );
    let executor = platform.into_executor();
    assert_eq!(executor.launcher, sentinel);
    assert_eq!(executor.launcher_bytes, sentinel_bytes);
    assert!(
        !executor
            .effects
            .iter()
            .any(|effect| effect.as_str() == "launcher")
    );
}

#[test]
fn layout_effect_rejects_record_drift_in_both_directions() {
    for (rollback, observation) in [(false, 2), (true, 3)] {
        let fixture = Fixture::new();
        let item = LinuxLayoutItem::Hypercolor;
        let sentinel = LinuxExactEntry::RegularFile {
            mode: 0o600,
            sha256: sha256(b"sentinel layout"),
            snapshot_unit: None,
            snapshot_path: None,
        };
        let mut executor =
            FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
        executor.layout_drift = Some((item, observation, sentinel.clone()));
        let mut platform =
            LinuxInstallPlatform::new(executor, config(), []).expect("Linux platform");
        let prior = InstallationState {
            active_unit: None,
            platform: platform.inspect().expect("prior inspection"),
        };
        let target = PlatformState {
            layout_unit: Some(fixture.candidate.id().clone()),
            launcher_unit: Some(fixture.candidate.id().clone()),
            loaded: true,
            running_unit: Some(fixture.candidate.id().clone()),
            autostart_enabled: true,
        };
        let prepared = platform
            .prepare_transaction(&fixture.candidate, &prior, &target)
            .expect("prepared transaction");
        if rollback {
            platform
                .install_layout_operation(
                    PlatformCheckpoint::CandidateLayout,
                    Some(fixture.candidate.id()),
                    0,
                    &prepared.record,
                )
                .expect("publish candidate layout entry");
        }

        let error = platform
            .install_layout_operation(
                if rollback {
                    PlatformCheckpoint::PriorLayoutRestored
                } else {
                    PlatformCheckpoint::CandidateLayout
                },
                (!rollback).then_some(fixture.candidate.id()),
                0,
                &prepared.record,
            )
            .expect_err("namespace drift must block layout mutation");

        assert!(error.to_string().contains("layout drifted"));
        let executor = platform.into_executor();
        assert_eq!(executor.layout[&item], sentinel);
        assert_eq!(
            executor
                .effects
                .iter()
                .filter(|effect| effect.as_str() == "layout:hypercolor")
                .count(),
            usize::from(rollback)
        );
    }
}

#[test]
fn unloaded_disabled_upgrade_preserves_service_state_and_user_data() {
    let fixture = Fixture::new();
    let config_sentinel = fixture.temp.path().join("config/hypercolor.toml");
    let data_sentinel = fixture.temp.path().join("data/database.sqlite");
    let effects_sentinel = fixture.temp.path().join("effects/user.html");
    for (path, bytes) in [
        (&config_sentinel, b"config".as_slice()),
        (&data_sentinel, b"data".as_slice()),
        (&effects_sentinel, b"effect".as_slice()),
    ] {
        fs::create_dir_all(path.parent().expect("sentinel parent")).expect("sentinel directory");
        fs::write(path, bytes).expect("sentinel");
    }

    let executor = FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    let mut first =
        LinuxInstallPlatform::new(executor, config(), []).expect("first Linux platform");
    let mut first_lock = fixture.store.acquire_lock().expect("first lock");
    InstallCoordinator::new(&fixture.store, &mut first)
        .install_with_lock(
            fixture.request(InstallTargetPolicy::EnableOnFirstInstall),
            &mut first_lock,
        )
        .expect("bootstrap direct service");
    drop(first_lock);
    let mut executor = first.into_executor();
    executor.set_runtime(false).expect("stop prior");
    executor.set_autostart(false).expect("disable prior");
    executor.effects.clear();
    executor.http_calls = 0;

    let mut upgrade = LinuxInstallPlatform::new(executor, config(), [fixture.candidate.clone()])
        .expect("upgrade Linux platform");
    let mut upgrade_lock = fixture.store.acquire_lock().expect("upgrade lock");
    InstallCoordinator::new(&fixture.store, &mut upgrade)
        .install_with_lock(
            fixture.request(InstallTargetPolicy::EnableOnFirstInstall),
            &mut upgrade_lock,
        )
        .expect("preserving upgrade");
    let executor = upgrade.into_executor();
    assert!(!executor.systemd.active);
    assert!(!executor.systemd.enabled);
    assert_eq!(executor.http_calls, 0);
    assert_eq!(fs::read(config_sentinel).expect("config"), b"config");
    assert_eq!(fs::read(data_sentinel).expect("data"), b"data");
    assert_eq!(fs::read(effects_sentinel).expect("effect"), b"effect");
}

#[test]
fn running_enabled_upgrade_preserves_policy_and_proves_fresh_owner() {
    let mut fixture = Fixture::new();
    let executor = FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    let mut first =
        LinuxInstallPlatform::new(executor, config(), []).expect("first Linux platform");
    let mut first_lock = fixture.store.acquire_lock().expect("first lock");
    InstallCoordinator::new(&fixture.store, &mut first)
        .install_with_lock(
            fixture.request(InstallTargetPolicy::EnableOnFirstInstall),
            &mut first_lock,
        )
        .expect("bootstrap direct service");
    drop(first_lock);

    let upgrade_candidate = fixture.stage_upgrade();
    let mut executor = first.into_executor();
    let prior_invocation = executor.systemd.invocation.clone();
    executor.effects.clear();
    executor.http_calls = 0;
    executor.daemon_digests.insert(
        fixture.candidate.id().as_str().to_owned(),
        fixture.daemon_digest.clone(),
    );
    executor.daemon_digests.insert(
        upgrade_candidate.id().as_str().to_owned(),
        sha256(b"daemon-upgrade"),
    );
    executor.versions.insert(
        fixture.candidate.id().as_str().to_owned(),
        VERSION.to_owned(),
    );
    executor.versions.insert(
        upgrade_candidate.id().as_str().to_owned(),
        UPGRADE_VERSION.to_owned(),
    );
    let prior_id = fixture.candidate.id().clone();
    let original_prior = std::mem::replace(&mut fixture.candidate, upgrade_candidate.clone());
    drop(original_prior);
    let upgrade_lock = fixture.store.acquire_lock().expect("upgrade lock");
    let rebound_prior = retain_linux_unit(&fixture.store, &upgrade_lock, &prior_id)
        .expect("cold-process prior rebind");

    let mut upgrade = LinuxInstallPlatform::new(
        executor,
        config(),
        [rebound_prior, upgrade_candidate.clone()],
    )
    .expect("upgrade Linux platform");
    let mut upgrade_lock = upgrade_lock;
    let outcome = InstallCoordinator::new(&fixture.store, &mut upgrade)
        .install_with_lock(
            fixture.request_for(
                upgrade_candidate.clone(),
                InstallTargetPolicy::Preserve,
                "running-upgrade",
            ),
            &mut upgrade_lock,
        )
        .expect("running preserving upgrade");
    assert!(format!("{outcome:?}").contains("Committed"));
    let executor = upgrade.into_executor();
    assert!(executor.systemd.active);
    assert!(executor.systemd.enabled);
    assert_ne!(executor.systemd.invocation, prior_invocation);
    assert_eq!(executor.http_calls, 4);
    assert_eq!(
        executor
            .effects
            .iter()
            .filter(|effect| effect.as_str() == "runtime:false")
            .count(),
        1
    );
    assert_eq!(
        executor
            .effects
            .iter()
            .filter(|effect| effect.as_str() == "runtime:true")
            .count(),
        1
    );
}

#[test]
fn same_digest_failure_restores_partial_layout_launcher_mode_and_fresh_owner() {
    let fixture = Fixture::new();
    let executor = FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    let mut first =
        LinuxInstallPlatform::new(executor, config(), []).expect("first Linux platform");
    let mut first_lock = fixture.store.acquire_lock().expect("first lock");
    InstallCoordinator::new(&fixture.store, &mut first)
        .install_with_lock(
            fixture.request(InstallTargetPolicy::EnableOnFirstInstall),
            &mut first_lock,
        )
        .expect("bootstrap direct service");
    drop(first_lock);

    let mut executor = first.into_executor();
    let baseline_invocation = executor.invocation;
    let baseline_identity = executor.systemd.invocation.clone();
    let LinuxExactEntry::RegularFile { mode, .. } = &mut executor.launcher else {
        panic!("installed launcher must be regular");
    };
    *mode = 0o600;
    executor
        .layout
        .insert(LinuxLayoutItem::Icon48, LinuxExactEntry::Absent);
    executor.effects.clear();
    executor.http_calls = 0;
    executor.fault = Some(("runtime:true".to_owned(), FaultPoint::After));

    let mut platform = LinuxInstallPlatform::new(executor, config(), [fixture.candidate.clone()])
        .expect("same-unit Linux platform");
    let mut lock = fixture.store.acquire_lock().expect("same-unit lock");
    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .install_with_lock(
            fixture.request_for(
                fixture.candidate.clone(),
                InstallTargetPolicy::Preserve,
                "same-digest-rollback",
            ),
            &mut lock,
        )
        .expect("same-unit failure rolls back");

    assert!(format!("{outcome:?}").contains("RolledBack"));
    let executor = platform.into_executor();
    assert!(matches!(
        executor.launcher,
        LinuxExactEntry::RegularFile { mode: 0o600, .. }
    ));
    assert_eq!(
        executor.layout[&LinuxLayoutItem::Icon48],
        LinuxExactEntry::Absent
    );
    assert!(executor.systemd.active);
    assert_eq!(executor.active().as_ref(), Some(fixture.candidate.id()));
    assert_ne!(executor.systemd.invocation, baseline_identity);
    assert_eq!(executor.invocation, baseline_invocation + 2);
    assert_eq!(executor.http_calls, 4);
}

#[test]
fn first_conversion_snapshots_loaded_raw_service_before_stop() {
    let fixture = Fixture::new();
    let (executor, _) = raw_conversion_executor(&fixture);

    let mut platform = LinuxInstallPlatform::new(executor, config(), []).expect("Linux platform");
    let mut lock = fixture.store.acquire_lock().expect("install lock");
    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .install_with_lock(fixture.request(InstallTargetPolicy::Preserve), &mut lock)
        .expect("first raw-direct conversion");
    assert!(format!("{outcome:?}").contains("Committed"));
    let executor = platform.into_executor();
    assert_eq!(
        executor.effects.first().map(String::as_str),
        Some("snapshot")
    );
    assert_eq!(
        executor
            .effects
            .iter()
            .position(|effect| effect == "runtime:false"),
        Some(1)
    );
    assert!(executor.systemd.active);
    assert!(executor.systemd.enabled);
    assert!(matches!(
        executor.launcher,
        LinuxExactEntry::RegularFile { mode: 0o644, .. }
    ));
    assert_eq!(executor.active().as_ref(), Some(fixture.candidate.id()));
    for path in [
        "bin/hyper",
        "bin/hypercolor-tray",
        "home/.config/fish/completions/hyper.fish",
        "share/bash-completion/completions/hyper",
        "share/zsh/site-functions/_hyper",
    ] {
        assert!(
            fixture
                .temp
                .path()
                .join("legacy-snapshot")
                .join(path)
                .is_file(),
            "historical owned leaf was not snapshotted: {path}"
        );
    }
}

#[test]
fn failed_conversion_restores_exact_0600_launcher_and_running_policy() {
    let fixture = Fixture::new();
    let (mut executor, prior_launcher_bytes) = raw_conversion_executor(&fixture);
    executor.fault = Some(("manager".to_owned(), FaultPoint::After));
    let mut platform = LinuxInstallPlatform::new(executor, config(), []).expect("Linux platform");
    let mut lock = fixture.store.acquire_lock().expect("install lock");
    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .install_with_lock(fixture.request(InstallTargetPolicy::Preserve), &mut lock)
        .expect("conversion failure rolls back");
    assert!(format!("{outcome:?}").contains("RolledBack"));
    let executor = platform.into_executor();
    assert_eq!(executor.launcher_bytes, prior_launcher_bytes);
    assert!(matches!(
        executor.launcher,
        LinuxExactEntry::RegularFile { mode: 0o600, .. }
    ));
    assert!(executor.systemd.active);
    assert!(executor.systemd.enabled);
    assert_eq!(
        executor.systemd.exec_start,
        "/opt/hypercolor/bin/hypercolor-daemon"
    );
    assert!(matches!(
        executor.layout[&LinuxLayoutItem::Icon48],
        LinuxExactEntry::RegularFile { mode: 0o644, .. }
    ));
}

#[test]
fn process_digest_mismatch_fails_before_http_and_rolls_back() {
    let fixture = Fixture::new();
    let executor = FakeExecutor::absent(fixture.store.active_path(), "00".repeat(32));
    let mut platform = LinuxInstallPlatform::new(executor, config(), []).expect("Linux platform");
    let mut lock = fixture.store.acquire_lock().expect("install lock");
    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .install_with_lock(
            fixture.request(InstallTargetPolicy::EnableOnFirstInstall),
            &mut lock,
        )
        .expect("failure rolls back");
    assert!(format!("{outcome:?}").contains("RolledBack"));
    let executor = platform.into_executor();
    assert_eq!(executor.http_calls, 0);
    assert!(!executor.systemd.active);
}

#[test]
fn retained_candidate_inode_mismatch_fails_before_http_and_rolls_back() {
    let fixture = Fixture::new();
    let mut executor =
        FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    executor.process_inode_mismatch_for = Some((fixture.candidate.id().as_str().to_owned(), 1));
    let mut platform = LinuxInstallPlatform::new(executor, config(), []).expect("Linux platform");
    let mut lock = fixture.store.acquire_lock().expect("install lock");
    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .install_with_lock(
            fixture.request(InstallTargetPolicy::EnableOnFirstInstall),
            &mut lock,
        )
        .expect("identity failure rolls back");
    assert!(format!("{outcome:?}").contains("RolledBack"));
    let executor = platform.into_executor();
    assert_eq!(executor.http_calls, 0);
    assert!(!executor.systemd.active);
}

#[test]
fn ordinary_prior_inode_mismatch_blocks_upgrade_before_stop_and_http() {
    let fixture = Fixture::new();
    let executor = FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    let mut first =
        LinuxInstallPlatform::new(executor, config(), []).expect("first Linux platform");
    let mut first_lock = fixture.store.acquire_lock().expect("first lock");
    InstallCoordinator::new(&fixture.store, &mut first)
        .install_with_lock(
            fixture.request(InstallTargetPolicy::EnableOnFirstInstall),
            &mut first_lock,
        )
        .expect("bootstrap prior service");
    drop(first_lock);
    let upgrade_candidate = fixture.stage_upgrade();
    let mut executor = first.into_executor();
    executor.effects.clear();
    executor.http_calls = 0;
    executor.process_calls = 0;
    executor.process_inode_mismatch_for = Some((fixture.candidate.id().as_str().to_owned(), 1));
    executor.daemon_digests.insert(
        upgrade_candidate.id().as_str().to_owned(),
        sha256(b"daemon-upgrade"),
    );
    executor.versions.insert(
        upgrade_candidate.id().as_str().to_owned(),
        UPGRADE_VERSION.to_owned(),
    );
    let mut platform = LinuxInstallPlatform::new(
        executor,
        config(),
        [fixture.candidate.clone(), upgrade_candidate.clone()],
    )
    .expect("upgrade platform");
    let mut lock = fixture.store.acquire_lock().expect("upgrade lock");

    let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
        .install_with_lock(
            fixture.request_for(
                upgrade_candidate,
                InstallTargetPolicy::Preserve,
                "prior-inode-baseline",
            ),
            &mut lock,
        )
        .expect("preflight failure must reconcile without mutation");

    let executor = platform.into_executor();
    assert!(format!("{outcome:?}").contains("prior /proc executable identity"));
    assert_eq!(executor.http_calls, 0);
    assert!(executor.systemd.active);
    assert_eq!(executor.active().as_ref(), Some(fixture.candidate.id()));
    assert!(
        !executor
            .effects
            .iter()
            .any(|effect| effect == "runtime:false")
    );
}

#[test]
fn ordinary_prior_inode_mismatch_blocks_rollback_proof_before_http() {
    let fixture = Fixture::new();
    let executor = FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    let mut first =
        LinuxInstallPlatform::new(executor, config(), []).expect("first Linux platform");
    let mut first_lock = fixture.store.acquire_lock().expect("first lock");
    InstallCoordinator::new(&fixture.store, &mut first)
        .install_with_lock(
            fixture.request(InstallTargetPolicy::EnableOnFirstInstall),
            &mut first_lock,
        )
        .expect("bootstrap prior service");
    drop(first_lock);
    let upgrade_candidate = fixture.stage_upgrade();
    let mut executor = first.into_executor();
    executor.effects.clear();
    executor.http_calls = 0;
    executor.process_calls = 0;
    executor.process_inode_mismatch_for = Some((fixture.candidate.id().as_str().to_owned(), 2));
    executor.fault = Some(("runtime:true".to_owned(), FaultPoint::After));
    executor.daemon_digests.insert(
        upgrade_candidate.id().as_str().to_owned(),
        sha256(b"daemon-upgrade"),
    );
    executor.versions.insert(
        upgrade_candidate.id().as_str().to_owned(),
        UPGRADE_VERSION.to_owned(),
    );
    let mut platform = LinuxInstallPlatform::new(
        executor,
        config(),
        [fixture.candidate.clone(), upgrade_candidate.clone()],
    )
    .expect("upgrade platform");
    let mut lock = fixture.store.acquire_lock().expect("upgrade lock");

    InstallCoordinator::new(&fixture.store, &mut platform)
        .install_with_lock(
            fixture.request_for(
                upgrade_candidate,
                InstallTargetPolicy::Preserve,
                "prior-inode-rollback",
            ),
            &mut lock,
        )
        .expect_err("foreign restored prior inode must block rollback proof");

    let executor = platform.into_executor();
    assert_eq!(
        executor.http_calls, 2,
        "rollback proof must fail before more HTTP"
    );
    assert_eq!(executor.process_calls, 2);
    assert!(executor.systemd.active);
    assert_eq!(executor.active().as_ref(), Some(fixture.candidate.id()));
}

#[test]
fn every_platform_effect_reconciles_before_and_error_after_boundaries() {
    for effect in first_install_effects() {
        let fixture = Fixture::new();
        let mut executor =
            FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
        executor
            .directories
            .values_mut()
            .for_each(|state| *state = LinuxDirectoryState::Absent);
        executor.fault = Some((effect.clone(), FaultPoint::Before));
        let mut platform =
            LinuxInstallPlatform::new(executor, config(), []).expect("Linux platform");
        let mut lock = fixture.store.acquire_lock().expect("install lock");
        let outcome = InstallCoordinator::new(&fixture.store, &mut platform)
            .install_with_lock(
                fixture.request(InstallTargetPolicy::EnableOnFirstInstall),
                &mut lock,
            )
            .expect("before-effect failure rolls back");
        assert!(
            format!("{outcome:?}").contains("RolledBack"),
            "before-effect {effect} did not roll back: {outcome:?}"
        );
        let executor = platform.into_executor();
        assert!(executor.fault.is_none());
        assert!(executor.active().is_none());
        assert!(!executor.systemd.active);
        let effect_index = first_install_effects()
            .iter()
            .position(|candidate| candidate == &effect)
            .expect("known effect");
        assert_eq!(
            executor
                .directories
                .values()
                .filter(|state| **state == LinuxDirectoryState::Present)
                .count(),
            effect_index.min(LINUX_DIRECTORY_ITEMS.len())
        );

        let fixture = Fixture::new();
        let mut executor =
            FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
        executor
            .directories
            .values_mut()
            .for_each(|state| *state = LinuxDirectoryState::Absent);
        executor.fault = Some((effect.clone(), FaultPoint::After));
        let mut platform =
            LinuxInstallPlatform::new(executor, config(), []).expect("Linux platform");
        let mut lock = fixture.store.acquire_lock().expect("install lock");
        let result = InstallCoordinator::new(&fixture.store, &mut platform).install_with_lock(
            fixture.request(InstallTargetPolicy::EnableOnFirstInstall),
            &mut lock,
        );
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                let effects = platform.into_executor().effects;
                panic!(
                    "error-after {effect} failed reconciliation: {error:?}; effects={effects:?}"
                );
            }
        };
        assert!(
            format!("{outcome:?}").contains("RolledBack"),
            "error-after {effect} did not roll back: {outcome:?}"
        );
        let executor = platform.into_executor();
        assert!(executor.fault.is_none());
        assert!(executor.active().is_none());
        assert!(!executor.systemd.active);
        assert_eq!(
            executor
                .directories
                .values()
                .filter(|state| **state == LinuxDirectoryState::Present)
                .count(),
            (effect_index + 1).min(LINUX_DIRECTORY_ITEMS.len())
        );
    }
}

#[test]
fn crash_replay_uses_persisted_candidate_receipt_and_finishes_rollback() {
    let fixture = Fixture::new();
    let mut executor =
        FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    executor.fault = Some(("runtime:true".to_owned(), FaultPoint::After));
    executor.secondary_fault = Some(("runtime:false".to_owned(), FaultPoint::Before));
    let mut platform = LinuxInstallPlatform::new(executor, config(), []).expect("Linux platform");
    let mut lock = fixture.store.acquire_lock().expect("install lock");
    let error = InstallCoordinator::new(&fixture.store, &mut platform)
        .install_with_lock(
            fixture.request(InstallTargetPolicy::EnableOnFirstInstall),
            &mut lock,
        )
        .expect_err("scripted rollback interruption");
    assert!(format!("{error:?}").contains("UnloadCandidateRuntime"));
    let mut executor = platform.into_executor();
    assert!(executor.systemd.active);
    assert!(executor.fault.is_none());
    assert!(executor.secondary_fault.is_none());
    executor.effects.clear();
    let candidate_id = fixture.candidate.id().clone();
    let Fixture {
        temp: _temp,
        store,
        candidate,
        daemon_digest: _,
    } = fixture;
    drop(candidate);
    let rebound =
        retain_linux_unit(&store, &lock, &candidate_id).expect("cold-process candidate rebind");

    let mut recovered =
        LinuxInstallPlatform::new(executor, config(), [rebound]).expect("recovery Linux platform");
    let outcome = InstallCoordinator::new(&store, &mut recovered)
        .recover_with_lock(&mut lock)
        .expect("recover rollback")
        .expect("pending journal");
    assert!(format!("{outcome:?}").contains("RolledBack"));
    let executor = recovered.into_executor();
    assert!(!executor.systemd.active);
    assert!(executor.active().is_none());
    assert!(matches!(executor.launcher, LinuxExactEntry::Absent));
    assert!(
        executor
            .layout
            .values()
            .all(|entry| matches!(entry, LinuxExactEntry::Absent))
    );
}

#[test]
fn semantic_record_validation_rejects_recovery_forgery() {
    let fixture = Fixture::new();
    let executor = FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    let mut platform = LinuxInstallPlatform::new(executor, config(), [fixture.candidate.clone()])
        .expect("Linux platform");
    let prior = InstallationState {
        active_unit: None,
        platform: platform.inspect().expect("prior inspection"),
    };
    let target = PlatformState {
        layout_unit: Some(fixture.candidate.id().clone()),
        launcher_unit: Some(fixture.candidate.id().clone()),
        loaded: true,
        running_unit: Some(fixture.candidate.id().clone()),
        autostart_enabled: true,
    };
    let prepared = platform
        .prepare_transaction(&fixture.candidate, &prior, &target)
        .expect("prepared Linux record");

    let mut forgeries = Vec::new();
    forgeries.push(forge_record(&prepared.record, |value| {
        value["candidate"]["daemon_sha256"] = json!("00".repeat(32));
    }));
    forgeries.push(forge_record(&prepared.record, |value| {
        value["layout"][0]["effect"]["candidate_target"] = json!("/tmp/foreign-daemon");
    }));
    forgeries.push(forge_record(&prepared.record, |value| {
        let first = value["layout"][0]["effect"]["item"].clone();
        value["layout"][1]["effect"]["item"] = first;
    }));
    forgeries.push(forge_record(&prepared.record, |value| {
        value["layout"].as_array_mut().expect("layout array").pop();
    }));
    forgeries.push(forge_record(&prepared.record, |value| {
        value["layout"]
            .as_array_mut()
            .expect("layout array")
            .swap(0, 1);
    }));
    forgeries.push(forge_record(&prepared.record, |value| {
        value["prior_directories"]["local"] = json!("absent");
    }));

    for forged in forgeries {
        assert!(
            platform
                .validate_transaction_plan(
                    &prior.platform,
                    &target,
                    &prepared.transitions,
                    prepared.layout_operation_count,
                    &forged,
                )
                .is_err()
        );
    }

    let forged_receipt = PlatformOwnerReceipt::linux(
        1,
        serde_json::to_vec(&json!({
            "invocation_id": "a".repeat(32),
            "main_pid": 0,
            "unit": fixture.candidate.id(),
        }))
        .expect("forged receipt JSON"),
    )
    .expect("bounded forged receipt");
    assert!(
        platform
            .matches_exact_state(
                hypercolor_cli::install::PlatformCheckpoint::PriorOriginal,
                &prior.platform,
                0,
                &prepared.record,
                Some(&forged_receipt),
            )
            .is_err()
    );
}

#[test]
fn byte_identical_unit_from_a_split_root_is_rejected_before_inspection() {
    let fixture = Fixture::new();
    let foreign_source = fixture.temp.path().join("foreign-source");
    fs::create_dir(&foreign_source).expect("foreign source");
    let foreign_file = write_release(&foreign_source);
    let manifest = fs::read(foreign_source.join("manifest.json")).expect("foreign manifest");
    let expected = UnitId::new(sha256(&manifest)).expect("foreign unit ID");
    assert_eq!(expected, *fixture.candidate.id());
    let foreign_store = InstallStore::new(fixture.temp.path().join("foreign-store"), 64 * 1024);
    let foreign_lock = foreign_store.acquire_lock().expect("foreign lock");
    let foreign = stage_release_payload(
        &foreign_store,
        &foreign_lock,
        &foreign_source,
        &foreign_file,
        &expected,
    )
    .expect("foreign candidate");
    drop(foreign_lock);

    let mut executor =
        FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    executor.expected_unit_authorities = Some(vec![foreign]);
    let Err(error) = LinuxInstallPlatform::new(executor, config(), [fixture.candidate.clone()])
    else {
        panic!("split root must fail before inspection");
    };
    assert!(error.to_string().contains("split retained unit authority"));

    let executor = FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    let split_config = LinuxInstallConfig {
        direct_fragment_path: FRAGMENT.to_owned(),
        immutable_units_root: PathBuf::from("/tmp/foreign/units"),
        active_root: PathBuf::from("/tmp/foreign/active"),
    };
    let Err(error) = LinuxInstallPlatform::new(executor, split_config, []) else {
        panic!("split config topology must fail before inspection");
    };
    assert!(error.to_string().contains("split install-store topology"));
}

#[test]
fn systemd_unsafe_or_lossy_install_roots_are_rejected_before_inspection() {
    for root in ["/tmp/hyper color", "/tmp/hyper%color"] {
        let config = LinuxInstallConfig {
            direct_fragment_path: FRAGMENT.to_owned(),
            immutable_units_root: PathBuf::from(root).join("units"),
            active_root: PathBuf::from(root).join("active"),
        };
        let executor = FakeExecutor::absent(PathBuf::from(root).join("active"), "00".repeat(32));
        let error = LinuxInstallPlatform::new(executor, config, [])
            .err()
            .expect("unsafe root must fail before inspection");
        assert!(error.to_string().contains("systemd ExecStart"));
    }

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;

        let parent = PathBuf::from(std::ffi::OsString::from_vec(vec![
            b'/', b't', b'm', b'p', b'/', 0xff,
        ]));
        let config = LinuxInstallConfig {
            direct_fragment_path: FRAGMENT.to_owned(),
            immutable_units_root: parent.join("units"),
            active_root: parent.join("active"),
        };
        let executor = FakeExecutor::absent(config.active_root.clone(), "00".repeat(32));
        let error = LinuxInstallPlatform::new(executor, config, [])
            .err()
            .expect("non-UTF-8 root must fail before inspection");
        assert!(error.to_string().contains("exact UTF-8"));
    }
}

fn forge_record(
    record: &PlatformTransactionRecord,
    mutate: impl FnOnce(&mut serde_json::Value),
) -> PlatformTransactionRecord {
    let PlatformTransactionRecord::Linux {
        schema_version,
        payload,
    } = record
    else {
        panic!("expected Linux record");
    };
    let mut value: serde_json::Value = serde_json::from_slice(payload).expect("record JSON");
    mutate(&mut value);
    PlatformTransactionRecord::linux(
        *schema_version,
        serde_json::to_vec(&value).expect("forged record JSON"),
    )
    .expect("bounded forged record")
}

#[test]
fn public_tree_consumes_fresh_home_authority_after_construction() {
    let temp = public_tree_fixture();
    let home = temp.path().join("home");
    fs::create_dir(&home).expect("HOME");
    let store = InstallStore::new(temp.path().join("store"), 64 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let mut tree = LinuxPublicTree::new(&lock, &home).expect("fresh public tree");

    for item in LINUX_DIRECTORY_ITEMS {
        assert_eq!(
            tree.state(item).expect("absent scaffold"),
            LinuxDirectoryState::Absent
        );
        tree.ensure_directory(item, LinuxDirectoryState::Absent)
            .expect("create scaffold");
        assert_eq!(
            tree.state(item).expect("present scaffold"),
            LinuxDirectoryState::Present
        );
    }

    for relative in required_directory_paths() {
        let metadata = fs::metadata(home.join(relative)).expect("created scaffold");
        assert!(metadata.is_dir());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o755);
    }
}

#[test]
fn public_tree_preserves_partial_restrictive_scaffolding_and_rejects_wrong_kind() {
    let temp = public_tree_fixture();
    let home = temp.path().join("home");
    fs::create_dir(&home).expect("HOME");
    fs::create_dir(home.join(".local")).expect(".local");
    fs::set_permissions(home.join(".local"), fs::Permissions::from_mode(0o700))
        .expect("restrict .local");
    fs::create_dir(home.join(".config")).expect(".config");
    fs::set_permissions(home.join(".config"), fs::Permissions::from_mode(0o711))
        .expect("restrict .config");
    let store = InstallStore::new(temp.path().join("store"), 64 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let mut tree = LinuxPublicTree::new(&lock, &home).expect("partial public tree");

    for item in LINUX_DIRECTORY_ITEMS {
        if tree.state(item).expect("scaffold state") == LinuxDirectoryState::Absent {
            tree.ensure_directory(item, LinuxDirectoryState::Absent)
                .expect("create missing scaffold");
        }
    }
    assert_eq!(
        fs::metadata(home.join(".local"))
            .expect(".local metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(home.join(".config"))
            .expect(".config metadata")
            .permissions()
            .mode()
            & 0o777,
        0o711
    );

    let hostile_home = temp.path().join("hostile-home");
    fs::create_dir(&hostile_home).expect("hostile HOME");
    fs::write(hostile_home.join(".local"), b"not a directory").expect("wrong-kind entry");
    let error = LinuxPublicTree::new(&lock, &hostile_home).expect_err("reject wrong-kind scaffold");
    assert!(error.to_string().contains("directory"));
}

fn required_directory_paths() -> [&'static str; 21] {
    [
        ".local",
        ".local/bin",
        ".local/share",
        ".local/share/applications",
        ".local/share/bash-completion",
        ".local/share/bash-completion/completions",
        ".local/share/zsh",
        ".local/share/zsh/site-functions",
        ".local/share/fish",
        ".local/share/fish/vendor_completions.d",
        ".local/share/icons",
        ".local/share/icons/hicolor",
        ".local/share/icons/hicolor/48x48",
        ".local/share/icons/hicolor/48x48/apps",
        ".local/share/icons/hicolor/128x128",
        ".local/share/icons/hicolor/128x128/apps",
        ".local/share/icons/hicolor/256x256",
        ".local/share/icons/hicolor/256x256/apps",
        ".config",
        ".config/systemd",
        ".config/systemd/user",
    ]
}

fn public_tree_fixture() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("linux-public-tree-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("public tree fixture")
}

fn raw_conversion_executor(fixture: &Fixture) -> (FakeExecutor, Vec<u8>) {
    let legacy_daemon = b"legacy-daemon";
    let legacy_exec = "/opt/hypercolor/bin/hypercolor-daemon";
    let launcher_bytes = format!(
        "[Unit]\nDescription=Legacy Hypercolor\n\n[Service]\nType=notify\nExecStart={legacy_exec}\n\n[Install]\nWantedBy=default.target\n"
    )
    .into_bytes();
    let launcher = LinuxExactEntry::RegularFile {
        mode: 0o600,
        sha256: sha256(&launcher_bytes),
        snapshot_unit: None,
        snapshot_path: None,
    };
    let mut layout = LINUX_LAYOUT_ITEMS
        .into_iter()
        .map(|item| (item, LinuxExactEntry::Absent))
        .collect::<BTreeMap<_, _>>();
    layout.insert(
        LinuxLayoutItem::HypercolorDaemon,
        LinuxExactEntry::RegularFile {
            mode: 0o755,
            sha256: sha256(legacy_daemon),
            snapshot_unit: None,
            snapshot_path: None,
        },
    );
    layout.insert(
        LinuxLayoutItem::Icon48,
        LinuxExactEntry::RegularFile {
            mode: 0o644,
            sha256: sha256(b"legacy-icon"),
            snapshot_unit: None,
            snapshot_path: None,
        },
    );
    let legacy_root = fixture.temp.path().join("legacy-snapshot");
    fs::create_dir_all(legacy_root.join("bin")).expect("legacy bin");
    fs::write(legacy_root.join("bin/hypercolor-daemon"), legacy_daemon).expect("legacy daemon");
    fs::set_permissions(
        legacy_root.join("bin/hypercolor-daemon"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("legacy daemon mode");
    fs::write(
        legacy_root.join("manifest.json"),
        serde_json::to_vec(&json!({"version":VERSION})).expect("legacy manifest"),
    )
    .expect("legacy manifest file");
    let legacy_icon = legacy_root.join("share/icons/hicolor/48x48/apps/hypercolor.png");
    fs::create_dir_all(legacy_icon.parent().expect("legacy icon parent"))
        .expect("legacy icon directory");
    fs::write(&legacy_icon, b"legacy-icon").expect("legacy icon");
    fs::set_permissions(&legacy_icon, fs::Permissions::from_mode(0o644)).expect("legacy icon mode");

    let mut executor =
        FakeExecutor::absent(fixture.store.active_path(), fixture.daemon_digest.clone());
    executor.launcher = launcher;
    executor.launcher_bytes.clone_from(&launcher_bytes);
    executor.layout = layout;
    executor.legacy_inventory = vec![
        LinuxLegacyFile {
            path: "bin/hyper".to_owned(),
            mode: 0o755,
            contents: b"old-cli".to_vec(),
        },
        LinuxLegacyFile {
            path: "bin/hypercolor-tray".to_owned(),
            mode: 0o755,
            contents: b"old-tray".to_vec(),
        },
        LinuxLegacyFile {
            path: "home/.config/fish/completions/hyper.fish".to_owned(),
            mode: 0o600,
            contents: b"old-fish".to_vec(),
        },
        LinuxLegacyFile {
            path: "home/.config/fish/completions/hypercolor.fish".to_owned(),
            mode: 0o644,
            contents: b"fish".to_vec(),
        },
        LinuxLegacyFile {
            path: "share/bash-completion/completions/hyper".to_owned(),
            mode: 0o644,
            contents: b"old-bash".to_vec(),
        },
        LinuxLegacyFile {
            path: "share/hypercolor/ui/assets/app.js".to_owned(),
            mode: 0o644,
            contents: b"script".to_vec(),
        },
        LinuxLegacyFile {
            path: "share/icons/hicolor/scalable/apps/hypercolor-symbolic.svg".to_owned(),
            mode: 0o644,
            contents: b"icon".to_vec(),
        },
        LinuxLegacyFile {
            path: "share/zsh/site-functions/_hyper".to_owned(),
            mode: 0o644,
            contents: b"old-zsh".to_vec(),
        },
    ];
    executor.systemd = FakeSystemd {
        load: "loaded",
        fragment: FRAGMENT.to_owned(),
        active: true,
        enabled: true,
        exec_start: legacy_exec.to_owned(),
        pid: 313,
        invocation: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
    };
    let legacy_metadata =
        fs::metadata(legacy_root.join("bin/hypercolor-daemon")).expect("legacy daemon metadata");
    executor.process_override = Some(LinuxProcessExecutable {
        path: legacy_exec.to_owned(),
        sha256: sha256(legacy_daemon),
        device: legacy_metadata.dev(),
        inode: legacy_metadata.ino(),
    });
    executor.legacy_snapshot_root = Some(legacy_root);
    (executor, launcher_bytes)
}

struct Fixture {
    temp: tempfile::TempDir,
    store: InstallStore,
    candidate: hypercolor_cli::install::UnitRecord,
    daemon_digest: String,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("source");
        fs::create_dir(&source).expect("source root");
        let candidate_file = write_release(&source);
        let manifest = fs::read(source.join("manifest.json")).expect("manifest");
        let expected = UnitId::new(sha256(&manifest)).expect("unit ID");
        let store = InstallStore::new(temp.path().join("store"), 64 * 1024);
        let lock = store.acquire_lock().expect("stage lock");
        let candidate = stage_release_payload(&store, &lock, &source, &candidate_file, &expected)
            .expect("stage candidate");
        drop(lock);
        Self {
            temp,
            store,
            candidate,
            daemon_digest: sha256(b"daemon"),
        }
    }

    fn request(&self, target_policy: InstallTargetPolicy) -> InstallRequest {
        self.request_for(self.candidate.clone(), target_policy, "base")
    }

    fn request_for(
        &self,
        candidate: hypercolor_cli::install::UnitRecord,
        target_policy: InstallTargetPolicy,
        suffix: &str,
    ) -> InstallRequest {
        InstallRequest {
            transaction_id: InstallTransactionId::new(format!("linux-{target_policy:?}-{suffix}"))
                .expect("transaction ID"),
            candidate,
            target_policy,
        }
    }

    fn stage_upgrade(&self) -> hypercolor_cli::install::UnitRecord {
        let source = self.temp.path().join("upgrade-source");
        fs::create_dir(&source).expect("upgrade source root");
        let candidate_file = write_release_with(&source, UPGRADE_VERSION, b"daemon-upgrade");
        let manifest = fs::read(source.join("manifest.json")).expect("upgrade manifest");
        let expected = UnitId::new(sha256(&manifest)).expect("upgrade unit ID");
        let lock = self.store.acquire_lock().expect("upgrade stage lock");
        stage_release_payload(&self.store, &lock, &source, &candidate_file, &expected)
            .expect("stage upgrade candidate")
    }
}

fn config() -> LinuxInstallConfig {
    LinuxInstallConfig {
        direct_fragment_path: FRAGMENT.to_owned(),
        immutable_units_root: PathBuf::from(UNITS_ROOT),
        active_root: PathBuf::from(ACTIVE_ROOT),
    }
}

fn write_release(root: &Path) -> File {
    write_release_with(root, VERSION, b"daemon")
}

fn write_release_with(root: &Path, version: &str, daemon: &[u8]) -> File {
    let directories = [
        "bin",
        "share",
        "share/hypercolor",
        "share/hypercolor/ui",
        "share/hypercolor/effects",
        "share/hypercolor/effects/bundled",
        "share/hypercolor/docs",
        "share/hypercolor/agents",
        "share/hypercolor/agents/skills",
        "share/hypercolor/agents/agents",
        "share/hypercolor/site",
    ];
    let files = [
        ("bin/hypercolor-daemon", daemon),
        ("bin/hypercolor", b"candidate".as_slice()),
        ("bin/hypercolor-app", b"app".as_slice()),
        ("bin/hypercolor-tui", b"tui".as_slice()),
        ("bin/hypercolor-open", b"open".as_slice()),
        ("share/hypercolor/ui/index.html", b"ui".as_slice()),
        (
            "share/hypercolor/effects/bundled/effect.html",
            b"effect".as_slice(),
        ),
        (
            "share/hypercolor/agents/skills/skill.md",
            b"skill".as_slice(),
        ),
        (
            "share/hypercolor/agents/agents/agent.md",
            b"agent".as_slice(),
        ),
    ];
    let mut members = Vec::new();
    for directory in directories {
        fs::create_dir_all(root.join(directory)).expect("directory");
        fs::set_permissions(root.join(directory), fs::Permissions::from_mode(0o755)).expect("mode");
        members.push(json!({"path":directory,"type":"directory","mode":0o755}));
    }
    for (path, bytes) in files {
        fs::write(root.join(path), bytes).expect("file");
        let mode = if path.starts_with("bin/") {
            0o755
        } else {
            0o644
        };
        fs::set_permissions(root.join(path), fs::Permissions::from_mode(mode)).expect("mode");
        members.push(json!({
            "path":path,"type":"file","mode":mode,"size":bytes.len(),"sha256":sha256(bytes)
        }));
    }
    members.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let manifest = serde_json::to_vec_pretty(&json!({
        "name":"hypercolor","version":version,"platform":"linux-x86_64",
        "rust_target":"x86_64-unknown-linux-gnu",
        "binaries":["hypercolor-daemon","hypercolor","hypercolor-app","hypercolor-tui","hypercolor-open"],
        "assets":{"ui_files":1,"bundled_effect_files":1,"docs_files":0,"skill_files":1,"agent_files":1,"site_files":0},
        "members":members,
    }))
    .expect("manifest JSON");
    fs::write(root.join("manifest.json"), manifest).expect("manifest");
    fs::set_permissions(
        root.join("manifest.json"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("manifest mode");
    File::open(root.join("bin/hypercolor")).expect("candidate")
}

fn launcher_exec(bytes: &[u8]) -> String {
    std::str::from_utf8(bytes)
        .expect("launcher UTF-8")
        .lines()
        .find_map(|line| line.strip_prefix("ExecStart="))
        .expect("ExecStart")
        .to_owned()
}

fn item_name(item: LinuxLayoutItem) -> &'static str {
    match item {
        LinuxLayoutItem::Hypercolor => "hypercolor",
        LinuxLayoutItem::HypercolorDaemon => "hypercolor-daemon",
        LinuxLayoutItem::HypercolorApp => "hypercolor-app",
        LinuxLayoutItem::HypercolorTui => "hypercolor-tui",
        LinuxLayoutItem::HypercolorOpen => "hypercolor-open",
        LinuxLayoutItem::DesktopEntry => "desktop-entry",
        LinuxLayoutItem::BashCompletion => "bash-completion",
        LinuxLayoutItem::ZshCompletion => "zsh-completion",
        LinuxLayoutItem::FishCompletion => "fish-completion",
        LinuxLayoutItem::Icon48 => "icon-48",
        LinuxLayoutItem::Icon128 => "icon-128",
        LinuxLayoutItem::Icon256 => "icon-256",
    }
}

fn legacy_item_path(item: LinuxLayoutItem) -> &'static str {
    match item {
        LinuxLayoutItem::Hypercolor => "bin/hypercolor",
        LinuxLayoutItem::HypercolorDaemon => "bin/hypercolor-daemon",
        LinuxLayoutItem::HypercolorApp => "bin/hypercolor-app",
        LinuxLayoutItem::HypercolorTui => "bin/hypercolor-tui",
        LinuxLayoutItem::HypercolorOpen => "bin/hypercolor-open",
        LinuxLayoutItem::DesktopEntry => "share/applications/hypercolor.desktop",
        LinuxLayoutItem::BashCompletion => "share/bash-completion/completions/hypercolor",
        LinuxLayoutItem::ZshCompletion => "share/zsh/site-functions/_hypercolor",
        LinuxLayoutItem::FishCompletion => "share/fish/vendor_completions.d/hypercolor.fish",
        LinuxLayoutItem::Icon48 => "share/icons/hicolor/48x48/apps/hypercolor.png",
        LinuxLayoutItem::Icon128 => "share/icons/hicolor/128x128/apps/hypercolor.png",
        LinuxLayoutItem::Icon256 => "share/icons/hicolor/256x256/apps/hypercolor.png",
    }
}

fn write_legacy_member(root: &Path, relative: &str, mode: u32, contents: &[u8]) {
    let destination = root.join(relative);
    fs::create_dir_all(destination.parent().expect("legacy member parent"))
        .expect("legacy member directories");
    fs::write(&destination, contents).expect("legacy member");
    fs::set_permissions(destination, fs::Permissions::from_mode(mode)).expect("legacy member mode");
}

fn first_install_effects() -> Vec<String> {
    let mut effects = LINUX_DIRECTORY_ITEMS
        .into_iter()
        .map(|item| format!("directory:{item:?}:create"))
        .collect::<Vec<_>>();
    effects.extend(
        LINUX_LAYOUT_ITEMS
            .into_iter()
            .map(|item| format!("layout:{}", item_name(item))),
    );
    effects.extend([
        "launcher".to_owned(),
        "manager".to_owned(),
        "autostart:true".to_owned(),
        "runtime:true".to_owned(),
    ]);
    effects
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn exact_entries_match(left: &LinuxExactEntry, right: &LinuxExactEntry) -> bool {
    match (left, right) {
        (LinuxExactEntry::Absent, LinuxExactEntry::Absent) => true,
        (LinuxExactEntry::Symlink { target: left }, LinuxExactEntry::Symlink { target: right }) => {
            left == right
        }
        (
            LinuxExactEntry::RegularFile {
                mode: left_mode,
                sha256: left_digest,
                ..
            },
            LinuxExactEntry::RegularFile {
                mode: right_mode,
                sha256: right_digest,
                ..
            },
        ) => left_mode == right_mode && left_digest == right_digest,
        _ => false,
    }
}
