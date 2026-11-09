//! The app's part in an update of its own bundle.
//!
//! macOS only: an update swaps the bundle this process runs from, so the app
//! answers a control socket that tells it to leave, and an app that starts
//! while the update is open, or from a translocated path, leaves every
//! daemon alone and says why in its window.
//!
//! The official build also finishes its own installation here. The DMG only
//! puts a bundle in Applications, so a build with the `managed-update`
//! feature adopts that bundle at launch, before it supervises anything. Every
//! other build never reaches the updater's paths.

use hypercolor_macos_owner::MacosAppStartupHold;
use tauri::State;
use url::Url;

use crate::supervisor::SupervisorState;

/// Tauri event carrying the current startup hold to the bundled UI.
pub const MACOS_APP_STARTUP_HOLD_EVENT: &str = "macos-app-startup-hold-changed";

/// Why this app holds instead of running a daemon, for the bundled UI.
#[tauri::command]
#[must_use]
pub fn macos_app_startup_hold(state: State<'_, SupervisorState>) -> Option<MacosAppStartupHold> {
    state.macos_startup_hold()
}

/// Whether macOS translocated this copy of the app.
#[must_use]
pub fn launched_translocated() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::env::current_exe()
            .is_ok_and(|executable| hypercolor_macos_owner::is_macos_translocated_path(&executable))
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Off macOS there is no bundle to swap, and so no socket to serve.
#[cfg(not(target_os = "macos"))]
pub const fn serve_app_control<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) {}

#[cfg(target_os = "macos")]
pub use macos::{hold_startup, serve_app_control};

#[cfg(all(target_os = "macos", feature = "managed-update"))]
pub use adoption::{PendingAdoption, prepare_adoption};

/// A build that manages no installation has nothing to adopt.
#[cfg(not(all(target_os = "macos", feature = "managed-update")))]
pub enum PendingAdoption {}

/// A build that manages no installation has nothing to adopt.
#[cfg(not(all(target_os = "macos", feature = "managed-update")))]
#[must_use]
pub const fn prepare_adoption() -> Option<PendingAdoption> {
    None
}

/// Start supervising the daemon, once any adoption this launch prepared has
/// finished. A failed adoption leaves an app that supervises its own daemon,
/// and the next launch tries again.
///
/// # Errors
/// Returns an error when supervision cannot be started right away.
pub fn start_supervision<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    daemon_url: Url,
    adoption: Option<PendingAdoption>,
) -> anyhow::Result<()> {
    let Some(adoption) = adoption else {
        return crate::supervisor::start(app, daemon_url);
    };
    #[cfg(all(target_os = "macos", feature = "managed-update"))]
    {
        // Publishing a bundle hashes and copies all of it, which the
        // window's thread must not wait for.
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            match adoption.run(&app, &daemon_url) {
                Ok(outcome) => tracing::info!(?outcome, "this bundle's installation is settled"),
                Err(error) => tracing::warn!(%error, "this bundle stays unmanaged for now"),
            }
            if let Err(error) = crate::supervisor::start(&app, daemon_url) {
                tracing::warn!(%error, "failed to start after adoption");
            }
        });
        Ok(())
    }
    #[cfg(not(all(target_os = "macos", feature = "managed-update")))]
    match adoption {}
}

#[cfg(target_os = "macos")]
mod macos {
    use hypercolor_macos_owner::{
        MacosAppControlServer, MacosAppStartupHold, MacosRunningApp, macos_app_bundle_of,
        macos_app_startup_hold, macos_user_update_state_root, wait_for_macos_update_end,
    };
    use tauri::{AppHandle, Emitter as _, Manager as _, Runtime};
    use url::Url;

    use super::MACOS_APP_STARTUP_HOLD_EVENT;
    use crate::supervisor::SupervisorState;

    /// Serve the update control socket for the life of this app.
    ///
    /// A build with no managed installation has no update state root and
    /// serves nothing. Neither does an app that does not run from a bundle.
    pub fn serve_app_control<R: Runtime>(app: &AppHandle<R>) {
        let Some(root) = macos_user_update_state_root().filter(|root| root.is_dir()) else {
            return;
        };
        let bundle = std::env::current_exe()
            .ok()
            .and_then(|executable| macos_app_bundle_of(&executable));
        let Some(bundle) = bundle else {
            return;
        };
        let running = MacosRunningApp {
            pid: std::process::id(),
            bundle,
        };
        let server = match MacosAppControlServer::bind(&root) {
            Ok(server) => server,
            Err(error) => {
                tracing::warn!(%error, "failed to bind the update control socket");
                return;
            }
        };
        let app = app.clone();
        let spawned = std::thread::Builder::new()
            .name("hypercolor-update-control".to_owned())
            .spawn(move || match server.serve_until_quit(&running) {
                Ok(()) => {
                    tracing::info!("quitting for an update of this installation");
                    app.exit(0);
                }
                Err(error) => tracing::warn!(%error, "update control socket stopped"),
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "failed to start the update control thread");
        }
    }

