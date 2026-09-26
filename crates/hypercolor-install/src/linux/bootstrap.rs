//! The installer-owned launcher a managed service starts through.
//!
//! `<release root>/launcher/` holds one copy of the CLI (`hypercolor`) and a
//! `contract.json` naming its digest, size, source release and the launcher
//! contract it implements. It sits beside `units/`, outside every release
//! directory, so the releases it selects between can never replace it. It is
//! published atomically from the candidate of the first managed install or
//! adoption, before that candidate starts. Once an install commits after
//! publishing it, it is the installation's for good, recorded durably in
//! the update state root: every later run only proves it unchanged, and an
//! ordinary install never rewrites it. Until then it came from a candidate
//! whose install rolled back or never finished, so the next install
//! replaces it with its own candidate's CLI. Replacing a settled
//! launcher is a launcher contract change, which contract 1 does not define.
//!
//! The directory also holds, under `units/`, the companion units the
//! publishing release shipped, rendered for this installation (see
//! [`super::companion`]); the contract record names each one's digest.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::super::{
    InstallDisposition, InstallLock, InstallPlatformError, InstallStore, UnitId, UnitRecord,
};
use super::LinuxInstallLocation;
use super::companion::LinuxCompanionUnit;
use super::model::error;
use hypercolor_platform_fs::{DirectoryEntryKind, ReadOnlyDirectoryAuthority};

/// The launcher directory beneath the release root.
pub const LINUX_LAUNCHER_DIRECTORY: &str = "launcher";
/// The launcher program inside [`LINUX_LAUNCHER_DIRECTORY`].
pub const LINUX_LAUNCHER_PROGRAM: &str = "hypercolor";
/// The CLI command the launcher program runs as.
pub const LINUX_LAUNCH_COMMAND: &str = "__launch";

const CONTRACT_NAME: &str = "contract.json";
const CONTRACT_SCHEMA_VERSION: u32 = 1;
const LAUNCHER_STAGE_PREFIX: &str = ".hypercolor-stage-launcher-";
const DIRECTORY_MODE: u32 = 0o555;
const PROGRAM_MODE: u32 = 0o555;
const CONTRACT_MODE: u32 = 0o444;
const MAX_PROGRAM_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_CONTRACT_BYTES: u64 = 4 * 1024;
static LAUNCHER_STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContractRecord {
    schema_version: u32,
    launcher_contract: u32,
    program_sha256: String,
    program_size: u64,
    source_unit: UnitId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    companion_units: Vec<CompanionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompanionRecord {
    unit: String,
    sha256: String,
    enable: bool,
}

const COMPANION_DIRECTORY: &str = "units";
const COMPANION_MODE: u32 = 0o444;

/// A managed installation's proven launcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxLauncherProgram {
    path: PathBuf,
    sha256: String,
    size: u64,
    source_unit: UnitId,
    published: bool,
    companion_units: Vec<LinuxCompanionUnit>,
}

impl LinuxLauncherProgram {
    /// Where the launcher program lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// SHA-256 of the launcher program.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Size of the launcher program in bytes.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// The release whose CLI the launcher was copied from.
    #[must_use]
    pub fn source_unit(&self) -> &UnitId {
        &self.source_unit
    }

    /// Whether this run published the launcher, rather than proving the one
    /// an earlier run published.
    #[must_use]
    pub const fn published(&self) -> bool {
        self.published
    }

    /// The companion units rendered when the launcher was published.
    #[must_use]
    pub fn companion_units(&self) -> &[LinuxCompanionUnit] {
        &self.companion_units
    }
}

/// The update state directory the daemon's update coordinator writes.
pub const LINUX_COORDINATOR_DIRECTORY: &str = "coordinator";
/// The update state directory the update activator writes.
pub const LINUX_ACTIVATOR_DIRECTORY: &str = "activator";
const UPDATE_DIRECTORY_MODE: u32 = 0o700;

