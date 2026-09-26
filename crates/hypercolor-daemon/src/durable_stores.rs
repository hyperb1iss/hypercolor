//! Every durable store a release reads or writes, and what each one holds.
//!
//! [`DURABLE_STORES`] is the inventory a managed release package declares in
//! its manifest (`managed_package.compatibility.stores`): one entry per store,
//! with the schema this release writes and the range it reads. The values
//! come from the stores themselves, never from the application version:
//!
//! - A store with a version field declares that field's values: the constant
//!   its writer stamps, and the oldest and newest its reader accepts.
//! - A store without one declares schema `0`, which names the only shape it
//!   has ever had. When such a store gains a version field, it starts at `1`,
//!   and a release that reads only `0` does not read it.
//!
//! [`probe_durable_stores`] reads, before any store opens, the schema each
//! one finds on disk, with the same field names and defaults its own reader
//! applies. The daemon keeps that [`DurableStoreReport`] on its state so an
//! extension can record what data a release actually met, which is what an
//! update's compatibility decision needs on top of the declarations.
//!
//! `packaging/managed/durable-stores.json` is the copy the release packager
//! embeds in the manifest; a test holds it equal to this table.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// The largest store file a probe reads.
const MAX_PROBED_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// The most files a directory store probe reads.
const MAX_PROBED_DIRECTORY_FILES: usize = 4096;

/// The directory a store lives under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreRoot {
    /// `$XDG_CONFIG_HOME/hypercolor`.
    Config,
    /// `$XDG_DATA_HOME/hypercolor`.
    Data,
    /// `$XDG_STATE_HOME/hypercolor`.
    State,
}

/// Where a store keeps its data beneath its root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreLocation {
    /// One file.
    File(&'static str),
    /// A directory of files.
    Directory(&'static str),
    /// The main configuration file, wherever the daemon was told to load it.
    ConfigFile,
}

/// How a store records its schema on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreSchema {
    /// No version field; any data present is schema `0`.
    Unversioned,
    /// A top-level integer field of a JSON object. `missing` is what the
    /// store's reader assumes when the field is absent; `None` means the
    /// reader refuses such a file.
    JsonField {
        field: &'static str,
        missing: Option<u32>,
    },
    /// A top-level integer field of a TOML document, which the reader
    /// requires.
    TomlField { field: &'static str },
    /// A JSON array whose every record carries the field, which the reader
    /// requires. The store holds the newest record schema.
    JsonArrayRecords { field: &'static str },
    /// A JSON object whose every value is a record carrying the field.
    JsonMapRecords {
        field: &'static str,
        missing: Option<u32>,
    },
    /// A directory tree of TOML files, each carrying the field.
    TomlDirectoryRecords {
        field: &'static str,
        missing: Option<u32>,
    },
}

/// Which program reads and writes a store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreOwner {
    /// The daemon opens the store at startup.
    Daemon,
    /// The CLI, TUI, desktop app or OpenRGB host reads it; the daemon never
    /// opens it, but it ships in the same release.
    Client,
}

/// One durable store and the compatibility this release declares for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurableStore {
    /// Stable name shared by every release that declares the store.
    pub name: &'static str,
    /// On-disk format family.
    pub storage_format: &'static str,
    pub owner: StoreOwner,
    pub root: StoreRoot,
    pub location: StoreLocation,
    /// The file's previous home under the data root, which the store still
    /// reads when its state-root file does not exist yet.
    pub legacy_data_file: Option<&'static str>,
    pub schema: StoreSchema,
    pub readable_schema_min: u32,
    pub readable_schema_max: u32,
    pub written_schema: u32,
    /// `backward_compatible`, `staged` or `manual`, as the manifest spells it.
    pub migration_mode: &'static str,
}

const BACKWARD_COMPATIBLE: &str = "backward_compatible";

