//! Local-only macOS daemon ownership coordination.

#[cfg(target_os = "macos")]
use hypercolor_macos_owner::{
    MACOS_APP_PRODUCT_NAME, MacosHandoverPhase, MacosOwnerExecutionError,
};
use hypercolor_macos_owner::{MacosDaemonOwner, MacosOwnerCoordinatorOutcome, MacosOwnerRemedy};
#[cfg(target_os = "macos")]
use tauri::Runtime;
use tauri::{AppHandle, State};

use crate::supervisor::{MacosDaemonOwnerOfflineStatus, SupervisorState};

/// Select one local macOS daemon topology through the durable coordinator.
#[tauri::command]
pub async fn choose_daemon_owner(
    app: AppHandle,
    state: State<'_, SupervisorState>,
    requested_owner: MacosDaemonOwner,
) -> Result<MacosOwnerCoordinatorOutcome, String> {
    #[cfg(target_os = "macos")]
    {
        let state = state.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            choose_daemon_owner_inner(&app, state, requested_owner)
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())?
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state, requested_owner);
        Err("macOS daemon owner selection is unavailable on this platform".to_owned())
    }
}

/// Return app-local external-owner state even when the daemon is offline.
#[tauri::command]
pub fn macos_daemon_owner_offline_status(
    state: State<'_, SupervisorState>,
) -> Option<MacosDaemonOwnerOfflineStatus> {
    state.macos_owner_offline()
}

/// Result of executing an app-local offline-owner remedy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MacosDaemonOwnerRemedyOutcome {
    /// The selected external owner published a newer healthy epoch.
    Started { owner: MacosDaemonOwner },
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingOfflineRemedy {
    status: MacosDaemonOwnerOfflineStatus,
    owner: MacosDaemonOwner,
    after_epoch: u64,
}

/// Owner vocabulary matching `SystemStatus.macos_daemon_ownership.active_owner`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MacosCaptureOwner {
    AppSidecar,
    LaunchdService,
    HomebrewService,
    Standalone,
}

impl From<MacosCaptureOwner> for MacosDaemonOwner {
    fn from(owner: MacosCaptureOwner) -> Self {
        match owner {
            MacosCaptureOwner::AppSidecar => Self::AppSidecar,
            MacosCaptureOwner::LaunchdService => Self::DirectLaunchd,
            MacosCaptureOwner::HomebrewService => Self::Homebrew,
            MacosCaptureOwner::Standalone => Self::Standalone,
        }
    }
}

impl From<MacosDaemonOwner> for MacosCaptureOwner {
    fn from(owner: MacosDaemonOwner) -> Self {
        match owner {
            MacosDaemonOwner::AppSidecar => Self::AppSidecar,
            MacosDaemonOwner::DirectLaunchd => Self::LaunchdService,
            MacosDaemonOwner::Homebrew => Self::HomebrewService,
            MacosDaemonOwner::Standalone => Self::Standalone,
        }
    }
}

/// Result of an explicit local restart of the authoritative capture owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MacosCaptureOwnerRestartOutcome {
    /// A managed owner published a new authoritative epoch after restart.
    Restarted {
        owner: MacosCaptureOwner,
        previous_owner_epoch: u64,
        owner_epoch: u64,
    },
    /// A standalone owner must be stopped by the terminal user.
    UserActionRequired {
        owner: MacosCaptureOwner,
        owner_epoch: u64,
        remedy: MacosOwnerRemedy,
    },
}

/// Execute the exact start remedy published by the current offline-owner status.
#[tauri::command]
pub async fn execute_macos_daemon_owner_offline_remedy(
    app: AppHandle,
    state: State<'_, SupervisorState>,
    remedy: MacosOwnerRemedy,
) -> Result<MacosDaemonOwnerRemedyOutcome, String> {
    #[cfg(target_os = "macos")]
    {
        let state = state.inner().clone();
        let start_state = state.clone();
        let (pending, daemon_url) = tauri::async_runtime::spawn_blocking(move || {
            execute_offline_remedy_inner(&app, start_state, remedy)
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())??;
        let converged = crate::supervisor::wait_for_authoritative_macos_owner(
            &reqwest::Client::new(),
            &daemon_url,
            pending.owner,
            Some(pending.after_epoch),
            hypercolor_macos_owner::MACOS_MANAGED_HANDOVER_TIMEOUT,
        )
        .await;
        complete_offline_remedy_with(&state, pending, converged).map_err(|error| error.to_string())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state, remedy);
        Err("macOS daemon owner remedies are unavailable on this platform".to_owned())
    }
}

/// Restart the exact authoritative owner named by a protected-source status.
#[tauri::command]
pub async fn restart_macos_capture_owner(
    app: AppHandle,
    state: State<'_, SupervisorState>,
    active_owner: MacosCaptureOwner,
    owner_epoch: u64,
) -> Result<MacosCaptureOwnerRestartOutcome, String> {
    #[cfg(target_os = "macos")]
    {
        let state = state.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            restart_capture_owner_inner(&app, state, active_owner, owner_epoch)
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())?
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state, active_owner, owner_epoch);
        Err("macOS capture-owner restart is unavailable on this platform".to_owned())
    }
}

