//! The managed package contract a Linux release declares in its manifest.
//!
//! A release that the per-user tarball installer manages says, beside its
//! member inventory, which installer owns it, which launcher contract its
//! service runs under, where its required components live, and what every
//! durable store it reads or writes looks like on disk. Nothing here
//! repeats a member hash: a component names a path whose bytes the member
//! inventory already binds, and the package's identity is the manifest's
//! own `name`, `version`, `platform` and `rust_target`, with the SHA-256 of
//! the manifest bytes (the unit ID) as its digest.
//!
//! Every new candidate except a macOS release must carry a complete block,
//! and only a Linux release may carry one. An installed release is read
//! tolerantly: one from before this contract, or one whose block this
//! build cannot read exactly (a newer schema, a field it does not know), is
//! `Undeclared` or `Unrecognized`, never an error, so it still retains and
//! rolls back, and [`evaluate_data_compatibility`] treats its data as
//! manual. Only the block and new top-level fields are tolerated this way;
//! the member inventory, binaries and asset counts stay exact.
//!
//! The installer's own records (the install journal, `installation.json`
//! and the permanent locator) are not durable stores here. Their formats
//! are part of the launcher contract a release declares, so a release that
//! changes them declares another contract.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ReleasePayloadError;
use super::manifest::ValidatedMember;

/// The `managed_package.schema_version` this build reads and writes.
pub const MANAGED_PACKAGE_SCHEMA_VERSION: u32 = 1;
/// The installer that owns a managed per-user Linux tarball.
pub const LINUX_USER_TARBALL_OWNER: &str = "linux-user-tarball";
/// The launcher contract a managed package runs under.
pub const MANAGED_LAUNCHER_CONTRACT: u32 = 1;
/// The most durable stores one release may declare.
pub const MAX_DURABLE_STORES: usize = 64;

const MAX_STORE_NAME_BYTES: usize = 64;
const MAX_STORAGE_FORMAT_BYTES: usize = 32;

/// A logical component every managed package must provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ManagedComponent {
    /// The daemon executable the service runs.
    Daemon,
    /// The CLI, which also carries the installer and the launcher.
    Cli,
    /// The web UI the daemon serves.
    Ui,
    /// The effects bundled with the release.
    BundledEffects,
}

impl ManagedComponent {
    /// Every component, in manifest order.
    pub const ALL: [Self; 4] = [Self::Daemon, Self::Cli, Self::Ui, Self::BundledEffects];

    /// The component's key in `managed_package.components`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::Cli => "cli",
            Self::Ui => "ui",
            Self::BundledEffects => "bundled_effects",
        }
    }

    /// The one release path launcher contract 1 binds the component to.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Daemon => "bin/hypercolor-daemon",
            Self::Cli => "bin/hypercolor",
            Self::Ui => "share/hypercolor/ui",
            Self::BundledEffects => "share/hypercolor/effects/bundled",
        }
    }

    const fn is_executable(self) -> bool {
        matches!(self, Self::Daemon | Self::Cli)
    }
}

/// How a release moves a durable store's data from an older schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationMode {
    /// Writes a schema every release inside its declared reader range can
    /// read; automatic activation and rollback may cross it.
    BackwardCompatible,
    /// Moves data in steps an automatic activation must not take yet.
    Staged,
    /// Needs a person to move the data; never automatic.
    Manual,
}

/// What one release reads and writes in one durable store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DurableStoreDeclaration {
    name: String,
    storage_format: String,
    readable_schema_min: u32,
    readable_schema_max: u32,
    written_schema: u32,
    migration_mode: MigrationMode,
}

impl DurableStoreDeclaration {
    /// The store's stable name, shared by every release that declares it.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The on-disk format family, such as `toml` or `json`.
    #[must_use]
    pub fn storage_format(&self) -> &str {
        &self.storage_format
    }

    /// The oldest schema this release reads.
    #[must_use]
    pub const fn readable_schema_min(&self) -> u32 {
        self.readable_schema_min
    }

