//! Rolling generation history kept by the atomic write path.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use hypercolor_persistence::{
    AtomicFileWriter, AtomicWriteOutcome, HistoryError, HistoryPolicy, capture_next_writes,
    list_generations, restore_generation, set_replacement_observer,
};

const NO_THROTTLE: Duration = Duration::ZERO;
const LONG_THROTTLE: Duration = Duration::from_hours(1);

struct Store {
    _root: tempfile::TempDir,
    path: PathBuf,
    history: PathBuf,
    writer: AtomicFileWriter,
}

impl Store {
    fn new(generations: usize, min_interval: Duration) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("store.json");
        let history = root.path().join("history").join("store");
        let writer = AtomicFileWriter::new(&path).expect("writer");
        writer.enable_history(HistoryPolicy::new(&history, generations, min_interval));
        Self {
            _root: root,
            path,
            history,
            writer,
        }
    }

    fn write(&self, payload: &str) {
        assert_eq!(
            self.writer.write(payload.as_bytes()).expect("write"),
            AtomicWriteOutcome::Written
        );
    }

    fn current(&self) -> String {
        fs::read_to_string(&self.path).expect("store readable")
    }

    fn kept(&self) -> Vec<String> {
        list_generations(&self.history)
            .expect("history lists")
            .iter()
            .map(|generation| fs::read_to_string(&generation.path).expect("generation"))
            .collect()
    }

    fn ids(&self) -> Vec<u64> {
        list_generations(&self.history)
            .expect("history lists")
            .iter()
            .map(|generation| generation.id)
            .collect()
    }
}

#[test]
fn history_never_grows_past_its_generation_budget() {
    let store = Store::new(3, NO_THROTTLE);
    for version in 0..10 {
        store.write(&format!("v{version}"));
        assert!(store.kept().len() <= 3, "history exceeded its budget");
    }

    assert_eq!(store.current(), "v9");
    assert_eq!(store.kept(), ["v6", "v7", "v8"]);
    let ids = store.ids();
    assert!(
        ids.windows(2).all(|pair| pair[0] < pair[1]),
        "ids grow: {ids:?}"
    );
}

#[test]
fn identical_content_is_never_rotated() {
    let store = Store::new(10, NO_THROTTLE);
    store.write("same");
    store.write("same");
    store.write("same");
    assert!(store.kept().is_empty());

    store.write("changed");
    store.write("changed");
    assert_eq!(store.kept(), ["same"]);
}

#[test]
fn content_already_in_the_newest_generation_is_not_copied_again() {
    let store = Store::new(10, NO_THROTTLE);
    store.write("a");
    store.write("b");
    // Writing "a" again retires "b"; the newest generation is then "b", so
    // retiring "b" a second time must not duplicate it.
    store.write("a");
    assert_eq!(store.kept(), ["a", "b"]);
}

#[test]
fn transient_content_is_skipped_and_the_first_write_keeps_the_older_file() {
    let store = Store::new(10, LONG_THROTTLE);
    fs::write(&store.path, "from a previous run").expect("seed");

    // The file predates this process, so its first replacement keeps it.
    store.write("burst 1");
    // Everything written since is younger than the interval: transient.
    store.write("burst 2");
    store.write("burst 3");
    assert_eq!(store.kept(), ["from a previous run"]);

    // Shutdown waives the interval once, then it applies again.
    capture_next_writes();
    store.write("final");
    store.write("after final");
    assert_eq!(store.kept(), ["from a previous run", "burst 3"]);
}

#[test]
fn restore_round_trips_and_is_itself_undoable() {
    let store = Store::new(10, LONG_THROTTLE);
    fs::write(&store.path, "good").expect("seed");
    store.write("bad");
    let good = list_generations(&store.history).expect("history")[0].clone();

    let restored = restore_generation(&store.path, good.id).expect("restore");
    assert_eq!(restored.outcome, AtomicWriteOutcome::Written);
    assert_eq!(store.current(), "good");
    // "bad" was written moments ago, which the throttle would normally call
    // transient; a restore keeps it anyway so it can be undone.
    assert_eq!(store.kept(), ["good", "bad"]);
    let undo = restored.previous_generation.expect("replaced content kept");

    restore_generation(&store.path, undo).expect("undo restore");
    assert_eq!(store.current(), "bad");
}