/// Every durable store this release reads or writes.
pub const DURABLE_STORES: &[DurableStore] = &[
    // Refuses every schema but 5; 4 is upgraded in memory
    // (hypercolor_core::config::ConfigManager::parse_toml).
    DurableStore {
        name: "config",
        storage_format: "toml",
        owner: StoreOwner::Daemon,
        root: StoreRoot::Config,
        location: StoreLocation::ConfigFile,
        legacy_data_file: None,
        schema: StoreSchema::TomlField {
            field: "schema_version",
        },
        readable_schema_min: 4,
        readable_schema_max: hypercolor_types::config::CURRENT_SCHEMA_VERSION,
        written_schema: hypercolor_types::config::CURRENT_SCHEMA_VERSION,
        migration_mode: BACKWARD_COMPATIBLE,
    },
    // A plain UUID; an unparseable one is regenerated (startup/config.rs).
    unversioned(
        "instance-id",
        "text",
        StoreOwner::Daemon,
        StoreRoot::Data,
        StoreLocation::File("instance_id"),
    ),
    // The index stamps `version` and its reader defaults a missing one to
    // the current value; objects are content-addressed blobs.
    DurableStore {
        name: "asset-library",
        storage_format: "json",
        owner: StoreOwner::Daemon,
        root: StoreRoot::Config,
        location: StoreLocation::File("assets/index.json"),
        legacy_data_file: None,
        schema: StoreSchema::JsonField {
            field: "version",
            missing: Some(hypercolor_core::asset::INDEX_VERSION),
        },
        readable_schema_min: hypercolor_core::asset::INDEX_VERSION,
        readable_schema_max: hypercolor_core::asset::INDEX_VERSION,
        written_schema: hypercolor_core::asset::INDEX_VERSION,
        migration_mode: BACKWARD_COMPATIBLE,
    },
    unversioned(
        "user-effects",
        "html",
        StoreOwner::Daemon,
        StoreRoot::Data,
        StoreLocation::Directory("effects"),
    ),
    // Every layout record carries a required `version`, always 1
    // (hypercolor_types::spatial::SpatialLayout).
    DurableStore {
        name: "layouts",
        storage_format: "json",
        owner: StoreOwner::Daemon,
        root: StoreRoot::Data,
        location: StoreLocation::File("layouts.json"),
        legacy_data_file: None,
        schema: StoreSchema::JsonArrayRecords { field: "version" },
        readable_schema_min: 1,
        readable_schema_max: 1,
        written_schema: 1,
        migration_mode: BACKWARD_COMPATIBLE,
    },
    unversioned(
        "layout-auto-exclusions",
        "json",
        StoreOwner::Daemon,
        StoreRoot::Data,
        StoreLocation::File("layout-auto-exclusions.json"),
    ),
    // Read once to import into scenes, then renamed aside; no release that
    // declares stores writes it.
    unversioned(
        "legacy-profiles",
        "json",
        StoreOwner::Daemon,
        StoreRoot::Data,
        StoreLocation::File("profiles.json"),
    ),
    DurableStore {
        name: "scenes",
        storage_format: "json",
        owner: StoreOwner::Daemon,
        root: StoreRoot::Data,
        location: StoreLocation::File("scenes.json"),
        legacy_data_file: None,
        schema: StoreSchema::JsonField {
            field: "schema_version",
            missing: None,
        },
        readable_schema_min: crate::scene_store::SCENE_STORE_SCHEMA_VERSION,
        readable_schema_max: crate::scene_store::SCENE_STORE_SCHEMA_VERSION,
        written_schema: crate::scene_store::SCENE_STORE_SCHEMA_VERSION,
        migration_mode: BACKWARD_COMPATIBLE,
    },
    DurableStore {
        name: "driver-inventory",
        storage_format: "json",
        owner: StoreOwner::Daemon,
        root: StoreRoot::State,
        location: StoreLocation::File(crate::driver_inventory::DRIVER_INVENTORY_FILENAME),
        legacy_data_file: Some(crate::driver_inventory::DRIVER_INVENTORY_FILENAME),
        schema: StoreSchema::JsonField {
            field: "schema_version",
            missing: Some(crate::driver_inventory::INVENTORY_SCHEMA_VERSION),
        },
        readable_schema_min: crate::driver_inventory::INVENTORY_SCHEMA_VERSION,
        readable_schema_max: crate::driver_inventory::INVENTORY_SCHEMA_VERSION,
        written_schema: crate::driver_inventory::INVENTORY_SCHEMA_VERSION,
        migration_mode: BACKWARD_COMPATIBLE,
    },
    // AES-GCM ciphertext of a JSON map, with its seed beside it.
    unversioned(
        "credentials",
        "encrypted-json",
        StoreOwner::Daemon,
        StoreRoot::Data,
        StoreLocation::File("credentials.json.enc"),
    ),
    unversioned(
        "logical-devices",
        "json",
        StoreOwner::Daemon,
        StoreRoot::Data,
        StoreLocation::File("logical-devices.json"),
    ),
    // Each template defaults a missing `schema_version` to 1
    // (hypercolor_types::attachment).
    DurableStore {
        name: "attachment-templates",
        storage_format: "toml",
        owner: StoreOwner::Daemon,
        root: StoreRoot::Data,
        location: StoreLocation::Directory("attachments"),
        legacy_data_file: None,
        schema: StoreSchema::TomlDirectoryRecords {
            field: "schema_version",
            missing: Some(1),
        },
        readable_schema_min: 1,
        readable_schema_max: 1,
        written_schema: 1,
        migration_mode: BACKWARD_COMPATIBLE,
    },
    DurableStore {
        name: "attachment-profiles",
        storage_format: "json",
        owner: StoreOwner::Daemon,
        root: StoreRoot::Data,
        location: StoreLocation::File("attachment-profiles.json"),
        legacy_data_file: None,
        schema: StoreSchema::JsonMapRecords {
            field: "schema_version",
            missing: Some(1),
        },
        readable_schema_min: 1,
        readable_schema_max: 1,
        written_schema: 1,
        migration_mode: BACKWARD_COMPATIBLE,
    },
    DurableStore {
        legacy_data_file: Some("display-preferences.json"),
        ..unversioned(
            "display-preferences",
            "json",
            StoreOwner::Daemon,
            StoreRoot::State,
            StoreLocation::File("display-preferences.json"),
        )
    },
    // Files before schema 3 are migrated with a backup; a missing version
    // is schema 1.
    DurableStore {
        name: "device-settings",
        storage_format: "json",
        owner: StoreOwner::Daemon,
        root: StoreRoot::State,
        location: StoreLocation::File("device-settings.json"),
        legacy_data_file: Some("device-settings.json"),
        schema: StoreSchema::JsonField {
            field: "schema_version",
            missing: Some(1),
        },
        readable_schema_min: 1,
        readable_schema_max: crate::device_settings::DEVICE_SETTINGS_SCHEMA_VERSION,
        written_schema: crate::device_settings::DEVICE_SETTINGS_SCHEMA_VERSION,
        migration_mode: BACKWARD_COMPATIBLE,
    },
    unversioned(
        "simulated-displays",
        "json",
        StoreOwner::Daemon,
        StoreRoot::Data,
        StoreLocation::File("simulated-displays.json"),
    ),
    DurableStore {
        legacy_data_file: Some("runtime-state.json"),
        ..unversioned(
            "runtime-state",
            "json",
            StoreOwner::Daemon,
            StoreRoot::State,
            StoreLocation::File("runtime-state.json"),
        )
    },
    // A missing version is schema 2; every other version is refused.
    DurableStore {
        name: "device-aliases",
        storage_format: "json",
        owner: StoreOwner::Daemon,
        root: StoreRoot::State,
        location: StoreLocation::File(crate::device_aliases::DEVICE_ALIASES_FILE),
        legacy_data_file: Some(crate::device_aliases::DEVICE_ALIASES_FILE),
        schema: StoreSchema::JsonField {
            field: "schema_version",
            missing: Some(crate::device_aliases::SCHEMA_VERSION),
        },
        readable_schema_min: crate::device_aliases::SCHEMA_VERSION,
        readable_schema_max: crate::device_aliases::SCHEMA_VERSION,
        written_schema: crate::device_aliases::SCHEMA_VERSION,
        migration_mode: BACKWARD_COMPATIBLE,
    },
    // `version`, not `schema_version`; a missing one is schema 1, which is
    // migrated in place.
    DurableStore {
        name: "library",
        storage_format: "json",
        owner: StoreOwner::Daemon,
        root: StoreRoot::Data,
        location: StoreLocation::File("library.json"),
        legacy_data_file: None,
        schema: StoreSchema::JsonField {
            field: "version",
            missing: Some(crate::library::LEGACY_LIBRARY_SCHEMA_VERSION),
        },
        readable_schema_min: crate::library::LEGACY_LIBRARY_SCHEMA_VERSION,
        readable_schema_max: crate::library::LIBRARY_SCHEMA_VERSION,
        written_schema: crate::library::LIBRARY_SCHEMA_VERSION,
        migration_mode: BACKWARD_COMPATIBLE,
    },
    DurableStore {
        name: "device-binding-journal",
        storage_format: "json",
        owner: StoreOwner::Daemon,
        root: StoreRoot::State,
        location: StoreLocation::File(
            crate::domain::device_binding::DEVICE_BINDING_MIGRATION_JOURNAL_FILE,
        ),
        legacy_data_file: None,
        schema: StoreSchema::JsonField {
            field: "schema_version",
            missing: None,
        },
        readable_schema_min: crate::device_binding_journal::DEVICE_BINDING_JOURNAL_SCHEMA_VERSION,
        readable_schema_max: crate::device_binding_journal::DEVICE_BINDING_JOURNAL_SCHEMA_VERSION,
        written_schema: crate::device_binding_journal::DEVICE_BINDING_JOURNAL_SCHEMA_VERSION,
        migration_mode: BACKWARD_COMPATIBLE,
    },
    unversioned(
        "cli-config",
        "toml",
        StoreOwner::Client,
        StoreRoot::Config,
        StoreLocation::File("cli.toml"),
    ),
    unversioned(
        "tui-config",
        "toml",
        StoreOwner::Client,
        StoreRoot::Config,
        StoreLocation::File("tui.toml"),
    ),
    unversioned(
        "app-servers",
        "toml",
        StoreOwner::Client,
        StoreRoot::Config,
        StoreLocation::File("servers.toml"),
    ),
    unversioned(
        "app-first-run",
        "marker",
        StoreOwner::Client,
        StoreRoot::Data,
        StoreLocation::File("first-run-complete"),
    ),
    unversioned(
        "openrgb-config",
        "json",
        StoreOwner::Client,
        StoreRoot::Data,
        StoreLocation::File("openrgb/OpenRGB.json"),
    ),
];

