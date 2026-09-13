use std::collections::BTreeSet;
use std::fs::File;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use hypercolor_platform_fs::ReadOnlyDirectoryAuthority;

use crate::InstallReleaseArgs;
use crate::install::{
    InstallCoordinator, InstallDisposition, InstallJournalV1, InstallLock, InstallRequest,
    InstallStore, InstallTargetPolicy, LinuxAdoption, LinuxInstallConfig, LinuxInstallElection,
    LinuxInstallLocation, LinuxInstallPlatform, LinuxNativeExecutor, LinuxPublicTree, UnitRecord,
    elect_linux_installation, retain_linux_unit, stage_release_payload_from_authority,
};

pub(super) fn execute(
    args: &InstallReleaseArgs,
    home: &Path,
    legacy_root: &Path,
    source: &ReadOnlyDirectoryAuthority,
    executable: &File,
) -> Result<()> {
    let elected = elect_linux_installation(home).context("failed to elect install authority")?;
    match elected {
        LinuxInstallElection::Legacy {
            store,
            mut lock,
            locator,
        } => {
            if store.root() != legacy_root {
                bail!("legacy authority disagrees with the install prefix");
            }
            let journal = store.load_journal(&lock)?;
            if let Some(journal) = journal.as_ref().filter(|journal| pending(journal)) {
                let mut platform = platform(home, &store, &lock, None, Some(journal), false, None)?;
                return recover(args, &store, &mut lock, &mut platform);
            }
            let uid = lock.open_public_directory(home)?.metadata()?.owner_uid();
            let proposed = proposed_location(home, uid)?;
            let adoption = LinuxAdoption::begin(
                home,
                LinuxInstallElection::Legacy {
                    store,
                    lock,
                    locator,
                },
                proposed,
            )?;
            let candidate = stage(args, adoption.store(), adoption.lock(), source, executable)?;
            let prepared = adoption.prepared_journal()?;
            let mut platform = platform(
                home,
                adoption.store(),
                adoption.lock(),
                Some(&candidate),
                prepared.as_ref(),
                true,
                adoption.original_prior(),
            )?;
            let journal = adoption.prepare(&mut platform, request(args, candidate)?)?;
            let LinuxInstallElection::Managed {
                store,
                mut lock,
                authority,
            } = adoption.publish(&journal, &mut platform)?
            else {
                bail!("adoption did not elect managed authority");
            };
            authority.confirm_durable()?;
            recover(args, &store, &mut lock, &mut platform)
        }
        LinuxInstallElection::Managed {
            store,
            mut lock,
            authority,
        } => {
            let journal = store
                .load_journal(&lock)?
                .ok_or_else(|| anyhow::anyhow!("managed journal disappeared"))?;
            authority.confirm_durable()?;
            if pending(&journal) {
                let mut platform = platform(home, &store, &lock, None, Some(&journal), true, None)?;
                return recover(args, &store, &mut lock, &mut platform);
            }
            let candidate = stage(args, &store, &lock, source, executable)?;
            let prior_record =
                (journal.disposition == InstallDisposition::RolledBack).then_some(&journal);
            let mut platform = platform(
                home,
                &store,
                &lock,
                Some(&candidate),
                prior_record,
                true,
                None,
            )?;
            authority.confirm_durable()?;
            let outcome = InstallCoordinator::new(&store, &mut platform)
                .install_with_lock(request(args, candidate)?, &mut lock)?;
            super::require_candidate_committed(outcome, &args.expected_manifest_sha256, false)
        }
    }
}

fn pending(journal: &InstallJournalV1) -> bool {
    matches!(
        journal.disposition,
        InstallDisposition::Forward | InstallDisposition::Rollback
    )
}

fn stage(
    args: &InstallReleaseArgs,
    store: &InstallStore,
    lock: &InstallLock,
    source: &ReadOnlyDirectoryAuthority,
    executable: &File,
) -> Result<UnitRecord> {
    stage_release_payload_from_authority(
        store,
        lock,
        source,
        executable,
        &args.expected_manifest_sha256,
    )
    .context("release revalidation and staging failed")
}