#[cfg(target_os = "macos")]
fn execute_offline_remedy_inner<R: Runtime>(
    app: &AppHandle<R>,
    state: SupervisorState,
    remedy: MacosOwnerRemedy,
) -> Result<(PendingOfflineRemedy, url::Url), anyhow::Error> {
    use hypercolor_core::config::paths::data_dir;
    use hypercolor_macos_owner::{MacosOwnerExecutor as _, MacosOwnerStore};

    let store = MacosOwnerStore::new(data_dir());
    let daemon_url: url::Url = std::env::var("HYPERCOLOR_URL")
        .unwrap_or_else(|_| crate::DEFAULT_DAEMON_URL.to_owned())
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid HYPERCOLOR_URL: {error}"))?;
    let after_epoch = store
        .load_owner_record()?
        .ok_or_else(|| anyhow::anyhow!("macOS daemon owner record is unavailable"))?
        .owner_epoch;
    let mut executor =
        AppOwnerExecutor::new(app.clone(), state.clone(), daemon_url.clone(), store)?;
    let pending =
        execute_offline_remedy_with(&state, remedy, after_epoch, |owner| executor.start(owner))?;
    Ok((pending, daemon_url))
}

#[cfg(target_os = "macos")]
fn restart_capture_owner_inner<R: Runtime>(
    app: &AppHandle<R>,
    state: SupervisorState,
    active_owner: MacosCaptureOwner,
    owner_epoch: u64,
) -> Result<MacosCaptureOwnerRestartOutcome, anyhow::Error> {
    use hypercolor_core::config::paths::data_dir;
    use hypercolor_macos_owner::MacosOwnerStore;

    let store = MacosOwnerStore::new(data_dir());
    let daemon_url = std::env::var("HYPERCOLOR_URL")
        .unwrap_or_else(|_| crate::DEFAULT_DAEMON_URL.to_owned())
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid HYPERCOLOR_URL: {error}"))?;
    let active_owner = MacosDaemonOwner::from(active_owner);
    let mut executor =
        AppOwnerExecutor::new(app.clone(), state.clone(), daemon_url, store.clone())?;
    restart_capture_owner_with(&store, &mut executor, active_owner, owner_epoch)
}

#[cfg(target_os = "macos")]
fn restart_capture_owner_with(
    store: &hypercolor_macos_owner::MacosOwnerStore,
    executor: &mut impl hypercolor_macos_owner::MacosOwnerExecutor,
    active_owner: MacosDaemonOwner,
    owner_epoch: u64,
) -> Result<MacosCaptureOwnerRestartOutcome, anyhow::Error> {
    let record = store
        .load_owner_record()?
        .ok_or_else(|| anyhow::anyhow!("macOS daemon owner record is unavailable"))?;
    if store
        .load_handover_journal()?
        .is_some_and(|journal| !journal.phase.is_terminal())
    {
        anyhow::bail!("macOS daemon owner handover requires recovery before capture-owner restart");
    }
    if record.active_owner != active_owner || record.owner_epoch != owner_epoch {
        anyhow::bail!(
            "macOS capture-owner status is stale: requested {active_owner:?} epoch {owner_epoch}, authoritative {:?} epoch {}",
            record.active_owner,
            record.owner_epoch
        );
    }
    if active_owner == MacosDaemonOwner::Standalone {
        return Ok(MacosCaptureOwnerRestartOutcome::UserActionRequired {
            owner: active_owner.into(),
            owner_epoch,
            remedy: MacosOwnerRemedy::RestartStandalone {
                pid: record.active_identity.pid,
            },
        });
    }
    let incarnation = record.incarnation();
    store
        .request_stop_if_current(&incarnation, || executor.flush_and_stop(&incarnation))
        .map_err(anyhow::Error::from)?;
    let restart = (|| {
        if !executor
            .wait_for_guard_release(hypercolor_macos_owner::MACOS_MANAGED_HANDOVER_TIMEOUT)
            .map_err(anyhow::Error::from)?
        {
            anyhow::bail!(
                "macOS capture owner did not release the daemon guard within ten seconds"
            );
        }
        executor.start(active_owner).map_err(anyhow::Error::from)?;
        if !executor
            .wait_for_owner(
                active_owner,
                owner_epoch,
                hypercolor_macos_owner::MACOS_MANAGED_HANDOVER_TIMEOUT,
            )
            .map_err(anyhow::Error::from)?
        {
            anyhow::bail!("restarted macOS capture owner did not publish within ten seconds");
        }
        let restarted = store
            .load_owner_record()?
            .filter(|record| {
                record.active_owner == active_owner && record.owner_epoch > owner_epoch
            })
            .ok_or_else(|| {
                anyhow::anyhow!("restarted macOS capture owner did not publish a new epoch")
            })?;
        Ok(MacosCaptureOwnerRestartOutcome::Restarted {
            owner: active_owner.into(),
            previous_owner_epoch: owner_epoch,
            owner_epoch: restarted.owner_epoch,
        })
    })();
    if restart.is_err() && active_owner == MacosDaemonOwner::AppSidecar {
        executor.start(active_owner).map_err(|rearm_error| {
            anyhow::anyhow!("failed to rearm the app-sidecar supervisor: {rearm_error}")
        })?;
    }
    restart
}