const fn unversioned(
    name: &'static str,
    storage_format: &'static str,
    owner: StoreOwner,
    root: StoreRoot,
    location: StoreLocation,
) -> DurableStore {
    DurableStore {
        name,
        storage_format,
        owner,
        root,
        location,
        legacy_data_file: None,
        schema: StoreSchema::Unversioned,
        readable_schema_min: 0,
        readable_schema_max: 0,
        written_schema: 0,
        migration_mode: BACKWARD_COMPATIBLE,
    }
}

/// What one store held on disk before the daemon opened it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum StoreFound {
    /// Nothing on disk a reader has to understand.
    Absent,
    /// Data at this schema; for a store of records, the newest record's.
    Schema(u32),
    /// Data whose schema could not be determined.
    Unreadable(String),
}

/// The schema one store found on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DurableStoreObservation {
    pub name: &'static str,
    pub found: StoreFound,
}

/// The schema every durable store found on disk when the daemon started.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DurableStoreReport {
    pub observations: Vec<DurableStoreObservation>,
}

impl DurableStoreReport {
    /// What one store found, when the report covers it.
    #[must_use]
    pub fn found(&self, name: &str) -> Option<&StoreFound> {
        self.observations
            .iter()
            .find(|observation| observation.name == name)
            .map(|observation| &observation.found)
    }
}