fn request(args: &InstallReleaseArgs, candidate: UnitRecord) -> Result<InstallRequest> {
    Ok(InstallRequest {
        transaction_id: super::transaction_id(&args.expected_manifest_sha256)?,
        candidate,
        target_policy: if args.no_service {
            InstallTargetPolicy::Preserve
        } else {
            InstallTargetPolicy::EnableOnFirstInstall
        },
    })
}

fn recover(
    args: &InstallReleaseArgs,
    store: &InstallStore,
    lock: &mut InstallLock,
    platform: &mut LinuxInstallPlatform<LinuxNativeExecutor>,
) -> Result<()> {
    let outcome = InstallCoordinator::new(store, platform)
        .recover_with_lock(lock)?
        .ok_or_else(|| anyhow::anyhow!("interrupted journal disappeared"))?;
    super::require_candidate_committed(outcome, &args.expected_manifest_sha256, true)
}

fn platform(
    home: &Path,
    store: &InstallStore,
    lock: &InstallLock,
    candidate: Option<&UnitRecord>,
    journal: Option<&InstallJournalV1>,
    managed: bool,
    original: Option<&UnitRecord>,
) -> Result<LinuxInstallPlatform<LinuxNativeExecutor>> {
    let known = known_units(store, lock, candidate, journal)?;
    let tree = LinuxPublicTree::new(lock, home)?;
    let executor = LinuxNativeExecutor::new(
        store,
        lock,
        tree,
        SocketAddr::from((Ipv4Addr::LOCALHOST, 9420)),
    )?;
    bind_platform(
        home,
        store,
        known,
        executor,
        journal
            .filter(|_| managed)
            .map(|journal| &journal.platform_record),
        original,
    )
}

fn bind_platform(
    home: &Path,
    store: &InstallStore,
    known: Vec<UnitRecord>,
    mut executor: LinuxNativeExecutor,
    record: Option<&crate::install::PlatformTransactionRecord>,
    original: Option<&UnitRecord>,
) -> Result<LinuxInstallPlatform<LinuxNativeExecutor>> {
    if original.is_some() && record.is_none() {
        executor.retain_prior_units()?;
    }
    let config = LinuxInstallConfig {
        direct_fragment_path: home
            .join(".config/systemd/user/hypercolor.service")
            .to_str()
            .expect("validated HOME")
            .to_owned(),
        immutable_units_root: store.root().join("units"),
        active_root: store.active_path(),
    };
    let mut platform = LinuxInstallPlatform::new(executor, config, known)?;
    if let Some(original) = original.filter(|_| record.is_none()) {
        platform = platform.with_prior_unit(original.clone())?;
    }
    match record {
        Some(record) => platform
            .with_recorded_prior(record)
            .context("failed to restore recorded prior authority"),
        None => Ok(platform),
    }
}

fn known_units(
    store: &InstallStore,
    lock: &InstallLock,
    candidate: Option<&UnitRecord>,
    journal: Option<&InstallJournalV1>,
) -> Result<Vec<UnitRecord>> {
    let mut units: Vec<_> = candidate.into_iter().cloned().collect();
    let mut seen: BTreeSet<_> = units
        .iter()
        .map(|unit| unit.id().as_str().to_owned())
        .collect();
    let mut ids: Vec<_> = store.active_unit(lock)?.into_iter().collect();
    if let Some(journal) = journal {
        ids.push(journal.candidate_unit.clone());
        ids.extend(journal.prior_active_unit.clone());
        for state in [&journal.prior_platform, &journal.target_platform] {
            ids.extend(
                [
                    state.layout_unit.clone(),
                    state.launcher_unit.clone(),
                    state.running_unit.clone(),
                ]
                .into_iter()
                .flatten(),
            );
        }
    }
    for id in ids {
        if seen.insert(id.as_str().to_owned()) {
            units.push(
                retain_linux_unit(store, lock, &id)
                    .with_context(|| format!("failed to retain installed unit {}", id.as_str()))?,
            );
        }
    }
    Ok(units)
}

fn proposed_location(home: &Path, uid: u32) -> Result<LinuxInstallLocation> {
    proposed_location_with(home, uid, |name| std::env::var_os(name))
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

#[cfg(test)]
#[path = "linux_binding_tests.rs"]
mod binding_tests;