    /// The newest schema this release reads.
    #[must_use]
    pub const fn readable_schema_max(&self) -> u32 {
        self.readable_schema_max
    }

    /// The schema this release writes.
    #[must_use]
    pub const fn written_schema(&self) -> u32 {
        self.written_schema
    }

    /// How this release migrates the store.
    #[must_use]
    pub const fn migration_mode(&self) -> MigrationMode {
        self.migration_mode
    }

    /// Whether this release reads data written at `schema`.
    #[must_use]
    pub const fn reads(&self, schema: u32) -> bool {
        self.readable_schema_min <= schema && schema <= self.readable_schema_max
    }
}

/// A release's complete managed package declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPackage {
    launcher_contract: u32,
    stores: Vec<DurableStoreDeclaration>,
    companion_units: Vec<CompanionUnitDeclaration>,
}

/// The most companion units one release may declare.
pub const MAX_COMPANION_UNITS: usize = 8;
/// The largest companion unit template.
pub const MAX_COMPANION_TEMPLATE_BYTES: u64 = 16 * 1024;

/// A systemd user unit the release ships as a template for the installer
/// to render with the installation's recorded paths.
///
/// Companion units belong to the installer contract, like the launcher:
/// they are rendered when an installation publishes its launcher (a first
/// managed install or adoption, a launcher no service has run through yet,
/// or one the user removed) and never by an ordinary install over a settled
/// launcher, so a release cannot change the unit text its own recovery
/// runs under. Every install still checks that its templates render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionUnitDeclaration {
    unit: String,
    template: String,
    enable: bool,
}

impl CompanionUnitDeclaration {
    /// The unit's file name, `hypercolor-<name>.service` or a template
    /// `hypercolor-<name>@.service`.
    #[must_use]
    pub fn unit(&self) -> &str {
        &self.unit
    }

    /// The release path of the template the installer renders.
    #[must_use]
    pub fn template(&self) -> &str {
        &self.template
    }

    /// Whether the installer enables the unit once it is installed.
    #[must_use]
    pub const fn enable(&self) -> bool {
        self.enable
    }
}

impl ManagedPackage {
    /// The launcher contract the release's service runs under.
    #[must_use]
    pub const fn launcher_contract(&self) -> u32 {
        self.launcher_contract
    }

    /// Every durable store the release reads or writes, by name.
    #[must_use]
    pub fn stores(&self) -> &[DurableStoreDeclaration] {
        &self.stores
    }

    /// The declaration for one store, when the release declares it.
    #[must_use]
    pub fn store(&self, name: &str) -> Option<&DurableStoreDeclaration> {
        self.stores.iter().find(|store| store.name == name)
    }

    /// The companion units the release ships, in manifest order.
    #[must_use]
    pub fn companion_units(&self) -> &[CompanionUnitDeclaration] {
        &self.companion_units
    }
}

/// What an installed release says about its durable data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclaredCompatibility {
    /// A complete declaration this build understands.
    Declared(ManagedPackage),
    /// The release predates the managed package contract.
    Undeclared,
    /// The release declares a managed package contract this build cannot
    /// interpret, such as a newer schema.
    Unrecognized {
        /// Why the declaration could not be read.
        reason: String,
    },
}

impl DeclaredCompatibility {
    /// The declaration, when it can be read.
    #[must_use]
    pub const fn declared(&self) -> Option<&ManagedPackage> {
        match self {
            Self::Declared(package) => Some(package),
            Self::Undeclared | Self::Unrecognized { .. } => None,
        }
    }
}

/// Whether a target release may replace the running one without a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompatibilityDecision {
    /// Every durable store stays readable across the change and its
    /// in-transaction rollback.
    Automatic,
    /// A person must decide; each reason names what failed.
    Manual(Vec<CompatibilityRefusal>),
}

