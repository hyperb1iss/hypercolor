//! Persistent child ownership for CLI-only OpenRGB control.

use std::fs::OpenOptions;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    DeviceFacts, DriverFacts, OpenRgbHoldReason, OpenRgbPlan, OpenRgbPlanInputs, ProcessSpec,
    ServerProbe, bridge_enabled, detect_binary, detector_prefixes_for_drivers, install_hints,
    known_detector_driver_ids, managed_config_dir, openrgb_plan, partition_driver_ids,
    permission_checks, probe_server, server_command_at, write_detector_partition,
};

/// Fresh daemon facts supplied over an authenticated local control connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartFacts {
    pub drivers: Vec<DriverFacts>,
    pub devices: Vec<DeviceFacts>,
    pub endpoint: SocketAddr,
}

/// Owns only the child this process spawned. External SDK servers are adopted
/// for output but never become eligible for stop.
/// Linux and Windows also terminate the child when its owner dies abruptly.
/// On macOS, an external binary has no parent-death hook; abrupt owner death
/// can leave an adopted server that must be stopped outside Hypercolor.
pub struct OpenRgbOwner {
    directory: PathBuf,
    child: Option<Child>,
    endpoint: Option<SocketAddr>,
    #[cfg(target_os = "windows")]
    guard: Option<win32job::Job>,
    status: Value,
    // Release authority only after native guards have terminated descendants.
    server_claim: Option<crate::ServerClaim>,
}

impl OpenRgbOwner {
    #[must_use]
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            child: None,
            endpoint: None,
            server_claim: None,
            status: json!({"managed_pid": null, "adopted": false}),
            #[cfg(target_os = "windows")]
            guard: None,
        }
    }

    /// Reap a completed child without restarting it.
    pub fn reap(&mut self) -> io::Result<bool> {
        if let Some(child) = &mut self.child
            && let Some(exit) = crate::reap_owned_server(child)?
        {
            self.child = None;
            self.endpoint = None;
            #[cfg(target_os = "windows")]
            {
                self.guard = None;
            }
            self.server_claim = None;
            self.status = json!({"managed_pid": null, "adopted": false, "exited": exit.code()});
            return Ok(true);
        }
        Ok(false)
    }

    #[must_use]
    pub fn status(&self) -> Value {
        self.status.clone()
    }

    /// Apply shared native ownership policy, then adopt or spawn the server.
    pub async fn start(&mut self, facts: StartFacts) -> io::Result<Value> {
        if !facts.endpoint.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "local OpenRGB owner requires a loopback endpoint",
            ));
        }
        self.reap()?;
        if self
            .endpoint
            .is_some_and(|endpoint| endpoint != facts.endpoint)
        {
            return Err(io::Error::other(
                "stop the managed OpenRGB server before changing its endpoint",
            ));
        }
        let binary = detect_binary().await;
        let probe = probe_server(facts.endpoint, Duration::from_millis(750)).await;
        let port_open = probe.reachable
            || matches!(
                tokio::time::timeout(
                    Duration::from_millis(750),
                    tokio::net::TcpStream::connect(facts.endpoint)
                )
                .await,
                Ok(Ok(_))
            );
        let config = managed_config_dir(&self.directory);
        let spawn = binary
            .as_ref()
            .map(|binary| server_command_at(binary, &config, facts.endpoint))
            .transpose()
            .map_err(io::Error::other)?;
        let plan = openrgb_plan(OpenRgbPlanInputs {
            bridge_enabled: bridge_enabled(&facts.drivers),
            binary,
            addr: facts.endpoint,
            probe,
            port_open,
            checks: permission_checks(),
            hints: install_hints(),
            spawn,
            managed_pid: self.child.as_ref().map(Child::id),
        });
        match plan {
            OpenRgbPlan::Adopt { probe, .. } => {
                self.status = json!({"managed_pid": self.child.as_ref().map(Child::id), "adopted": self.child.is_none(), "probe": probe});
            }
            OpenRgbPlan::Hold { reason } => {
                let message = match &reason {
                    OpenRgbHoldReason::Starting { .. } => "OpenRGB is still starting",
                    OpenRgbHoldReason::BridgeDisabled => {
                        "Enable the OpenRGB bridge driver before starting its server"
                    }
                    OpenRgbHoldReason::NotInstalled { .. } => {
                        "Install OpenRGB before starting its server"
                    }
                    OpenRgbHoldReason::PermissionsMissing { .. } => {
                        "OpenRGB requires host permissions; inspect the permission checks and remedies"
                    }
                    OpenRgbHoldReason::PortOwnedByUnknown { .. } => {
                        "The configured OpenRGB port belongs to another service"
                    }
                };
                self.status = json!({"managed_pid": self.child.as_ref().map(Child::id), "adopted": false, "hold": reason, "message": message});
                if !matches!(reason, OpenRgbHoldReason::Starting { .. }) {
                    return Err(io::Error::other(message));
                }
            }
            OpenRgbPlan::Spawn { mut spec } => {
                let Some(claim) = crate::try_claim_server(&self.directory, facts.endpoint)? else {
                    self.status = json!({"managed_pid": null, "adopted": false, "hold": {"kind": "other_owner"}, "message": "Another Hypercolor process owns this OpenRGB server or its startup"});
                    return Ok(self.status());
                };
                let partition = partition_driver_ids(
                    &facts.drivers,
                    &facts.devices,
                    &known_detector_driver_ids(),
                );
                write_detector_partition(
                    &config,
                    &detector_prefixes_for_drivers(&partition.disabled_driver_ids),
                    &detector_prefixes_for_drivers(&partition.re_enable_driver_ids),
                    None,
                )
                .map_err(io::Error::other)?;
                apply_offscreen(&mut spec);
                self.spawn(&spec)?;
                self.endpoint = Some(facts.endpoint);
                self.server_claim = Some(claim);
                let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
                let mut probe = ServerProbe::default();
                while tokio::time::Instant::now() < deadline {
                    if self.reap()? {
                        return Err(io::Error::other(
                            "OpenRGB exited before its SDK server answered; see logs/openrgb.log",
                        ));
                    }
                    probe = probe_server(facts.endpoint, Duration::from_millis(750)).await;
                    if probe.reachable {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                self.status = json!({"managed_pid": self.child.as_ref().map(Child::id), "adopted": false, "probe": probe,
                    "last_error": if probe.reachable { None } else { Some("OpenRGB is still starting; see logs/openrgb.log") }});
            }
        }
        Ok(self.status())
    }

    fn spawn(&mut self, spec: &ProcessSpec) -> io::Result<()> {
        let log_dir = self.directory.join("logs");
        std::fs::create_dir_all(&log_dir)?;
        let log = OpenOptions::new()
            .append(true)
            .create(true)
            .open(log_dir.join("openrgb.log"))?;
        let mut command = Command::new(&spec.program);
        command
            .env_remove("HYPERCOLOR_API_KEY")
            .args(&spec.args)
            .envs(&spec.env)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        configure_detached(&mut command);
        #[cfg(target_os = "linux")]
        hypercolor_linux_session::arm_parent_death(&mut command, std::process::id());
        let child = command.spawn()?;
        #[cfg(target_os = "windows")]
        let child = {
            let mut child = child;
            use std::os::windows::io::AsRawHandle;
            let result = (|| {
                let mut limits = win32job::ExtendedLimitInfo::new();
                limits.limit_kill_on_job_close();
                let job =
                    win32job::Job::create_with_limit_info(&limits).map_err(io::Error::other)?;
                job.assign_process(child.as_raw_handle() as isize)
                    .map_err(io::Error::other)?;
                Ok::<_, io::Error>(job)
            })();
            match result {
                Ok(job) => self.guard = Some(job),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            }
            child
        };
        self.child = Some(child);
        Ok(())
    }

    /// Stop only the retained child. A child is never addressed through a
    /// saved pid and cannot have its pid recycled before it is reaped.
    pub fn stop(&mut self) -> io::Result<Value> {
        let stopped = self.child.is_some();
        if let Some(child) = &mut self.child {
            crate::stop_owned_server(child, Duration::from_secs(3))?;
            self.child = None;
            self.endpoint = None;
            #[cfg(target_os = "windows")]
            {
                self.guard = None;
            }
            self.server_claim = None;
        }
        self.status = json!({"managed_pid": null, "adopted": false, "stopped": stopped});
        Ok(self.status())
    }
}

