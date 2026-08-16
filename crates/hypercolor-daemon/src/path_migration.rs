//! One-time relocation of daemon-owned persisted files between storage tiers.
//!
//! A store declares its old→new path table, hands this harness a codec for its
//! own document, and receives back the authoritative document plus a receipt
//! describing what happened to the legacy file. The harness owns the policy
//! (precedence, ordering, backup, idempotence) and the store owns the codec.
//!
//! The contract every migration honors:
//!
//! - **Declarative table.** Paths arrive as [`PathMigrationEntry`] values, never
//!   as literals baked into the migration engine.
//! - **Precedence.** When both paths hold a document, the newer schema version
//!   wins and ties go to the canonical path.
//! - **One-time import with a backup.** A legacy file the store owns is moved
//!   aside to a timestamped sibling once, on the run that imports it.
//! - **Atomic copy and fsync ordering.** The canonical write goes through
//!   [`AtomicFileWriter`]'s two-phase path, so a crash at any point leaves one
//!   readable state: legacy intact, canonical intact, or both.
//! - **Restart idempotence.** A second run over a migrated pair is a no-op.
//! - **Rollback.** The legacy file is untouched until the canonical write is
//!   proven durable, so an interrupted migration leaves the old daemon readable.
//!
//! # Un-retired legacy files are residue, by design
//!
//! Retirement happens only on the run that imports, and only after that run's
//! canonical write is durable. A crash in the window between those two steps
//! therefore leaves the legacy file in place with no backup taken, and the next
//! run resolves precedence in the canonical document's favor and reports
//! [`MigrationOutcome::AlreadyMigrated`] without retrying the retirement.
//!
//! That residue is the intended contract, not an unfinished backup step. Both
//! copies are readable, the canonical path stays authoritative, and no data is
//! lost. The alternative, retiring the legacy file on any run where the
//! canonical side wins, would turn an otherwise read-only outcome into a
//! mutating one, so the harness leaves the stale file for an operator to remove
//! and logs it at debug level instead.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::persistence::{AtomicFileWriter, AtomicWriteCommitResult};

/// Schema version reported by legacy homes that predate versioned documents.
///
/// Pinning a legacy home to the floor makes any versioned canonical document
/// outrank it, which is the precedence a pure relocation wants.
pub const UNVERSIONED: u32 = 0;

/// Errors produced while relocating a persisted file between storage tiers.
#[derive(Debug, thiserror::Error)]
pub enum PathMigrationError {
    /// A migration source could not be read.
    #[error("failed to read {subject} at {path}: {source}")]
    Read {
        /// Store whose file could not be read.
        subject: &'static str,
        /// Unreadable path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The legacy file could not be moved aside after a durable import.
    #[error("failed to back up legacy {subject} from {path} to {backup}: {source}")]
    Backup {
        /// Store whose legacy file could not be retired.
        subject: &'static str,
        /// Legacy path.
        path: PathBuf,
        /// Reserved backup path.
        backup: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
}

/// A decoded store document paired with the schema version it declared on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionedDocument<D> {
    /// Schema version read from the stored document.
    pub version: u32,
    /// The decoded document.
    pub document: D,
}

impl<D> VersionedDocument<D> {
    /// Pair a document with the schema version it declared.
    pub const fn new(version: u32, document: D) -> Self {
        Self { version, document }
    }

    /// Pair a document from a home that predates schema versioning.
    pub const fn unversioned(document: D) -> Self {
        Self::new(UNVERSIONED, document)
    }
}

/// What happens to the legacy file once the canonical path holds a durable copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyDisposition {
    /// Move the legacy file aside to a timestamped sibling backup.
    Backup,
    /// Leave the legacy file in place because another store still owns it.
    Retain,
}

/// One declarative old→new entry in a storage-tier path table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMigrationEntry {
    subject: &'static str,
    legacy: PathBuf,
    canonical: PathBuf,
    disposition: LegacyDisposition,
}

impl PathMigrationEntry {
    /// Declare a relocation whose legacy file this store owns outright.
    #[must_use]
    pub const fn new(subject: &'static str, legacy: PathBuf, canonical: PathBuf) -> Self {
        Self {
            subject,
            legacy,
            canonical,
            disposition: LegacyDisposition::Backup,
        }
    }

