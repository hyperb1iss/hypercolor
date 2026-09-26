//! The raw Linux install, adoption and recovery orchestration.
//!
//! The hidden `__install-release` command and the fake-platform suites drive
//! this exact sequence. Hosts supply only what depends on the process: the
//! environment-derived root proposal, the running candidate, and the platform
//! executor. Authority election, adoption ordering and recovery live here.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use super::super::{
    InstallCoordinator, InstallCoordinatorError, InstallDisposition, InstallJournalV1, InstallLock,
    InstallOutcome, InstallPlatformError, InstallRequest, InstallStore, InstallStoreError,
    InstallTargetPolicy, InstallTransactionId, OwnershipPolicy, PlatformTransactionRecord,
    UnitCollection, UnitId, UnitRecord,
};
use super::bootstrap::{
    ensure_linux_launcher, ensure_linux_update_directories, inspect_linux_launcher,
};
use super::{
    LinuxAdoption, LinuxAdoptionError, LinuxInstallConfig, LinuxInstallElection,
    LinuxInstallExecutor, LinuxInstallLocation, LinuxInstallPlatform, LinuxLocatorError,
    LinuxPublicTree, elect_linux_installation_with, retain_linux_unit,
};

/// Durable boundaries of one install run where a crash leaves disk state.
///
/// Hosts observe each boundary after it is durable. Returning an error from
/// [`LinuxInstallHost::checkpoint`] stops the run exactly there, which the
/// recovery suites use to model process loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinuxInstallCheckpoint {
    /// The historical root still holds authority and its lock is held.
    LegacyElected,
    /// Recorded roots and the installation identity exist.
    RootsBootstrapped,
    /// The proven adoption target is recorded beside the locator.
    IntentRecorded,
    /// The historical active unit is copied into the release root.
    PriorCopied,
    /// The new release root's pointer names the copied historical unit.
    PriorActivated,
    /// The running candidate is staged into the elected release root.
    CandidateStaged,
    /// The installation's launcher is proven, published from the staged
    /// candidate when the installation had none, and the update state's
    /// `coordinator/` and `activator/` directories exist.
    LauncherReady,
    /// The adoption receipt binding legacy state to the journal is durable.
    AdoptionReceipt,
    /// The initial managed journal is durable in the state root.
    StateJournal,
    /// The permanent locator names the managed installation.
    LocatorPublished,
    /// A recorded managed installation was elected through its state lock.
    ManagedElected,
}

/// Process-dependent inputs to [`run_linux_install`].
pub trait LinuxInstallHost {
    /// The platform executor bound to one elected store.
    type Executor: LinuxInstallExecutor;

    /// Propose fresh managed roots from the caller's environment.
    ///
    /// This runs only while the historical root holds authority. Once a
    /// managed location is recorded, installs, updates and recovery follow
    /// that record and never call this method.
    ///
    /// # Errors
    /// Returns an error when the environment names unusable roots.
    fn propose_location(
        &mut self,
        home: &Path,
        uid: u32,
    ) -> Result<LinuxInstallLocation, InstallPlatformError>;

    /// Revalidate and stage the running candidate into the elected store.
    ///
    /// # Errors
    /// Returns an error when the candidate cannot be proven or staged.
    fn stage_candidate(
        &mut self,
        store: &InstallStore,
        lock: &InstallLock,
    ) -> Result<UnitRecord, InstallPlatformError>;

    /// Bind a platform executor to the elected store and lock.
    ///
    /// # Errors
    /// Returns an error when the executor cannot retain its authority.
    fn executor(
        &mut self,
        store: &InstallStore,
        lock: &InstallLock,
        tree: LinuxPublicTree,
    ) -> Result<Self::Executor, InstallPlatformError>;

    /// Observe one durable boundary.
    ///
    /// # Errors
    /// An error stops the run at this boundary without further writes.
    fn checkpoint(
        &mut self,
        checkpoint: LinuxInstallCheckpoint,
    ) -> Result<(), InstallPlatformError> {
        let _ = checkpoint;
        Ok(())
    }
}

/// What one invocation asks the installer to make active.
#[derive(Debug, Clone)]
pub struct LinuxInstallRequest {
    pub candidate: UnitId,
    pub transaction_id: InstallTransactionId,
    pub target_policy: InstallTargetPolicy,
    /// How long a started candidate must stay up before it commits, for this
    /// run and any transaction it recovers. The raw installer passes
    /// [`DEFAULT_PROBATION_WINDOW`](super::DEFAULT_PROBATION_WINDOW) unless
    /// told otherwise.
    pub probation: Duration,
}