/// One reason a data compatibility check refuses automatic activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompatibilityRefusal {
    /// The target release carries no readable declaration.
    TargetUndeclared,
    /// The running release carries no readable declaration.
    RunningUndeclared,
    /// The store holds data now at a schema the target does not read.
    OnDiskUnreadable {
        store: String,
        schema: u32,
        readable_schema_min: u32,
        readable_schema_max: u32,
    },
    /// Data already on disk is outside what the target reads.
    HighWaterUnreadable {
        store: String,
        high_water: u32,
        readable_schema_min: u32,
        readable_schema_max: u32,
    },
    /// The running release could not read what the target writes, so a
    /// rollback to it would be unsafe.
    RollbackUnreadable {
        store: String,
        written_schema: u32,
        readable_schema_min: u32,
        readable_schema_max: u32,
    },
    /// The target migrates a store in a way automatic activation never takes.
    NotBackwardCompatible {
        store: String,
        migration_mode: MigrationMode,
    },
}

impl std::fmt::Display for CompatibilityRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TargetUndeclared => {
                formatter.write_str("the target release declares no durable-data compatibility")
            }
            Self::RunningUndeclared => {
                formatter.write_str("the running release declares no durable-data compatibility")
            }
            Self::OnDiskUnreadable {
                store,
                schema,
                readable_schema_min,
                readable_schema_max,
            } => write!(
                formatter,
                "store {store} holds schema {schema} now, outside the target's \
                 {readable_schema_min}..={readable_schema_max}"
            ),
            Self::HighWaterUnreadable {
                store,
                high_water,
                readable_schema_min,
                readable_schema_max,
            } => write!(
                formatter,
                "store {store} holds schema {high_water}, outside the target's \
                 {readable_schema_min}..={readable_schema_max}"
            ),
            Self::RollbackUnreadable {
                store,
                written_schema,
                readable_schema_min,
                readable_schema_max,
            } => write!(
                formatter,
                "store {store}: the target writes schema {written_schema}, which the running \
                 release reads only in {readable_schema_min}..={readable_schema_max}"
            ),
            Self::NotBackwardCompatible {
                store,
                migration_mode,
            } => write!(
                formatter,
                "store {store} migrates as {migration_mode:?}, never automatically"
            ),
        }
    }
}

/// What is known about the data already in each durable store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObservedStores {
    /// The highest schema any release is known to have written, per store.
    pub high_water: BTreeMap<String, u32>,
    /// The schema each store holds on disk now, per store, as the daemon's
    /// startup report found it. An older schema can outlive the releases
    /// that wrote it (a configuration upgraded in memory is rewritten only
    /// when something saves it), so the high-water mark alone does not
    /// bound it from below.
    pub on_disk: BTreeMap<String, u32>,
}

