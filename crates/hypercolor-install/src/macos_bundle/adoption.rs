//! Adopting the bundle a person dragged out of the DMG.
//!
//! The DMG is the first install, and it knows nothing of the release store:
//! it leaves a signed `Hypercolor.app` in Applications. Adoption turns that
//! bundle into an installation the updater can act on. It publishes the
//! bundle as the first unit, copies the launcher out of it, writes the launchd
//! agents, hands the daemon to the direct agent and proves it there, and only
//! then writes the records that say all of that happened. `installation.json`
//! comes last, so its presence means the rest is durable.
//!
//! Every step observes what exists and does what is missing, so the next
//! launch of the app finishes an interrupted adoption by running it again.
//! The disk is the journal: nothing between the first step and the last has
//! a state of its own to resume.
//!
//! The same run repairs an installation someone replaced by dragging a newer
//! app over it. That bundle is no unit, so an update would refuse it, and the
//! old transaction's terminal journal would never settle against it. Adoption
//! publishes it, clears the staging root and leaves a journal that matches.
//! That journal reads as a committed first install, with nothing displaced
//! and nothing to go back to, because that is what happened.
//!
//! Adoption only reads the installed bundle. App Management guards changes to
//! a signed app, and nothing here makes one; the swap that does runs later,
//! in an agent signed by the app's own team.

use std::fs;
use std::io;
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use uuid::Uuid;

use super::super::{
    InstallDisposition, InstallJournalV1, InstallLock, InstallPlatformError, InstallStore,
    InstallTargetPolicy, InstallTransactionId, PlatformState, UnitId, UnitRecord,
};
use super::agents::{
    MacosBundleAgentDefinition, macos_bundle_daemon_agent, macos_bundle_update_agents,
};
use super::executor::MacosBundleExecutor;
use super::installation::{
    MACOS_BUNDLE_INSTALLATION_FILE, MACOS_BUNDLE_LAUNCHER_NAME, MacosBundleInstallation,
    MacosBundleRoots,
};
use super::model::{
    MACOS_BUNDLE_INVENTORY, MACOS_BUNDLE_NAME, MacosBundleConfig, MacosBundleIdentity,
    MacosBundleObservation, MacosBundleRunningDaemon, MacosBundleServiceObservation,
    MacosBundleSlot, MacosBundleUnit, error,
};
use super::platform::transition_states;
use super::record::{BundleRecord, encode_receipt};
use super::service::MacosBundleService;
use super::swap::SWAP_OPERATION_COUNT;

/// Directories the daemon and the activator write, which exist before either
/// runs.
const STATE_DIRECTORIES: [&str; 2] = ["coordinator", "activator"];
/// Where a run assembles the unit it publishes. A run that dies leaves it for
/// the next run to replace.
const SCRATCH_DIRECTORY: &str = "adoption";
/// The app's executable inside its bundle.
const APP_IN_BUNDLE: &str = "Contents/MacOS/hypercolor-app";
/// The marker launchd watches to keep recovery running.
const PENDING_MARKER: &str = "pending";
/// Where every unit keeps its store inventory.
const UNIT_INVENTORY: &str = "share/hypercolor/durable-stores.json";
const MAX_INSTALLATION_BYTES: usize = 64 * 1024;
const DEFINITION_MODE: u32 = 0o644;
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;

/// What one account's adoption of one bundle needs to know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosBundleAdoptionRequest {
    /// The bundle to adopt and the release root.
    pub config: MacosBundleConfig,
    /// The bundle's version, which its daemon must report.
    pub version: String,
    /// `~/Library/LaunchAgents`.
    pub launch_agents: PathBuf,
    /// Where the agents' output goes.
    pub logs: PathBuf,
    /// The daemon's data directory.
    pub data_root: PathBuf,
}

/// How one adoption run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacosBundleAdoptionOutcome {
    /// The records already describe the installed bundle.
    Current,
    /// A transaction is open, and the bundle is its own until it ends.
    Deferred,
    /// The installed bundle is now the active unit.
    Adopted { unit: UnitId },
}

/// What adoption asks of launchd and the owner coordinator, beyond the
/// daemon's agent.
pub trait MacosBundleAdoptionHost {
    /// Load the agent defined at `definition`. A job already loaded stays
    /// unless `replace` is set, because its definition just changed.
    ///
    /// # Errors
    /// Returns an error when launchd refuses.
    fn load_agent(
        &mut self,
        label: &str,
        definition: &Path,
        replace: bool,
    ) -> Result<(), InstallPlatformError>;