    /// Leave the legacy file in place after the import.
    ///
    /// Stores whose legacy home is a file another store still owns cannot
    /// retire it; the import reads a projection out and leaves the file alone.
    #[must_use]
    pub const fn retaining_legacy(mut self) -> Self {
        self.disposition = LegacyDisposition::Retain;
        self
    }

    /// The store this entry relocates.
    #[must_use]
    pub const fn subject(&self) -> &'static str {
        self.subject
    }

    /// The path the document is migrating away from.
    #[must_use]
    pub fn legacy(&self) -> &Path {
        &self.legacy
    }

    /// The path the document is migrating to.
    #[must_use]
    pub fn canonical(&self) -> &Path {
        &self.canonical
    }

    /// What happens to the legacy file after a durable import.
    #[must_use]
    pub const fn disposition(&self) -> LegacyDisposition {
        self.disposition
    }

    /// Whether both sides of this entry resolve to the same file.
    ///
    /// Tier moves that collapse on some platforms declare the same path twice
    /// rather than dropping the entry, so the harness treats the pair as a
    /// store that has nothing to relocate.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.legacy == self.canonical
    }
}

/// A declarative per-file old→new path table for one storage-tier move.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathMigrationTable {
    entries: Vec<PathMigrationEntry>,
}

impl PathMigrationTable {
    /// Start an empty table.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Declare one store's relocation.
    #[must_use]
    pub fn with(mut self, entry: PathMigrationEntry) -> Self {
        self.entries.push(entry);
        self
    }

    /// Every declared relocation.
    #[must_use]
    pub fn entries(&self) -> &[PathMigrationEntry] {
        &self.entries
    }

    /// Look one store's relocation up by subject.
    #[must_use]
    pub fn entry(&self, subject: &str) -> Option<&PathMigrationEntry> {
        self.entries.iter().find(|entry| entry.subject == subject)
    }
}

/// Where the authoritative document came from and what became of the legacy file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// Neither path held a document, so the store starts empty.
    FreshInstall,
    /// The canonical path already held the winning document.
    AlreadyMigrated,
    /// The legacy document was imported and the canonical path is durable.
    Imported {
        /// The retired legacy file, absent when the legacy file is retained.
        backup: Option<PathBuf>,
    },
    /// The legacy document is live in memory but its canonical write has not
    /// proven durable. The retry supervisor still owns the payload and the
    /// legacy file is untouched, so an interrupted run re-imports on restart.
    ImportRetrying,
    /// A concurrent writer admitted newer bytes for the canonical destination
    /// before this import could replace it, so the imported payload will never
    /// be persisted and must not be trusted. The harness discards it, re-reads
    /// the canonical path, and returns whatever authoritative state it finds
    /// there; the legacy file is not retired.
    ///
    /// Reachable only when the writer is already shared at migration time. The
    /// intended usage runs the migration during store construction, before the
    /// writer reaches any other caller.
    ImportSuperseded,
}

/// The authoritative document produced by one migration run, plus its receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migrated<D> {
    /// The authoritative document, absent only on a fresh install.
    pub document: Option<D>,
    /// Where that document came from.
    pub outcome: MigrationOutcome,
}

impl<D: Default> Migrated<D> {
    /// Take the authoritative document, substituting an empty one for a fresh install.
    #[must_use]
    pub fn into_document_or_default(self) -> D {
        self.document.unwrap_or_default()
    }
}

/// A store's view of its own persisted document, supplied to the harness.
pub trait MigratedStore {
    /// The decoded document both paths hold.
    type Document;
    /// The store's own error type.
    type Error: From<PathMigrationError>;

    /// Decode the document stored at the canonical path, which is known to exist.
    ///
    /// Stores that quarantine corrupt payloads do so here and return whatever
    /// document they choose to start from.
    fn decode_current(&self, path: &Path)
    -> Result<VersionedDocument<Self::Document>, Self::Error>;