/// The settled result of one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxInstallRun {
    pub outcome: InstallOutcome,
    /// Whether the run settled a journal instead of preparing a fresh one.
    pub recovered: bool,
    /// Releases the run removed once it settled a managed installation,
    /// or why it could not. `None` when it did not collect: the run
    /// settled the historical root, whose next install adopts it.
    pub collection: Option<Result<UnitCollection, String>>,
}

/// A run that could not settle, with the stage that refused it.
#[derive(Debug, thiserror::Error)]
pub enum LinuxInstallCommandError {
    #[error("failed to elect install authority: {0}")]
    Election(#[source] LinuxLocatorError),
    #[error("managed adoption failed: {0}")]
    Adoption(#[from] LinuxAdoptionError),
    #[error("managed authority could not be reconfirmed: {0}")]
    Authority(#[source] LinuxLocatorError),
    #[error(transparent)]
    Store(#[from] InstallStoreError),
    #[error(transparent)]
    Coordinator(#[from] InstallCoordinatorError),
    #[error(transparent)]
    Platform(#[from] InstallPlatformError),
    #[error("the recorded install journal disappeared under its lock")]
    MissingJournal,
    #[error("adoption did not elect managed authority")]
    AdoptionNotManaged,
    #[error("the run stopped at checkpoint {0:?}: {1}")]
    Stopped(LinuxInstallCheckpoint, InstallPlatformError),
    #[error("the uninstall stopped at checkpoint {0:?}: {1}")]
    UninstallStopped(
        super::uninstall::LinuxUninstallCheckpoint,
        InstallPlatformError,
    ),
    #[error(
        "refusing to uninstall without writes; these are not the entries this installer \
         generates, so remove them manually or with the package manager that owns them: {}",
        .0.join(", ")
    )]
    ForeignInstallation(Vec<String>),
    #[error(
        "refusing to remove beneath {path}: {refusal}",
        path = .0.display(),
        refusal = .1
    )]
    UnsafeDirectory(std::path::PathBuf, crate::DirectoryRefusal),
}

/// Elect authority, adopt or recover, and settle one install request.
///
/// Legacy authority with a pending journal is recovered with legacy
/// semantics first. Otherwise legacy authority is adopted into the host's
/// proposed roots before the candidate activates. Managed authority always
/// follows its recorded location and recovers a pending journal before
/// preparing another candidate.
///
/// # Errors
/// Returns the first refusal. Locks are released and nothing is rolled back
/// by the caller; a later run resumes from the durable state.
pub fn run_linux_install<H: LinuxInstallHost>(
    home: &Path,
    request: &LinuxInstallRequest,
    ownership: &OwnershipPolicy,
    host: &mut H,
) -> Result<LinuxInstallRun, LinuxInstallCommandError> {
    let elected = elect_linux_installation_with(home, ownership)
        .map_err(LinuxInstallCommandError::Election)?;
    match elected {
        LinuxInstallElection::Legacy {
            store,
            mut lock,
            locator,
        } => {
            stop(host, LinuxInstallCheckpoint::LegacyElected)?;
            let journal = store.load_journal(&lock)?;
            if let Some(journal) = journal.as_ref().filter(|journal| pending(journal)) {
                let mut platform = platform(
                    home,
                    host,
                    &store,
                    &lock,
                    LinuxPlatformInputs {
                        candidate: None,
                        journal: Some(journal),
                        managed: None,
                        original: None,
                        probation: request.probation,
                    },
                )?;
                return recover(&store, &mut lock, &mut platform);
            }
            let uid = lock
                .open_public_directory(home)?
                .metadata()
                .map_err(|source| {
                    LinuxInstallCommandError::Election(LinuxLocatorError::Io(source))
                })?;
            // An adoption that already started keeps its recorded roots; only
            // a first attempt consults the environment.
            let proposed = match locator
                .adoption_intent()
                .map_err(LinuxInstallCommandError::Election)?
            {
                Some(recorded) => recorded,
                None => host.propose_location(home, uid.owner_uid())?,
            };
            let adoption = LinuxAdoption::begin_observed(
                home,
                LinuxInstallElection::Legacy {
                    store,
                    lock,
                    locator,
                },
                proposed,
                &mut |checkpoint| stop(host, checkpoint).map_err(boxed_stop),
            )?;
            let candidate = host.stage_candidate(adoption.store(), adoption.lock())?;
            require_managed_candidate(&candidate, adoption.location())?;
            stop(host, LinuxInstallCheckpoint::CandidateStaged)?;
            ensure_linux_launcher(
                adoption.store(),
                adoption.lock(),
                adoption.location(),
                &candidate,
            )?;
            ensure_linux_update_directories(adoption.lock(), adoption.location())?;
            stop(host, LinuxInstallCheckpoint::LauncherReady)?;
            let (journal, mut platform) =
                prepare_or_replace(home, host, &adoption, request, &candidate)?;
            stop(host, LinuxInstallCheckpoint::AdoptionReceipt)?;
            adoption.store().write_journal(&journal, adoption.lock())?;
            stop(host, LinuxInstallCheckpoint::StateJournal)?;
            let LinuxInstallElection::Managed {
                store,
                mut lock,
                authority,
            } = adoption.publish(&journal, &mut platform)?
            else {
                return Err(LinuxInstallCommandError::AdoptionNotManaged);
            };
            stop(host, LinuxInstallCheckpoint::LocatorPublished)?;
            authority
                .confirm_durable()
                .map_err(LinuxInstallCommandError::Authority)?;
            let run = recover(&store, &mut lock, &mut platform)?;
            Ok(collect_settled(&store, &lock, run))
        }
        LinuxInstallElection::Managed {
            store,
            mut lock,
            authority,
        } => {
            stop(host, LinuxInstallCheckpoint::ManagedElected)?;
            let journal = store
                .load_journal(&lock)?
                .ok_or(LinuxInstallCommandError::MissingJournal)?;
            authority
                .confirm_durable()
                .map_err(LinuxInstallCommandError::Authority)?;
            ensure_linux_update_directories(&lock, authority.location())?;
            if pending(&journal) {
                let mut platform = platform(
                    home,
                    host,
                    &store,
                    &lock,
                    LinuxPlatformInputs {
                        candidate: None,
                        journal: Some(&journal),
                        managed: Some(authority.location()),
                        original: None,
                        probation: request.probation,
                    },
                )?;
                let run = recover(&store, &mut lock, &mut platform)?;
                return Ok(collect_settled(&store, &lock, run));
            }
            let candidate = host.stage_candidate(&store, &lock)?;
            require_managed_candidate(&candidate, authority.location())?;
            stop(host, LinuxInstallCheckpoint::CandidateStaged)?;
            ensure_linux_launcher(&store, &lock, authority.location(), &candidate)?;
            stop(host, LinuxInstallCheckpoint::LauncherReady)?;
            let prior_record =
                (journal.disposition == InstallDisposition::RolledBack).then_some(&journal);
            let mut platform = platform(
                home,
                host,
                &store,
                &lock,
                LinuxPlatformInputs {
                    candidate: Some(&candidate),
                    journal: prior_record,
                    managed: Some(authority.location()),
                    original: None,
                    probation: request.probation,
                },
            )?;
            authority
                .confirm_durable()
                .map_err(LinuxInstallCommandError::Authority)?;
            let outcome = InstallCoordinator::new(&store, &mut platform)
                .install_with_lock(install_request(request, candidate), &mut lock)?;
            drop(platform);
            Ok(collect_settled(
                &store,
                &lock,
                LinuxInstallRun {
                    outcome,
                    recovered: false,
                    collection: None,
                },
            ))
        }
    }
}

/// Settle an unsettled install, staging and proposing nothing.
///
/// Elects authority exactly as an install does. When its journal is still
/// forward or rolling back, recovers it (with the historical root's own
/// rules before adoption) and, on a managed installation, collects the
/// releases nothing needs any more. Returns `None` when nothing was
/// pending. This is what a recovery unit runs, through the launcher's
/// update-executor role, so a transaction left unsettled by a candidate is
/// resumed by the prior release's code; `host` is asked only for its
/// executor, never to propose roots or stage a candidate.
///
/// # Errors
/// Returns the first refusal; a later run resumes from the durable state.
pub fn run_linux_recovery<H: LinuxInstallHost>(
    home: &Path,
    probation: Duration,
    ownership: &OwnershipPolicy,
    host: &mut H,
) -> Result<Option<LinuxInstallRun>, LinuxInstallCommandError> {
    match elect_linux_installation_with(home, ownership)
        .map_err(LinuxInstallCommandError::Election)?
    {
        LinuxInstallElection::Legacy {
            store, mut lock, ..
        } => {
            let Some(journal) = store.load_journal(&lock)?.filter(pending) else {
                return Ok(None);
            };
            let mut platform = platform(
                home,
                host,
                &store,
                &lock,
                LinuxPlatformInputs {
                    candidate: None,
                    journal: Some(&journal),
                    managed: None,
                    original: None,
                    probation,
                },
            )?;
            recover(&store, &mut lock, &mut platform).map(Some)
        }
        LinuxInstallElection::Managed {
            store,
            mut lock,
            authority,
        } => {
            let journal = store
                .load_journal(&lock)?
                .ok_or(LinuxInstallCommandError::MissingJournal)?;
            authority
                .confirm_durable()
                .map_err(LinuxInstallCommandError::Authority)?;
            ensure_linux_update_directories(&lock, authority.location())?;
            if !pending(&journal) {
                return Ok(None);
            }
            let mut platform = platform(
                home,
                host,
                &store,
                &lock,
                LinuxPlatformInputs {
                    candidate: None,
                    journal: Some(&journal),
                    managed: Some(authority.location()),
                    original: None,
                    probation,
                },
            )?;
            let run = recover(&store, &mut lock, &mut platform)?;
            Ok(Some(collect_settled(&store, &lock, run)))
        }
    }
}

/// Resume the unpublished preparation or replace one that cannot resume.
///
/// Before the locator publishes, a prepared journal holds no authority and no
/// platform effect has run for it. One that names another candidate, whose
/// receipt no longer matches the legacy state, or whose recorded prior no
/// longer matches the live platform (a restart after a crash, say) is
/// discarded and prepared again from the present state.
fn prepare_or_replace<H: LinuxInstallHost>(
    home: &Path,
    host: &mut H,
    adoption: &LinuxAdoption,
    request: &LinuxInstallRequest,
    candidate: &UnitRecord,
) -> Result<(InstallJournalV1, LinuxInstallPlatform<H::Executor>), LinuxInstallCommandError> {
    let mut replaced = false;
    loop {
        let prepared = match adoption.prepared_journal() {
            Ok(Some(journal)) if journal.candidate_unit == request.candidate => Some(journal),
            Ok(None) => None,
            Ok(Some(_))
            | Err(
                LinuxAdoptionError::ConflictingPreparation
                | LinuxAdoptionError::Locator(LinuxLocatorError::Unprepared),
            ) if !replaced => {
                adoption.discard_unpublished_preparation()?;
                replaced = true;
                continue;
            }
            Ok(Some(_)) => return Err(LinuxAdoptionError::ConflictingPreparation.into()),
            Err(error) => return Err(error.into()),
        };
        let resumed = prepared.is_some();
        let mut platform = platform(
            home,
            host,
            adoption.store(),
            adoption.lock(),
            LinuxPlatformInputs {
                candidate: Some(candidate),
                journal: prepared.as_ref(),
                managed: Some(adoption.location()),
                original: adoption.original_prior(),
                probation: request.probation,
            },
        )?;
        match adoption.prepare(&mut platform, install_request(request, candidate.clone())) {
            Ok(journal) => return Ok((journal, platform)),
            Err(LinuxAdoptionError::Locator(LinuxLocatorError::Unprepared))
                if resumed && !replaced =>
            {
                drop(platform);
                adoption.discard_unpublished_preparation()?;
                replaced = true;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Refuse a candidate that does not declare the managed package contract
/// this installation runs under.
///
/// The manifest parser already requires the block from every release that
/// is not labeled macOS; this binds the requirement to the Linux installer
/// itself, whatever label a candidate carries.
fn require_managed_candidate(
    candidate: &UnitRecord,
    location: &LinuxInstallLocation,
) -> Result<(), LinuxInstallCommandError> {
    let declared = super::super::read_declared_compatibility(candidate).map_err(|source| {
        InstallPlatformError::new(format!("cannot read the candidate's contract: {source}"))
    })?;
    match declared.declared() {
        Some(package) if package.launcher_contract() == location.launcher_contract() => Ok(()),
        Some(package) => Err(InstallPlatformError::new(format!(
            "the candidate runs under launcher contract {}, but this installation has \
             contract {}",
            package.launcher_contract(),
            location.launcher_contract()
        ))
        .into()),
        None => Err(InstallPlatformError::new(
            "a Linux release must declare its managed_package contract",
        )
        .into()),
    }
}

fn stop<H: LinuxInstallHost>(
    host: &mut H,
    checkpoint: LinuxInstallCheckpoint,
) -> Result<(), LinuxInstallCommandError> {
    host.checkpoint(checkpoint)
        .map_err(|source| LinuxInstallCommandError::Stopped(checkpoint, source))
}

fn boxed_stop(error: LinuxInstallCommandError) -> LinuxAdoptionError {
    match error {
        LinuxInstallCommandError::Stopped(checkpoint, source) => {
            LinuxAdoptionError::Stopped(format!("{checkpoint:?}: {source}"))
        }
        other => LinuxAdoptionError::Stopped(other.to_string()),
    }
}

pub(super) fn pending(journal: &InstallJournalV1) -> bool {
    matches!(
        journal.disposition,
        InstallDisposition::Forward | InstallDisposition::Rollback
    )
}

fn install_request(request: &LinuxInstallRequest, candidate: UnitRecord) -> InstallRequest {
    InstallRequest {
        transaction_id: request.transaction_id.clone(),
        candidate,
        target_policy: request.target_policy,
    }
}

pub(super) fn recover<E: LinuxInstallExecutor>(
    store: &InstallStore,
    lock: &mut InstallLock,
    platform: &mut LinuxInstallPlatform<E>,
) -> Result<LinuxInstallRun, LinuxInstallCommandError> {
    let outcome = InstallCoordinator::new(store, platform)
        .recover_with_lock(lock)?
        .ok_or(LinuxInstallCommandError::MissingJournal)?;
    Ok(LinuxInstallRun {
        outcome,
        recovered: true,
        collection: None,
    })
}

/// Remove the releases a settled managed installation no longer needs.
///
/// Keeps the active release and, after a commit, the release it replaced,
/// so one previous release stays on disk. After a rollback the store keeps
/// both of its sides anyway (see [`InstallStore::referenced_units`]). A
/// failure is reported, never raised: the transaction already settled, and
/// the next run collects again.
fn collect_settled(
    store: &InstallStore,
    lock: &InstallLock,
    mut run: LinuxInstallRun,
) -> LinuxInstallRun {
    let collection = store
        .load_journal(lock)
        .map_err(|error| error.to_string())
        .and_then(|journal| {
            let retain: Vec<UnitId> = match (&run.outcome, journal) {
                (InstallOutcome::Committed { .. }, Some(journal)) => {
                    journal.prior_active_unit.into_iter().collect()
                }
                (InstallOutcome::Committed { .. }, None) => {
                    return Err("the settled install journal is missing".to_owned());
                }
                (InstallOutcome::RolledBack { .. }, _) => Vec::new(),
            };
            store
                .collect_units(lock, &retain)
                .map_err(|error| error.to_string())
        });
    run.collection = Some(collection);
    run
}

/// What a Linux platform binds to beyond the elected store.
#[derive(Debug, Clone, Copy)]
pub struct LinuxPlatformInputs<'a> {
    /// A candidate staged for a new transaction.
    pub candidate: Option<&'a UnitRecord>,
    /// The journal to resume, or the last settled one a new transaction
    /// follows; its units are retained and, for a managed store, its
    /// recorded prior authority is restored.
    pub journal: Option<&'a InstallJournalV1>,
    /// The recorded installation a managed store belongs to; `None` for the
    /// historical root.
    pub managed: Option<&'a LinuxInstallLocation>,
    /// The historical unit an adoption copies, before any journal exists.
    pub original: Option<&'a UnitRecord>,
    /// How long a started candidate must stay up before it commits.
    pub probation: Duration,
}

fn platform<H: LinuxInstallHost>(
    home: &Path,
    host: &mut H,
    store: &InstallStore,
    lock: &InstallLock,
    inputs: LinuxPlatformInputs<'_>,
) -> Result<LinuxInstallPlatform<H::Executor>, LinuxInstallCommandError> {
    bind_linux_platform(
        home,
        |store, lock, tree| host.executor(store, lock, tree),
        store,
        lock,
        inputs,
    )
}

/// Bind a Linux platform to a store elected through
/// [`elect_linux_installation`](super::elect_linux_installation).
///
/// Retains every unit the inputs and the active pointer name, opens the
/// public layout through `lock`, builds the executor, and restores the prior
/// authority a managed journal recorded. A caller that drives
/// [`InstallCoordinator`] itself (preparing, binding and writing a journal,
/// then recovering it) gets the same platform the raw installer uses.
///
/// Only the historical root of an installation that has not been adopted
/// binds without a recorded location; any other store must pass its
/// location, so its service is always rendered through the launcher and
/// its sandbox. A candidate bound for a managed
/// installation must declare that installation's launcher contract, and
/// the installation's launcher must already be published and exact, so a
/// host that skips [`ensure_linux_launcher`] still cannot start a release
/// the launcher would not run.
///
/// # Errors
/// Refuses units, executors, topology or prior roles that cannot be proven,
/// a managed store bound without its location, and a managed candidate
/// without its contract or launcher.
pub fn bind_linux_platform<E: LinuxInstallExecutor>(
    home: &Path,
    executor: impl FnOnce(
        &InstallStore,
        &InstallLock,
        LinuxPublicTree,
    ) -> Result<E, InstallPlatformError>,
    store: &InstallStore,
    lock: &InstallLock,
    inputs: LinuxPlatformInputs<'_>,
) -> Result<LinuxInstallPlatform<E>, LinuxInstallCommandError> {
    if inputs.managed.is_none()
        && (store.root() != home.join(".local/lib/hypercolor")
            || matches!(
                super::locator::read_hint(home),
                Ok(super::LinuxInstallAuthority::Managed(_))
            ))
    {
        return Err(InstallPlatformError::new(
            "a managed installation must be bound with its recorded location",
        )
        .into());
    }
    if let (Some(candidate), Some(location)) = (inputs.candidate, inputs.managed) {
        require_managed_candidate(candidate, location)?;
        if inspect_linux_launcher(location)?.is_none() {
            return Err(InstallPlatformError::new(
                "publish the installation's launcher before binding a candidate to it",
            )
            .into());
        }
    }
    let known = known_units(store, lock, inputs.candidate, inputs.journal)?;
    let tree = LinuxPublicTree::new(lock, home)?;
    let executor = executor(store, lock, tree)?;
    Ok(bind_platform(
        home,
        store,
        known,
        executor,
        inputs
            .journal
            .filter(|_| inputs.managed.is_some())
            .map(|journal| &journal.platform_record),
        inputs.original,
        inputs.probation,
        inputs.managed,
    )?)
}

/// Bind retained units and any recorded or original prior role to a platform.
///
/// # Errors
/// Refuses executor, topology, or prior-role bindings that cannot be proven.
pub(crate) fn bind_platform<E: LinuxInstallExecutor>(
    home: &Path,
    store: &InstallStore,
    known: Vec<UnitRecord>,
    mut executor: E,
    record: Option<&PlatformTransactionRecord>,
    original: Option<&UnitRecord>,
    probation: Duration,
    managed: Option<&LinuxInstallLocation>,
) -> Result<LinuxInstallPlatform<E>, InstallPlatformError> {
    if original.is_some() && record.is_none() {
        executor.retain_prior_units()?;
    }
    let config = LinuxInstallConfig {
        direct_fragment_path: home
            .join(".config/systemd/user/hypercolor.service")
            .to_str()
            .ok_or_else(|| InstallPlatformError::new("Linux HOME must be exact UTF-8"))?
            .to_owned(),
        immutable_units_root: store.root().join("units"),
        active_root: store.active_path(),
        probation,
        managed: managed.cloned(),
    };
    let mut platform = LinuxInstallPlatform::new(executor, config, known)?;
    if let Some(original) = original.filter(|_| record.is_none()) {
        platform = platform.with_prior_unit(original.clone())?;
    }
    match record {
        Some(record) => platform.with_recorded_prior(record).map_err(|source| {
            InstallPlatformError::new(format!(
                "failed to restore recorded prior authority: {source}"
            ))
        }),
        None => Ok(platform),
    }
}

fn known_units(
    store: &InstallStore,
    lock: &InstallLock,
    candidate: Option<&UnitRecord>,
    journal: Option<&InstallJournalV1>,
) -> Result<Vec<UnitRecord>, LinuxInstallCommandError> {
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
            units.push(retain_linux_unit(store, lock, &id).map_err(|source| {
                InstallPlatformError::new(format!(
                    "failed to retain installed unit {}: {source}",
                    id.as_str()
                ))
            })?);
        }
    }
    Ok(units)
}

#[cfg(test)]
#[path = "binding_tests.rs"]
mod binding_tests;
