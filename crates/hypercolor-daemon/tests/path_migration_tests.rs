use std::path::{Path, PathBuf};

use hypercolor_daemon::path_migration::{
    LegacyDisposition, MigratedStore, MigrationOutcome, PathMigrationEntry, PathMigrationError,
    PathMigrationTable, RawDocument, RawJsonRelocation, UNVERSIONED, VersionedDocument, migrate,
};
use hypercolor_daemon::persistence::AtomicFileWriter;
use tempfile::TempDir;

const SUBJECT: &str = "device settings";

struct Tiers {
    _tempdir: TempDir,
    legacy: PathBuf,
    canonical: PathBuf,
}

impl Tiers {
    fn new() -> Self {
        let tempdir = TempDir::new().expect("tempdir");
        let legacy_dir = tempdir.path().join("data");
        let canonical_dir = tempdir.path().join("state");
        std::fs::create_dir_all(&legacy_dir).expect("legacy tier");
        std::fs::create_dir_all(&canonical_dir).expect("canonical tier");
        Self {
            legacy: legacy_dir.join("device-settings.json"),
            canonical: canonical_dir.join("device-settings.json"),
            _tempdir: tempdir,
        }
    }

    fn entry(&self) -> PathMigrationEntry {
        PathMigrationEntry::new(SUBJECT, self.legacy.clone(), self.canonical.clone())
    }

    fn writer(&self) -> AtomicFileWriter {
        AtomicFileWriter::new(&self.canonical).expect("canonical writer")
    }

    fn run(&self, entry: &PathMigrationEntry) -> (Option<RawDocument>, MigrationOutcome) {
        let writer = self.writer();
        let migrated =
            migrate(&RawJsonRelocation::new(SUBJECT), entry, &writer).expect("run migration");
        (migrated.document, migrated.outcome)
    }
}

fn versioned(version: u32, marker: &str) -> Vec<u8> {
    format!("{{\n  \"schema_version\": {version},\n  \"marker\": \"{marker}\"\n}}").into_bytes()
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("read stored document")
}

fn backups_beside(path: &Path) -> Vec<PathBuf> {
    let parent = path.parent().expect("parent directory");
    let mut found = std::fs::read_dir(parent)
        .expect("list tier")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|entry| {
            entry
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".migrated-"))
        })
        .collect::<Vec<_>>();
    found.sort();
    found
}

#[test]
fn fresh_install_touches_neither_tier() {
    let tiers = Tiers::new();

    let (document, outcome) = tiers.run(&tiers.entry());

    assert_eq!(outcome, MigrationOutcome::FreshInstall);
    assert!(document.is_none());
    assert!(!tiers.canonical.exists());
    assert!(!tiers.legacy.exists());
    assert!(backups_beside(&tiers.legacy).is_empty());
}

#[test]
fn legacy_only_imports_and_retires_the_old_file() {
    let tiers = Tiers::new();
    let original = versioned(2, "legacy");
    std::fs::write(&tiers.legacy, &original).expect("seed legacy tier");

    let (document, outcome) = tiers.run(&tiers.entry());

    let MigrationOutcome::Imported { backup } = outcome else {
        panic!("expected an import, got {outcome:?}");
    };
    let backup = backup.expect("owned legacy files are backed up");
    assert_eq!(document, Some(RawDocument(original.clone())));
    assert_eq!(read(&tiers.canonical), original);
    assert_eq!(read(&backup), original);
    assert!(
        !tiers.legacy.exists(),
        "the legacy path is retired once the canonical write is durable"
    );
    assert_eq!(backups_beside(&tiers.legacy), vec![backup]);
}

#[test]
fn backup_preserves_unknown_fields_and_formatting_verbatim() {
    let tiers = Tiers::new();
    let original =
        b"{\"schema_version\":4,\"future_key\":{\"keep\":true},\"trailing\":\"  spaced  \"}"
            .to_vec();
    std::fs::write(&tiers.legacy, &original).expect("seed legacy tier");

    let (_, outcome) = tiers.run(&tiers.entry());

    let MigrationOutcome::Imported { backup } = outcome else {
        panic!("expected an import, got {outcome:?}");
    };
    let backup = backup.expect("owned legacy files are backed up");
    assert_eq!(read(&backup), original);
    assert_eq!(read(&tiers.canonical), original);
}