    /// Decode the legacy document, translating shape where the legacy home differed.
    ///
    /// `Ok(None)` means the legacy home held nothing worth importing.
    fn decode_legacy(
        &self,
        path: &Path,
    ) -> Result<Option<VersionedDocument<Self::Document>>, Self::Error>;

    /// Encode a document for the canonical path.
    fn encode(&self, document: &Self::Document) -> Result<Vec<u8>, Self::Error>;
}

/// Raw bytes of a relocated document, carried verbatim across the move.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawDocument(pub Vec<u8>);

impl RawDocument {
    /// Borrow the stored bytes.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

/// Byte-preserving relocation for stores whose document shape the move leaves alone.
///
/// The schema version is probed from the document's top-level `schema_version`
/// field. Payloads without one, including files that are not JSON at all, are
/// [`UNVERSIONED`], so unknown fields and hand-edited files survive the move
/// untouched and a versioned canonical document always outranks an
/// unversioned legacy one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawJsonRelocation {
    subject: &'static str,
}

impl RawJsonRelocation {
    /// Relocate `subject`'s file without reinterpreting its contents.
    #[must_use]
    pub const fn new(subject: &'static str) -> Self {
        Self { subject }
    }

    fn read(&self, path: &Path) -> Result<VersionedDocument<RawDocument>, PathMigrationError> {
        let bytes = std::fs::read(path).map_err(|source| PathMigrationError::Read {
            subject: self.subject,
            path: path.to_path_buf(),
            source,
        })?;
        Ok(VersionedDocument::new(
            probe_schema_version(&bytes),
            RawDocument(bytes),
        ))
    }
}

impl MigratedStore for RawJsonRelocation {
    type Document = RawDocument;
    type Error = PathMigrationError;

    fn decode_current(
        &self,
        path: &Path,
    ) -> Result<VersionedDocument<Self::Document>, Self::Error> {
        self.read(path)
    }

    fn decode_legacy(
        &self,
        path: &Path,
    ) -> Result<Option<VersionedDocument<Self::Document>>, Self::Error> {
        self.read(path).map(Some)
    }

    fn encode(&self, document: &Self::Document) -> Result<Vec<u8>, Self::Error> {
        Ok(document.0.clone())
    }
}

#[derive(Deserialize)]
struct SchemaProbe {
    #[serde(default)]
    schema_version: u32,
}

fn probe_schema_version(bytes: &[u8]) -> u32 {
    serde_json::from_slice::<SchemaProbe>(bytes).map_or(UNVERSIONED, |probe| probe.schema_version)
}

/// Resolve one declared relocation and return the authoritative document.
///
/// `writer` must already address `entry.canonical()`; the store keeps it for
/// its own subsequent writes, so the migration and the store share one
/// destination coordinator and one generation sequence.
///
/// # Errors
///
/// Returns the store's error when either side fails to decode or encode, and
/// the harness's own error when the canonical write cannot be prepared or the
/// legacy file cannot be retired after a durable import.
pub fn migrate<S>(
    store: &S,
    entry: &PathMigrationEntry,
    writer: &AtomicFileWriter,
) -> Result<Migrated<S::Document>, S::Error>
where
    S: MigratedStore,
{
    let current = if entry.canonical.exists() {
        Some(store.decode_current(&entry.canonical)?)
    } else {
        None
    };

    if entry.is_noop() {
        return Ok(match current {
            Some(current) => Migrated {
                document: Some(current.document),
                outcome: MigrationOutcome::AlreadyMigrated,
            },
            None => Migrated {
                document: None,
                outcome: MigrationOutcome::FreshInstall,
            },
        });
    }

    let legacy = if entry.legacy.exists() {
        store.decode_legacy(&entry.legacy)?
    } else {
        None
    };

    match (current, legacy) {
        (None, None) => Ok(Migrated {
            document: None,
            outcome: MigrationOutcome::FreshInstall,
        }),
        (None, Some(legacy)) => import(store, entry, writer, legacy),
        (Some(current), None) => Ok(Migrated {
            document: Some(current.document),
            outcome: MigrationOutcome::AlreadyMigrated,
        }),
        (Some(current), Some(legacy)) => {
            if legacy.version > current.version {
                info!(
                    subject = entry.subject,
                    legacy_version = legacy.version,
                    canonical_version = current.version,
                    "Legacy store declares a newer schema and wins precedence"
                );
                return import(store, entry, writer, legacy);
            }
            if entry.disposition == LegacyDisposition::Backup {
                debug!(
                    subject = entry.subject,
                    legacy = %entry.legacy.display(),
                    "Legacy store survives a migration that was interrupted before its backup"
                );
            }
            Ok(Migrated {
                document: Some(current.document),
                outcome: MigrationOutcome::AlreadyMigrated,
            })
        }
    }
}