/// The directories and main configuration file a probe reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreRoots {
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
    pub config_file: PathBuf,
}

impl StoreRoots {
    fn path(&self, root: StoreRoot, relative: &str) -> PathBuf {
        match root {
            StoreRoot::Config => self.config.join(relative),
            StoreRoot::Data => self.data.join(relative),
            StoreRoot::State => self.state.join(relative),
        }
    }
}

/// Read the schema every store in [`DURABLE_STORES`] finds on disk.
///
/// Reads only; never creates, repairs or migrates anything, so it must run
/// before the stores open and rewrite their files.
#[must_use]
pub fn probe_durable_stores(roots: &StoreRoots) -> DurableStoreReport {
    DurableStoreReport {
        observations: DURABLE_STORES
            .iter()
            .map(|store| DurableStoreObservation {
                name: store.name,
                found: probe_store(store, roots),
            })
            .collect(),
    }
}

fn probe_store(store: &DurableStore, roots: &StoreRoots) -> StoreFound {
    let path = match store.location {
        StoreLocation::ConfigFile => roots.config_file.clone(),
        StoreLocation::File(relative) | StoreLocation::Directory(relative) => {
            roots.path(store.root, relative)
        }
    };
    let path = match store.legacy_data_file {
        Some(legacy) if !path.exists() => roots.data.join(legacy),
        _ => path,
    };
    match store.location {
        StoreLocation::Directory(_) => probe_directory(&path, store.schema),
        StoreLocation::File(_) | StoreLocation::ConfigFile => probe_file(&path, store.schema),
    }
}