#[cfg(target_os = "macos")]
fn execute_offline_remedy_with(
    state: &SupervisorState,
    remedy: MacosOwnerRemedy,
    after_epoch: u64,
    start_owner: impl FnOnce(MacosDaemonOwner) -> Result<(), MacosOwnerExecutionError>,
) -> Result<PendingOfflineRemedy, MacosOwnerExecutionError> {
    let status = state.macos_owner_offline().ok_or_else(|| {
        MacosOwnerExecutionError::new("no selected macOS daemon owner is currently offline")
    })?;
    if status.remedy != remedy {
        return Err(MacosOwnerExecutionError::new(
            "offline-owner remedy is stale or does not match the selected topology",
        ));
    }
    let owner = match remedy {
        MacosOwnerRemedy::StartLaunchdService => MacosDaemonOwner::DirectLaunchd,
        MacosOwnerRemedy::StartHomebrewService => MacosDaemonOwner::Homebrew,
        MacosOwnerRemedy::StartAppSidecar
        | MacosOwnerRemedy::RestartStandalone { .. }
        | MacosOwnerRemedy::StopStandaloneOwner { .. } => {
            return Err(MacosOwnerExecutionError::new(
                "offline-owner status only permits an external service start",
            ));
        }
    };
    if owner != status.selected_owner {
        return Err(MacosOwnerExecutionError::new(
            "offline-owner remedy does not match the selected owner",
        ));
    }
    start_owner(owner)?;
    Ok(PendingOfflineRemedy {
        status,
        owner,
        after_epoch,
    })
}

#[cfg(target_os = "macos")]
fn complete_offline_remedy_with(
    state: &SupervisorState,
    pending: PendingOfflineRemedy,
    authoritative_owner_converged: bool,
) -> Result<MacosDaemonOwnerRemedyOutcome, MacosOwnerExecutionError> {
    if !authoritative_owner_converged {
        return Err(MacosOwnerExecutionError::new(
            "selected macOS daemon owner did not publish a newer healthy epoch within ten seconds",
        ));
    }
    if !state.clear_macos_owner_offline_if(pending.status) {
        return Err(MacosOwnerExecutionError::new(
            "offline-owner status changed while the selected owner was starting",
        ));
    }
    Ok(MacosDaemonOwnerRemedyOutcome::Started {
        owner: pending.owner,
    })
}

