use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Barrier};
#[cfg(feature = "persistence-test-hooks")]
use std::time::Duration;

use hypercolor_daemon::effect_layouts;
#[cfg(feature = "persistence-test-hooks")]
use hypercolor_daemon::library::{JsonLibraryStore, LibraryStore};
use hypercolor_daemon::logical_devices::{self, LogicalDevice, LogicalDeviceKind};
use hypercolor_daemon::persistence::{
    AtomicFileWriter, AtomicWriteOutcome, PersistenceError, write_atomic,
};
use hypercolor_daemon::runtime_state::{RuntimeSessionSnapshot, load, reserve_save, save_reserved};
use hypercolor_types::device::DeviceId;
#[cfg(feature = "persistence-test-hooks")]
use hypercolor_types::effect::EffectId;

#[test]
fn atomic_write_replaces_an_existing_file() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("state.json");

    write_atomic(&path, br#"{"generation":1}"#).expect("initial write");
    write_atomic(&path, br#"{"generation":2}"#).expect("replacement write");

    assert_eq!(
        fs::read(&path).expect("read replaced state"),
        br#"{"generation":2}"#
    );
}

#[test]
fn atomic_write_creates_missing_parent_directories() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("nested").join("state.json");

    write_atomic(&path, b"ready").expect("nested write");

    assert_eq!(fs::read(&path).expect("read nested state"), b"ready");
}

#[test]
fn newer_reservation_rejects_a_stale_prepared_payload() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");
    let older = writer.reserve();
    let newer = writer.reserve();

    assert_eq!(
        newer.write(br#"{"generation":2}"#).expect("newer write"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(
        older.write(br#"{"generation":1}"#).expect("older write"),
        AtomicWriteOutcome::Superseded
    );
    assert_eq!(
        fs::read(&path).expect("read newest state"),
        br#"{"generation":2}"#
    );
}

#[test]
fn concurrent_distinct_payloads_commit_only_the_newest_generation() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");
    let worker_count = 16;
    let barrier = Arc::new(Barrier::new(worker_count));
    let mut workers = Vec::with_capacity(worker_count);

    for generation in 0..worker_count {
        let reservation = writer.reserve();
        let barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            let payload = format!("generation={generation}");
            reservation
                .write(payload.as_bytes())
                .expect("concurrent write")
        }));
    }

    let outcomes = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker should not panic"))
        .collect::<Vec<_>>();
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == AtomicWriteOutcome::Written)
            .count(),
        1
    );
    assert_eq!(
        fs::read_to_string(&path).expect("read newest payload"),
        "generation=15"
    );
}

