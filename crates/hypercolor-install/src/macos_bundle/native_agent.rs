//! The macOS calls behind the daemon's launchd agent.

use std::fs::File;
use std::io::Read as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use hypercolor_macos_owner::{
    MACOS_MANAGED_HANDOVER_TIMEOUT, MacosDirectLaunchdBootstrapExpectation,
    MacosDirectLaunchdBootstrapSource, MacosDirectLaunchdExecutableExpectation,
    MacosDirectLaunchdMutationOutcome, MacosDirectLaunchdMutator,
    MacosDirectLaunchdPublicationExpectation, MacosLaunchdJob, MacosOwnerExecutionError,
    MacosOwnerStore, MacosOwnerStoreError, NativeMacosDirectLaunchdInspector,
    NativeMacosDirectLaunchdMutator, canonical_macos_daemon_guard_path,
    wait_for_exact_direct_launchd_publication, wait_for_macos_guard_release,
    wait_for_process_exit_within,
};
use sha2::{Digest as _, Sha256};

use super::super::InstallPlatformError;
use super::launchd::{MacosBundleAgent, MacosBundleAgentJob};
use super::model::{MacosBundleRunningDaemon, error};
use crate::local_http::{MAX_HTTP_RESPONSE_BYTES, http_get, verify_http};

/// Bound on one launchctl mutation, which launchd answers promptly.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// Bound on unloading a daemon, which launchd gives its own exit timeout.
const UNLOAD_TIMEOUT: Duration = Duration::from_mins(3);
const MAX_PROPERTY_LIST_BYTES: u64 = 256 * 1024;

/// The native [`MacosBundleAgent`]: the fixed direct-launchd label, its
/// definition in `~/Library/LaunchAgents`, its owner store and its local API.
pub struct MacosBundleNativeAgent {
    property_list: PathBuf,
    local_api: SocketAddr,
    owner_store: MacosOwnerStore,
    mutator: NativeMacosDirectLaunchdMutator,
    inspector: NativeMacosDirectLaunchdInspector,
}

impl MacosBundleNativeAgent {
    /// An agent defined by `property_list`, whose daemon keeps its owner
    /// records in `owner_data_dir` and serves `local_api`.
    ///
    /// # Errors
    /// Returns an error unless `local_api` is a loopback address.
    pub fn new(
        property_list: PathBuf,
        owner_data_dir: &Path,
        local_api: SocketAddr,
    ) -> Result<Self, InstallPlatformError> {
        if !local_api.ip().is_loopback() {
            return Err(error("the macOS daemon proof must stay on loopback"));
        }
        let owner_store = MacosOwnerStore::new(owner_data_dir);
        Ok(Self {
            property_list,
            local_api,
            mutator: NativeMacosDirectLaunchdMutator::new(owner_store.clone()),
            owner_store,
            inspector: NativeMacosDirectLaunchdInspector::new(),
        })
    }

    /// The agent's definition, opened once and pinned to the bytes launchd
    /// is about to read.
    fn bootstrap_source(&self) -> Result<MacosDirectLaunchdBootstrapSource, InstallPlatformError> {
        let mut file: File = hypercolor_platform_fs::open_no_follow(&self.property_list)
            .map_err(|source| error(format!("open the macOS agent definition: {source}")))?;
        let metadata = file
            .metadata()
            .map_err(|source| error(format!("inspect the macOS agent definition: {source}")))?;
        let mut bytes = Vec::new();
        file.by_ref()
            .take(MAX_PROPERTY_LIST_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| error(format!("read the macOS agent definition: {source}")))?;
        let expectation = {
            use std::os::unix::fs::MetadataExt as _;
            MacosDirectLaunchdBootstrapExpectation::new(
                &self.property_list,
                hex::encode(Sha256::digest(&bytes)),
                metadata.mode() & 0o777,
                metadata.len(),
                metadata.dev(),
                metadata.ino(),
            )
        }
        .map_err(store_error)?;
        Ok(MacosDirectLaunchdBootstrapSource::new(file, expectation))
    }

    /// The epoch of the owner record now on disk, if there is one.
    fn owner_epoch(&self, pid: Option<u32>) -> Result<u64, InstallPlatformError> {
        let record = self.owner_store.load_owner_record().map_err(store_error)?;
        Ok(record.map_or(0, |record| match pid {
            // A publication by this very process counts however old it is.
            Some(pid) if record.active_identity.pid == pid => record.owner_epoch.saturating_sub(1),
            _ => record.owner_epoch,
        }))
    }
}