    /// Unload the agent `label`. An agent that is not loaded is done.
    ///
    /// # Errors
    /// Returns an error when the agent stays loaded.
    fn unload_agent(&mut self, label: &str) -> Result<(), InstallPlatformError>;

    /// Record the direct agent as the daemon's owner. Its daemon runs, proven,
    /// when this is called.
    ///
    /// # Errors
    /// Returns an error when the owner coordinator does not end there.
    fn hand_over_daemon(&mut self) -> Result<(), InstallPlatformError>;
}

/// The bundle `executable` runs from, when that is where a person installs
/// one: `Hypercolor.app` in `/Applications` or in `home`'s `Applications`. A
/// copy still on the DMG, or anywhere else, is not an installation.
#[must_use]
pub fn macos_bundle_in_applications(executable: &Path, home: &Path) -> Option<PathBuf> {
    [Path::new("/Applications"), &home.join("Applications")]
        .into_iter()
        .map(|folder| folder.join(MACOS_BUNDLE_NAME))
        .find(|bundle| {
            executable
                .strip_prefix(bundle)
                .is_ok_and(|inside| inside == Path::new(APP_IN_BUNDLE))
        })
}

/// Adopt the installed bundle, or finish an adoption that was interrupted.
///
/// A first adoption that fails before the direct agent owns the daemon takes
/// the agents back out, so the app goes on running its own daemon and the
/// next launch starts clean. Past that point the daemon stays with its agent
/// and the next launch finishes the records.
///
/// # Errors
/// Returns an error when the bundle cannot be published, its daemon cannot be
/// proven under the direct agent, or the records cannot be written.
/// `installation.json` is absent or unchanged then, so nothing claims the
/// bundle.
pub fn adopt_macos_bundle<E, S, H>(
    store: &InstallStore,
    lock: &InstallLock,
    executor: &mut E,
    service: &mut S,
    host: &mut H,
    request: &MacosBundleAdoptionRequest,
) -> Result<MacosBundleAdoptionOutcome, InstallPlatformError>
where
    E: MacosBundleExecutor,
    S: MacosBundleService,
    H: MacosBundleAdoptionHost,
{
    request.config.validate()?;
    if request.config.release_root != store.root() {
        return Err(error("adoption names another release root than its store"));
    }
    let roots = MacosBundleRoots {
        release_root: store.root().to_path_buf(),
        state_root: store.state_root().to_path_buf(),
    };
    Adoption {
        store,
        lock,
        executor,
        service,
        host,
        request,
        roots,
    }
    .run()
}

struct Adoption<'a, E, S, H> {
    store: &'a InstallStore,
    lock: &'a InstallLock,
    executor: &'a mut E,
    service: &'a mut S,
    host: &'a mut H,
    request: &'a MacosBundleAdoptionRequest,
    roots: MacosBundleRoots,
}

