//! Detection of RGB software that competes with Hypercolor for devices.
//!
//! Vendor suites and other lighting tools hold USB interfaces, drive the
//! same HID collections, or issue SMBus transactions while Hypercolor runs.
//! The symptom is a device that is discovered but never lights, or one
//! that flaps between connected and failed. Nothing in a device error says
//! "SignalRGB is still running in the tray", so this module looks for the
//! culprit directly: platform crates report a [`HostSoftwareSnapshot`], and
//! [`SoftwareCatalog`] matches it against the embedded `catalog.toml`.
//!
//! [`SoftwareConflictStore`] keeps the latest result for the API, the
//! diagnostics report, and device-failure hints, and publishes
//! [`HypercolorEvent::SoftwareConflictsChanged`] when the set changes.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, PoisonError, RwLock};
use std::time::{Duration, Instant};

use hypercolor_types::api::system::{SoftwareConflict, SoftwareConflictsStatus};
use hypercolor_types::event::HypercolorEvent;
use hypercolor_types::host_software::{HostInventory, HostProcess, HostSoftwareSnapshot};
use serde::Deserialize;
use tokio::sync::Notify;

use crate::bus::HypercolorBus;

/// The catalog compiled into the daemon.
const BUILTIN_CATALOG: &str = include_str!("catalog.toml");

static BUILTIN: LazyLock<SoftwareCatalog> = LazyLock::new(|| {
    SoftwareCatalog::from_toml(BUILTIN_CATALOG)
        .expect("the embedded conflicting-software catalog parses; a test covers it")
});

/// Programs launched through an interpreter report the interpreter as
/// their process name; the script is the next command-line word.
const INTERPRETERS: &[&str] = &[
    "python", "node", "perl", "ruby", "java", "javaw", "dotnet", "mono", "bash", "sh",
];

/// One competing program, as the catalog describes it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoftwareSpec {
    /// Stable id, used by clients to key dismissals.
    pub id: String,
    /// Product name for display.
    pub name: String,
    /// Process names to look for. Matching ignores case and a trailing
    /// `.exe`, so one entry covers a Windows image and a Linux binary.
    #[serde(default)]
    pub processes: Vec<String>,
    /// Windows service names (the short name, not the display name).
    #[serde(default)]
    pub services: Vec<String>,
    /// Hypercolor driver ids whose devices it holds or fights over.
    #[serde(default)]
    pub drivers: Vec<String>,
    /// Competes with every driver.
    #[serde(default)]
    pub all_drivers: bool,
    /// Drives SMBus lighting (motherboard, RAM, GPU).
    #[serde(default)]
    pub smbus: bool,
    /// What the user should do about it.
    pub remedy: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    software: Vec<SoftwareSpec>,
}

/// The programs Hypercolor knows to compete with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftwareCatalog {
    entries: Vec<SoftwareSpec>,
}

/// Why a catalog failed to load.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// The TOML did not parse into the catalog shape.
    #[error("conflicting-software catalog does not parse: {0}")]
    Parse(#[from] toml::de::Error),
    /// Two entries share an id.
    #[error("conflicting-software catalog repeats the id `{0}`")]
    DuplicateId(String),
    /// An entry can never match anything.
    #[error("conflicting-software entry `{0}` lists no processes or services")]
    Unmatchable(String),
    /// An entry claims no devices at all.
    #[error("conflicting-software entry `{0}` names no drivers and does not touch SMBus")]
    NoScope(String),
}