impl MacosBundleAgent for MacosBundleNativeAgent {
    fn registered(&mut self) -> Result<bool, InstallPlatformError> {
        match std::fs::symlink_metadata(&self.property_list) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(error(format!(
                "inspect the macOS agent definition: {source}"
            ))),
        }
    }

    fn autostart_enabled(&mut self) -> Result<bool, InstallPlatformError> {
        self.mutator.autostart_enabled().map_err(owner_error)
    }

    fn set_autostart(&mut self, enabled: bool) -> Result<(), InstallPlatformError> {
        confirmed(
            self.mutator
                .set_autostart(enabled, COMMAND_TIMEOUT)
                .map_err(owner_error)?,
            "launchd did not confirm the agent's autostart state",
        )
    }

    fn job(&mut self) -> Result<MacosBundleAgentJob, InstallPlatformError> {
        Ok(
            match self
                .inspector
                .inspect_direct_launchd_job()
                .map_err(owner_error)?
            {
                MacosLaunchdJob::NotLoaded => MacosBundleAgentJob::NotLoaded,
                MacosLaunchdJob::Idle => MacosBundleAgentJob::Idle,
                MacosLaunchdJob::Running { pid } => MacosBundleAgentJob::Running { pid },
            },
        )
    }

    fn process_image(
        &mut self,
        pid: u32,
    ) -> Result<Option<(PathBuf, String)>, InstallPlatformError> {
        self.inspector.process_image(pid).map_err(owner_error)
    }

    fn start(
        &mut self,
        executable: &Path,
        cdhash: &str,
        timeout: Duration,
    ) -> Result<(), InstallPlatformError> {
        let expectation = MacosDirectLaunchdPublicationExpectation::new(
            self.owner_epoch(None)?,
            MacosDirectLaunchdExecutableExpectation::bundled(executable, cdhash)
                .map_err(store_error)?,
        )
        .map_err(store_error)?;
        let mut source = self.bootstrap_source()?;
        let outcome = self
            .mutator
            .bootstrap_and_kickstart_exact(&mut source, &expectation, timeout)
            .map_err(owner_error)?;
        confirmed(
            match outcome {
                MacosDirectLaunchdMutationOutcome::Complete(_) => {
                    MacosDirectLaunchdMutationOutcome::Complete(())
                }
                MacosDirectLaunchdMutationOutcome::SubmittedUnknown => {
                    MacosDirectLaunchdMutationOutcome::SubmittedUnknown
                }
            },
            "the macOS daemon did not claim the installation after launchd started it",
        )
    }

    fn unload(&mut self) -> Result<(), InstallPlatformError> {
        confirmed(
            self.mutator
                .bootout_unconditional(UNLOAD_TIMEOUT)
                .map_err(owner_error)?,
            "launchd did not confirm the agent unloaded",
        )
    }

    fn wait_for_guard_release(&mut self) -> Result<bool, InstallPlatformError> {
        let guard = canonical_macos_daemon_guard_path().map_err(owner_error)?;
        wait_for_macos_guard_release(MACOS_MANAGED_HANDOVER_TIMEOUT, &guard.to_string_lossy())
            .map_err(owner_error)
    }

    fn wait_for_publication(
        &mut self,
        daemon: &MacosBundleRunningDaemon,
        timeout: Duration,
    ) -> Result<bool, InstallPlatformError> {
        let expectation = MacosDirectLaunchdPublicationExpectation::new(
            self.owner_epoch(Some(daemon.pid))?,
            MacosDirectLaunchdExecutableExpectation::bundled(&daemon.executable, &daemon.cdhash)
                .map_err(store_error)?,
        )
        .map_err(store_error)?;
        let published = wait_for_exact_direct_launchd_publication(
            &self.owner_store,
            &expectation,
            timeout,
            &mut self.inspector,
        )
        .map_err(owner_error)?;
        Ok(published.is_some_and(|record| record.active_identity.pid == daemon.pid))
    }

    fn prove_local_api(&mut self, version: &str) -> Result<(), InstallPlatformError> {
        verify_http(
            http_get(self.local_api, "/health", MAX_HTTP_RESPONSE_BYTES)?,
            http_get(self.local_api, "/api/v1/system", MAX_HTTP_RESPONSE_BYTES)?,
            version,
        )
    }

    fn wait_for_exit(&mut self, pid: u32, timeout: Duration) -> Result<bool, InstallPlatformError> {
        wait_for_process_exit_within(pid, timeout).map_err(owner_error)
    }
}

fn confirmed(
    outcome: MacosDirectLaunchdMutationOutcome<()>,
    detail: &str,
) -> Result<(), InstallPlatformError> {
    match outcome {
        MacosDirectLaunchdMutationOutcome::Complete(()) => Ok(()),
        MacosDirectLaunchdMutationOutcome::SubmittedUnknown => Err(error(detail)),
    }
}

fn owner_error(source: MacosOwnerExecutionError) -> InstallPlatformError {
    error(source.to_string())
}

fn store_error(source: MacosOwnerStoreError) -> InstallPlatformError {
    error(source.to_string())
}