fn probe_file(path: &Path, schema: StoreSchema) -> StoreFound {
    let bytes = match read_bounded(path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return StoreFound::Absent,
        Err(error) => return StoreFound::Unreadable(error),
    };
    match schema {
        StoreSchema::Unversioned => StoreFound::Schema(0),
        StoreSchema::TomlField { field } => match toml_document(&bytes) {
            Ok(document) => toml_version(document.get(field), None, field)
                .unwrap_or_else(StoreFound::Unreadable),
            Err(error) => StoreFound::Unreadable(error),
        },
        StoreSchema::JsonField { field, missing } => {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(Value::Object(fields)) if fields.is_empty() => StoreFound::Absent,
                Ok(Value::Object(fields)) => json_version(fields.get(field), missing, field)
                    .unwrap_or_else(StoreFound::Unreadable),
                Ok(_) => StoreFound::Unreadable("the store is not a JSON object".to_owned()),
                Err(error) => StoreFound::Unreadable(error.to_string()),
            }
        }
        StoreSchema::JsonArrayRecords { field } => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Array(records)) => newest(
                records
                    .iter()
                    .map(|record| json_version(record.get(field), None, field)),
            ),
            Ok(_) => StoreFound::Unreadable("the store is not a JSON array".to_owned()),
            Err(error) => StoreFound::Unreadable(error.to_string()),
        },
        StoreSchema::JsonMapRecords { field, missing } => {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(Value::Object(records)) => newest(
                    records
                        .values()
                        .map(|record| json_version(record.get(field), missing, field)),
                ),
                Ok(_) => StoreFound::Unreadable("the store is not a JSON object".to_owned()),
                Err(error) => StoreFound::Unreadable(error.to_string()),
            }
        }
        StoreSchema::TomlDirectoryRecords { .. } => {
            StoreFound::Unreadable("a directory store was declared as one file".to_owned())
        }
    }
}