/// Make sure the update state root holds its `coordinator/` and
/// `activator/` directories, each `0700` and owned by the recorded user.
///
/// The generated service can write only `coordinator/` inside an otherwise
/// read-only update state root, so neither the daemon nor the activator can
/// create its own directory there; the installer creates both, whatever the
/// umask. The daemon owns `coordinator/` and can change its mode, so a
/// directory the recorded user owns gets its `0700` back instead of
/// refusing every later install and recovery.
///
/// # Errors
/// Refuses a directory entry of another kind or owner.
pub fn ensure_linux_update_directories(
    lock: &InstallLock,
    location: &LinuxInstallLocation,
) -> Result<(), InstallPlatformError> {
    let state = lock
        .open_public_directory(location.state_root())
        .map_err(|source| error(source.to_string()))?;
    for name in [LINUX_COORDINATOR_DIRECTORY, LINUX_ACTIVATOR_DIRECTORY] {
        if let Ok(existing) = state.open_child_directory(Path::new(name)) {
            let metadata = existing
                .metadata()
                .map_err(io_error("inspect an update state directory"))?;
            if metadata.owner_uid() == location.uid() && metadata.mode() != UPDATE_DIRECTORY_MODE {
                existing
                    .into_directory_authority()
                    .and_then(|directory| directory.set_mode(UPDATE_DIRECTORY_MODE))
                    .map_err(io_error("restore an update state directory's mode"))?;
            }
        }
        let directory = state
            .durable_ensure_child_directory(Path::new(name), UPDATE_DIRECTORY_MODE)
            .map_err(|source| {
                error(format!(
                    "{} must be a {UPDATE_DIRECTORY_MODE:o} directory: {source}",
                    location.state_root().join(name).display()
                ))
            })?;
        let metadata = directory
            .metadata()
            .map_err(io_error("inspect an update state directory"))?;
        if metadata.owner_uid() != location.uid() || metadata.mode() != UPDATE_DIRECTORY_MODE {
            return Err(error(format!(
                "{} must be a {UPDATE_DIRECTORY_MODE:o} directory owned by uid {}",
                location.state_root().join(name).display(),
                location.uid()
            )));
        }
    }
    Ok(())
}

/// Where a managed installation's launcher program lives.
#[must_use]
pub fn linux_launcher_path(location: &LinuxInstallLocation) -> PathBuf {
    location
        .release_root()
        .join(LINUX_LAUNCHER_DIRECTORY)
        .join(LINUX_LAUNCHER_PROGRAM)
}

/// Prove the installation's launcher, publishing it from `candidate`'s CLI
/// when the installation has none yet, or has only one that no settled
/// service has started through.
///
/// A settled launcher is never replaced. Any launcher that differs from its
/// own contract record, carries another launcher contract, or has lost its
/// modes or owner refuses the whole run before any service change.
///
/// # Errors
/// Refuses a store or lock of another installation, a candidate that runs
/// under another launcher contract, and a launcher that cannot be published
/// or proven exact.
pub fn ensure_linux_launcher(
    store: &InstallStore,
    lock: &InstallLock,
    location: &LinuxInstallLocation,
    home: &Path,
    candidate: &UnitRecord,
) -> Result<LinuxLauncherProgram, InstallPlatformError> {
    require_store(store, location)?;
    let declared = super::super::read_declared_compatibility(candidate)
        .map_err(|source| error(format!("cannot read the candidate's contract: {source}")))?;
    let package = match declared.declared() {
        Some(package) if package.launcher_contract() == location.launcher_contract() => package,
        Some(package) => {
            return Err(error(format!(
                "the candidate runs under launcher contract {}, but this installation \
                 has contract {}; changing contracts is not supported",
                package.launcher_contract(),
                location.launcher_contract()
            )));
        }
        None => {
            return Err(error(
                "the candidate declares no managed package contract to launch under",
            ));
        }
    };
    // Every install renders its candidate's templates, whether or not it
    // publishes them, so a release with a template that could never render
    // is refused on upgrades too, before any service change.
    let rendered = render_companions(location, home, candidate, package)?;
    let root = store
        .root_authority(lock)
        .map_err(|source| error(source.to_string()))?;
    if root
        .entry_metadata(Path::new(LINUX_LAUNCHER_DIRECTORY))
        .map_err(io_error("inspect the launcher"))?
        .is_some()
    {
        let existing = prove(location, false)?;
        if existing.source_unit() == candidate.id()
            || launcher_settled(store, lock, location, &existing)?
        {
            return Ok(existing);
        }
        lock.open_store_public_directory()
            .map_err(|source| error(source.to_string()))?
            .durable_remove_child_tree(Path::new(LINUX_LAUNCHER_DIRECTORY))
            .map_err(io_error("retire a launcher no settled service started"))?;
    }
    remove_leftover_stages(lock)?;
    let sequence = LAUNCHER_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let staging = root
        .create_private_staging_directory(Path::new(&format!(
            "{LAUNCHER_STAGE_PREFIX}{}-{sequence}",
            std::process::id()
        )))
        .map_err(io_error("create the launcher staging directory"))?;
    let populated = populate(staging.directory(), location, candidate, &rendered);
    if let Err(failure) = populated {
        return match staging.remove() {
            Ok(()) => Err(failure),
            Err(cleanup) => Err(error(format!(
                "{failure}; removing the launcher staging directory also failed: {cleanup}"
            ))),
        };
    }
    staging
        .publish_or_remove(Path::new(LINUX_LAUNCHER_DIRECTORY))
        .map_err(io_error("publish the launcher"))?;
    prove(location, true)
}