#[test]
fn canonical_only_is_a_no_op() {
    let tiers = Tiers::new();
    let stored = versioned(2, "canonical");
    std::fs::write(&tiers.canonical, &stored).expect("seed canonical tier");

    let (document, outcome) = tiers.run(&tiers.entry());

    assert_eq!(outcome, MigrationOutcome::AlreadyMigrated);
    assert_eq!(document, Some(RawDocument(stored.clone())));
    assert_eq!(read(&tiers.canonical), stored);
    assert!(backups_beside(&tiers.legacy).is_empty());
}

#[test]
fn a_newer_legacy_schema_wins_precedence() {
    let tiers = Tiers::new();
    let legacy = versioned(3, "legacy");
    let canonical = versioned(2, "canonical");
    std::fs::write(&tiers.legacy, &legacy).expect("seed legacy tier");
    std::fs::write(&tiers.canonical, &canonical).expect("seed canonical tier");

    let (document, outcome) = tiers.run(&tiers.entry());

    assert!(matches!(outcome, MigrationOutcome::Imported { .. }));
    assert_eq!(document, Some(RawDocument(legacy.clone())));
    assert_eq!(read(&tiers.canonical), legacy);
}

#[test]
fn a_newer_canonical_schema_wins_precedence() {
    let tiers = Tiers::new();
    let legacy = versioned(2, "legacy");
    let canonical = versioned(3, "canonical");
    std::fs::write(&tiers.legacy, &legacy).expect("seed legacy tier");
    std::fs::write(&tiers.canonical, &canonical).expect("seed canonical tier");

    let (document, outcome) = tiers.run(&tiers.entry());

    assert_eq!(outcome, MigrationOutcome::AlreadyMigrated);
    assert_eq!(document, Some(RawDocument(canonical.clone())));
    assert_eq!(read(&tiers.canonical), canonical);
    assert_eq!(read(&tiers.legacy), legacy);
    assert!(backups_beside(&tiers.legacy).is_empty());
}

#[test]
fn equal_schema_versions_tie_to_the_canonical_path() {
    let tiers = Tiers::new();
    let legacy = versioned(3, "legacy");
    let canonical = versioned(3, "canonical");
    std::fs::write(&tiers.legacy, &legacy).expect("seed legacy tier");
    std::fs::write(&tiers.canonical, &canonical).expect("seed canonical tier");

    let (document, outcome) = tiers.run(&tiers.entry());

    assert_eq!(outcome, MigrationOutcome::AlreadyMigrated);
    assert_eq!(document, Some(RawDocument(canonical.clone())));
    assert_eq!(read(&tiers.canonical), canonical);
}

#[test]
fn an_unversioned_legacy_document_never_outranks_a_versioned_one() {
    let tiers = Tiers::new();
    let legacy = b"{\"marker\":\"legacy\"}".to_vec();
    let canonical = versioned(1, "canonical");
    std::fs::write(&tiers.legacy, &legacy).expect("seed legacy tier");
    std::fs::write(&tiers.canonical, &canonical).expect("seed canonical tier");

    let (document, outcome) = tiers.run(&tiers.entry());

    assert_eq!(outcome, MigrationOutcome::AlreadyMigrated);
    assert_eq!(document, Some(RawDocument(canonical)));
    assert_eq!(read(&tiers.legacy), legacy);
}

#[test]
fn a_partial_temporary_file_from_a_crash_does_not_block_recovery() {
    let tiers = Tiers::new();
    let legacy = versioned(2, "legacy");
    std::fs::write(&tiers.legacy, &legacy).expect("seed legacy tier");
    let stray = tiers.canonical.with_file_name(".tmpCrashedMidMigration");
    std::fs::write(&stray, b"{\"schema_ver").expect("seed partial temporary file");

    let (document, outcome) = tiers.run(&tiers.entry());

    assert!(matches!(outcome, MigrationOutcome::Imported { .. }));
    assert_eq!(document, Some(RawDocument(legacy.clone())));
    assert_eq!(read(&tiers.canonical), legacy);
    assert_eq!(
        read(&stray),
        b"{\"schema_ver",
        "a stray temporary file is inert, never adopted as the canonical document"
    );
}

#[test]
fn a_crash_between_the_canonical_write_and_the_backup_recovers_to_the_new_path() {
    let tiers = Tiers::new();
    let document = versioned(2, "legacy");
    std::fs::write(&tiers.legacy, &document).expect("seed legacy tier");
    std::fs::write(&tiers.canonical, &document).expect("replay the interrupted canonical write");

    let (recovered, outcome) = tiers.run(&tiers.entry());

    assert_eq!(outcome, MigrationOutcome::AlreadyMigrated);
    assert_eq!(recovered, Some(RawDocument(document.clone())));
    assert_eq!(read(&tiers.canonical), document);
    assert_eq!(
        read(&tiers.legacy),
        document,
        "an un-retired legacy file is residue, never data loss"
    );
}

