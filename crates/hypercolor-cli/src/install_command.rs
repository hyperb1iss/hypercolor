use std::path::{Component, Path};

use anyhow::{Context as _, Result, bail};

use crate::InstallReleaseArgs;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
pub(crate) fn execute(args: &InstallReleaseArgs) -> Result<()> {
    execute_linux(args)
}

#[cfg(target_os = "macos")]
pub(crate) fn execute(args: &InstallReleaseArgs) -> Result<()> {
    macos::execute(args)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn execute(_args: &InstallReleaseArgs) -> Result<()> {
    bail!("raw release installation is unsupported on this platform")
}

pub(crate) fn parse_manifest_digest(value: &str) -> Result<crate::install::UnitId, String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("manifest digest must be exactly 64 lowercase hexadecimal characters".into());
    }
    crate::install::UnitId::new(value).map_err(|error| error.to_string())
}

#[cfg(target_os = "linux")]
fn execute_linux(args: &InstallReleaseArgs) -> Result<()> {
    use std::collections::BTreeSet;
    use std::net::{Ipv4Addr, SocketAddr};

    use crate::install::{
        InstallCoordinator, InstallDisposition, InstallRequest, InstallStore, InstallTargetPolicy,
        LinuxInstallConfig, LinuxInstallPlatform, LinuxNativeExecutor, LinuxPublicTree,
        MAX_INSTALL_JOURNAL_BYTES, stage_release_payload_from_authority,
        validate_release_payload_from_authority,
    };

    let home = linux_home()?;
    require_bounded_absolute(&home, "HOME")?;
    let topology = LinuxInstallTopology::new(&args.install_prefix, &args.install_dir, &home)?;
    let (source, candidate_executable) = running_linux_candidate()?;

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
    let public_tree = LinuxPublicTree::new(&lock, &home)
        .context("failed to retain the Linux public install tree")?;
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

    let config = LinuxInstallConfig {
        direct_fragment_path: home
            .join(".config/systemd/user/hypercolor.service")
            .to_str()
            .expect("HOME was validated as exact UTF-8")
            .to_owned(),
        immutable_units_root: topology.store_root.join("units"),
        active_root: topology.store_root.join("active"),
    };
    let executor = LinuxNativeExecutor::new(
        &store,
        &lock,
        public_tree,
        SocketAddr::from((Ipv4Addr::LOCALHOST, 9420)),
    )
    .context("failed to construct the native Linux install executor")?;
    let mut platform = LinuxInstallPlatform::new(executor, config, known_units)
        .context("failed to bind the Linux install transaction")?;
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
        target_policy: if args.no_service {
            InstallTargetPolicy::Preserve
        } else {
            InstallTargetPolicy::EnableOnFirstInstall
        },
    };
    let outcome = coordinator
        .install_with_lock(request, &mut lock)
        .context("transactional Linux release installation failed")?;
    require_candidate_committed(outcome, &args.expected_manifest_sha256, false)
}

#[cfg(target_os = "linux")]
fn retain_platform_units(
    store: &crate::install::InstallStore,
    lock: &crate::install::InstallLock,
    state: &crate::install::PlatformState,
    seen: &mut std::collections::BTreeSet<String>,
    known_units: &mut Vec<crate::install::UnitRecord>,
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

#[cfg(target_os = "linux")]
fn retain_unit(
    store: &crate::install::InstallStore,
    lock: &crate::install::InstallLock,
    unit: crate::install::UnitId,
    seen: &mut std::collections::BTreeSet<String>,
    known_units: &mut Vec<crate::install::UnitRecord>,
) -> Result<()> {
    if seen.insert(unit.as_str().to_owned()) {
        known_units.push(
            crate::install::retain_linux_unit(store, lock, &unit)
                .with_context(|| format!("failed to retain installed unit {}", unit.as_str()))?,
        );
    }
    Ok(())
}

fn require_candidate_committed(
    outcome: crate::install::InstallOutcome,
    candidate: &crate::install::UnitId,
    recovery: bool,
) -> Result<()> {
    match outcome {
        crate::install::InstallOutcome::Committed { active_unit } if &active_unit == candidate => {
            Ok(())
        }
        crate::install::InstallOutcome::Committed { active_unit } => bail!(
            "recovered release unit {}; rerun the installer to install {}",
            active_unit.as_str(),
            candidate.as_str()
        ),
        crate::install::InstallOutcome::RolledBack {
            active_unit,
            failure,
        } => {
            let action = if recovery { "recovery" } else { "installation" };
            let restored = active_unit
                .as_ref()
                .map_or("none", crate::install::UnitId::as_str);
            bail!("release {action} rolled back to {restored}: {failure}")
        }
    }
}

fn transaction_id(unit: &crate::install::UnitId) -> Result<crate::install::InstallTransactionId> {
    let value = format!("release-{}", &unit.as_str()[..56]);
    crate::install::InstallTransactionId::new(value).context("failed to derive transaction ID")
}

#[cfg(target_os = "linux")]
fn running_linux_candidate() -> Result<(
    hypercolor_platform_fs::ReadOnlyDirectoryAuthority,
    std::fs::File,
)> {
    let executable = std::fs::File::open("/proc/self/exe")
        .context("failed to open the running candidate through /proc/self/exe")?;
    let executable_path = std::fs::read_link("/proc/self/exe")
        .context("failed to resolve the running candidate through /proc/self/exe")?;
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
    Ok((source, executable))
}

#[cfg(target_os = "linux")]
fn linux_home() -> Result<std::path::PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is required for a raw Linux release install"))
}