fn import<S>(
    store: &S,
    entry: &PathMigrationEntry,
    writer: &AtomicFileWriter,
    legacy: VersionedDocument<S::Document>,
) -> Result<Migrated<S::Document>, S::Error>
where
    S: MigratedStore,
{
    // Reserve before encoding so the generation is claimed at the snapshot
    // boundary, matching how the other stores order their writes.
    let reservation = writer.reserve();
    let payload = store.encode(&legacy.document)?;
    match reservation.admit(payload).commit_stage_aware() {
        AtomicWriteCommitResult::DurableWritten => {}
        AtomicWriteCommitResult::Superseded => {
            warn!(
                subject = entry.subject,
                canonical = %entry.canonical.display(),
                legacy = %entry.legacy.display(),
                "Newer bytes superseded the migration import; discarding it for the canonical state"
            );
            let winner = if entry.canonical.exists() {
                Some(store.decode_current(&entry.canonical)?.document)
            } else {
                None
            };
            return Ok(Migrated {
                document: winner,
                outcome: MigrationOutcome::ImportSuperseded,
            });
        }
        ref outcome @ (AtomicWriteCommitResult::FailedBeforeReplacement(ref error)
        | AtomicWriteCommitResult::ReplacementVisibleButNotDurable(ref error)) => {
            warn!(
                subject = entry.subject,
                canonical = %entry.canonical.display(),
                legacy = %entry.legacy.display(),
                ?outcome,
                %error,
                "Migrated store is not durable yet; legacy file stays authoritative for restart"
            );
            return Ok(Migrated {
                document: Some(legacy.document),
                outcome: MigrationOutcome::ImportRetrying,
            });
        }
    }

    let backup = match entry.disposition {
        LegacyDisposition::Retain => None,
        LegacyDisposition::Backup => Some(retire_legacy(entry)?),
    };
    info!(
        subject = entry.subject,
        legacy = %entry.legacy.display(),
        canonical = %entry.canonical.display(),
        backup = ?backup,
        "Migrated store to its canonical storage tier"
    );
    Ok(Migrated {
        document: Some(legacy.document),
        outcome: MigrationOutcome::Imported { backup },
    })
}

fn retire_legacy(entry: &PathMigrationEntry) -> Result<PathBuf, PathMigrationError> {
    let backup = backup_path(&entry.legacy);
    std::fs::rename(&entry.legacy, &backup).map_err(|source| PathMigrationError::Backup {
        subject: entry.subject,
        path: entry.legacy.clone(),
        backup: backup.clone(),
        source,
    })?;

    // The rename is already visible and the canonical document is already
    // durable, so a failed barrier degrades to an un-retired legacy file on the
    // next boot, never to lost data.
    #[cfg(unix)]
    if let Err(source) = sync_legacy_directory(&entry.legacy) {
        warn!(
            subject = entry.subject,
            backup = %backup.display(),
            %source,
            "Legacy backup is visible but its directory entry is not proven durable"
        );
    }
    Ok(backup)
}

#[cfg(unix)]
fn sync_legacy_directory(legacy: &Path) -> Result<(), std::io::Error> {
    let parent = legacy
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::File::open(parent).and_then(|directory| directory.sync_all())
}

fn backup_path(path: &Path) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let mut name = path
        .file_name()
        .map_or_else(|| OsString::from("store"), std::borrow::ToOwned::to_owned);
    name.push(format!(".migrated-{timestamp}"));
    path.with_file_name(name)
}