/// Prove the installation's launcher without publishing one.
///
/// # Errors
/// Refuses a launcher that exists but is not exact.
pub fn inspect_linux_launcher(
    location: &LinuxInstallLocation,
) -> Result<Option<LinuxLauncherProgram>, InstallPlatformError> {
    match std::fs::symlink_metadata(location.release_root().join(LINUX_LAUNCHER_DIRECTORY)) {
        Ok(_) => prove(location, false).map(Some),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(error(format!("cannot inspect the launcher: {source}"))),
    }
}

fn require_store(
    store: &InstallStore,
    location: &LinuxInstallLocation,
) -> Result<(), InstallPlatformError> {
    if store.root() != location.release_root() || store.state_root() != location.state_root() {
        return Err(error(
            "the launcher belongs to another installation's release root",
        ));
    }
    Ok(())
}

fn populate(
    staging: &hypercolor_platform_fs::DirectoryAuthority,
    location: &LinuxInstallLocation,
    candidate: &UnitRecord,
    rendered: &[RenderedCompanion],
) -> Result<(), InstallPlatformError> {
    let companion_units = populate_companions(staging, rendered)?;
    let mut source = candidate
        .directory()
        .open_child_directory(Path::new("bin"))
        .and_then(|bin| bin.open_regular_file(Path::new(LINUX_LAUNCHER_PROGRAM)))
        .map_err(io_error("open the candidate CLI"))?;
    let size = source.metadata().size();
    if size == 0 || size > MAX_PROGRAM_BYTES {
        return Err(error("the candidate CLI is empty or too large to launch"));
    }
    let mut hashing = HashingReader {
        inner: source.file_mut(),
        hasher: Sha256::new(),
    };
    staging
        .create_regular_file(
            Path::new(LINUX_LAUNCHER_PROGRAM),
            PROGRAM_MODE,
            size,
            &mut hashing,
        )
        .map_err(io_error("copy the candidate CLI into the launcher"))?;
    let record = ContractRecord {
        schema_version: CONTRACT_SCHEMA_VERSION,
        launcher_contract: location.launcher_contract(),
        program_sha256: hex::encode(hashing.hasher.finalize()),
        program_size: size,
        source_unit: candidate.id().clone(),
        companion_units,
    };
    let bytes = serde_json::to_vec_pretty(&record)
        .map_err(|source| error(format!("encode the launcher contract: {source}")))?;
    staging
        .create_regular_file(
            Path::new(CONTRACT_NAME),
            CONTRACT_MODE,
            bytes.len() as u64,
            &mut bytes.as_slice(),
        )
        .map_err(io_error("write the launcher contract"))?;
    staging
        .set_mode(DIRECTORY_MODE)
        .map_err(io_error("seal the launcher directory"))
}

/// Render each companion template the candidate ships into `units/`.
/// One companion template of a candidate, rendered for this installation.
struct RenderedCompanion {
    unit: String,
    enable: bool,
    text: Vec<u8>,
}