#[test]
fn restore_rejects_unknown_generations_and_unmanaged_paths() {
    let store = Store::new(10, NO_THROTTLE);
    store.write("a");
    store.write("b");
    assert!(matches!(
        restore_generation(&store.path, 999),
        Err(HistoryError::UnknownGeneration { id: 999, .. })
    ));

    let root = tempfile::tempdir().expect("tempdir");
    let unmanaged = root.path().join("plain.json");
    fs::write(&unmanaged, "x").expect("seed");
    assert!(matches!(
        restore_generation(&unmanaged, 1),
        Err(HistoryError::NotEnabled { .. })
    ));
}

#[test]
fn a_crash_between_rotation_and_replacement_recovers_without_duplicates() {
    let store = Store::new(10, NO_THROTTLE);
    store.write("committed");
    store.write("next");
    // Simulate the crash window after "next" was copied into history but
    // before "newer" replaced it: history already holds the live bytes, and
    // an interrupted copy left a partial file behind.
    fs::write(
        store.history.join("000002-20260926T221014.123Z.json"),
        "next",
    )
    .expect("seed the copied generation");
    fs::write(store.history.join(".partial-crash"), "torn").expect("seed partial");
    assert_eq!(
        store.kept(),
        ["committed", "next"],
        "partials are not listed"
    );

    store.write("newer");
    assert_eq!(
        store.kept(),
        ["committed", "next"],
        "no duplicate of the live bytes"
    );

    store.write("newest");
    assert_eq!(store.kept(), ["committed", "next", "newer"]);
    assert!(
        !store.history.join(".partial-crash").exists(),
        "the next rotation clears partial copies"
    );
}

#[test]
fn a_failed_rotation_never_blocks_the_write() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("store.json");
    let blocked = root.path().join("history-is-a-file");
    fs::write(&blocked, "not a directory").expect("seed blocker");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    writer.enable_history(HistoryPolicy::new(blocked.join("store"), 10, NO_THROTTLE));

    writer.write(b"one").expect("first write");
    writer
        .write(b"two")
        .expect("write proceeds without history");
    assert_eq!(fs::read_to_string(&path).expect("store"), "two");
}

#[test]
fn zero_generations_turns_history_off() {
    let store = Store::new(10, NO_THROTTLE);
    store
        .writer
        .enable_history(HistoryPolicy::new(&store.history, 0, NO_THROTTLE));
    assert!(store.writer.history_policy().is_none());
    store.write("a");
    store.write("b");
    assert!(store.kept().is_empty());
}

#[test]
fn listings_carry_both_timestamps() {
    let store = Store::new(10, NO_THROTTLE);
    store.write("first");
    std::thread::sleep(Duration::from_millis(20));
    store.write("second");

    let generations = list_generations(&store.history).expect("history");
    let generation = generations.first().expect("one generation");
    let saved_at = generation.saved_at.expect("filesystem keeps mtimes");
    assert!(saved_at <= generation.replaced_at);
    assert_eq!(generation.size, 5);
}

static OBSERVED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn observe(path: &Path) {
    OBSERVED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(path.to_path_buf());
}

fn observed(path: &Path) -> usize {
    let file_name = path.file_name();
    let parent = fs::canonicalize(path.parent().expect("parent")).expect("canonical parent");
    OBSERVED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|seen| seen.parent() == Some(parent.as_path()) && seen.file_name() == file_name)
        .count()
}

#[test]
fn the_observer_hears_only_replacements_that_change_bytes() {
    assert!(set_replacement_observer(observe));
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("observed.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");

    writer.write(b"created").expect("create");
    assert_eq!(observed(&path), 1, "creating a file changes it");
    writer.write(b"created").expect("rewrite");
    assert_eq!(observed(&path), 1, "identical bytes are not a change");
    writer.write(b"edited").expect("edit");
    assert_eq!(observed(&path), 2);
}