#[test]
fn effect_layout_save_rejects_an_overtaken_snapshot() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("effect-layouts.json");
    let older = HashMap::from([("effect".to_owned(), "older".to_owned())]);
    let newer = HashMap::from([("effect".to_owned(), "newer".to_owned())]);
    let older_save = effect_layouts::reserve_save(&path, &older).expect("reserve older snapshot");
    let newer_save = effect_layouts::reserve_save(&path, &newer).expect("reserve newer snapshot");

    assert_eq!(
        effect_layouts::save_reserved(newer_save).expect("save newer snapshot"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(
        effect_layouts::save_reserved(older_save).expect("reject older snapshot"),
        AtomicWriteOutcome::Superseded
    );
    assert_eq!(
        effect_layouts::load(&path).expect("reload effect layouts"),
        newer
    );
}

#[test]
fn logical_device_save_rejects_an_overtaken_snapshot() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("logical-devices.json");
    let physical_device_id = DeviceId::new();
    let entry = |id: &str| LogicalDevice {
        id: id.to_owned(),
        physical_device_id,
        name: id.to_owned(),
        led_start: 0,
        led_count: 16,
        enabled: true,
        kind: LogicalDeviceKind::Segment,
    };
    let older = HashMap::from([("older".to_owned(), entry("older"))]);
    let newer = HashMap::from([("newer".to_owned(), entry("newer"))]);
    let older_save = logical_devices::reserve_save_segments(&path, &older)
        .expect("reserve older logical-device snapshot");
    let newer_save = logical_devices::reserve_save_segments(&path, &newer)
        .expect("reserve newer logical-device snapshot");

    assert_eq!(
        logical_devices::save_reserved_segments(newer_save).expect("save newer snapshot"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(
        logical_devices::save_reserved_segments(older_save).expect("reject older snapshot"),
        AtomicWriteOutcome::Superseded
    );
    let loaded = logical_devices::load_segments(&path).expect("reload logical devices");
    assert!(loaded.contains_key("newer"));
    assert!(!loaded.contains_key("older"));
}

#[test]
fn equivalent_parent_aliases_share_generation_order() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let nested = directory.path().join("nested");
    fs::create_dir(&nested).expect("nested directory");
    let path = directory.path().join("state.json");
    let alias = nested.join("..").join("state.json");
    let first = AtomicFileWriter::new(&path).expect("first writer");
    let older = first.reserve();
    let second = AtomicFileWriter::new(&alias).expect("alias writer");
    let newer = second.reserve();

    assert_eq!(
        newer.write(b"new").expect("newer alias write"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(
        older.write(b"old").expect("older alias write"),
        AtomicWriteOutcome::Superseded
    );
    assert_eq!(fs::read(&path).expect("read aliased state"), b"new");
}

#[cfg(windows)]
#[test]
fn windows_case_aliases_share_generation_order() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let uppercase = directory.path().join("STATE.JSON");
    let lowercase = directory.path().join("state.json");
    let first = AtomicFileWriter::new(&uppercase).expect("uppercase writer");
    let older = first.reserve();
    let second = AtomicFileWriter::new(&lowercase).expect("lowercase writer");
    let newer = second.reserve();

    assert_eq!(
        newer.write(b"new").expect("newer case-alias write"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(
        older.write(b"old").expect("older case-alias write"),
        AtomicWriteOutcome::Superseded
    );
    assert_eq!(fs::read(&lowercase).expect("read case alias"), b"new");
}

#[test]
fn replacement_failures_retain_the_typed_stage() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let error = write_atomic(directory.path(), b"not a directory")
        .expect_err("a file must not replace a directory");

    assert!(matches!(error, PersistenceError::Replace { .. }));
}

#[test]
fn runtime_reservations_prevent_stale_snapshot_resurrection() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("runtime-state.json");
    let older = reserve_save(&path).expect("older reservation");
    let newer = reserve_save(&path).expect("newer reservation");
    let older_snapshot = RuntimeSessionSnapshot {
        active_scene_id: Some("older".to_owned()),
        ..RuntimeSessionSnapshot::default()
    };
    let newer_snapshot = RuntimeSessionSnapshot {
        active_scene_id: Some("newer".to_owned()),
        ..RuntimeSessionSnapshot::default()
    };

    assert_eq!(
        save_reserved(newer, &newer_snapshot).expect("newer snapshot"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(
        save_reserved(older, &older_snapshot).expect("stale snapshot"),
        AtomicWriteOutcome::Superseded
    );

    let loaded = load(&path)
        .expect("load runtime snapshot")
        .expect("runtime snapshot exists");
    assert_eq!(loaded.active_scene_id.as_deref(), Some("newer"));
}

#[test]
fn flush_reports_clean_after_a_direct_success() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");
    writer.write(b"current").expect("write current state");

    assert_eq!(
        writer
            .flush(std::time::Duration::from_secs(1))
            .expect("clean flush"),
        hypercolor_daemon::persistence::PersistenceFlushOutcome::Clean
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[test]
fn failed_writes_retry_only_the_latest_snapshot() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");
    writer.write(b"seed").expect("seed snapshot");
    writer.set_injected_replace_failures(usize::MAX);

    assert!(matches!(
        writer.write(b"older"),
        Err(PersistenceError::Replace { .. })
    ));
    assert!(matches!(
        writer.write(b"newest"),
        Err(PersistenceError::Replace { .. })
    ));

    writer.set_injected_replace_failures(0);
    writer.kick();
    assert_eq!(
        writer
            .flush(Duration::from_secs(5))
            .expect("latest snapshot should converge"),
        hypercolor_daemon::persistence::PersistenceFlushOutcome::Written
    );
    assert_eq!(fs::read(&path).expect("read retried state"), b"newest");
}

#[cfg(feature = "persistence-test-hooks")]
#[test]
fn superseded_completion_cannot_clear_a_newer_dirty_snapshot() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");
    let older = writer.reserve();
    writer.set_injected_replace_failures(usize::MAX);

    assert!(matches!(
        writer.write(b"newest"),
        Err(PersistenceError::Replace { .. })
    ));
    assert_eq!(
        older.write(b"older").expect("older write is superseded"),
        AtomicWriteOutcome::Superseded
    );

    writer.set_injected_replace_failures(0);
    writer.kick();
    writer
        .flush(Duration::from_secs(5))
        .expect("newer dirty snapshot should converge");
    assert_eq!(fs::read(&path).expect("read retried state"), b"newest");
}

#[cfg(feature = "persistence-test-hooks")]
#[test]
fn flush_deadline_reports_a_persistent_failure_without_stopping_retry() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");
    writer.set_injected_replace_failures(usize::MAX);
    assert!(matches!(
        writer.write(b"dirty"),
        Err(PersistenceError::Replace { .. })
    ));
    let started = std::time::Instant::now();

    writer
        .flush(Duration::from_millis(25))
        .expect_err("persistent failure should exceed the flush deadline");
    assert!(started.elapsed() < Duration::from_secs(1));

    writer.set_injected_replace_failures(0);
    writer.kick();
    writer
        .flush(Duration::from_secs(5))
        .expect("runtime retry remains active after bounded flush");
    assert_eq!(fs::read(&path).expect("read retried state"), b"dirty");
}

#[cfg(feature = "persistence-test-hooks")]
#[test]
fn failed_logical_device_delete_does_not_resurrect_after_reload() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("logical-devices.json");
    let physical_device_id = DeviceId::new();
    let entry = LogicalDevice {
        id: "segment".to_owned(),
        physical_device_id,
        name: "Segment".to_owned(),
        led_start: 0,
        led_count: 16,
        enabled: true,
        kind: LogicalDeviceKind::Segment,
    };
    logical_devices::save_segments(&path, &HashMap::from([("segment".to_owned(), entry)]))
        .expect("seed logical devices");
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");
    writer.set_injected_replace_failures(usize::MAX);

    let pending = logical_devices::reserve_save_segments(&path, &HashMap::new())
        .expect("reserve deletion snapshot");
    assert!(logical_devices::save_reserved_segments(pending).is_err());

    writer.set_injected_replace_failures(0);
    logical_devices::kick_pending(&path).expect("kick logical-device retry");
    writer
        .flush(Duration::from_secs(5))
        .expect("deletion snapshot should converge");
    assert!(
        logical_devices::load_segments(&path)
            .expect("reload logical devices")
            .is_empty()
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[test]
fn failed_runtime_snapshot_create_eventually_converges() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("runtime-state.json");
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");
    writer.set_injected_replace_failures(usize::MAX);
    let pending = reserve_save(&path).expect("reserve runtime snapshot");
    let snapshot = RuntimeSessionSnapshot {
        active_scene_id: Some("created".to_owned()),
        ..RuntimeSessionSnapshot::default()
    };

    assert!(save_reserved(pending, &snapshot).is_err());

    writer.set_injected_replace_failures(0);
    writer.kick();
    writer
        .flush(Duration::from_secs(5))
        .expect("runtime snapshot should converge");
    assert_eq!(
        load(&path)
            .expect("reload runtime snapshot")
            .expect("runtime snapshot should exist")
            .active_scene_id
            .as_deref(),
        Some("created")
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn library_no_op_retriggers_a_failed_delete() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("library.json");
    let store = JsonLibraryStore::open(path.clone()).expect("library store");
    let effect_id = EffectId::new(uuid::Uuid::now_v7());
    store.upsert_favorite(effect_id, 42).await;
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");
    writer.set_injected_replace_failures(usize::MAX);

    assert!(store.remove_favorite(effect_id).await);
    writer.set_injected_replace_failures(0);
    assert!(!store.remove_favorite(effect_id).await);
    writer
        .flush(Duration::from_secs(5))
        .expect("library deletion should converge");
    drop(store);

    let reloaded = JsonLibraryStore::open(path).expect("reload library store");
    assert!(reloaded.list_favorites().await.is_empty());
}