#[test]
fn repeated_runs_are_idempotent() {
    let tiers = Tiers::new();
    let original = versioned(2, "legacy");
    std::fs::write(&tiers.legacy, &original).expect("seed legacy tier");

    let (_, first) = tiers.run(&tiers.entry());
    let (document, second) = tiers.run(&tiers.entry());
    let (_, third) = tiers.run(&tiers.entry());

    assert!(matches!(first, MigrationOutcome::Imported { .. }));
    assert_eq!(second, MigrationOutcome::AlreadyMigrated);
    assert_eq!(third, MigrationOutcome::AlreadyMigrated);
    assert_eq!(document, Some(RawDocument(original.clone())));
    assert_eq!(read(&tiers.canonical), original);
    assert_eq!(
        backups_beside(&tiers.legacy).len(),
        1,
        "the legacy file is retired exactly once"
    );
}

#[test]
fn a_retained_legacy_file_survives_the_import() {
    let tiers = Tiers::new();
    let original = versioned(2, "legacy");
    std::fs::write(&tiers.legacy, &original).expect("seed legacy tier");
    let entry = tiers.entry().retaining_legacy();

    let (document, outcome) = tiers.run(&entry);

    assert_eq!(outcome, MigrationOutcome::Imported { backup: None });
    assert_eq!(document, Some(RawDocument(original.clone())));
    assert_eq!(read(&tiers.canonical), original);
    assert_eq!(read(&tiers.legacy), original);
    assert!(backups_beside(&tiers.legacy).is_empty());

    let (_, second) = tiers.run(&entry);
    assert_eq!(second, MigrationOutcome::AlreadyMigrated);
}

#[test]
fn an_entry_whose_tiers_collapse_relocates_nothing() {
    let tiers = Tiers::new();
    let stored = versioned(2, "canonical");
    std::fs::write(&tiers.canonical, &stored).expect("seed canonical tier");
    let collapsed =
        PathMigrationEntry::new(SUBJECT, tiers.canonical.clone(), tiers.canonical.clone());
    assert!(collapsed.is_noop());

    let (document, outcome) = tiers.run(&collapsed);

    assert_eq!(outcome, MigrationOutcome::AlreadyMigrated);
    assert_eq!(document, Some(RawDocument(stored.clone())));
    assert_eq!(read(&tiers.canonical), stored);
    assert!(backups_beside(&tiers.canonical).is_empty());
}

#[test]
fn an_empty_collapsed_entry_reports_a_fresh_install() {
    let tiers = Tiers::new();
    let collapsed =
        PathMigrationEntry::new(SUBJECT, tiers.canonical.clone(), tiers.canonical.clone());

    let (document, outcome) = tiers.run(&collapsed);

    assert_eq!(outcome, MigrationOutcome::FreshInstall);
    assert!(document.is_none());
}

#[test]
fn a_path_table_resolves_declared_entries_by_subject() {
    let tiers = Tiers::new();
    let table = PathMigrationTable::new()
        .with(tiers.entry())
        .with(PathMigrationEntry::new(
            "driver inventory",
            tiers.legacy.with_file_name("runtime-state.json"),
            tiers.canonical.with_file_name("driver-inventory.json"),
        ));

    assert_eq!(table.entries().len(), 2);
    let resolved = table.entry(SUBJECT).expect("declared entry");
    assert_eq!(resolved.legacy(), tiers.legacy);
    assert_eq!(resolved.canonical(), tiers.canonical);
    assert_eq!(resolved.disposition(), LegacyDisposition::Backup);
    assert!(table.entry("scene library").is_none());
}

#[derive(Debug, thiserror::Error)]
enum RefusingStoreError {
    #[error("the store refused to decode its legacy document")]
    Refused,
    #[error(transparent)]
    Migration(#[from] PathMigrationError),
}

struct RefusingStore;

impl MigratedStore for RefusingStore {
    type Document = RawDocument;
    type Error = RefusingStoreError;

    fn decode_current(
        &self,
        _path: &Path,
    ) -> Result<VersionedDocument<Self::Document>, Self::Error> {
        Ok(VersionedDocument::new(UNVERSIONED, RawDocument(Vec::new())))
    }