impl<E: MacosBundleExecutor, S: MacosBundleService, H: MacosBundleAdoptionHost>
    Adoption<'_, E, S, H>
{
    fn run(&mut self) -> Result<MacosBundleAdoptionOutcome, InstallPlatformError> {
        let journal = self.store.load_journal(self.lock).map_err(store_error)?;
        if journal.as_ref().is_some_and(is_open) {
            return Ok(MacosBundleAdoptionOutcome::Deferred);
        }
        let bundles = self.executor.observe()?;
        let MacosBundleSlot::Bundle(installed) = &bundles.installed else {
            return Err(error("no readable bundle is installed to adopt"));
        };
        installed.validate()?;
        let recorded = self.recorded()?;
        let active = self.store.active_unit(self.lock).map_err(store_error)?;
        let agent = self.service.observe()?;
        let daemon_agent = macos_bundle_daemon_agent(&self.request.config, &self.request.logs)?;
        let update_agents = macos_bundle_update_agents(
            &self.roots.launcher(),
            &self.roots.state_root.join(PENDING_MARKER),
            &self.request.logs,
        )?;
        if agent.registered
            && self.roots.launcher().is_file()
            && update_agents
                .iter()
                .all(|agent| self.definition_path(agent).is_file())
            && self.is_current(
                recorded.as_ref(),
                active.as_ref(),
                journal.as_ref(),
                installed,
            )
        {
            return Ok(MacosBundleAdoptionOutcome::Current);
        }
        if recorded.is_none() {
            self.refuse_foreign_agent(&daemon_agent, &agent)?;
        }

        for directory in STATE_DIRECTORIES {
            self.store
                .ensure_state_directory(Path::new(directory), self.lock)
                .map_err(store_error)?;
        }
        let (record, unit) = self.publish_unit(installed)?;
        crate::payload::publish_macos_bundle_launcher(
            self.store,
            self.lock,
            &record,
            Path::new(MACOS_BUNDLE_LAUNCHER_NAME),
        )
        .map_err(|source| error(source.to_string()))?;
        self.clear_staging(&bundles)?;

        let running = match self.take_over_daemon(&daemon_agent, &update_agents, &unit) {
            Ok(running) => running,
            Err(failure) if recorded.is_none() => {
                return Err(self.abandon(&failure, &daemon_agent, &update_agents));
            }
            Err(failure) => return Err(failure),
        };

        // The direct agent owns the daemon from here, and the supervisor now
        // waits on that owner instead of starting a sidecar. A failure below
        // is an interruption like a crash: the daemon stays with its agent
        // and the next launch finishes these records.
        let journal = adopted_journal(&self.request.config, &unit, &running)?;
        self.store
            .write_journal(&journal, self.lock)
            .map_err(store_error)?;
        self.store
            .set_active(Some(unit.id()), self.lock)
            .map_err(store_error)?;
        self.write_installation(recorded)?;
        Ok(MacosBundleAdoptionOutcome::Adopted {
            unit: unit.id().clone(),
        })
    }

    fn recorded(&self) -> Result<Option<MacosBundleInstallation>, InstallPlatformError> {
        self.store
            .read_state_record(
                Path::new(MACOS_BUNDLE_INSTALLATION_FILE),
                MAX_INSTALLATION_BYTES,
                self.lock,
            )
            .map_err(store_error)?
            .map(|bytes| MacosBundleInstallation::decode(&bytes).map_err(error))
            .transpose()
    }

    /// Whether the records say the installed bundle is what the last
    /// transaction left active. This reads no unit, so it costs a launch
    /// nothing once the installation is adopted.
    fn is_current(
        &self,
        recorded: Option<&MacosBundleInstallation>,
        active: Option<&UnitId>,
        journal: Option<&InstallJournalV1>,
        installed: &MacosBundleIdentity,
    ) -> bool {
        let (Some(recorded), Some(active), Some(journal)) = (recorded, active, journal) else {
            return false;
        };
        recorded.installed_bundle == self.request.config.installed_bundle
            && BundleRecord::decode(&journal.platform_record).is_ok_and(|record| {
                record.outcome(journal.disposition).is_some_and(|binding| {
                    &binding.unit == active && binding.identity == *installed
                })
            })
    }

    /// A first adoption leaves another installer's agent alone: the raw
    /// macOS install shares the label.
    fn refuse_foreign_agent(
        &self,
        daemon_agent: &MacosBundleAgentDefinition,
        agent: &MacosBundleServiceObservation,
    ) -> Result<(), InstallPlatformError> {
        let definition = read_definition(&self.definition_path(daemon_agent))
            .map_err(|source| error(format!("read the daemon agent's definition: {source}")))?;
        if definition.is_some_and(|bytes| bytes != daemon_agent.bytes) {
            return Err(error(
                "another installer defined the daemon's launchd agent, so this bundle stays \
                 unmanaged",
            ));
        }
        if let Some(running) = &agent.running {
            running.validate()?;
            if running.executable != self.request.config.installed_daemon() {
                return Err(error(
                    "the daemon's launchd agent runs another installation, so this bundle stays \
                     unmanaged",
                ));
            }
        }
        Ok(())
    }

    /// Publish the bundle as a unit: a private copy of it, the store
    /// inventory it seals moved to where a unit keeps it, and a manifest of
    /// exactly those bytes.
    fn publish_unit(
        &mut self,
        installed: &MacosBundleIdentity,
    ) -> Result<(UnitRecord, MacosBundleUnit), InstallPlatformError> {
        let scratch = self.roots.state_root.join(SCRATCH_DIRECTORY);
        remove_scratch(&scratch)?;
        fs::DirBuilder::new()
            .mode(PRIVATE_DIRECTORY_MODE)
            .create(&scratch)
            .map_err(|source| error(format!("create the adoption scratch copy: {source}")))?;
        let published = self.publish_from(&scratch);
        let removed = remove_scratch(&scratch);
        let (record, unit) = published?;
        removed?;
        if unit.identity() != installed {
            return Err(error(
                "the installed bundle changed while it was being adopted",
            ));
        }
        Ok((record, unit))
    }

    fn publish_from(
        &mut self,
        scratch: &Path,
    ) -> Result<(UnitRecord, MacosBundleUnit), InstallPlatformError> {
        self.executor.snapshot_installed(scratch)?;
        let inventory = fs::read(scratch.join(MACOS_BUNDLE_NAME).join(MACOS_BUNDLE_INVENTORY))
            .map_err(|source| error(format!("read the bundle's store inventory: {source}")))?;
        let unit_inventory = scratch.join(UNIT_INVENTORY);
        unit_inventory
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&unit_inventory, &inventory))
            .map_err(|source| error(format!("place the unit's store inventory: {source}")))?;
        let digest = crate::describe_macos_bundle_unit(scratch, &self.request.version)
            .map_err(|source| error(source.to_string()))?;
        let record = crate::stage_macos_bundle_unit(self.store, self.lock, scratch, &digest)
            .map_err(|source| error(source.to_string()))?;
        let unit =
            crate::bind_macos_bundle_unit(&record).map_err(|source| error(source.to_string()))?;
        Ok((record, unit))
    }

    /// Remove whatever an earlier transaction left beside the bundle. It
    /// describes bundles that are no longer the installation's, and a
    /// transaction cannot be prepared while anything sits there.
    fn clear_staging(
        &mut self,
        bundles: &MacosBundleObservation,
    ) -> Result<(), InstallPlatformError> {
        let leftovers = bundles.interrupted.iter().filter_map(|name| {
            name.strip_suffix(super::MACOS_BUNDLE_PARTIAL_SUFFIX)
                .or_else(|| name.strip_suffix(super::MACOS_BUNDLE_DISCARD_SUFFIX))
        });
        for name in bundles.staged.keys().map(String::as_str).chain(leftovers) {
            self.executor.discard(name)?;
        }
        Ok(())
    }

    /// Put the installation under launchd: every agent defined, the update
    /// agents loaded, the daemon running and proven under the direct agent,
    /// and the owner coordinator told.
    fn take_over_daemon(
        &mut self,
        daemon_agent: &MacosBundleAgentDefinition,
        update_agents: &[MacosBundleAgentDefinition],
        unit: &MacosBundleUnit,
    ) -> Result<MacosBundleRunningDaemon, InstallPlatformError> {
        for directory in [&self.request.launch_agents, &self.request.logs] {
            fs::create_dir_all(directory)
                .map_err(|source| error(format!("create {}: {source}", directory.display())))?;
        }
        let replaced = self.write_definition(daemon_agent)?;
        for agent in update_agents {
            let changed = self.write_definition(agent)?;
            let path = self.definition_path(agent);
            self.host.load_agent(agent.label, &path, changed)?;
        }
        let running = self.run_daemon(unit, replaced)?;
        self.service
            .prove(&running, unit.version(), Duration::ZERO)?;
        self.host.hand_over_daemon()?;
        Ok(running)
    }

    /// The daemon of `unit` under the direct agent. One that already runs
    /// stays, unless launchd holds an older definition of it. Anything else
    /// the agent runs goes first: a bundle dragged over a running daemon
    /// leaves the old code running from a deleted file.
    fn run_daemon(
        &mut self,
        unit: &MacosBundleUnit,
        replaced: bool,
    ) -> Result<MacosBundleRunningDaemon, InstallPlatformError> {
        let cdhash = unit.identity().daemon.as_str();
        let observed = self.service.observe()?;
        if let Some(running) = observed.running.as_ref().filter(|running| {
            !replaced
                && running.cdhash == cdhash
                && running.executable == self.request.config.installed_daemon()
        }) {
            return Ok(running.clone());
        }
        self.service.stop(observed.running.as_ref())?;
        self.service.wait_for_release()?;
        if !observed.enabled {
            self.service.set_enabled(true)?;
        }
        self.service.start(cdhash)?;
        self.service
            .observe()?
            .running
            .ok_or_else(|| error("the daemon is not running after its start"))
    }

    /// Take a first adoption that never handed the daemon over back out, so
    /// the app keeps supervising its own daemon and no login starts this
    /// one. The unit and the launcher stay: the next run proves them again
    /// and nothing reads them until then.
    fn abandon(
        &mut self,
        failure: &InstallPlatformError,
        daemon_agent: &MacosBundleAgentDefinition,
        update_agents: &[MacosBundleAgentDefinition],
    ) -> InstallPlatformError {
        let mut detail = failure.to_string();
        let mut note = |step: Result<(), InstallPlatformError>| {
            if let Err(cleanup) = step {
                detail.push_str("; taking the adoption back out also failed: ");
                detail.push_str(&cleanup.to_string());
            }
        };
        let stopped = self
            .service
            .observe()
            .and_then(|observed| self.service.stop(observed.running.as_ref()));
        note(stopped);
        note(remove_definition(&self.definition_path(daemon_agent)));
        for agent in update_agents {
            note(self.host.unload_agent(agent.label));
            note(remove_definition(&self.definition_path(agent)));
        }
        error(detail)
    }

    fn write_installation(
        &self,
        recorded: Option<MacosBundleInstallation>,
    ) -> Result<(), InstallPlatformError> {
        let installation_id =
            recorded.map_or_else(Uuid::new_v4, |recorded| recorded.installation_id);
        let installation = MacosBundleInstallation::new(
            installation_id,
            self.request.config.installed_bundle.clone(),
            &self.roots,
            self.request.data_root.clone(),
        );
        let bytes = installation
            .encode()
            .map_err(|source| error(format!("encode installation.json: {source}")))?;
        self.store
            .write_state_record(Path::new(MACOS_BUNDLE_INSTALLATION_FILE), &bytes, self.lock)
            .map_err(store_error)
    }

    fn definition_path(&self, agent: &MacosBundleAgentDefinition) -> PathBuf {
        self.request.launch_agents.join(agent.file_name())
    }

    /// Durably put `agent`'s definition in place, and report whether that
    /// changed anything. launchd reads the file by its path.
    fn write_definition(
        &self,
        agent: &MacosBundleAgentDefinition,
    ) -> Result<bool, InstallPlatformError> {
        let path = self.definition_path(agent);
        let failed = |source: io::Error| error(format!("write {}: {source}", path.display()));
        if read_definition(&path).map_err(failed)?.as_deref() == Some(agent.bytes.as_slice()) {
            return Ok(false);
        }
        let temporary = path.with_extension(format!("plist.{}", std::process::id()));
        remove_definition(&temporary)?;
        let written = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(DEFINITION_MODE)
                .open(&temporary)?;
            file.set_permissions(fs::Permissions::from_mode(DEFINITION_MODE))?;
            file.write_all(&agent.bytes)?;
            file.sync_all()?;
            hypercolor_platform_fs::durable_replace(&temporary, &path)
        })();
        if written.is_err() {
            drop(fs::remove_file(&temporary));
        }
        written.map_err(failed)?;
        Ok(true)
    }
}

