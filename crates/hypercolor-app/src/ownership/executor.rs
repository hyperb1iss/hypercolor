use std::path::PathBuf;
use std::time::Duration;

use hypercolor_macos_owner::{
    MACOS_APP_PRODUCT_NAME, MacosDaemonOwner, MacosOwnerExecutionError, MacosOwnerIncarnation,
    MacosOwnerStore,
};
use tauri::{AppHandle, Runtime};

use super::homebrew;
use super::launchd::{
    command_output, command_stdout, launchctl_service_disabled, run_command,
    service_autostart_enabled, service_plist, service_target,
};
use crate::supervisor::SupervisorState;

pub(super) struct AppOwnerExecutor<R: Runtime> {
    app: AppHandle<R>,
    state: SupervisorState,
    daemon_url: url::Url,
    store: MacosOwnerStore,
    uid: String,
    launch_agents: PathBuf,
    pub(super) app_sidecar_supervisor_started: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AppOwnerStopAuthority {
    SupervisorChild,
    LaunchctlService(String),
    HomebrewService(&'static str),
    UserDirected,
}

impl<R: Runtime> AppOwnerExecutor<R> {
    pub(super) fn new(
        app: AppHandle<R>,
        state: SupervisorState,
        daemon_url: url::Url,
        store: MacosOwnerStore,
    ) -> Result<Self, anyhow::Error> {
        let uid = command_stdout("/usr/bin/id", &["-u"])?;
        let launch_agents = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("failed to resolve the user home directory"))?
            .join("Library/LaunchAgents");
        Ok(Self {
            app,
            state,
            daemon_url,
            store,
            uid,
            launch_agents,
            app_sidecar_supervisor_started: false,
        })
    }

    fn stop_authority(
        &self,
        owner: MacosDaemonOwner,
    ) -> Result<AppOwnerStopAuthority, MacosOwnerExecutionError> {
        app_owner_stop_authority(owner, &self.uid)
    }
}

impl<R: Runtime> hypercolor_macos_owner::MacosOwnerExecutor for AppOwnerExecutor<R> {
    fn autostart_enabled(
        &mut self,
        owner: MacosDaemonOwner,
    ) -> Result<bool, MacosOwnerExecutionError> {
        use tauri_plugin_autostart::ManagerExt;

        match owner {
            MacosDaemonOwner::AppSidecar => self
                .app
                .autolaunch()
                .is_enabled()
                .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
                .and_then(|enabled| {
                    if !enabled {
                        Ok(false)
                    } else {
                        let output = command_output(
                            "/bin/launchctl",
                            &["print-disabled", &format!("gui/{}", self.uid)],
                        )?;
                        if !output.status.success() {
                            return Err(MacosOwnerExecutionError::new(
                                "launchctl failed to inspect app autostart state",
                            ));
                        }
                        Ok(!launchctl_service_disabled(
                            &String::from_utf8_lossy(&output.stdout),
                            MACOS_APP_PRODUCT_NAME,
                        ))
                    }
                }),
            MacosDaemonOwner::DirectLaunchd | MacosDaemonOwner::Homebrew => {
                service_autostart_enabled(owner, &self.uid, &self.launch_agents)
            }
            MacosDaemonOwner::Standalone => Err(MacosOwnerExecutionError::new(
                "standalone has no autostart state",
            )),
        }
    }

    fn set_autostart(
        &mut self,
        owner: MacosDaemonOwner,
        enabled: bool,
    ) -> Result<(), MacosOwnerExecutionError> {
        use tauri_plugin_autostart::ManagerExt;

        match owner {
            MacosDaemonOwner::AppSidecar => {
                update_app_sidecar_gate_for_autostart(&self.state, enabled);
                if enabled {
                    self.app
                        .autolaunch()
                        .enable()
                        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
                    run_command(
                        "/bin/launchctl",
                        &[
                            "enable",
                            &format!("gui/{}/{}", self.uid, MACOS_APP_PRODUCT_NAME),
                        ],
                    )
                } else {
                    run_command(
                        "/bin/launchctl",
                        &[
                            "disable",
                            &format!("gui/{}/{}", self.uid, MACOS_APP_PRODUCT_NAME),
                        ],
                    )?;
                    self.app
                        .autolaunch()
                        .disable()
                        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
                }
            }
            MacosDaemonOwner::DirectLaunchd | MacosDaemonOwner::Homebrew => {
                let action = if enabled { "enable" } else { "disable" };
                run_command(
                    "/bin/launchctl",
                    &[action, &service_target(owner, &self.uid)?],
                )
            }
            MacosDaemonOwner::Standalone => Err(MacosOwnerExecutionError::new(
                "standalone has no autostart state",
            )),
        }
    }