    /// Hold this startup if it must leave every daemon alone, and report
    /// whether it did. The caller's supervision then runs only once the
    /// update ends, through [`crate::supervisor::start`]; a translocated app
    /// holds until it exits.
    pub fn hold_startup<R: Runtime>(app: &AppHandle<R>, daemon_url: &Url) -> bool {
        let root = macos_user_update_state_root();
        let hold = std::env::current_exe()
            .ok()
            .and_then(|executable| macos_app_startup_hold(&executable, root.as_deref()));
        let Some(hold) = hold else {
            publish(app, None);
            return false;
        };
        tracing::info!(?hold, "holding daemon startup");
        publish(app, Some(hold));
        if let (MacosAppStartupHold::UpdateInProgress, Some(root)) = (hold, root) {
            let app = app.clone();
            let daemon_url = daemon_url.clone();
            tauri::async_runtime::spawn_blocking(move || match wait_for_macos_update_end(&root) {
                Ok(()) => {
                    if let Err(error) = crate::supervisor::start(&app, daemon_url) {
                        tracing::warn!(%error, "failed to start after the update ended");
                    }
                }
                Err(error) => tracing::warn!(%error, "lost track of the update"),
            });
        }
        true
    }

    fn publish<R: Runtime>(app: &AppHandle<R>, hold: Option<MacosAppStartupHold>) {
        let state = app.state::<SupervisorState>();
        if state.macos_startup_hold() == hold {
            return;
        }
        state.set_macos_startup_hold(hold);
        if let Err(error) = app.emit(MACOS_APP_STARTUP_HOLD_EVENT, hold) {
            tracing::warn!(%error, "failed to publish the startup hold");
        }
    }
}

#[cfg(all(target_os = "macos", feature = "managed-update"))]
mod adoption {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use anyhow::{Context as _, bail};
    use hypercolor_core::config::paths::{data_dir, macos_launch_agents_dir, macos_user_log_dir};
    use hypercolor_install::macos_bundle::{
        MACOS_BUNDLE_DAEMON_LABEL, MacosBundleAdoptionHost, MacosBundleAdoptionOutcome,
        MacosBundleAdoptionRequest, MacosBundleConfig, MacosBundleLaunchdService,
        MacosBundleNativeAgent, MacosBundleNativeExecutor, MacosBundleRoots, adopt_macos_bundle,
        macos_bundle_in_applications,
    };
    use hypercolor_install::{
        InstallLock, InstallPlatformError, InstallStore, InstallStoreError,
        MAX_MANAGED_INSTALL_JOURNAL_BYTES,
    };
    use hypercolor_macos_owner::{
        LaunchdAdapter, MacosExternalOwnerMode, MacosOwnerStore, macos_app_startup_hold,
        macos_user_update_state_root,
    };
    use tauri::{AppHandle, Runtime};
    use url::Url;

    use crate::supervisor::{DEFAULT_DAEMON_PORT, DaemonListen, daemon_listen_from_url};

    /// What Homebrew keeps of a cask it installed. A cask moves the bundle
    /// into Applications, so nothing else says who put it there.
    const CASK_RECEIPTS: [&str; 2] = [
        "/opt/homebrew/Caskroom/hypercolor-app",
        "/usr/local/Caskroom/hypercolor-app",
    ];

    /// One launch's claim on its bundle, with the install lock held.
    pub struct PendingAdoption {
        store: InstallStore,
        lock: InstallLock,
        installed_bundle: PathBuf,
    }

