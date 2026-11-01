//! Write-behind: admission never waits on the file, generations coalesce
//! the writes, the newest payload always wins, and a flush waits for the
//! attempt still in flight.

#![cfg(feature = "persistence-test-hooks")]

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::thread::{self, ThreadId};
use std::time::Duration;

use hypercolor_persistence::{
    AtomicFileWriter, AtomicWriteOutcome, PersistenceError, PersistenceFlushOutcome,
    set_replacement_observer,
};

const WAIT: Duration = Duration::from_secs(5);

fn destination() -> (tempfile::TempDir, PathBuf, AtomicFileWriter) {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("session.json");
    let writer = AtomicFileWriter::new(&path).expect("writer");
    (root, path, writer)
}

/// The outcome of an attempt that must not fail.
fn landed(
    attempt: thread::JoinHandle<Result<AtomicWriteOutcome, PersistenceError>>,
) -> AtomicWriteOutcome {
    attempt
        .join()
        .expect("attempt thread")
        .expect("attempt should not fail")
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("destination should hold a payload")
}

#[test]
fn admitting_returns_while_the_attempt_in_flight_is_stalled() {
    let (_root, path, writer) = destination();
    writer.write(b"before").expect("seed write lands");
    let hold = writer.hold_writes();

    let first = writer.reserve().admit_write_behind(b"first".to_vec());
    let in_flight = thread::spawn(move || first.commit());
    assert!(
        hold.wait_for_held(1, WAIT),
        "the first attempt reached the disk"
    );

    // The disk is stalled, and admitting the next payload still returns.
    let second = writer.reserve().admit_write_behind(b"second".to_vec());
    assert_eq!(read(&path), b"before", "nothing has landed yet");
    let queued = thread::spawn(move || second.commit());

    hold.release();
    assert_eq!(
        landed(in_flight),
        AtomicWriteOutcome::Written,
        "an attempt that started lands even though a newer one was admitted"
    );
    assert_eq!(landed(queued), AtomicWriteOutcome::Written);
    assert_eq!(read(&path), b"second");
    assert_eq!(
        writer.flush(WAIT).expect("nothing pending"),
        PersistenceFlushOutcome::Written,
        "the flush reports the background write it found landed"
    );
}

#[test]
fn a_burst_writes_the_attempt_in_flight_and_then_only_the_newest() {
    let (_root, path, writer) = destination();
    let hold = writer.hold_writes();

    let first = writer.reserve().admit_write_behind(b"payload-0".to_vec());
    let mut attempts = vec![thread::spawn(move || first.commit())];
    assert!(
        hold.wait_for_held(1, WAIT),
        "the first attempt reached the disk"
    );
    for index in 1..=8 {
        let write = writer
            .reserve()
            .admit_write_behind(format!("payload-{index}").into_bytes());
        attempts.push(thread::spawn(move || write.commit()));
    }
    hold.release();

    let outcomes = attempts.into_iter().map(landed).collect::<Vec<_>>();
    assert_eq!(outcomes[0], AtomicWriteOutcome::Written);
    assert_eq!(outcomes[8], AtomicWriteOutcome::Written);
    assert!(
        outcomes[1..8]
            .iter()
            .all(|outcome| *outcome == AtomicWriteOutcome::Superseded),
        "every payload between the one in flight and the newest stood down: {outcomes:?}"
    );
    assert_eq!(writer.replacement_count(), 2, "nine admissions, two writes");
    assert_eq!(read(&path), b"payload-8");
}

#[test]
fn a_stale_write_behind_never_lands_over_a_newer_write() {
    let (_root, path, writer) = destination();
    let stale = writer.reserve().admit_write_behind(b"stale".to_vec());
    writer.write(b"newer").expect("newer write lands");

    assert_eq!(
        stale.commit().expect("a stale attempt stands down"),
        AtomicWriteOutcome::Superseded
    );
    assert_eq!(read(&path), b"newer");
    assert_eq!(writer.replacement_count(), 1);
}

#[test]
fn a_write_behind_in_flight_lands_before_the_newer_write_that_follows_it() {
    let (_root, path, writer) = destination();
    let hold = writer.hold_writes();
    let older = writer.reserve().admit_write_behind(b"older".to_vec());
    let in_flight = thread::spawn(move || older.commit());
    assert!(
        hold.wait_for_held(1, WAIT),
        "the older attempt reached the disk"
    );

    // The newer foreground write is admitted before the older attempt
    // reaches its replacement, so the older one lands past a newer intent.
    let newer = writer.reserve().admit(b"newer".to_vec());
    let foreground = thread::spawn(move || newer.commit());
    hold.release();

    assert_eq!(
        landed(in_flight),
        AtomicWriteOutcome::Written,
        "the started attempt lands although a newer one was admitted"
    );
    assert_eq!(landed(foreground), AtomicWriteOutcome::Written);
    assert_eq!(read(&path), b"newer", "the newer bytes land last");
    assert_eq!(writer.replacement_count(), 2);
}