#[cfg(target_os = "macos")]
fn choose_daemon_owner_inner(
    app: &AppHandle,
    state: SupervisorState,
    requested_owner: MacosDaemonOwner,
) -> Result<MacosOwnerCoordinatorOutcome, anyhow::Error> {
    use hypercolor_core::config::paths::data_dir;
    use hypercolor_macos_owner::{
        MacosHandoverTransactionId, MacosOwnerStore, choose_daemon_owner,
    };

    let store = MacosOwnerStore::new(data_dir());
    let daemon_url = std::env::var("HYPERCOLOR_URL")
        .unwrap_or_else(|_| crate::DEFAULT_DAEMON_URL.to_owned())
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid HYPERCOLOR_URL: {error}"))?;
    let mut executor =
        AppOwnerExecutor::new(app.clone(), state.clone(), daemon_url, store.clone())?;
    let transaction_id = MacosHandoverTransactionId::new(format!(
        "owner-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ))?;
    let outcome = choose_daemon_owner(&store, &mut executor, requested_owner, transaction_id)
        .map_err(anyhow::Error::from)?;
    apply_owner_choice_outcome(&state, &outcome);
    Ok(outcome)
}

#[cfg(target_os = "macos")]
fn apply_owner_choice_outcome(state: &SupervisorState, outcome: &MacosOwnerCoordinatorOutcome) {
    match outcome {
        MacosOwnerCoordinatorOutcome::Active { owner, .. } => {
            state.set_macos_external_owner(match owner {
                MacosDaemonOwner::DirectLaunchd => {
                    Some(hypercolor_macos_owner::MacosExternalOwnerMode::DirectLaunchd)
                }
                MacosDaemonOwner::Homebrew => {
                    Some(hypercolor_macos_owner::MacosExternalOwnerMode::Homebrew)
                }
                MacosDaemonOwner::AppSidecar | MacosDaemonOwner::Standalone => None,
            });
            state.set_macos_owner_offline(None);
        }
        MacosOwnerCoordinatorOutcome::RolledBack {
            prior_owner: MacosDaemonOwner::AppSidecar,
            ..
        } => release_app_sidecar_supervisor(state),
        MacosOwnerCoordinatorOutcome::PendingStandalone { .. }
        | MacosOwnerCoordinatorOutcome::RolledBack { .. }
        | MacosOwnerCoordinatorOutcome::RecoveryRequired { .. } => {}
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MacosStartupRecoveryDisposition {
    Continue,
    SupervisorStarted,
    SuppressSupervisor,
}

#[cfg(target_os = "macos")]
pub(crate) fn recover_daemon_owner_before_supervisor<R: Runtime>(
    app: &AppHandle<R>,
    state: SupervisorState,
    daemon_url: url::Url,
    store: hypercolor_macos_owner::MacosOwnerStore,
) -> Result<MacosStartupRecoveryDisposition, anyhow::Error> {
    let mut executor = AppOwnerExecutor::new(app.clone(), state, daemon_url, store.clone())?;
    if store.load_handover_journal()?.is_some_and(|journal| {
        app_sidecar_recovery_needs_rearm(journal.requested_owner, journal.phase)
    }) {
        hypercolor_macos_owner::MacosOwnerExecutor::start(
            &mut executor,
            MacosDaemonOwner::AppSidecar,
        )?;
    }
    let outcome = hypercolor_macos_owner::recover_daemon_owner(&store, &mut executor)?;
    Ok(startup_recovery_disposition(
        outcome.as_ref(),
        executor.app_sidecar_supervisor_started,
    ))
}

#[cfg(target_os = "macos")]
const fn app_sidecar_recovery_needs_rearm(
    requested_owner: MacosDaemonOwner,
    phase: MacosHandoverPhase,
) -> bool {
    matches!(
        (requested_owner, phase),
        (
            MacosDaemonOwner::AppSidecar,
            MacosHandoverPhase::RequestedOwnerStarted
        )
    )
}

#[cfg(target_os = "macos")]
fn startup_recovery_disposition(
    outcome: Option<&MacosOwnerCoordinatorOutcome>,
    app_sidecar_supervisor_started: bool,
) -> MacosStartupRecoveryDisposition {
    if app_sidecar_supervisor_started {
        MacosStartupRecoveryDisposition::SupervisorStarted
    } else if matches!(
        outcome,
        Some(MacosOwnerCoordinatorOutcome::PendingStandalone { .. })
    ) {
        MacosStartupRecoveryDisposition::SuppressSupervisor
    } else {
        MacosStartupRecoveryDisposition::Continue
    }
}

#[cfg(target_os = "macos")]
struct AppOwnerExecutor<R: Runtime> {
    app: AppHandle<R>,
    state: SupervisorState,
    daemon_url: url::Url,
    store: hypercolor_macos_owner::MacosOwnerStore,
    uid: String,
    launch_agents: std::path::PathBuf,
    app_sidecar_supervisor_started: bool,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
enum AppOwnerStopAuthority {
    SupervisorChild,
    LaunchctlService(String),
    HomebrewService(&'static str),
    UserDirected,
}

#[cfg(target_os = "macos")]
impl<R: Runtime> AppOwnerExecutor<R> {
    fn new(
        app: AppHandle<R>,
        state: SupervisorState,
        daemon_url: url::Url,
        store: hypercolor_macos_owner::MacosOwnerStore,
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

    fn service_target(&self, owner: MacosDaemonOwner) -> Result<String, MacosOwnerExecutionError> {
        Ok(format!("gui/{}/{}", self.uid, service_label(owner)?))
    }

    fn service_plist(
        &self,
        owner: MacosDaemonOwner,
    ) -> Result<std::path::PathBuf, MacosOwnerExecutionError> {
        Ok(self
            .launch_agents
            .join(format!("{}.plist", service_label(owner)?)))
    }

    fn service_autostart_enabled(
        &self,
        owner: MacosDaemonOwner,
    ) -> Result<bool, MacosOwnerExecutionError> {
        let plist = self.service_plist(owner)?;
        if !plist.is_file() {
            return Ok(false);
        }
        let output = command_output(
            "/bin/launchctl",
            &["print-disabled", &format!("gui/{}", self.uid)],
        )?;
        if !output.status.success() {
            return Err(MacosOwnerExecutionError::new(
                "launchctl failed to inspect service autostart state",
            ));
        }
        Ok(!launchctl_service_disabled(
            &String::from_utf8_lossy(&output.stdout),
            service_label(owner)?,
        ))
    }

    fn stop_authority(
        &self,
        owner: MacosDaemonOwner,
    ) -> Result<AppOwnerStopAuthority, MacosOwnerExecutionError> {
        app_owner_stop_authority(owner, &self.uid)
    }
}

#[cfg(target_os = "macos")]
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
                self.service_autostart_enabled(owner)
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
                run_command("/bin/launchctl", &[action, &self.service_target(owner)?])
            }
            MacosDaemonOwner::Standalone => Err(MacosOwnerExecutionError::new(
                "standalone has no autostart state",
            )),
        }
    }

    fn preflight_stop_authority(
        &mut self,
        incarnation: &hypercolor_macos_owner::MacosOwnerIncarnation,
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
        incarnation: &hypercolor_macos_owner::MacosOwnerIncarnation,
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
            AppOwnerStopAuthority::HomebrewService(formula) => {
                let brew = homebrew_binary()?;
                run_command(&brew.to_string_lossy(), &["services", "stop", formula])
            }
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
                let target = self.service_target(owner)?;
                if command_output("/bin/launchctl", &["print", &target])?
                    .status
                    .success()
                {
                    run_command("/bin/launchctl", &["kickstart", &target])
                } else {
                    let plist = self.service_plist(owner)?;
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
            MacosDaemonOwner::Homebrew => {
                let brew = homebrew_binary()?;
                run_command(
                    &brew.to_string_lossy(),
                    &["services", "start", "hypercolor"],
                )
            }
            MacosDaemonOwner::Standalone => Err(MacosOwnerExecutionError::new(
                "standalone cannot be started by the app coordinator",
            )),
        }
    }

    fn wait_for_guard_release(
        &mut self,
        timeout: std::time::Duration,
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
        timeout: std::time::Duration,
    ) -> Result<bool, MacosOwnerExecutionError> {
        hypercolor_macos_owner::wait_for_owner_publication(&self.store, owner, after_epoch, timeout)
    }
}

#[cfg(target_os = "macos")]
fn update_app_sidecar_gate_for_autostart(state: &SupervisorState, enabled: bool) {
    if !enabled {
        hold_app_sidecar_supervisor(state);
    }
}

#[cfg(target_os = "macos")]
fn hold_app_sidecar_supervisor(state: &SupervisorState) {
    state.set_owner_handover_stop(true);
}

#[cfg(target_os = "macos")]
fn release_app_sidecar_supervisor(state: &SupervisorState) {
    state.set_owner_handover_stop(false);
}

#[cfg(target_os = "macos")]
fn app_owner_stop_authority(
    owner: MacosDaemonOwner,
    uid: &str,
) -> Result<AppOwnerStopAuthority, MacosOwnerExecutionError> {
    Ok(match owner {
        MacosDaemonOwner::AppSidecar => AppOwnerStopAuthority::SupervisorChild,
        MacosDaemonOwner::DirectLaunchd => {
            AppOwnerStopAuthority::LaunchctlService(format!("gui/{uid}/{}", service_label(owner)?))
        }
        MacosDaemonOwner::Homebrew => AppOwnerStopAuthority::HomebrewService("hypercolor"),
        MacosDaemonOwner::Standalone => AppOwnerStopAuthority::UserDirected,
    })
}

#[cfg(target_os = "macos")]
fn service_label(owner: MacosDaemonOwner) -> Result<&'static str, MacosOwnerExecutionError> {
    match owner {
        MacosDaemonOwner::AppSidecar => Ok(MACOS_APP_PRODUCT_NAME),
        MacosDaemonOwner::DirectLaunchd => Ok("tech.hyperbliss.hypercolor"),
        MacosDaemonOwner::Homebrew => Ok("homebrew.mxcl.hypercolor"),
        MacosDaemonOwner::Standalone => Err(MacosOwnerExecutionError::new(
            "owner does not use a service label",
        )),
    }
}

#[cfg(target_os = "macos")]
fn launchctl_service_disabled(output: &str, label: &str) -> bool {
    output.lines().any(|line| {
        let line = line.trim();
        line.contains(&format!("\"{label}\"")) && line.ends_with("=> true")
    })
}

#[cfg(target_os = "macos")]
fn homebrew_binary() -> Result<std::path::PathBuf, MacosOwnerExecutionError> {
    ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
        .into_iter()
        .map(std::path::PathBuf::from)
        .find(|path| path.is_file())
        .ok_or_else(|| MacosOwnerExecutionError::new("Homebrew executable is unavailable"))
}

#[cfg(target_os = "macos")]
fn command_stdout(program: &str, args: &[&str]) -> Result<String, anyhow::Error> {
    let output = std::process::Command::new(program).args(args).output()?;
    if !output.status.success() {
        anyhow::bail!("{program} failed with {}", output.status);
    }
    if output.stdout.len() > 64 * 1024 {
        anyhow::bail!("{program} output exceeds 64 KiB");
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

#[cfg(target_os = "macos")]
fn command_output(
    program: &str,
    args: &[&str],
) -> Result<std::process::Output, MacosOwnerExecutionError> {
    std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))
}

#[cfg(target_os = "macos")]
fn run_command(program: &str, args: &[&str]) -> Result<(), MacosOwnerExecutionError> {
    let output = command_output(program, args)?;
    if output.status.success() {
        Ok(())
    } else {
        let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        stderr.truncate(4_096);
        Err(MacosOwnerExecutionError::new(format!(
            "{program} failed with {}: {}",
            output.status,
            stderr.trim()
        )))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::time::Duration;

    use super::{
        AppOwnerStopAuthority, MacosCaptureOwner, MacosCaptureOwnerRestartOutcome,
        MacosDaemonOwnerRemedyOutcome, MacosStartupRecoveryDisposition, app_owner_stop_authority,
        app_sidecar_recovery_needs_rearm, apply_owner_choice_outcome, complete_offline_remedy_with,
        execute_offline_remedy_with, hold_app_sidecar_supervisor, launchctl_service_disabled,
        release_app_sidecar_supervisor, restart_capture_owner_with, service_label,
        startup_recovery_disposition, update_app_sidecar_gate_for_autostart,
    };
    use hypercolor_macos_owner::{
        MacosDaemonOwner, MacosHandoverOperation, MacosHandoverPhase, MacosOwnerCoordinatorOutcome,
        MacosOwnerExecutionError, MacosOwnerExecutor, MacosOwnerIdentity, MacosOwnerIncarnation,
        MacosOwnerRemedy, MacosOwnerStore,
    };

    use crate::supervisor::{MacosDaemonOwnerOfflineStatus, SupervisorState};

    struct RestartFixtureExecutor {
        store: MacosOwnerStore,
        operations: Vec<MacosHandoverOperation>,
        stopped_incarnations: Vec<MacosOwnerIncarnation>,
        next_pid: u32,
        guard_released: bool,
    }

    impl RestartFixtureExecutor {
        fn new(store: MacosOwnerStore) -> Self {
            Self {
                store,
                operations: Vec::new(),
                stopped_incarnations: Vec::new(),
                next_pid: 1_000,
                guard_released: true,
            }
        }
    }

    impl MacosOwnerExecutor for RestartFixtureExecutor {
        fn autostart_enabled(
            &mut self,
            _owner: MacosDaemonOwner,
        ) -> Result<bool, MacosOwnerExecutionError> {
            Ok(false)
        }

        fn set_autostart(
            &mut self,
            _owner: MacosDaemonOwner,
            _enabled: bool,
        ) -> Result<(), MacosOwnerExecutionError> {
            Err(MacosOwnerExecutionError::new(
                "restart must not mutate autostart",
            ))
        }

        fn preflight_stop_authority(
            &mut self,
            _incarnation: &MacosOwnerIncarnation,
        ) -> Result<(), MacosOwnerExecutionError> {
            Ok(())
        }

        fn flush_and_stop(
            &mut self,
            incarnation: &MacosOwnerIncarnation,
        ) -> Result<(), MacosOwnerExecutionError> {
            self.stopped_incarnations.push(incarnation.clone());
            self.operations.push(match incarnation.owner {
                MacosDaemonOwner::AppSidecar => MacosHandoverOperation::FlushAndStopAppSidecar {},
                MacosDaemonOwner::DirectLaunchd => {
                    MacosHandoverOperation::FlushAndStopDirectLaunchd {}
                }
                MacosDaemonOwner::Homebrew => MacosHandoverOperation::FlushAndStopHomebrew {},
                MacosDaemonOwner::Standalone => {
                    return Err(MacosOwnerExecutionError::new(
                        "standalone restart must remain user-directed",
                    ));
                }
            });
            Ok(())
        }

        fn start(&mut self, owner: MacosDaemonOwner) -> Result<(), MacosOwnerExecutionError> {
            self.operations.push(match owner {
                MacosDaemonOwner::AppSidecar => MacosHandoverOperation::StartAppSidecar {},
                MacosDaemonOwner::DirectLaunchd => MacosHandoverOperation::StartDirectLaunchd {},
                MacosDaemonOwner::Homebrew => MacosHandoverOperation::StartHomebrew {},
                MacosDaemonOwner::Standalone => {
                    return Err(MacosOwnerExecutionError::new(
                        "standalone restart must remain user-directed",
                    ));
                }
            });
            self.next_pid += 1;
            self.store
                .publish_owner(
                    owner,
                    MacosOwnerIdentity::new(
                        "restart-audit",
                        "/fixture/hypercolor-daemon",
                        "restart-requirement",
                        self.next_pid,
                    )
                    .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?,
                )
                .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
            Ok(())
        }

        fn wait_for_guard_release(
            &mut self,
            _timeout: Duration,
        ) -> Result<bool, MacosOwnerExecutionError> {
            Ok(self.guard_released)
        }

        fn wait_for_owner(
            &mut self,
            owner: MacosDaemonOwner,
            after_epoch: u64,
            _timeout: Duration,
        ) -> Result<bool, MacosOwnerExecutionError> {
            Ok(self.store.load_owner_record().is_ok_and(|record| {
                record.is_some_and(|record| {
                    record.active_owner == owner && record.owner_epoch > after_epoch
                })
            }))
        }
    }

    fn restart_identity(pid: u32) -> MacosOwnerIdentity {
        MacosOwnerIdentity::new(
            "active-audit",
            "/fixture/active-hypercolor-daemon",
            "active-requirement",
            pid,
        )
        .expect("fixture identity should build")
    }

    #[test]
    fn disabled_service_parser_is_exact_to_the_requested_label() {
        let output = r#"disabled services = {
            "tech.hyperbliss.hypercolor" => true
            "homebrew.mxcl.hypercolor" => false
        }"#;
        assert!(launchctl_service_disabled(
            output,
            "tech.hyperbliss.hypercolor"
        ));
        assert!(!launchctl_service_disabled(
            output,
            "homebrew.mxcl.hypercolor"
        ));
    }

    #[test]
    fn app_sidecar_service_label_matches_tauri_product_name() {
        assert_eq!(
            service_label(MacosDaemonOwner::AppSidecar).expect("app label should resolve"),
            hypercolor_macos_owner::MACOS_APP_PRODUCT_NAME
        );
    }

    #[test]
    fn app_sidecar_gate_stays_held_until_explicit_start() {
        let state = SupervisorState::default();
        update_app_sidecar_gate_for_autostart(&state, true);
        assert!(!state.owner_handover_stop());

        hold_app_sidecar_supervisor(&state);
        update_app_sidecar_gate_for_autostart(&state, true);
        assert!(state.owner_handover_stop());

        release_app_sidecar_supervisor(&state);
        assert!(!state.owner_handover_stop());
        update_app_sidecar_gate_for_autostart(&state, false);
        assert!(state.owner_handover_stop());
    }

    #[test]
    fn requested_app_sidecar_recovery_rearms_the_supervisor() {
        assert!(app_sidecar_recovery_needs_rearm(
            MacosDaemonOwner::AppSidecar,
            MacosHandoverPhase::RequestedOwnerStarted,
        ));
        assert!(!app_sidecar_recovery_needs_rearm(
            MacosDaemonOwner::AppSidecar,
            MacosHandoverPhase::StartRequested,
        ));
        assert!(!app_sidecar_recovery_needs_rearm(
            MacosDaemonOwner::DirectLaunchd,
            MacosHandoverPhase::RequestedOwnerStarted,
        ));
    }

    #[test]
    fn newer_prior_app_sidecar_rollback_releases_only_proven_terminal_gate() {
        let state = SupervisorState::default();
        hold_app_sidecar_supervisor(&state);

        apply_owner_choice_outcome(
            &state,
            &MacosOwnerCoordinatorOutcome::RecoveryRequired {
                requested_owner: MacosDaemonOwner::DirectLaunchd,
                prior_owner: MacosDaemonOwner::AppSidecar,
                phase: hypercolor_macos_owner::MacosHandoverPhase::RollbackAutostartsRestored,
            },
        );
        assert!(state.owner_handover_stop());

        apply_owner_choice_outcome(
            &state,
            &MacosOwnerCoordinatorOutcome::RolledBack {
                prior_owner: MacosDaemonOwner::DirectLaunchd,
                failure: "fixture rollback".to_owned(),
            },
        );
        assert!(state.owner_handover_stop());

        apply_owner_choice_outcome(
            &state,
            &MacosOwnerCoordinatorOutcome::RolledBack {
                prior_owner: MacosDaemonOwner::AppSidecar,
                failure: "newer prior publication restored ownership".to_owned(),
            },
        );
        assert!(!state.owner_handover_stop());
    }

    #[test]
    fn managed_topologies_resolve_only_their_exact_stop_authority() {
        assert_eq!(
            app_owner_stop_authority(MacosDaemonOwner::AppSidecar, "501")
                .expect("app sidecar authority should resolve"),
            AppOwnerStopAuthority::SupervisorChild
        );
        assert_eq!(
            app_owner_stop_authority(MacosDaemonOwner::DirectLaunchd, "501")
                .expect("launchd authority should resolve"),
            AppOwnerStopAuthority::LaunchctlService(
                "gui/501/tech.hyperbliss.hypercolor".to_owned()
            )
        );
        assert_eq!(
            app_owner_stop_authority(MacosDaemonOwner::Homebrew, "501")
                .expect("Homebrew authority should resolve"),
            AppOwnerStopAuthority::HomebrewService("hypercolor")
        );
    }

    #[test]
    fn exact_offline_remedy_clears_only_after_new_healthy_owner_epoch() {
        let state = SupervisorState::default();
        let status = MacosDaemonOwnerOfflineStatus {
            code: "macos_daemon_owner_offline",
            selected_owner: MacosDaemonOwner::DirectLaunchd,
            remedy: MacosOwnerRemedy::StartLaunchdService,
        };
        state.set_macos_owner_offline(Some(status));

        let pending = execute_offline_remedy_with(
            &state,
            MacosOwnerRemedy::StartLaunchdService,
            7,
            |owner| {
                assert_eq!(owner, MacosDaemonOwner::DirectLaunchd);
                Ok(())
            },
        )
        .expect("matching remedy should start");

        assert_eq!(pending.status, status);
        assert_eq!(pending.owner, MacosDaemonOwner::DirectLaunchd);
        assert_eq!(pending.after_epoch, 7);
        assert_eq!(state.macos_owner_offline(), Some(status));
        assert!(complete_offline_remedy_with(&state, pending, false).is_err());
        assert_eq!(state.macos_owner_offline(), Some(status));
        let outcome = complete_offline_remedy_with(&state, pending, true)
            .expect("new authoritative owner epoch should complete the remedy");
        assert_eq!(
            outcome,
            MacosDaemonOwnerRemedyOutcome::Started {
                owner: MacosDaemonOwner::DirectLaunchd
            }
        );
        assert_eq!(state.macos_owner_offline(), None);
    }

    #[test]
    fn failed_or_stale_offline_remedy_preserves_status() {
        let state = SupervisorState::default();
        let status = MacosDaemonOwnerOfflineStatus {
            code: "macos_daemon_owner_offline",
            selected_owner: MacosDaemonOwner::Homebrew,
            remedy: MacosOwnerRemedy::StartHomebrewService,
        };
        state.set_macos_owner_offline(Some(status));

        assert!(
            execute_offline_remedy_with(
                &state,
                MacosOwnerRemedy::StartLaunchdService,
                7,
                |_| panic!("stale remedy must not execute"),
            )
            .is_err()
        );
        assert_eq!(state.macos_owner_offline(), Some(status));
        assert!(
            execute_offline_remedy_with(
                &state,
                MacosOwnerRemedy::StartHomebrewService,
                7,
                |_| Err(MacosOwnerExecutionError::new("injected start failure")),
            )
            .is_err()
        );
        assert_eq!(state.macos_owner_offline(), Some(status));
    }

    #[test]
    fn capture_owner_restart_revalidates_and_publishes_a_new_epoch() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let store = MacosOwnerStore::new(directory.path());
        let record = store
            .publish_owner(MacosDaemonOwner::DirectLaunchd, restart_identity(42))
            .expect("fixture owner should publish");
        let mut executor = RestartFixtureExecutor::new(store.clone());

        let outcome = restart_capture_owner_with(
            &store,
            &mut executor,
            record.active_owner,
            record.owner_epoch,
        )
        .expect("managed owner should restart");

        assert_eq!(
            outcome,
            MacosCaptureOwnerRestartOutcome::Restarted {
                owner: MacosCaptureOwner::LaunchdService,
                previous_owner_epoch: 1,
                owner_epoch: 2,
            }
        );
        assert_eq!(
            executor.operations,
            [
                MacosHandoverOperation::FlushAndStopDirectLaunchd {},
                MacosHandoverOperation::StartDirectLaunchd {},
            ]
        );
        assert_eq!(executor.stopped_incarnations, [record.incarnation()]);
    }

    #[test]
    fn capture_owner_restart_rejects_stale_epoch_and_wrong_owner_without_mutation() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let store = MacosOwnerStore::new(directory.path());
        let record = store
            .publish_owner(MacosDaemonOwner::Homebrew, restart_identity(42))
            .expect("fixture owner should publish");
        let mut executor = RestartFixtureExecutor::new(store.clone());

        assert!(
            restart_capture_owner_with(
                &store,
                &mut executor,
                MacosDaemonOwner::Homebrew,
                record.owner_epoch + 1,
            )
            .is_err()
        );
        assert!(
            restart_capture_owner_with(
                &store,
                &mut executor,
                MacosDaemonOwner::DirectLaunchd,
                record.owner_epoch,
            )
            .is_err()
        );
        assert!(executor.operations.is_empty());
    }

    #[test]
    fn failed_app_sidecar_restart_rearms_the_supervisor_after_stop() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let store = MacosOwnerStore::new(directory.path());
        let record = store
            .publish_owner(MacosDaemonOwner::AppSidecar, restart_identity(42))
            .expect("fixture owner should publish");
        let mut executor = RestartFixtureExecutor::new(store.clone());
        executor.guard_released = false;

        assert!(
            restart_capture_owner_with(
                &store,
                &mut executor,
                record.active_owner,
                record.owner_epoch,
            )
            .is_err()
        );
        assert_eq!(
            executor.operations,
            [
                MacosHandoverOperation::FlushAndStopAppSidecar {},
                MacosHandoverOperation::StartAppSidecar {},
            ]
        );
        assert!(
            store
                .load_owner_record()
                .expect("owner record should load")
                .is_some_and(|current| {
                    current.active_owner == MacosDaemonOwner::AppSidecar
                        && current.owner_epoch > record.owner_epoch
                })
        );
    }

    #[test]
    fn standalone_capture_owner_restart_returns_typed_user_remedy() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let store = MacosOwnerStore::new(directory.path());
        let record = store
            .publish_owner(MacosDaemonOwner::Standalone, restart_identity(77))
            .expect("fixture owner should publish");
        let mut executor = RestartFixtureExecutor::new(store.clone());

        let outcome = restart_capture_owner_with(
            &store,
            &mut executor,
            record.active_owner,
            record.owner_epoch,
        )
        .expect("standalone owner should return a local remedy");

        assert_eq!(
            outcome,
            MacosCaptureOwnerRestartOutcome::UserActionRequired {
                owner: MacosCaptureOwner::Standalone,
                owner_epoch: 1,
                remedy: MacosOwnerRemedy::RestartStandalone { pid: 77 },
            }
        );
        assert_eq!(
            serde_json::to_value(outcome).expect("restart outcome should serialize"),
            serde_json::json!({
                "status": "user_action_required",
                "owner": "standalone",
                "owner_epoch": 1,
                "remedy": {
                    "kind": "restart_standalone",
                    "pid": 77
                }
            })
        );
        assert!(executor.operations.is_empty());
    }

    #[test]
    fn startup_recovery_suppresses_normal_watchdog_until_pending_standalone_exits() {
        let pending = MacosOwnerCoordinatorOutcome::PendingStandalone {
            requested_owner: MacosDaemonOwner::Homebrew,
            remedy: MacosOwnerRemedy::StopStandaloneOwner { pid: 42 },
        };
        assert_eq!(
            startup_recovery_disposition(Some(&pending), false),
            MacosStartupRecoveryDisposition::SuppressSupervisor
        );
        assert_eq!(
            startup_recovery_disposition(None, false),
            MacosStartupRecoveryDisposition::Continue
        );
        assert_eq!(
            startup_recovery_disposition(Some(&pending), true),
            MacosStartupRecoveryDisposition::SupervisorStarted
        );
    }
}
