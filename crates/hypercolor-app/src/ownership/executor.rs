use std::path::PathBuf;
use std::time::Duration;

use hypercolor_macos_owner::{
    LaunchdAdapter, MACOS_APP_PRODUCT_NAME, MacosDaemonOwner, MacosOwnerExecutionError,
    MacosOwnerIncarnation, MacosOwnerStore, launch_agent_plist,
};
use hypercolor_types::service::{ServiceManager, StopAuthority};
use tauri::{AppHandle, Runtime};

use super::homebrew;
use super::launchd::service_label;
use super::model::service_identity;
use crate::supervisor::SupervisorState;

pub(super) struct AppOwnerExecutor<R: Runtime> {
    app: AppHandle<R>,
    state: SupervisorState,
    daemon_url: url::Url,
    store: MacosOwnerStore,
    launchd: LaunchdAdapter,
    launch_agents: PathBuf,
    pub(super) app_sidecar_supervisor_started: bool,
}

impl<R: Runtime> AppOwnerExecutor<R> {
    pub(super) fn new(
        app: AppHandle<R>,
        state: SupervisorState,
        daemon_url: url::Url,
        store: MacosOwnerStore,
    ) -> Result<Self, anyhow::Error> {
        let launchd = LaunchdAdapter::for_current_user()?;
        let launch_agents = hypercolor_core::config::paths::macos_launch_agents_dir()
            .ok_or_else(|| anyhow::anyhow!("failed to resolve the user home directory"))?;
        Ok(Self {
            app,
            state,
            daemon_url,
            store,
            launchd,
            launch_agents,
            app_sidecar_supervisor_started: false,
        })
    }

    fn launchd_target(
        &self,
        owner: MacosDaemonOwner,
    ) -> Result<hypercolor_macos_owner::LaunchdTarget, MacosOwnerExecutionError> {
        Ok(self.launchd.target(service_label(owner)?))
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
                    if enabled {
                        self.launchd
                            .autostart_enabled(&self.launchd.target(MACOS_APP_PRODUCT_NAME))
                    } else {
                        Ok(false)
                    }
                }),
            MacosDaemonOwner::DirectLaunchd | MacosDaemonOwner::Homebrew => {
                let target = self.launchd_target(owner)?;
                if !launch_agent_plist(&self.launch_agents, &target).is_file() {
                    return Ok(false);
                }
                self.launchd.autostart_enabled(&target)
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
                let target = self.launchd.target(MACOS_APP_PRODUCT_NAME);
                if enabled {
                    self.app
                        .autolaunch()
                        .enable()
                        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
                    self.launchd.set_autostart(&target, true)
                } else {
                    self.launchd.set_autostart(&target, false)?;
                    self.app
                        .autolaunch()
                        .disable()
                        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
                }
            }
            MacosDaemonOwner::DirectLaunchd | MacosDaemonOwner::Homebrew => self
                .launchd
                .set_autostart(&self.launchd_target(owner)?, enabled),
            MacosDaemonOwner::Standalone => Err(MacosOwnerExecutionError::new(
                "standalone has no autostart state",
            )),
        }
    }

    fn preflight_stop_authority(
        &mut self,
        incarnation: &MacosOwnerIncarnation,
    ) -> Result<(), MacosOwnerExecutionError> {
        match app_owner_stop_authority(incarnation.owner) {
            StopAuthority::SupervisedChild => self.state.preflight_app_sidecar_stop(incarnation),
            StopAuthority::ServiceManager(_) => Ok(()),
            StopAuthority::UserDirected => Err(MacosOwnerExecutionError::new(
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
        match app_owner_stop_authority(incarnation.owner) {
            StopAuthority::SupervisedChild => self.state.stop_app_sidecar(incarnation),
            StopAuthority::ServiceManager(identity) => match identity.manager {
                Some(ServiceManager::Launchd) => {
                    self.launchd.stop(&self.launchd_target(incarnation.owner)?)
                }
                Some(ServiceManager::Homebrew) => homebrew::stop_service(homebrew::FORMULA),
                Some(ServiceManager::Systemd | ServiceManager::WindowsScm) | None => {
                    Err(MacosOwnerExecutionError::new(format!(
                        "{identity} is not a macOS service manager"
                    )))
                }
            },
            StopAuthority::UserDirected => Err(MacosOwnerExecutionError::new(
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
                let target = self.launchd_target(owner)?;
                let plist = launch_agent_plist(&self.launch_agents, &target);
                self.launchd.start(&target, &plist)
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

/// Who may stop a macOS owner, derived from its neutral identity.
pub(super) fn app_owner_stop_authority(owner: MacosDaemonOwner) -> StopAuthority {
    StopAuthority::for_identity(&service_identity(owner))
}