#[test]
fn a_flush_returns_once_its_own_snapshot_lands_despite_newer_admissions() {
    let (_root, path, writer) = destination();
    let hold = writer.hold_writes();
    let first = writer.reserve().admit_write_behind(b"first".to_vec());
    let in_flight = thread::spawn(move || first.commit());
    assert!(
        hold.wait_for_held(1, WAIT),
        "the first attempt reached the disk"
    );

    // Once the flush below is waiting, a newer admission keeps the
    // destination dirty, as a slider drag does, and only then does the disk
    // come back. Nothing commits the newer payload while the flush waits.
    let racer = writer.clone();
    let newer = thread::spawn(move || {
        let deadline = std::time::Instant::now() + WAIT;
        while racer.flush_wait_count() == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the flush never began waiting"
            );
            thread::sleep(Duration::from_millis(1));
        }
        let newer = racer.reserve().admit_write_behind(b"newer".to_vec());
        hold.release();
        newer
    });
    assert_eq!(
        writer
            .flush(WAIT)
            .expect("the flush returns once the snapshot it found lands"),
        PersistenceFlushOutcome::Written
    );
    assert_eq!(landed(in_flight), AtomicWriteOutcome::Written);
    assert_eq!(read(&path), b"first", "the newer payload is still pending");

    let newer = newer.join().expect("racer thread");
    assert_eq!(
        newer.commit().expect("the newer write lands"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(read(&path), b"newer");
}

#[test]
fn a_failed_write_behind_reports_its_error_and_the_supervisor_lands_it() {
    let (_root, path, writer) = destination();
    writer.set_injected_replace_failures(1);
    let write = writer.reserve().admit_write_behind(b"retried".to_vec());

    let error = write.commit().expect_err("the injected failure surfaces");
    assert!(
        error.to_string().contains("session.json"),
        "the error names its destination: {error}"
    );
    let lag = writer
        .pending_failure()
        .expect("the destination reports the lag");
    assert_eq!(lag, error.to_string());

    assert_eq!(
        writer.flush(WAIT).expect("the retry converges"),
        PersistenceFlushOutcome::Written
    );
    assert_eq!(read(&path), b"retried");
    assert_eq!(writer.pending_failure(), None);
}

#[test]
fn a_write_behind_dropped_before_its_worker_runs_still_lands() {
    let (_root, path, writer) = destination();
    drop(writer.reserve().admit_write_behind(b"orphaned".to_vec()));

    assert_eq!(
        writer.flush(WAIT).expect("the supervisor lands it"),
        PersistenceFlushOutcome::Written
    );
    assert_eq!(read(&path), b"orphaned");
}

#[test]
fn a_flush_waits_for_the_write_behind_in_flight() {
    let (_root, path, writer) = destination();
    let hold = writer.hold_writes();
    let write = writer.reserve().admit_write_behind(b"flushed".to_vec());
    let in_flight = thread::spawn(move || write.commit());
    assert!(hold.wait_for_held(1, WAIT), "the attempt reached the disk");

    let flushing = writer.clone();
    let flush = thread::spawn(move || flushing.flush(WAIT));
    thread::sleep(Duration::from_millis(50));
    assert!(
        !flush.is_finished(),
        "a flush must not report clean while the write is in flight"
    );

    hold.release();
    assert_eq!(
        flush
            .join()
            .expect("flush thread")
            .expect("flush converges"),
        PersistenceFlushOutcome::Written
    );
    assert_eq!(landed(in_flight), AtomicWriteOutcome::Written);
    assert_eq!(read(&path), b"flushed");
}

/// Every observer call, with the thread that made it.
static OBSERVED: LazyLock<Mutex<Vec<(ThreadId, PathBuf)>>> = LazyLock::new(Mutex::default);

fn observe(path: &Path) {
    OBSERVED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((thread::current().id(), path.to_path_buf()));
}

fn observed_here(path: &Path) -> usize {
    let here = thread::current().id();
    let canonical = std::fs::canonicalize(path.parent().expect("parent"))
        .expect("canonical parent")
        .join(path.file_name().expect("file name"));
    OBSERVED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|(thread, observed)| *thread == here && *observed == canonical)
        .count()
}

#[test]
fn the_admitting_thread_hears_of_a_change_its_write_behind_carries() {
    set_replacement_observer(observe);
    let (_root, path, writer) = destination();

    let first = writer.reserve().admit_write_behind(b"changed".to_vec());
    assert_eq!(
        observed_here(&path),
        1,
        "the change is reported where it was admitted, before any write"
    );
    thread::spawn(move || first.commit())
        .join()
        .expect("attempt thread")
        .expect("write lands");

    let identical = writer.reserve().admit_write_behind(b"changed".to_vec());
    assert_eq!(
        observed_here(&path),
        1,
        "bytes the destination already holds change nothing"
    );
    thread::spawn(move || identical.commit())
        .join()
        .expect("attempt thread")
        .expect("identical write confirms");
}