/// The journal a finished adoption leaves: a committed first install of
/// `unit`, whose agent the installer had already registered.
fn adopted_journal(
    config: &MacosBundleConfig,
    unit: &MacosBundleUnit,
    running: &MacosBundleRunningDaemon,
) -> Result<InstallJournalV1, InstallPlatformError> {
    let model = |source: crate::InstallModelError| error(source.to_string());
    let policy = InstallTargetPolicy::EnabledAndRunning;
    let prior = PlatformState {
        layout_unit: None,
        launcher_unit: Some(unit.id().clone()),
        loaded: false,
        running_unit: None,
        autostart_enabled: true,
    };
    let target = policy.target_platform(&prior, unit.id());
    let transaction =
        InstallTransactionId::new(format!("adopt-{}", &unit.id().as_str()[..56])).map_err(model)?;
    let mut journal = InstallJournalV1::new(
        transaction,
        None,
        unit.id().clone(),
        prior.clone(),
        policy,
        transition_states(&prior, &target),
        SWAP_OPERATION_COUNT,
        BundleRecord::adopted(config.installed_bundle.clone(), unit).encode()?,
    )
    .map_err(model)?;
    journal.layout_operation_index = SWAP_OPERATION_COUNT;
    journal.candidate_owner_receipt = Some(encode_receipt(running)?);
    journal
        .advance(InstallDisposition::Committed, None)
        .map_err(model)?;
    Ok(journal)
}

const fn is_open(journal: &InstallJournalV1) -> bool {
    matches!(
        journal.disposition,
        InstallDisposition::Forward | InstallDisposition::Rollback
    )
}

/// The regular file at `path`, never through a link.
fn read_definition(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match hypercolor_platform_fs::open_no_follow(path) {
        Ok(mut file) => {
            let mut bytes = Vec::new();
            io::Read::read_to_end(&mut file, &mut bytes)?;
            Ok(Some(bytes))
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(source),
    }
}

fn remove_definition(path: &Path) -> Result<(), InstallPlatformError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(error(format!("remove {}: {source}", path.display()))),
    }
}

fn remove_scratch(scratch: &Path) -> Result<(), InstallPlatformError> {
    match fs::remove_dir_all(scratch) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(error(format!("remove the adoption scratch copy: {source}"))),
    }
}

fn store_error(source: crate::InstallStoreError) -> InstallPlatformError {
    error(source.to_string())
}