/// Render every companion template `candidate` declares for `location`.
fn render_companions(
    location: &LinuxInstallLocation,
    home: &Path,
    candidate: &UnitRecord,
    package: &crate::ManagedPackage,
) -> Result<Vec<RenderedCompanion>, InstallPlatformError> {
    package
        .companion_units()
        .iter()
        .map(|declared| {
            let template = read_unit_member(candidate, declared.template())?;
            let text = super::companion::render_linux_companion_unit(&template, location, home)
                .map_err(|source| error(format!("companion unit {}: {source}", declared.unit())))?;
            Ok(RenderedCompanion {
                unit: declared.unit().to_owned(),
                enable: declared.enable(),
                text,
            })
        })
        .collect()
}

fn populate_companions(
    staging: &hypercolor_platform_fs::DirectoryAuthority,
    rendered: &[RenderedCompanion],
) -> Result<Vec<CompanionRecord>, InstallPlatformError> {
    if rendered.is_empty() {
        return Ok(Vec::new());
    }
    let units = staging
        .create_child_directory(Path::new(COMPANION_DIRECTORY))
        .map_err(io_error("create the companion unit directory"))?;
    let mut records = Vec::new();
    for companion in rendered {
        units
            .create_regular_file(
                Path::new(&companion.unit),
                COMPANION_MODE,
                companion.text.len() as u64,
                &mut companion.text.as_slice(),
            )
            .map_err(io_error("write a companion unit"))?;
        records.push(CompanionRecord {
            unit: companion.unit.clone(),
            sha256: hex::encode(Sha256::digest(&companion.text)),
            enable: companion.enable,
        });
    }
    units
        .set_mode(DIRECTORY_MODE)
        .map_err(io_error("seal the companion unit directory"))?;
    Ok(records)
}

/// Read one bounded file of a retained release by its release path.
fn read_unit_member(unit: &UnitRecord, path: &str) -> Result<Vec<u8>, InstallPlatformError> {
    let mut components: Vec<&str> = path.split('/').collect();
    let name = components
        .pop()
        .ok_or_else(|| error("a companion template path is empty"))?;
    let mut opened = match components.split_first() {
        None => unit.directory().open_regular_file(Path::new(name)),
        Some((first, rest)) => {
            let mut directory = unit
                .directory()
                .open_child_directory(Path::new(first))
                .map_err(io_error("open a companion template"))?;
            for component in rest {
                directory = directory
                    .open_child_directory(Path::new(component))
                    .map_err(io_error("open a companion template"))?;
            }
            directory.open_regular_file(Path::new(name))
        }
    }
    .map_err(io_error("open a companion template"))?;
    let mut bytes = Vec::new();
    let limit = crate::MAX_COMPANION_TEMPLATE_BYTES;
    opened
        .file_mut()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error("read a companion template"))?;
    if bytes.len() as u64 > limit {
        return Err(error("a companion template exceeds its byte bound"));
    }
    Ok(bytes)
}