    fn decode_legacy(
        &self,
        _path: &Path,
    ) -> Result<Option<VersionedDocument<Self::Document>>, Self::Error> {
        Err(RefusingStoreError::Refused)
    }

    fn encode(&self, document: &Self::Document) -> Result<Vec<u8>, Self::Error> {
        Ok(document.0.clone())
    }
}

#[test]
fn store_decode_errors_surface_without_touching_either_tier() {
    let tiers = Tiers::new();
    let original = versioned(2, "legacy");
    std::fs::write(&tiers.legacy, &original).expect("seed legacy tier");
    let writer = tiers.writer();

    let error = migrate(&RefusingStore, &tiers.entry(), &writer)
        .expect_err("the store's decode error propagates");

    assert!(matches!(error, RefusingStoreError::Refused));
    assert_eq!(read(&tiers.legacy), original);
    assert!(!tiers.canonical.exists());
}

/// Admits a newer generation for the same destination while the harness is
/// encoding, which is the window a concurrent writer would win.
struct SupersedingStore {
    writer: AtomicFileWriter,
    winner: Vec<u8>,
}

impl MigratedStore for SupersedingStore {
    type Document = RawDocument;
    type Error = PathMigrationError;

    fn decode_current(
        &self,
        path: &Path,
    ) -> Result<VersionedDocument<Self::Document>, Self::Error> {
        RawJsonRelocation::new(SUBJECT).decode_current(path)
    }

    fn decode_legacy(
        &self,
        path: &Path,
    ) -> Result<Option<VersionedDocument<Self::Document>>, Self::Error> {
        RawJsonRelocation::new(SUBJECT).decode_legacy(path)
    }

    fn encode(&self, document: &Self::Document) -> Result<Vec<u8>, Self::Error> {
        self.writer
            .write(&self.winner)
            .expect("the competing write lands");
        Ok(document.0.clone())
    }
}

#[test]
fn a_superseded_import_yields_the_winning_canonical_document() {
    let tiers = Tiers::new();
    let legacy = versioned(2, "legacy");
    let winner = versioned(2, "winner");
    std::fs::write(&tiers.legacy, &legacy).expect("seed legacy tier");
    let writer = tiers.writer();
    let store = SupersedingStore {
        writer: writer.clone(),
        winner: winner.clone(),
    };

    let migrated = migrate(&store, &tiers.entry(), &writer).expect("run migration");

    assert_eq!(migrated.outcome, MigrationOutcome::ImportSuperseded);
    assert_eq!(
        migrated.document,
        Some(RawDocument(winner.clone())),
        "the superseded payload is discarded for the winning canonical state"
    );
    assert_eq!(read(&tiers.canonical), winner);
    assert_eq!(
        read(&tiers.legacy),
        legacy,
        "a superseded import never retires the legacy file"
    );
    assert!(backups_beside(&tiers.legacy).is_empty());
}

#[cfg(feature = "persistence-test-hooks")]
mod rollback {
    use std::time::Duration;

    use super::{MigrationOutcome, RawDocument, Tiers, backups_beside, read, versioned};

    #[test]
    fn an_undurable_canonical_write_leaves_the_legacy_file_authoritative() {
        let tiers = Tiers::new();
        let original = versioned(2, "legacy");
        std::fs::write(&tiers.legacy, &original).expect("seed legacy tier");
        let writer = tiers.writer();
        writer.set_injected_replace_failures(usize::MAX);

        let (document, outcome) = tiers.run(&tiers.entry());

        assert_eq!(outcome, MigrationOutcome::ImportRetrying);
        assert_eq!(document, Some(RawDocument(original.clone())));
        assert!(
            !tiers.canonical.exists(),
            "the canonical path stays absent while the write is failing"
        );
        assert_eq!(
            read(&tiers.legacy),
            original,
            "the legacy file is untouched until the canonical write is durable"
        );
        assert!(backups_beside(&tiers.legacy).is_empty());

        writer.set_injected_replace_failures(0);
        writer.kick();
        writer
            .flush(Duration::from_secs(5))
            .expect("the retained payload converges");
        assert_eq!(read(&tiers.canonical), original);

        let (recovered, second) = tiers.run(&tiers.entry());
        assert_eq!(second, MigrationOutcome::AlreadyMigrated);
        assert_eq!(recovered, Some(RawDocument(original)));
    }
}
