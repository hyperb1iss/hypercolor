use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use super::{require_bounded_absolute, require_candidate_committed, transaction_id};
use crate::InstallReleaseArgs;
use crate::install::macos::{
    MacosInstallConfig, MacosInstallPlatform, MacosNativeExecutor, retain_macos_unit,
};
use crate::install::{
    InstallCoordinator, InstallDisposition, InstallRequest, InstallStore, InstallTargetPolicy,
    MAX_INSTALL_JOURNAL_BYTES, PlatformState, UnitId, UnitRecord,
    stage_release_payload_from_authority, validate_release_payload_from_authority,
};

pub(super) fn execute(args: &InstallReleaseArgs) -> Result<()> {
    let home = macos_home()?;
    require_bounded_absolute(&home, "HOME")?;
    let topology = MacosInstallTopology::new(&args.install_prefix, &args.install_dir, &home)?;
    let (source, candidate_executable) = running_candidate()?;

    validate_release_payload_from_authority(
        &source,
        &candidate_executable,
        &args.expected_manifest_sha256,
    )
    .context("release candidate validation failed before install bootstrap")?;

    let store = InstallStore::new(&topology.store_root, MAX_INSTALL_JOURNAL_BYTES);
    let mut lock = store
        .acquire_anchored_lock(&home)
        .context("failed to acquire the release install lock")?;
    let candidate = stage_release_payload_from_authority(
        &store,
        &lock,
        &source,
        &candidate_executable,
        &args.expected_manifest_sha256,
    )
    .context("release candidate revalidation and staging failed")?;
    let journal = store
        .load_journal(&lock)
        .context("failed to inspect the release install journal")?;
    let pending_recovery = journal.as_ref().is_some_and(|journal| {
        matches!(
            journal.disposition,
            InstallDisposition::Forward | InstallDisposition::Rollback
        )
    });
    let active_unit = store
        .active_unit(&lock)
        .context("failed to inspect the active release unit")?;

    let mut known_units = vec![candidate.clone()];
    let mut seen = BTreeSet::from([candidate.id().as_str().to_owned()]);
    if let Some(unit) = active_unit {
        retain_unit(&store, &lock, unit, &mut seen, &mut known_units)?;
    }
    if let Some(journal) = journal.as_ref().filter(|_| pending_recovery) {
        retain_unit(
            &store,
            &lock,
            journal.candidate_unit.clone(),
            &mut seen,
            &mut known_units,
        )?;
        if let Some(unit) = journal.prior_active_unit.clone() {
            retain_unit(&store, &lock, unit, &mut seen, &mut known_units)?;
        }
        retain_platform_units(
            &store,
            &lock,
            &journal.prior_platform,
            &mut seen,
            &mut known_units,
        )?;
        retain_platform_units(
            &store,
            &lock,
            &journal.target_platform,
            &mut seen,
            &mut known_units,
        )?;
    }

    let config = MacosInstallConfig {
        direct_plist_path: exact_text(
            &home.join("Library/LaunchAgents/tech.hyperbliss.hypercolor.plist"),
        )?,
        immutable_units_root: topology.store_root.join("units"),
        active_root: topology.store_root.join("active"),
        log_directory: home.join("Library/Logs/hypercolor"),
    };
    let executor = MacosNativeExecutor::new(
        &store,
        &mut lock,
        &home,
        &args.install_prefix,
        &args.install_dir,
        &home.join("Library/Application Support/Hypercolor"),
        config.clone(),
    )
    .context("failed to construct the native macOS install executor")?;
    let mut platform = MacosInstallPlatform::new(executor, config, known_units)
        .context("failed to bind the macOS install transaction")?;
    let mut coordinator = InstallCoordinator::new(&store, &mut platform);

    if pending_recovery {
        let outcome = coordinator
            .recover_with_lock(&mut lock)
            .context("failed to recover the interrupted release transaction")?
            .ok_or_else(|| anyhow::anyhow!("the interrupted release journal disappeared"))?;
        return require_candidate_committed(outcome, &args.expected_manifest_sha256, true);
    }

    let request = InstallRequest {
        transaction_id: transaction_id(&args.expected_manifest_sha256)?,
        candidate,
        target_policy: target_policy(args.no_service),
    };
    let outcome = coordinator
        .install_with_lock(request, &mut lock)
        .context("transactional macOS release installation failed")?;
    require_candidate_committed(outcome, &args.expected_manifest_sha256, false)
}