impl SoftwareCatalog {
    /// The catalog compiled into this build.
    #[must_use]
    pub fn builtin() -> &'static Self {
        &BUILTIN
    }

    /// Parse and validate a catalog.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] when the TOML does not parse, an id repeats,
    /// an entry lists nothing to match, or an entry claims no devices.
    pub fn from_toml(source: &str) -> Result<Self, CatalogError> {
        let file: CatalogFile = toml::from_str(source)?;
        let mut ids = BTreeSet::new();
        for entry in &file.software {
            if !ids.insert(entry.id.as_str()) {
                return Err(CatalogError::DuplicateId(entry.id.clone()));
            }
            if entry.processes.is_empty() && entry.services.is_empty() {
                return Err(CatalogError::Unmatchable(entry.id.clone()));
            }
            if entry.drivers.is_empty() && !entry.all_drivers && !entry.smbus {
                return Err(CatalogError::NoScope(entry.id.clone()));
            }
        }
        Ok(Self {
            entries: file.software,
        })
    }

    /// Every entry, in catalog order.
    #[must_use]
    pub fn entries(&self) -> &[SoftwareSpec] {
        &self.entries
    }

    /// Every driver id any entry names.
    #[must_use]
    pub fn driver_ids(&self) -> BTreeSet<&str> {
        self.entries
            .iter()
            .flat_map(|entry| entry.drivers.iter().map(String::as_str))
            .collect()
    }

    /// The entries running in `snapshot`, in catalog order.
    #[must_use]
    pub fn detect(&self, snapshot: &HostSoftwareSnapshot) -> Vec<SoftwareConflict> {
        let processes: Vec<(&HostProcess, BTreeSet<String>)> = snapshot
            .processes
            .iter()
            .map(|process| (process, process_keys(process)))
            .collect();
        let services: Vec<(&str, String)> = snapshot
            .services
            .iter()
            .map(|service| (service.as_str(), service.trim().to_ascii_lowercase()))
            .collect();

        self.entries
            .iter()
            .filter_map(|entry| {
                let wanted: BTreeSet<String> = entry
                    .processes
                    .iter()
                    .map(|name| normalize_name(name))
                    .collect();
                let mut matched = BTreeSet::new();
                for (process, keys) in &processes {
                    if !keys.is_disjoint(&wanted) {
                        matched.insert(process.name.clone());
                    }
                }
                for (original, key) in &services {
                    if entry
                        .services
                        .iter()
                        .any(|service| service.trim().eq_ignore_ascii_case(key))
                    {
                        matched.insert((*original).to_owned());
                    }
                }
                (!matched.is_empty()).then(|| SoftwareConflict {
                    id: entry.id.clone(),
                    name: entry.name.clone(),
                    matched: matched.into_iter().collect(),
                    driver_ids: if entry.all_drivers {
                        Vec::new()
                    } else {
                        entry.drivers.clone()
                    },
                    all_drivers: entry.all_drivers,
                    smbus: entry.smbus,
                    remedy: entry.remedy.clone(),
                })
            })
            .collect()
    }
}

/// Lowercase, trimmed, without a trailing `.exe`.
fn normalize_name(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    match lower.strip_suffix(".exe") {
        Some(stem) => stem.to_owned(),
        None => lower,
    }
}

/// The last path component of a command-line word.
fn file_name(word: &str) -> &str {
    word.rsplit(['/', '\\']).next().unwrap_or(word)
}

/// The leading words of a command line, honoring double quotes so a
/// Windows path with spaces stays one word.
fn command_words(command_line: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut rest = command_line.trim_start();
    while !rest.is_empty() && words.len() < 2 {
        let (word, tail) = if let Some(quoted) = rest.strip_prefix('"') {
            match quoted.find('"') {
                Some(end) => (&quoted[..end], &quoted[end + 1..]),
                None => (quoted, ""),
            }
        } else {
            match rest.find(char::is_whitespace) {
                Some(end) => (&rest[..end], &rest[end..]),
                None => (rest, ""),
            }
        };
        if !word.is_empty() {
            words.push(word);
        }
        rest = tail.trim_start();
    }
    words
}

/// Every name a process answers to: its reported name, the program it
/// was launched as, and the script an interpreter is running.
fn process_keys(process: &HostProcess) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let name = normalize_name(&process.name);
    if !name.is_empty() {
        keys.insert(name);
    }
    if let Some(command_line) = &process.command_line {
        let words = command_words(command_line);
        if let Some(program) = words.first().map(|word| normalize_name(file_name(word))) {
            if is_interpreter(&program)
                && let Some(script) = words.get(1)
            {
                let script = normalize_name(file_name(script));
                if !script.is_empty() && !script.starts_with('-') {
                    keys.insert(script);
                }
            }
            if !program.is_empty() {
                keys.insert(program);
            }
        }
    }
    keys
}

/// Entries of `from` whose id is missing in `other`.
fn absent_from(from: &[SoftwareConflict], other: &[SoftwareConflict]) -> Vec<SoftwareConflict> {
    from.iter()
        .filter(|conflict| !other.iter().any(|seen| seen.id == conflict.id))
        .cloned()
        .collect()
}

/// `python3`, `python3.12`, and `node` are interpreters; `shotwell` is not.
fn is_interpreter(program: &str) -> bool {
    INTERPRETERS.iter().any(|interpreter| {
        program.strip_prefix(interpreter).is_some_and(|version| {
            version
                .chars()
                .all(|character| character.is_ascii_digit() || character == '.')
        })
    })
}

/// What one scan changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConflictChanges {
    /// Programs running now that were not before.
    pub appeared: Vec<SoftwareConflict>,
    /// Programs that were running and are gone.
    pub cleared: Vec<SoftwareConflict>,
}

impl ConflictChanges {
    /// Whether the scan changed nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.appeared.is_empty() && self.cleared.is_empty()
    }
}

/// Where a scan sits in the order scans began. Scans run concurrently
/// (the watch, `POST /scan`, diagnostics, a failing device), and a slow
/// one must not overwrite the result of one that began after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScanTicket(u64);

#[derive(Debug, Default)]
struct ConflictRecord {
    status: SoftwareConflictsStatus,
    /// The newest ticket recorded so far.
    last_ticket: u64,
    /// When the latest successful scan was recorded.
    recorded_at: Option<Instant>,
}