/// Decide whether `target` may replace `running` without a person.
///
/// The running release's own written schema always counts toward each
/// store's high-water mark, whatever `observed` recorded, since the running
/// release is writing it now. Every condition must hold:
///
/// 1. For every store the target declares that has a high-water mark, the
///    target reads that schema; and it reads the schema the store holds on
///    disk now, when that is known.
/// 2. The running release reads what the target writes, for every store
///    both declare, so the in-transaction rollback to it is safe. The
///    running release's declaration is authoritative about itself.
/// 3. The target migrates every store it declares as backward compatible.
/// 4. Missing or unreadable metadata on either side is manual.
///
/// Any failure is a manual decision listing every reason.
#[must_use]
pub fn evaluate_data_compatibility(
    running: &DeclaredCompatibility,
    target: &DeclaredCompatibility,
    observed: &ObservedStores,
) -> CompatibilityDecision {
    let mut refusals = Vec::new();
    let (Some(running), Some(target)) = (running.declared(), target.declared()) else {
        if target.declared().is_none() {
            refusals.push(CompatibilityRefusal::TargetUndeclared);
        }
        if running.declared().is_none() {
            refusals.push(CompatibilityRefusal::RunningUndeclared);
        }
        return CompatibilityDecision::Manual(refusals);
    };
    for store in target.stores() {
        if let Some(&schema) = observed.on_disk.get(store.name())
            && !store.reads(schema)
        {
            refusals.push(CompatibilityRefusal::OnDiskUnreadable {
                store: store.name().to_owned(),
                schema,
                readable_schema_min: store.readable_schema_min(),
                readable_schema_max: store.readable_schema_max(),
            });
        }
        let recorded = observed.high_water.get(store.name()).copied();
        let running_writes = running
            .store(store.name())
            .map(DurableStoreDeclaration::written_schema);
        if let Some(mark) = recorded.max(running_writes)
            && !store.reads(mark)
        {
            refusals.push(CompatibilityRefusal::HighWaterUnreadable {
                store: store.name().to_owned(),
                high_water: mark,
                readable_schema_min: store.readable_schema_min(),
                readable_schema_max: store.readable_schema_max(),
            });
        }
        if let Some(reader) = running.store(store.name())
            && !reader.reads(store.written_schema())
        {
            refusals.push(CompatibilityRefusal::RollbackUnreadable {
                store: store.name().to_owned(),
                written_schema: store.written_schema(),
                readable_schema_min: reader.readable_schema_min(),
                readable_schema_max: reader.readable_schema_max(),
            });
        }
        if store.migration_mode() != MigrationMode::BackwardCompatible {
            refusals.push(CompatibilityRefusal::NotBackwardCompatible {
                store: store.name().to_owned(),
                migration_mode: store.migration_mode(),
            });
        }
    }
    if refusals.is_empty() {
        CompatibilityDecision::Automatic
    } else {
        CompatibilityDecision::Manual(refusals)
    }
}

/// How strictly a manifest's `managed_package` block is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ManagedPolicy {
    /// A new candidate: a Linux release must carry a complete, current
    /// block, and any block present must be exact.
    Candidate,
    /// An installed release: an absent block is undeclared and one this
    /// build cannot interpret is unrecognized, never an error.
    Installed,
}