#[cfg(target_os = "linux")]
struct LinuxInstallTopology {
    store_root: std::path::PathBuf,
}

#[cfg(target_os = "linux")]
impl LinuxInstallTopology {
    fn new(install_prefix: &Path, install_dir: &Path, home: &Path) -> Result<Self> {
        require_bounded_absolute(install_prefix, "install prefix")?;
        require_bounded_absolute(install_dir, "install directory")?;
        if install_prefix != home.join(".local") {
            bail!("install prefix must be exactly $HOME/.local for a raw Linux install");
        }
        if install_dir != install_prefix.join("bin") {
            bail!("install directory must be exactly <install-prefix>/bin");
        }
        let store_root = install_prefix.join("lib/hypercolor");
        let store_text = store_root
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("install topology must be exact UTF-8"))?;
        if !store_text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte))
        {
            bail!("install topology is not safely representable in systemd ExecStart");
        }
        Ok(Self { store_root })
    }
}

fn require_bounded_absolute(path: &Path, label: &str) -> Result<()> {
    let text = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("{label} must be exact UTF-8"))?;
    if text.is_empty() || text.len() > 4096 || !path.is_absolute() {
        bail!("{label} must be one bounded absolute path");
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        bail!("{label} must be lexically normalized");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{require_bounded_absolute, require_candidate_committed};
    use crate::install::{InstallOutcome, UnitId};
    use std::path::Path;

    #[cfg(target_os = "linux")]
    #[test]
    fn install_topology_is_exact_and_systemd_safe() {
        let topology = super::LinuxInstallTopology::new(
            Path::new("/home/test/.local"),
            Path::new("/home/test/.local/bin"),
            Path::new("/home/test"),
        )
        .expect("valid install topology");
        assert_eq!(
            topology.store_root,
            Path::new("/home/test/.local/lib/hypercolor")
        );

        for (prefix, install_dir) in [
            ("relative", "/home/test/.local/bin"),
            ("/home/other/.local", "/home/other/.local/bin"),
            ("/home/test/.local", "/home/test/bin"),
            ("/home/test/space prefix", "/home/test/space prefix/bin"),
            ("/home/test/../test/.local", "/home/test/../test/.local/bin"),
        ] {
            assert!(
                super::LinuxInstallTopology::new(
                    Path::new(prefix),
                    Path::new(install_dir),
                    Path::new("/home/test")
                )
                .is_err(),
                "unsafe topology accepted: {prefix} {install_dir}"
            );
        }
    }

    #[test]
    fn bounded_absolute_path_rejects_relative_and_parent_components() {
        assert!(require_bounded_absolute(Path::new("/home/test"), "test").is_ok());
        assert!(require_bounded_absolute(Path::new("home/test"), "test").is_err());
        assert!(require_bounded_absolute(Path::new("/home/../test"), "test").is_err());
    }

    #[test]
    fn hidden_install_reports_only_the_requested_committed_unit_as_success() {
        let candidate = UnitId::new("a".repeat(64)).expect("candidate unit");
        let other = UnitId::new("b".repeat(64)).expect("other unit");
        assert!(
            require_candidate_committed(
                InstallOutcome::Committed {
                    active_unit: candidate.clone(),
                },
                &candidate,
                false,
            )
            .is_ok()
        );
        let recovered_other = require_candidate_committed(
            InstallOutcome::Committed { active_unit: other },
            &candidate,
            true,
        )
        .expect_err("a different recovered unit requires a fresh invocation");
        assert!(recovered_other.to_string().contains("rerun the installer"));

        let rolled_back = require_candidate_committed(
            InstallOutcome::RolledBack {
                active_unit: None,
                failure: "candidate owner proof failed".to_owned(),
            },
            &candidate,
            false,
        )
        .expect_err("rollback is not a successful candidate install");
        assert!(
            rolled_back
                .to_string()
                .contains("candidate owner proof failed")
        );
    }
}