    fn preflight_stop_authority(
        &mut self,
        incarnation: &MacosOwnerIncarnation,
    ) -> Result<(), MacosOwnerExecutionError> {
        match self.stop_authority(incarnation.owner)? {
            AppOwnerStopAuthority::SupervisorChild => {
                self.state.preflight_app_sidecar_stop(incarnation)
            }
            AppOwnerStopAuthority::LaunchctlService(_)
            | AppOwnerStopAuthority::HomebrewService(_) => Ok(()),
            AppOwnerStopAuthority::UserDirected => Err(MacosOwnerExecutionError::new(
                "standalone termination requires its terminal user",
            )),
        }
    }

    fn flush_and_stop(
        &mut self,
        incarnation: &MacosOwnerIncarnation,
    ) -> Result<(), MacosOwnerExecutionError> {
        if incarnation.owner == MacosDaemonOwner::AppSidecar {
            hold_app_sidecar_supervisor(&self.state);
        }
        match self.stop_authority(incarnation.owner)? {
            AppOwnerStopAuthority::SupervisorChild => self.state.stop_app_sidecar(incarnation),
            AppOwnerStopAuthority::LaunchctlService(target) => {
                let output = command_output("/bin/launchctl", &["print", &target])?;
                if !output.status.success() {
                    return Ok(());
                }
                run_command("/bin/launchctl", &["kill", "SIGTERM", &target])
            }
            AppOwnerStopAuthority::HomebrewService(formula) => homebrew::stop_service(formula),
            AppOwnerStopAuthority::UserDirected => Err(MacosOwnerExecutionError::new(
                "standalone termination requires its terminal user",
            )),
        }
    }

    fn start(&mut self, owner: MacosDaemonOwner) -> Result<(), MacosOwnerExecutionError> {
        match owner {
            MacosDaemonOwner::AppSidecar => {
                crate::supervisor::start_app_sidecar_for_handover(
                    &self.app,
                    self.daemon_url.clone(),
                )
                .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
                release_app_sidecar_supervisor(&self.state);
                self.app_sidecar_supervisor_started = true;
                Ok(())
            }
            MacosDaemonOwner::DirectLaunchd => {
                let target = service_target(owner, &self.uid)?;
                if command_output("/bin/launchctl", &["print", &target])?
                    .status
                    .success()
                {
                    run_command("/bin/launchctl", &["kickstart", &target])
                } else {
                    let plist = service_plist(owner, &self.launch_agents)?;
                    run_command(
                        "/bin/launchctl",
                        &[
                            "bootstrap",
                            &format!("gui/{}", self.uid),
                            &plist.to_string_lossy(),
                        ],
                    )
                }
            }
            MacosDaemonOwner::Homebrew => homebrew::start_service(),
            MacosDaemonOwner::Standalone => Err(MacosOwnerExecutionError::new(
                "standalone cannot be started by the app coordinator",
            )),
        }
    }

    fn wait_for_guard_release(
        &mut self,
        timeout: Duration,
    ) -> Result<bool, MacosOwnerExecutionError> {
        hypercolor_macos_owner::wait_for_macos_guard_release(
            timeout,
            &std::env::temp_dir()
                .join("hypercolor-daemon.lock")
                .to_string_lossy(),
        )
    }

    fn wait_for_owner(
        &mut self,
        owner: MacosDaemonOwner,
        after_epoch: u64,
        timeout: Duration,
    ) -> Result<bool, MacosOwnerExecutionError> {
        hypercolor_macos_owner::wait_for_owner_publication(&self.store, owner, after_epoch, timeout)
    }
}

pub(super) fn update_app_sidecar_gate_for_autostart(state: &SupervisorState, enabled: bool) {
    if !enabled {
        hold_app_sidecar_supervisor(state);
    }
}

pub(super) fn hold_app_sidecar_supervisor(state: &SupervisorState) {
    state.set_owner_handover_stop(true);
}

pub(super) fn release_app_sidecar_supervisor(state: &SupervisorState) {
    state.set_owner_handover_stop(false);
}

pub(super) fn app_owner_stop_authority(
    owner: MacosDaemonOwner,
    uid: &str,
) -> Result<AppOwnerStopAuthority, MacosOwnerExecutionError> {
    Ok(match owner {
        MacosDaemonOwner::AppSidecar => AppOwnerStopAuthority::SupervisorChild,
        MacosDaemonOwner::DirectLaunchd => {
            AppOwnerStopAuthority::LaunchctlService(service_target(owner, uid)?)
        }
        MacosDaemonOwner::Homebrew => AppOwnerStopAuthority::HomebrewService("hypercolor"),
        MacosDaemonOwner::Standalone => AppOwnerStopAuthority::UserDirected,
    })
}
