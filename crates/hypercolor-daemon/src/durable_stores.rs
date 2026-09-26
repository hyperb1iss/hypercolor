//! Every durable store a release reads or writes, and what each one holds.
//!
//! [`DURABLE_STORES`] is the inventory a Linux release ships as
//! `share/hypercolor/durable-stores.json`: one entry per store, with the
//! schema this release writes, the range it reads and its migration mode.
//! The table also records, for tools that read stores on disk, where each
//! store lives and which field holds its schema; the shipped file carries
//! only the declarations. The values come from the stores themselves, never
//! from the application version:
//!
//! - A store with a version field declares that field's values: the constant
//!   its writer stamps, and the oldest and newest its reader accepts.
//! - A store without one declares schema `0`, which names the only shape it
//!   has ever had. When such a store gains a version field, it starts at `1`,
//!   and a release that reads only `0` does not read it.
//!
//! Nothing in this crate decides anything from the inventory; it is the
//! public description other tools read. `packaging/managed/durable-stores.json`
//! is the copy the release packager ships, and a test holds it equal to this
//! table.

use serde::Serialize;

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
    /// reader refuses such a file. `empty_is_absent` marks a reader that
    /// takes a bare `{}` as no data at all.
    JsonField {
        field: &'static str,
        missing: Option<u32>,
        empty_is_absent: bool,
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
    /// The file's previous home under the data root. The store's migration
    /// reads both and takes whichever holds the newer schema.
    pub legacy_data_file: Option<&'static str>,
    pub schema: StoreSchema,
    pub readable_schema_min: u32,
    pub readable_schema_max: u32,
    pub written_schema: u32,
    /// `backward_compatible`, `staged` or `manual`, as the shipped file
    /// spells it.
    pub migration_mode: &'static str,
}

const BACKWARD_COMPATIBLE: &str = "backward_compatible";
const ATTACHMENT: u32 = hypercolor_types::attachment::CURRENT_ATTACHMENT_SCHEMA_VERSION;

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
    // The index stamps `version`; its reader accepts any value and rebuilds
    // an index it cannot parse from the content-addressed objects. The
    // declaration claims only the version this release writes.
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
            empty_is_absent: false,
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
    // Every layout record carries a required `version`, which every writer
    // sets to 1 (hypercolor_types::spatial::SpatialLayout has no constant).
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
            empty_is_absent: true,
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
            empty_is_absent: false,
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
            missing: Some(ATTACHMENT),
        },
        readable_schema_min: ATTACHMENT,
        readable_schema_max: ATTACHMENT,
        written_schema: ATTACHMENT,
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
            missing: Some(ATTACHMENT),
        },
        readable_schema_min: ATTACHMENT,
        readable_schema_max: ATTACHMENT,
        written_schema: ATTACHMENT,
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
            empty_is_absent: false,
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
            empty_is_absent: false,
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
            empty_is_absent: false,
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
            empty_is_absent: false,
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