    /// Claim this launch's adoption, if this bundle is one to adopt, and make
    /// sure the update state root exists before the control socket binds in
    /// it. Takes no time worth a thread: everything slow happens in
    /// [`PendingAdoption::run`].
    #[must_use]
    pub fn prepare_adoption() -> Option<PendingAdoption> {
        let executable = std::env::current_exe().ok()?;
        let state_root = macos_user_update_state_root()?;
        if macos_app_startup_hold(&executable, Some(&state_root)).is_some() {
            return None;
        }
        let launch_agents = macos_launch_agents_dir()?;
        let home = launch_agents.parent()?.parent()?;
        let installed_bundle = macos_bundle_in_applications(&executable, home)?;
        if homebrew_owns_this_installation() {
            return None;
        }
        let roots = MacosBundleRoots::of_state_root(&state_root)?;
        let support = state_root.parent()?.parent()?;
        let store = InstallStore::with_roots(
            roots.release_root,
            roots.state_root,
            MAX_MANAGED_INSTALL_JOURNAL_BYTES,
        )
        .inspect_err(|error| tracing::warn!(%error, "the installation's roots are unusable"))
        .ok()?;
        let lock = match store.acquire_anchored_lock(support) {
            Ok(lock) => lock,
            Err(InstallStoreError::LockContended) => return None,
            Err(error) => {
                tracing::warn!(%error, "failed to take the install lock");
                return None;
            }
        };
        Some(PendingAdoption {
            store,
            lock,
            installed_bundle,
        })
    }

    fn homebrew_owns_this_installation() -> bool {
        CASK_RECEIPTS
            .iter()
            .any(|receipt| Path::new(receipt).exists())
            || MacosOwnerStore::new(data_dir())
                .load_owner_record()
                .ok()
                .flatten()
                .is_some_and(|record| {
                    record.selected_external_owner == Some(MacosExternalOwnerMode::Homebrew)
                })
    }

    impl PendingAdoption {
        /// Adopt this bundle, or find it already adopted.
        pub(super) fn run<R: Runtime>(
            self,
            app: &AppHandle<R>,
            daemon_url: &Url,
        ) -> anyhow::Result<MacosBundleAdoptionOutcome> {
            let Self {
                store,
                lock,
                installed_bundle,
            } = self;
            // The agent's daemon listens where it always does, so a pinned
            // endpoint could never answer its proof.
            let local_api = match daemon_listen_from_url(daemon_url) {
                DaemonListen::Port(DEFAULT_DAEMON_PORT) => {
                    std::net::SocketAddr::from(([127, 0, 0, 1], DEFAULT_DAEMON_PORT))
                }
                listen => bail!("the daemon endpoint is pinned to {listen:?}"),
            };
            let launch_agents =
                macos_launch_agents_dir().context("failed to resolve the home directory")?;
            let logs = macos_user_log_dir().context("failed to resolve the home directory")?;
            let data_root = data_dir();
            let config = MacosBundleConfig {
                installed_bundle,
                release_root: store.root().to_path_buf(),
                probation: Duration::ZERO,
            };
            let mut executor = MacosBundleNativeExecutor::new(&config)?;
            let agent = MacosBundleNativeAgent::new(
                launch_agents.join(format!("{MACOS_BUNDLE_DAEMON_LABEL}.plist")),
                &data_root,
                local_api,
            )?;
            let mut service = MacosBundleLaunchdService::new(agent, &config);
            let mut host = AppAdoptionHost {
                app: app.clone(),
                launchd: LaunchdAdapter::for_current_user()?,
            };
            let request = MacosBundleAdoptionRequest {
                config,
                version: app.package_info().version.to_string(),
                launch_agents,
                logs,
                data_root,
            };
            adopt_macos_bundle(
                &store,
                &lock,
                &mut executor,
                &mut service,
                &mut host,
                &request,
            )
            .map_err(Into::into)
        }
    }

    struct AppAdoptionHost<R: Runtime> {
        app: AppHandle<R>,
        launchd: LaunchdAdapter,
    }

    impl<R: Runtime> MacosBundleAdoptionHost for AppAdoptionHost<R> {
        fn load_agent(
            &mut self,
            label: &str,
            definition: &Path,
            replace: bool,
        ) -> Result<(), InstallPlatformError> {
            let launchd_error = |error: hypercolor_macos_owner::MacosOwnerExecutionError| {
                InstallPlatformError::new(error.to_string())
            };
            let target = self.launchd.target(label);
            if self.launchd.is_loaded(&target).map_err(launchd_error)? {
                if !replace {
                    return Ok(());
                }
                self.launchd.bootout(&target).map_err(launchd_error)?;
            }
            self.launchd.bootstrap(definition).map_err(launchd_error)
        }

        fn unload_agent(&mut self, label: &str) -> Result<(), InstallPlatformError> {
            self.launchd
                .bootout(&self.launchd.target(label))
                .map_err(|error| InstallPlatformError::new(error.to_string()))
        }

        fn hand_over_daemon(&mut self) -> Result<(), InstallPlatformError> {
            crate::ownership::hand_daemon_to_direct_agent(&self.app)
                .map_err(|error| InstallPlatformError::new(error.to_string()))
        }
    }
}