fn probe_directory(path: &Path, schema: StoreSchema) -> StoreFound {
    let files = match directory_files(path) {
        Ok(Some(files)) => files,
        Ok(None) => return StoreFound::Absent,
        Err(error) => return StoreFound::Unreadable(error),
    };
    match schema {
        StoreSchema::Unversioned if files.is_empty() => StoreFound::Absent,
        StoreSchema::Unversioned => StoreFound::Schema(0),
        StoreSchema::TomlDirectoryRecords { field, missing } => newest(
            files
                .iter()
                .filter(|file| {
                    file.extension()
                        .is_some_and(|extension| extension == "toml")
                })
                .map(|file| match read_bounded(file) {
                    Ok(Some(bytes)) => toml_document(&bytes)
                        .and_then(|document| toml_version(document.get(field), missing, field)),
                    Ok(None) => Ok(StoreFound::Absent),
                    Err(error) => Err(error),
                }),
        ),
        StoreSchema::JsonField { .. }
        | StoreSchema::TomlField { .. }
        | StoreSchema::JsonArrayRecords { .. }
        | StoreSchema::JsonMapRecords { .. } => {
            StoreFound::Unreadable("a file store was declared as a directory".to_owned())
        }
    }
}

/// The newest schema among records; any unreadable record makes the whole
/// store unreadable, since its reader could not load it either.
fn newest(records: impl Iterator<Item = Result<StoreFound, String>>) -> StoreFound {
    let mut newest = None;
    for record in records {
        match record {
            Ok(StoreFound::Schema(schema)) => {
                newest = Some(newest.map_or(schema, |current: u32| current.max(schema)));
            }
            Ok(StoreFound::Absent) => {}
            Ok(StoreFound::Unreadable(error)) | Err(error) => {
                return StoreFound::Unreadable(error);
            }
        }
    }
    newest.map_or(StoreFound::Absent, StoreFound::Schema)
}

fn json_version(
    value: Option<&Value>,
    missing: Option<u32>,
    field: &str,
) -> Result<StoreFound, String> {
    match value {
        None => missing
            .map(StoreFound::Schema)
            .ok_or_else(|| format!("the store has no {field}")),
        Some(value) => value
            .as_u64()
            .and_then(|version| u32::try_from(version).ok())
            .map(StoreFound::Schema)
            .ok_or_else(|| format!("{field} is not a schema number")),
    }
}

fn toml_version(
    value: Option<&toml::Value>,
    missing: Option<u32>,
    field: &str,
) -> Result<StoreFound, String> {
    match value {
        None => missing
            .map(StoreFound::Schema)
            .ok_or_else(|| format!("the store has no {field}")),
        Some(value) => value
            .as_integer()
            .and_then(|version| u32::try_from(version).ok())
            .map(StoreFound::Schema)
            .ok_or_else(|| format!("{field} is not a schema number")),
    }
}

fn toml_document(bytes: &[u8]) -> Result<toml::Table, String> {
    let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    toml::from_str::<toml::Table>(text).map_err(|error| error.to_string())
}

fn read_bounded(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let mut bytes = Vec::new();
    file.take(MAX_PROBED_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_PROBED_FILE_BYTES {
        return Err(format!(
            "{} exceeds {MAX_PROBED_FILE_BYTES} bytes",
            path.display()
        ));
    }
    Ok(Some(bytes))
}

/// Every regular file beneath `root`, without following symbolic links.
fn directory_files(root: &Path) -> Result<Option<Vec<PathBuf>>, String> {
    match std::fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(format!("{} is not a directory", root.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", root.display())),
    }
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("{}: {error}", directory.display()))?;
            let kind = entry
                .file_type()
                .map_err(|error| format!("{}: {error}", entry.path().display()))?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
                if files.len() > MAX_PROBED_DIRECTORY_FILES {
                    return Err(format!(
                        "{} holds more than {MAX_PROBED_DIRECTORY_FILES} files",
                        root.display()
                    ));
                }
            }
        }
    }
    Ok(Some(files))
}