impl Drop for OpenRgbOwner {
    fn drop(&mut self) {
        if self.stop().is_err()
            && let Some(child) = &mut self.child
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Spawn a persistent owner without inheriting terminal streams.
pub fn spawn_owner(executable: &Path, directory: &Path) -> io::Result<Child> {
    let logs = directory.join("logs");
    std::fs::create_dir_all(&logs)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs.join("openrgb-owner.log"))?;
    let mut command = Command::new(executable);
    command
        .env_remove("HYPERCOLOR_API_KEY")
        .env_remove("HYPERCOLOR_HOST")
        .env_remove("HYPERCOLOR_PORT")
        .env_remove("HYPERCOLOR_PROFILE")
        .arg("openrgb-owner")
        .arg("--data-dir")
        .arg(directory)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    configure_detached(&mut command);
    command.spawn()
}

fn configure_detached(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
}

fn apply_offscreen(spec: &mut ProcessSpec) {
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_none_or(|value| value.is_empty())
        && std::env::var_os("WAYLAND_DISPLAY").is_none_or(|value| value.is_empty())
        && std::env::var_os("QT_QPA_PLATFORM").is_none_or(|value| value.is_empty())
    {
        spec.env
            .entry("QT_QPA_PLATFORM".to_owned())
            .or_insert_with(|| "offscreen".to_owned());
    }
    #[cfg(not(target_os = "linux"))]
    let _ = spec;
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn stop_reaps_only_the_retained_process_tree() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let mut owner = OpenRgbOwner::new(dir.path().to_owned());
        let spec = ProcessSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".to_owned(), "sleep 60 & wait".to_owned()],
            ..ProcessSpec::default()
        };
        owner.spawn(&spec).expect("spawn test child");
        assert!(owner.child.is_some());
        owner.stop().expect("stop test child");
        assert!(owner.child.is_none());
        owner.stop().expect("repeated stop is harmless");
    }

    #[tokio::test]
    async fn changing_endpoint_requires_stopping_the_retained_child() {
        let dir = tempfile::tempdir().expect("directory");
        let mut owner = OpenRgbOwner::new(dir.path().to_owned());
        owner
            .spawn(&ProcessSpec {
                program: "/bin/sh".into(),
                args: vec!["-c".to_owned(), "sleep 60".to_owned()],
                ..ProcessSpec::default()
            })
            .expect("fixture");
        owner.endpoint = Some("127.0.0.1:6742".parse().expect("old endpoint"));
        let result = owner
            .start(StartFacts {
                drivers: vec![],
                devices: vec![],
                endpoint: "127.0.0.1:6789".parse().expect("new endpoint"),
            })
            .await;
        assert!(
            result
                .expect_err("endpoint drift")
                .to_string()
                .contains("stop the managed OpenRGB server")
        );
        assert!(owner.child.is_some());
        owner.stop().expect("stop fixture");
        assert!(owner.endpoint.is_none());
    }
}