/// Load the rendered companion units a published launcher carries.
fn prove_companions(
    directory: &ReadOnlyDirectoryAuthority,
    location: &LinuxInstallLocation,
    records: &[CompanionRecord],
) -> Result<Vec<LinuxCompanionUnit>, InstallPlatformError> {
    if records.is_empty() {
        return Ok(Vec::new());
    }
    let units = directory
        .open_child_directory(Path::new(COMPANION_DIRECTORY))
        .map_err(io_error("open the companion units"))?;
    let metadata = units
        .metadata()
        .map_err(io_error("inspect the companion units"))?;
    if metadata.mode() != DIRECTORY_MODE || metadata.owner_uid() != location.uid() {
        return Err(error(
            "the launcher's companion unit directory is not exact",
        ));
    }
    let mut expected: Vec<std::ffi::OsString> = records
        .iter()
        .map(|record| std::ffi::OsString::from(&record.unit))
        .collect();
    expected.sort();
    if units
        .entries()
        .map_err(io_error("list the companion units"))?
        != expected
    {
        return Err(error(
            "the launcher's companion units are not the ones it recorded",
        ));
    }
    let mut loaded = Vec::with_capacity(records.len());
    for record in records {
        let mut file = units
            .open_regular_file(Path::new(&record.unit))
            .map_err(io_error("open a companion unit"))?;
        let file_metadata = file.metadata();
        if file_metadata.mode() != COMPANION_MODE
            || file_metadata.owner_uid() != location.uid()
            || file_metadata.link_count() != 1
        {
            return Err(error(format!(
                "companion unit {} lost its mode or owner",
                record.unit
            )));
        }
        let mut bytes = Vec::new();
        let limit = super::companion::MAX_COMPANION_UNIT_BYTES as u64;
        file.file_mut()
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error("read a companion unit"))?;
        if bytes.len() as u64 > limit || hex::encode(Sha256::digest(&bytes)) != record.sha256 {
            return Err(error(format!(
                "companion unit {} changed since the launcher was published",
                record.unit
            )));
        }
        loaded.push(LinuxCompanionUnit::new(
            record.unit.clone(),
            bytes,
            record.enable,
        ));
    }
    Ok(loaded)
}

fn prove(
    location: &LinuxInstallLocation,
    published: bool,
) -> Result<LinuxLauncherProgram, InstallPlatformError> {
    let directory_path = location.release_root().join(LINUX_LAUNCHER_DIRECTORY);
    let directory =
        ReadOnlyDirectoryAuthority::open(&directory_path).map_err(io_error("open the launcher"))?;
    let metadata = directory
        .metadata()
        .map_err(io_error("inspect the launcher"))?;
    if metadata.owner_uid() != location.uid() || metadata.mode() != DIRECTORY_MODE {
        return Err(error(format!(
            "{} must be a {DIRECTORY_MODE:o} directory owned by uid {}",
            directory_path.display(),
            location.uid()
        )));
    }
    let names: Vec<_> = directory.entries().map_err(io_error("list the launcher"))?;
    let allowed = [CONTRACT_NAME, LINUX_LAUNCHER_PROGRAM, COMPANION_DIRECTORY];
    if names
        .iter()
        .any(|name| !allowed.iter().any(|allowed| name.as_os_str() == *allowed))
    {
        return Err(error(format!(
            "{} holds entries other than its program, contract and companion units",
            directory_path.display()
        )));
    }
    let mut contract = directory
        .open_regular_file(Path::new(CONTRACT_NAME))
        .map_err(io_error("open the launcher contract"))?;
    let contract_metadata = contract.metadata();
    if contract_metadata.mode() != CONTRACT_MODE
        || contract_metadata.owner_uid() != location.uid()
        || contract_metadata.size() > MAX_CONTRACT_BYTES
    {
        return Err(error("the launcher contract record is not exact"));
    }
    let mut bytes = Vec::new();
    contract
        .file_mut()
        .take(MAX_CONTRACT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error("read the launcher contract"))?;
    let record: ContractRecord = serde_json::from_slice(&bytes)
        .map_err(|source| error(format!("the launcher contract record is invalid: {source}")))?;
    if record.schema_version != CONTRACT_SCHEMA_VERSION
        || record.launcher_contract != location.launcher_contract()
    {
        return Err(error(format!(
            "the launcher implements contract {} (record schema {}), not this installation's {}",
            record.launcher_contract,
            record.schema_version,
            location.launcher_contract()
        )));
    }
    let mut program = directory
        .open_regular_file(Path::new(LINUX_LAUNCHER_PROGRAM))
        .map_err(io_error("open the launcher program"))?;
    let program_metadata = program.metadata();
    if program_metadata.kind() != DirectoryEntryKind::RegularFile
        || program_metadata.mode() != PROGRAM_MODE
        || program_metadata.owner_uid() != location.uid()
        || program_metadata.link_count() != 1
        || program_metadata.size() != record.program_size
    {
        return Err(error(
            "the launcher program's mode, owner, links or size changed since it was published",
        ));
    }
    let mut hasher = Sha256::new();
    let copied = io::copy(
        &mut program.file_mut().take(record.program_size + 1),
        &mut super::model::Sha256Writer(&mut hasher),
    )
    .map_err(io_error("hash the launcher program"))?;
    if copied != record.program_size || hex::encode(hasher.finalize()) != record.program_sha256 {
        return Err(error(
            "the launcher program's bytes changed since it was published",
        ));
    }
    let has_companions = names
        .iter()
        .any(|name| name.as_os_str() == COMPANION_DIRECTORY);
    if has_companions == record.companion_units.is_empty() {
        return Err(error(
            "the launcher's companion units do not match its contract record",
        ));
    }
    let companion_units = prove_companions(&directory, location, &record.companion_units)?;
    Ok(LinuxLauncherProgram {
        path: directory_path.join(LINUX_LAUNCHER_PROGRAM),
        sha256: record.program_sha256,
        size: record.program_size,
        source_unit: record.source_unit,
        published,
        companion_units,
    })
}