/// The managed block, typed and exact. It is parsed from the manifest
/// bytes themselves, so a duplicated key anywhere in it is refused, and
/// every level refuses fields this build does not know.
#[derive(Deserialize)]
struct Envelope {
    managed_package: Option<StrictPackage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictPackage {
    schema_version: u32,
    owner: String,
    launcher_contract: u32,
    components: StrictComponents,
    compatibility: StrictCompatibility,
    #[serde(default)]
    companion_units: Vec<StrictCompanionUnit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictCompanionUnit {
    unit: String,
    template: String,
    enable: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictComponents {
    daemon: String,
    cli: String,
    ui: String,
    bundled_effects: String,
}

impl StrictComponents {
    fn path(&self, component: ManagedComponent) -> &str {
        match component {
            ManagedComponent::Daemon => &self.daemon,
            ManagedComponent::Cli => &self.cli,
            ManagedComponent::Ui => &self.ui,
            ManagedComponent::BundledEffects => &self.bundled_effects,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictCompatibility {
    stores: Vec<RawStore>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStore {
    name: String,
    storage_format: String,
    readable_schema_min: u32,
    readable_schema_max: u32,
    written_schema: u32,
    migration_mode: MigrationMode,
}

/// Read a manifest's `managed_package` block under `policy`.
///
/// `present` is the block as the manifest's field map holds it, `bytes`
/// the manifest itself, and `members` the validated inventory the
/// components must bind to. A candidate must carry a complete, current
/// block unless it is a macOS release (a `macos-*` platform built for an
/// `*-apple-darwin` target), and only a `linux-*` release may carry one.
/// An installed release that declares nothing is undeclared, and one whose
/// block this build cannot read exactly is unrecognized; neither is an
/// error, so the release still retains and rolls back.
pub(super) fn read_managed_package(
    bytes: &[u8],
    present: Option<&Value>,
    platform: &str,
    rust_target: &str,
    policy: ManagedPolicy,
    members: &BTreeMap<String, ValidatedMember>,
) -> Result<DeclaredCompatibility, ReleasePayloadError> {
    let Some(value) = present else {
        let macos = platform.starts_with("macos-") && rust_target.ends_with("-apple-darwin");
        if policy == ManagedPolicy::Candidate && !macos {
            return Err(invalid(format!(
                "a Linux release must declare its managed_package contract \
                 (only a macOS release may omit it; this one is {platform} for {rust_target})"
            )));
        }
        return Ok(DeclaredCompatibility::Undeclared);
    };
    let parsed = parse_package(bytes, value, platform, members);
    match (policy, parsed) {
        (_, Ok(package)) => Ok(DeclaredCompatibility::Declared(package)),
        (ManagedPolicy::Candidate, Err(error)) => Err(error),
        (ManagedPolicy::Installed, Err(error)) => Ok(DeclaredCompatibility::Unrecognized {
            reason: error.to_string(),
        }),
    }
}

fn parse_package(
    bytes: &[u8],
    value: &Value,
    platform: &str,
    members: &BTreeMap<String, ValidatedMember>,
) -> Result<ManagedPackage, ReleasePayloadError> {
    if !platform.starts_with("linux-") {
        return Err(invalid(format!(
            "managed_package is the Linux per-user contract; a {platform} release cannot declare it"
        )));
    }
    let schema = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("managed_package.schema_version must be a whole number"))?;
    if schema != u64::from(MANAGED_PACKAGE_SCHEMA_VERSION) {
        return Err(invalid(format!(
            "managed_package.schema_version {schema} is not the supported {MANAGED_PACKAGE_SCHEMA_VERSION}"
        )));
    }
    let raw = serde_json::from_slice::<Envelope>(bytes)
        .map_err(|source| invalid(format!("managed_package is malformed: {source}")))?
        .managed_package
        .ok_or_else(|| invalid("managed_package is null"))?;
    debug_assert_eq!(raw.schema_version, MANAGED_PACKAGE_SCHEMA_VERSION);
    if raw.owner != LINUX_USER_TARBALL_OWNER {
        return Err(invalid(format!(
            "managed_package.owner {:?} is not {LINUX_USER_TARBALL_OWNER:?}",
            raw.owner
        )));
    }
    if raw.launcher_contract != MANAGED_LAUNCHER_CONTRACT {
        return Err(invalid(format!(
            "managed_package.launcher_contract {} is not the supported {MANAGED_LAUNCHER_CONTRACT}",
            raw.launcher_contract
        )));
    }
    validate_components(&raw.components, members)?;
    let stores = validate_stores(raw.compatibility.stores)?;
    let companion_units = validate_companion_units(raw.companion_units, members)?;
    Ok(ManagedPackage {
        launcher_contract: raw.launcher_contract,
        stores,
        companion_units,
    })
}

fn validate_companion_units(
    raw: Vec<StrictCompanionUnit>,
    members: &BTreeMap<String, ValidatedMember>,
) -> Result<Vec<CompanionUnitDeclaration>, ReleasePayloadError> {
    if raw.len() > MAX_COMPANION_UNITS {
        return Err(invalid(format!(
            "managed_package.companion_units may declare at most {MAX_COMPANION_UNITS} units"
        )));
    }
    let mut seen = BTreeSet::new();
    let mut units = Vec::with_capacity(raw.len());
    for unit in raw {
        let (stem, template) = match unit.unit.strip_suffix("@.service") {
            Some(stem) => (stem, true),
            None => (
                unit.unit.strip_suffix(".service").unwrap_or_default(),
                false,
            ),
        };
        let name = stem.strip_prefix("hypercolor-").unwrap_or_default();
        if !valid_token(name, 48) {
            return Err(invalid(format!(
                "companion unit {:?} must be named hypercolor-<name>.service or \
                 hypercolor-<name>@.service",
                unit.unit
            )));
        }
        if !seen.insert(unit.unit.clone()) {
            return Err(invalid(format!(
                "companion unit {} is declared twice",
                unit.unit
            )));
        }
        if template && unit.enable {
            return Err(invalid(format!(
                "companion unit {} is a template and cannot be enabled without an instance",
                unit.unit
            )));
        }
        match members.get(&unit.template) {
            Some(ValidatedMember::File { size, .. }) if *size <= MAX_COMPANION_TEMPLATE_BYTES => {}
            _ => {
                return Err(invalid(format!(
                    "companion unit {} template {} must be a declared file of at most \
                     {MAX_COMPANION_TEMPLATE_BYTES} bytes",
                    unit.unit, unit.template
                )));
            }
        }
        units.push(CompanionUnitDeclaration {
            unit: unit.unit,
            template: unit.template,
            enable: unit.enable,
        });
    }
    Ok(units)
}

fn validate_components(
    components: &StrictComponents,
    members: &BTreeMap<String, ValidatedMember>,
) -> Result<(), ReleasePayloadError> {
    for component in ManagedComponent::ALL {
        let path = components.path(component);
        if path != component.path() {
            return Err(invalid(format!(
                "managed_package component {} must be {}, not {path}",
                component.name(),
                component.path()
            )));
        }
        let bound = if component.is_executable() {
            matches!(
                members.get(path),
                Some(ValidatedMember::File { source_mode, .. }) if *source_mode == 0o755
            )
        } else {
            let prefix = format!("{path}/");
            members.get(path).is_some_and(ValidatedMember::is_directory)
                && members.iter().any(|(member, entry)| {
                    member.starts_with(&prefix) && matches!(entry, ValidatedMember::File { .. })
                })
        };
        if !bound {
            return Err(invalid(format!(
                "managed_package component {} does not bind a {} in the member inventory",
                component.name(),
                if component.is_executable() {
                    "0755 regular file"
                } else {
                    "directory holding at least one file"
                }
            )));
        }
    }
    Ok(())
}

fn validate_stores(
    raw: Vec<RawStore>,
) -> Result<Vec<DurableStoreDeclaration>, ReleasePayloadError> {
    if raw.is_empty() || raw.len() > MAX_DURABLE_STORES {
        return Err(invalid(format!(
            "managed_package.compatibility.stores must declare 1..={MAX_DURABLE_STORES} stores"
        )));
    }
    let mut seen = BTreeSet::new();
    let mut stores = Vec::with_capacity(raw.len());
    for store in raw {
        if !valid_token(&store.name, MAX_STORE_NAME_BYTES) {
            return Err(invalid(format!(
                "durable store name {:?} must be 1..={MAX_STORE_NAME_BYTES} lowercase letters, digits and '-'",
                store.name
            )));
        }
        if !seen.insert(store.name.clone()) {
            return Err(invalid(format!(
                "durable store {} is declared twice",
                store.name
            )));
        }
        if !valid_token(&store.storage_format, MAX_STORAGE_FORMAT_BYTES) {
            return Err(invalid(format!(
                "durable store {} has an invalid storage_format",
                store.name
            )));
        }
        if store.readable_schema_min > store.readable_schema_max
            || !(store.readable_schema_min..=store.readable_schema_max)
                .contains(&store.written_schema)
        {
            return Err(invalid(format!(
                "durable store {} must read the schema it writes: {}..={} does not hold {}",
                store.name,
                store.readable_schema_min,
                store.readable_schema_max,
                store.written_schema
            )));
        }
        stores.push(DurableStoreDeclaration {
            name: store.name,
            storage_format: store.storage_format,
            readable_schema_min: store.readable_schema_min,
            readable_schema_max: store.readable_schema_max,
            written_schema: store.written_schema,
            migration_mode: store.migration_mode,
        });
    }
    Ok(stores)
}

fn valid_token(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}

fn invalid(detail: impl Into<String>) -> ReleasePayloadError {
    ReleasePayloadError::InvalidManifest(detail.into())
}