/// Shared record of the latest conflict scan.
///
/// Cloning shares one record, so the scan loop, the API, diagnostics, and
/// lifecycle hints all read the same result.
#[derive(Clone)]
pub struct SoftwareConflictStore {
    inner: Arc<RwLock<ConflictRecord>>,
    tickets: Arc<AtomicU64>,
    catalog: &'static SoftwareCatalog,
    event_bus: Option<Arc<HypercolorBus>>,
    scan_requests: Arc<Notify>,
}

impl Default for SoftwareConflictStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SoftwareConflictStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoftwareConflictStore")
            .field("conflicts", &self.read().status.conflicts.len())
            .finish_non_exhaustive()
    }
}

impl SoftwareConflictStore {
    /// A store over the built-in catalog that publishes nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::with_catalog(SoftwareCatalog::builtin())
    }

    /// A store over a specific catalog.
    #[must_use]
    pub fn with_catalog(catalog: &'static SoftwareCatalog) -> Self {
        Self {
            inner: Arc::default(),
            tickets: Arc::default(),
            catalog,
            event_bus: None,
            scan_requests: Arc::default(),
        }
    }

    /// Publish [`HypercolorEvent::SoftwareConflictsChanged`] on `bus`
    /// whenever the set of running conflicts changes.
    #[must_use]
    pub fn with_event_bus(mut self, bus: Arc<HypercolorBus>) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// Take a ticket before reading the host inventory.
    #[must_use]
    pub fn begin_scan(&self) -> ScanTicket {
        ScanTicket(self.tickets.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// Record the inventory a ticketed scan produced, and report which
    /// programs appeared or cleared.
    ///
    /// A scan that began before the newest recorded one is dropped. A
    /// failed inventory keeps the last known conflicts and marks the
    /// status failed instead of claiming nothing is running.
    pub fn finish_scan(&self, ticket: ScanTicket, inventory: &HostInventory) -> ConflictChanges {
        let detected = match inventory {
            HostInventory::Listed(snapshot) => Some(self.catalog.detect(snapshot)),
            HostInventory::Unsupported => Some(Vec::new()),
            HostInventory::Failed => None,
        };
        let (changes, published) = {
            let mut record = self.write();
            if ticket.0 <= record.last_ticket {
                return ConflictChanges::default();
            }
            record.last_ticket = ticket.0;
            let Some(conflicts) = detected else {
                record.status.supported = true;
                record.status.scanned = true;
                record.status.scan_failed = true;
                return ConflictChanges::default();
            };
            let changes = ConflictChanges {
                appeared: absent_from(&conflicts, &record.status.conflicts),
                cleared: absent_from(&record.status.conflicts, &conflicts),
            };
            let published = (conflicts != record.status.conflicts).then_some(conflicts.len());
            record.status = SoftwareConflictsStatus {
                supported: !matches!(inventory, HostInventory::Unsupported),
                scanned: true,
                scan_failed: false,
                conflicts,
            };
            record.recorded_at = Some(Instant::now());
            (changes, published)
        };
        if let (Some(count), Some(bus)) = (published, self.event_bus.as_ref()) {
            bus.publish(HypercolorEvent::SoftwareConflictsChanged { count });
        }
        changes
    }

    /// Record an inventory taken outside the ticketed scan path, as tests
    /// and one-off callers do.
    pub fn record(&self, inventory: &HostInventory) -> ConflictChanges {
        let ticket = self.begin_scan();
        self.finish_scan(ticket, inventory)
    }

    /// How long ago the latest successful scan was recorded. `None` until
    /// one has been.
    #[must_use]
    pub fn age(&self) -> Option<Duration> {
        self.read().recorded_at.map(|at| at.elapsed())
    }

    /// Ask whoever runs scans for a fresh one, for example after a device
    /// failed to open. Requests made before the scanner waits again
    /// coalesce into one scan.
    pub fn request_scan(&self) {
        self.scan_requests.notify_one();
    }

    /// Resolve when [`Self::request_scan`] has been called.
    pub async fn scan_requested(&self) {
        self.scan_requests.notified().await;
    }

    /// The latest scan result.
    #[must_use]
    pub fn status(&self) -> SoftwareConflictsStatus {
        self.read().status.clone()
    }

    /// Running conflicts that compete for a device of `driver_id`.
    #[must_use]
    pub fn affecting(&self, driver_id: &str, smbus_device: bool) -> Vec<SoftwareConflict> {
        self.read()
            .status
            .conflicts
            .iter()
            .filter(|conflict| conflict.affects(driver_id, smbus_device))
            .cloned()
            .collect()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, ConflictRecord> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, ConflictRecord> {
        self.inner.write().unwrap_or_else(PoisonError::into_inner)
    }
}
