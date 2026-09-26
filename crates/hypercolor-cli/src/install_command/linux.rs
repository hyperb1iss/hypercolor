use std::fs::File;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result};
use hypercolor_platform_fs::ReadOnlyDirectoryAuthority;

use crate::InstallReleaseArgs;
use crate::install::{
    InstallLock, InstallPlatformError, InstallStore, InstallTargetPolicy, LinuxInstallHost,
    LinuxInstallLocation, LinuxInstallRequest, LinuxNativeExecutor, LinuxPublicTree,
    LinuxUninstallHost, OwnershipPolicy, UnitRecord, run_linux_install, run_linux_uninstall,
    stage_release_payload_from_authority,
};

pub(super) fn execute(
    args: &InstallReleaseArgs,
    home: &Path,
    legacy_root: &Path,
    source: &ReadOnlyDirectoryAuthority,
    executable: &File,
) -> Result<()> {
    if legacy_root != home.join(".local/lib/hypercolor") {
        anyhow::bail!("legacy authority disagrees with the install prefix");
    }
    let request = LinuxInstallRequest {
        candidate: args.expected_manifest_sha256.clone(),
        transaction_id: super::transaction_id(&args.expected_manifest_sha256)?,
        target_policy: if args.no_service {
            InstallTargetPolicy::Preserve
        } else {
            InstallTargetPolicy::EnableOnFirstInstall
        },
        probation: Duration::from_secs(args.probation_seconds),
    };
    if args.probation_seconds > 0 {
        println!(
            "A newly started release must stay up for {} seconds before the install commits it.",
            args.probation_seconds
        );
    }
    let mut host = NativeHost {
        args,
        source,
        executable,
    };
    let run = run_linux_install(home, &request, &OwnershipPolicy::system(), &mut host)?;
    super::require_candidate_committed(run.outcome, &args.expected_manifest_sha256, run.recovered)
}

pub(super) fn execute_uninstall(home: &Path) -> Result<()> {
    let run = run_linux_uninstall(home, &OwnershipPolicy::system(), &mut NativeUninstallHost)?;
    if let Some(outcome) = &run.recovered {
        println!("Settled an interrupted installation before removal: {outcome:?}");
    }
    if let Some(reason) = &run.unsettled {
        println!("Removed an interrupted installation that could not settle: {reason}");
    }
    if run.removed.is_empty() && run.recovered.is_none() && run.unsettled.is_none() {
        println!("No raw Hypercolor installation is recorded for this user.");
    }
    for path in &run.removed {
        println!("Removed {}", path.display());
    }
    for path in &run.preserved {
        println!("Preserved {}", path.display());
    }
    Ok(())
}

struct NativeUninstallHost;

impl LinuxUninstallHost for NativeUninstallHost {
    type Executor = LinuxNativeExecutor;

    fn executor(
        &mut self,
        store: &InstallStore,
        lock: &InstallLock,
        tree: LinuxPublicTree,
    ) -> Result<LinuxNativeExecutor, InstallPlatformError> {
        native_executor(store, lock, tree)
    }
}

fn native_executor(
    store: &InstallStore,
    lock: &InstallLock,
    tree: LinuxPublicTree,
) -> Result<LinuxNativeExecutor, InstallPlatformError> {
    LinuxNativeExecutor::new(
        store,
        lock,
        tree,
        SocketAddr::from((Ipv4Addr::LOCALHOST, 9420)),
    )
}

/// The running process: its environment, candidate and native platform.
struct NativeHost<'a> {
    args: &'a InstallReleaseArgs,
    source: &'a ReadOnlyDirectoryAuthority,
    executable: &'a File,
}

impl LinuxInstallHost for NativeHost<'_> {
    type Executor = LinuxNativeExecutor;

    fn propose_location(
        &mut self,
        home: &Path,
        uid: u32,
    ) -> Result<LinuxInstallLocation, InstallPlatformError> {
        proposed_location_with(home, uid, |name| std::env::var_os(name))
            .map_err(|error| InstallPlatformError::new(format!("{error:#}")))
    }

    fn stage_candidate(
        &mut self,
        store: &InstallStore,
        lock: &InstallLock,
    ) -> Result<UnitRecord, InstallPlatformError> {
        stage_release_payload_from_authority(
            store,
            lock,
            self.source,
            self.executable,
            &self.args.expected_manifest_sha256,
        )
        .map_err(|error| {
            InstallPlatformError::new(format!("release revalidation and staging failed: {error}"))
        })
    }

    fn executor(
        &mut self,
        store: &InstallStore,
        lock: &InstallLock,
        tree: LinuxPublicTree,
    ) -> Result<LinuxNativeExecutor, InstallPlatformError> {
        native_executor(store, lock, tree)
    }
}

fn proposed_location_with(
    home: &Path,
    uid: u32,
    lookup: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<LinuxInstallLocation> {
    let base = |name, fallback: &str| {
        lookup(name)
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| home.join(fallback))
    };
    LinuxInstallLocation::new(
        home,
        &base("XDG_DATA_HOME", ".local/share"),
        &base("XDG_STATE_HOME", ".local/state"),
        &base("XDG_CONFIG_HOME", ".config"),
        uid,
    )
    .context("invalid managed XDG topology")
}

#[cfg(test)]
#[path = "linux_tests.rs"]
mod tests;