/// A companion unit the launcher's contract record says the installer
/// rendered for this installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RecordedCompanion {
    pub(super) unit: String,
    pub(super) sha256: String,
    pub(super) enable: bool,
}

/// The companion units the launcher's contract record names, read without
/// proving the launcher program, so uninstall can still find them when the
/// launcher changed. `None` when there is no readable record: the launcher
/// is gone, or its record is not one this build can read.
pub(super) fn recorded_companion_units(
    location: &LinuxInstallLocation,
) -> Option<Vec<RecordedCompanion>> {
    #[derive(Deserialize)]
    struct Recorded {
        #[serde(default)]
        companion_units: Vec<CompanionRecord>,
    }
    let directory =
        ReadOnlyDirectoryAuthority::open(&location.release_root().join(LINUX_LAUNCHER_DIRECTORY))
            .ok()?;
    let mut contract = directory.open_regular_file(Path::new(CONTRACT_NAME)).ok()?;
    let mut bytes = Vec::new();
    contract
        .file_mut()
        .take(MAX_CONTRACT_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_CONTRACT_BYTES {
        return None;
    }
    let recorded: Recorded = serde_json::from_slice(&bytes).ok()?;
    Some(
        recorded
            .companion_units
            .into_iter()
            .map(|record| RecordedCompanion {
                unit: record.unit,
                sha256: record.sha256,
                enable: record.enable,
            })
            .collect(),
    )
}

/// The update state record that names the settled launcher.
const SETTLED_NAME: &str = "launcher-settled.json";
const SETTLED_MODE: u32 = 0o600;
const SETTLED_SCHEMA_VERSION: u32 = 1;
const MAX_SETTLED_BYTES: u64 = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettledRecord {
    schema_version: u32,
    program_sha256: String,
}

/// Record that the installation's launcher is settled: an install committed
/// after it was published, so no later install may replace it.
///
/// The record names the launcher program's digest and lives in the update
/// state root, which the service cannot write, so it holds across every
/// later journal, whatever unit each transaction carries, until uninstall
/// removes the state root. A launcher published again after a user
/// removed it has a new record to earn. Writing a record that already names
/// this launcher changes nothing.
///
/// # Errors
/// Refuses a launcher that cannot be proven, and a record that cannot be
/// written durably.
pub fn record_linux_launcher_settled(
    lock: &InstallLock,
    location: &LinuxInstallLocation,
) -> Result<(), InstallPlatformError> {
    let Some(launcher) = inspect_linux_launcher(location)? else {
        return Ok(());
    };
    let bytes = serde_json::to_vec(&SettledRecord {
        schema_version: SETTLED_SCHEMA_VERSION,
        program_sha256: launcher.sha256().to_owned(),
    })
    .map_err(|source| error(format!("encode the settled launcher record: {source}")))?;
    let state = lock
        .open_public_directory(location.state_root())
        .map_err(|source| error(source.to_string()))?;
    let observed = state
        .observe_entry(Path::new(SETTLED_NAME))
        .map_err(io_error("inspect the settled launcher record"))?;
    if let hypercolor_platform_fs::ExactEntry::RegularFile { sha256, mode, .. } = &observed
        && *mode == SETTLED_MODE
        && sha256[..] == Sha256::digest(&bytes)[..]
    {
        return Ok(());
    }
    state
        .durable_replace_entry(
            Path::new(SETTLED_NAME),
            &observed,
            hypercolor_platform_fs::EntryReplacement::RegularFile {
                mode: SETTLED_MODE,
                contents: &bytes,
            },
        )
        .map(drop)
        .map_err(io_error("write the settled launcher record"))
}

/// Whether the settled launcher record names `launcher`.
fn recorded_settled(location: &LinuxInstallLocation, launcher: &LinuxLauncherProgram) -> bool {
    let Ok(state) = ReadOnlyDirectoryAuthority::open(location.state_root()) else {
        return false;
    };
    let Ok(mut record) = state.open_regular_file(Path::new(SETTLED_NAME)) else {
        return false;
    };
    let mut bytes = Vec::new();
    if record
        .file_mut()
        .take(MAX_SETTLED_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 > MAX_SETTLED_BYTES
    {
        return false;
    }
    serde_json::from_slice::<SettledRecord>(&bytes).is_ok_and(|record| {
        record.schema_version == SETTLED_SCHEMA_VERSION
            && record.program_sha256 == launcher.sha256()
    })
}

/// Whether the launcher is the installation's for good.
///
/// The settled record decides when it names this launcher. Without one
/// (a crash between a commit and writing it), the journal decides: its
/// last transaction committed with a service unit that starts the
/// launcher, or rolled back to one, so an install did commit through it.
/// The unit is matched by its `ExecStart` naming this installation's
/// launcher, not by its whole text. Anything the journal cannot settle (a
/// transaction still pending, or a record this build cannot read) counts
/// as settled, so doubt never replaces a launcher. Only a launcher that no
/// install ever committed after (its install rolled back or never wrote a
/// journal) is unsettled.
fn launcher_settled(
    store: &InstallStore,
    lock: &InstallLock,
    location: &LinuxInstallLocation,
    launcher: &LinuxLauncherProgram,
) -> Result<bool, InstallPlatformError> {
    if recorded_settled(location, launcher) {
        return Ok(true);
    }
    let journal = match store.load_journal(lock) {
        Ok(Some(journal)) => journal,
        Ok(None) => return Ok(false),
        Err(_) => return Ok(true),
    };
    let Ok(record) = super::record::decode_record(&journal.platform_record) else {
        return Ok(true);
    };
    let expected =
        super::proof::render_launcher(&location.release_root().join("active"), Some(location))?
            .exec_start;
    let starts_launcher = |unit: &[u8]| {
        super::proof::require_notify_launcher(unit).is_ok_and(|exec| exec == expected)
    };
    Ok(match journal.disposition {
        InstallDisposition::Committed => record
            .candidate_launcher
            .is_some_and(|launcher| starts_launcher(&launcher.bytes)),
        InstallDisposition::RolledBack => starts_launcher(&record.prior_launcher_bytes),
        _ => true,
    })
}

/// Remove launcher staging directories a crashed run left beside `units/`,
/// and what a crash left of retiring an unsettled launcher.
fn remove_leftover_stages(lock: &InstallLock) -> Result<(), InstallPlatformError> {
    let root = lock
        .open_store_public_directory()
        .map_err(|source| error(source.to_string()))?;
    root.durable_remove_tombstones(Path::new(LINUX_LAUNCHER_DIRECTORY))
        .map_err(io_error("remove a retired launcher"))?;
    for name in root
        .child_names()
        .map_err(io_error("list the release root"))?
    {
        if name
            .to_str()
            .is_some_and(|name| name.starts_with(LAUNCHER_STAGE_PREFIX))
        {
            root.durable_remove_child_tree(Path::new(&name))
                .map_err(io_error("remove a leftover launcher stage"))?;
        }
    }
    Ok(())
}

struct HashingReader<'a> {
    inner: &'a mut std::fs::File,
    hasher: Sha256,
}

impl Read for HashingReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.hasher.update(&buffer[..read]);
        Ok(read)
    }
}

fn io_error(operation: &'static str) -> impl Fn(io::Error) -> InstallPlatformError {
    move |source| error(format!("failed to {operation}: {source}"))
}