fn retain_platform_units(
    store: &InstallStore,
    lock: &crate::install::InstallLock,
    state: &PlatformState,
    seen: &mut BTreeSet<String>,
    known_units: &mut Vec<UnitRecord>,
) -> Result<()> {
    for unit in [
        state.layout_unit.clone(),
        state.launcher_unit.clone(),
        state.running_unit.clone(),
    ]
    .into_iter()
    .flatten()
    {
        retain_unit(store, lock, unit, seen, known_units)?;
    }
    Ok(())
}

fn retain_unit(
    store: &InstallStore,
    lock: &crate::install::InstallLock,
    unit: UnitId,
    seen: &mut BTreeSet<String>,
    known_units: &mut Vec<UnitRecord>,
) -> Result<()> {
    if seen.insert(unit.as_str().to_owned()) {
        known_units.push(
            retain_macos_unit(store, lock, &unit)
                .with_context(|| format!("failed to retain installed unit {}", unit.as_str()))?,
        );
    }
    Ok(())
}

fn running_candidate() -> Result<(
    hypercolor_platform_fs::ReadOnlyDirectoryAuthority,
    std::fs::File,
)> {
    let executable_path =
        std::env::current_exe().context("failed to resolve the running candidate")?;
    require_bounded_absolute(&executable_path, "running candidate")?;
    if executable_path.file_name() != Some(std::ffi::OsStr::new("hypercolor"))
        || executable_path.parent().and_then(Path::file_name) != Some(std::ffi::OsStr::new("bin"))
    {
        bail!("running candidate must be the exact <release-root>/bin/hypercolor executable");
    }
    let root = executable_path
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow::anyhow!("running candidate has no extracted release root"))?;
    let source = hypercolor_platform_fs::ReadOnlyDirectoryAuthority::open(root)
        .context("failed to retain the extracted release root")?;
    let executable = source
        .open_child_directory(Path::new("bin"))
        .and_then(|bin| bin.open_regular_file(Path::new("hypercolor")))
        .map(hypercolor_platform_fs::OpenedRegularFile::into_file)
        .context("failed to retain the running release candidate")?;
    Ok((source, executable))
}

fn macos_home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is required for a raw macOS release install"))
}

struct MacosInstallTopology {
    store_root: PathBuf,
}

impl MacosInstallTopology {
    fn new(install_prefix: &Path, install_dir: &Path, home: &Path) -> Result<Self> {
        require_bounded_absolute(install_prefix, "install prefix")?;
        require_bounded_absolute(install_dir, "install directory")?;
        if install_dir != install_prefix.join("bin") {
            bail!("install directory must be exactly <install-prefix>/bin");
        }
        let relative = install_prefix
            .strip_prefix(home)
            .map_err(|_| anyhow::anyhow!("macOS raw install prefix must be below HOME"))?;
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!("macOS raw install prefix must be a normalized descendant of HOME");
        }
        Ok(Self {
            store_root: install_prefix.join("lib/hypercolor"),
        })
    }
}

fn exact_text(path: &Path) -> Result<String> {
    require_bounded_absolute(path, "macOS install path")?;
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("macOS install path must be exact UTF-8"))
}

const fn target_policy(no_service: bool) -> InstallTargetPolicy {
    if no_service {
        InstallTargetPolicy::Preserve
    } else {
        InstallTargetPolicy::EnableOnFirstInstall
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::install::InstallTargetPolicy;

    use super::{MacosInstallTopology, target_policy};

    #[test]
    fn topology_is_bounded_to_one_home_descendant() {
        let topology = MacosInstallTopology::new(
            Path::new("/Users/test/.local"),
            Path::new("/Users/test/.local/bin"),
            Path::new("/Users/test"),
        )
        .expect("valid macOS topology");
        assert_eq!(
            topology.store_root,
            Path::new("/Users/test/.local/lib/hypercolor")
        );
        for (prefix, install_dir) in [
            ("relative", "/Users/test/.local/bin"),
            ("/Users/other/.local", "/Users/other/.local/bin"),
            ("/Users/test", "/Users/test/bin"),
            ("/Users/test/.local", "/Users/test/bin"),
            (
                "/Users/test/../test/.local",
                "/Users/test/../test/.local/bin",
            ),
        ] {
            assert!(
                MacosInstallTopology::new(
                    Path::new(prefix),
                    Path::new(install_dir),
                    Path::new("/Users/test")
                )
                .is_err(),
                "unsafe topology accepted: {prefix} {install_dir}"
            );
        }
    }

    #[test]
    fn no_service_selects_preserve_policy() {
        assert_eq!(target_policy(true), InstallTargetPolicy::Preserve);
        assert_eq!(
            target_policy(false),
            InstallTargetPolicy::EnableOnFirstInstall
        );
    }
}
