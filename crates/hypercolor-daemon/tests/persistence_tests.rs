use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Barrier};
#[cfg(feature = "persistence-test-hooks")]
use std::time::Duration;

#[cfg(feature = "persistence-test-hooks")]
use axum::body::Body;
#[cfg(feature = "persistence-test-hooks")]
use hypercolor_daemon::display_preferences::{DisplayPreference, DisplayPreferencesStore};
#[cfg(feature = "persistence-test-hooks")]
use hypercolor_daemon::library::{JsonLibraryStore, LibraryStore};
use hypercolor_daemon::logical_devices::{self, LogicalDevice, LogicalDeviceKind};
use hypercolor_daemon::path_migration::MigrationOutcome;
use hypercolor_daemon::persistence::{
    AtomicFileWriter, AtomicWriteOutcome, PersistenceError, write_atomic,
};
use hypercolor_daemon::runtime_state::{
    RuntimeSessionSnapshot, load, load_migrated, reserve_save, save, save_reserved,
};
use hypercolor_types::device::DeviceId;
#[cfg(feature = "persistence-test-hooks")]
use hypercolor_types::effect::EffectId;
#[cfg(feature = "persistence-test-hooks")]
use tower::ServiceExt;

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

    // Admissions are ordered before any commit races: exactly-one-Written is
    // only guaranteed among payloads whose admission the newest one observed.
    // Racing admit+commit together lets an older write finish before a newer
    // admission exists, making two truthful Written outcomes possible.
    let admitted_writes: Vec<_> = (0..worker_count)
        .map(|generation| {
            let payload = format!("generation={generation}");
            writer.reserve().admit(payload.into_bytes())
        })
        .collect();

    for admitted in admitted_writes {
        let barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            admitted.commit().expect("concurrent commit")
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
        outcomes
            .iter()
            .filter(|outcome| **outcome == AtomicWriteOutcome::Superseded)
            .count(),
        worker_count - 1
    );
    assert_eq!(
        fs::read_to_string(&path).expect("read newest payload"),
        "generation=15"
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
    let uppercase = directory.path().join("ÉTAT.JSON");
    let lowercase = directory.path().join("état.json");
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

#[cfg(windows)]
#[test]
fn windows_parent_symlink_aliases_share_generation_order() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let target = directory.path().join("target");
    let alias = directory.path().join("alias");
    fs::create_dir(&target).expect("target directory");
    if let Err(error) = std::os::windows::fs::symlink_dir(&target, &alias) {
        eprintln!("skipping parent symlink identity test: {error}");
        return;
    }
    let first = AtomicFileWriter::new(&target.join("state.json")).expect("target writer");
    let older = first.reserve();
    let second = AtomicFileWriter::new(&alias.join("state.json")).expect("symlink writer");
    let newer = second.reserve();

    assert_eq!(
        newer.write(b"new").expect("newer symlink write"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(
        older.write(b"old").expect("older symlink write"),
        AtomicWriteOutcome::Superseded
    );
    assert_eq!(
        fs::read(target.join("state.json")).expect("read symlink target"),
        b"new"
    );
}

#[cfg(windows)]
#[test]
fn windows_case_sensitive_directory_keeps_case_variants_distinct() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let parent = directory.path().join("sensitive");
    fs::create_dir(&parent).expect("case-sensitive candidate directory");
    let output = std::process::Command::new("fsutil.exe")
        .args(["file", "SetCaseSensitiveInfo"])
        .arg(&parent)
        .arg("enable")
        .output()
        .expect("fsutil should launch");
    if !output.status.success() {
        eprintln!(
            "skipping case-sensitive directory test: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return;
    }

    let uppercase = AtomicFileWriter::new(&parent.join("STATE.JSON")).expect("uppercase writer");
    let older = uppercase.reserve();
    let lowercase = AtomicFileWriter::new(&parent.join("state.json")).expect("lowercase writer");

    assert_eq!(
        lowercase.write(b"lower").expect("lowercase write"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(
        older.write(b"upper").expect("uppercase write"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(
        fs::read(parent.join("STATE.JSON")).expect("read upper"),
        b"upper"
    );
    assert_eq!(
        fs::read(parent.join("state.json")).expect("read lower"),
        b"lower"
    );
}

#[cfg(windows)]
#[test]
fn windows_verbatim_trailing_names_remain_distinct_when_supported() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let parent = fs::canonicalize(directory.path()).expect("canonical verbatim parent");
    let plain = parent.join("state.json");
    let dotted = parent.join("state.json.");
    let spaced = parent.join("state.json ");
    fs::write(&plain, b"plain-probe").expect("write plain probe");
    if let Err(error) = fs::write(&dotted, b"dot-probe") {
        eprintln!("skipping trailing-name identity test: {error}");
        return;
    }
    if let Err(error) = fs::write(&spaced, b"space-probe") {
        eprintln!("skipping trailing-name identity test: {error}");
        return;
    }
    if fs::read(&plain).expect("read plain probe") != b"plain-probe" {
        eprintln!("skipping trailing-name identity test: filesystem aliases trailing names");
        return;
    }

    let plain_writer = AtomicFileWriter::new(&plain).expect("plain writer");
    let older_plain = plain_writer.reserve();
    let dotted_writer = AtomicFileWriter::new(&dotted).expect("dotted writer");
    let spaced_writer = AtomicFileWriter::new(&spaced).expect("spaced writer");
    assert_eq!(
        dotted_writer.write(b"dot").expect("dotted write"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(
        spaced_writer.write(b"space").expect("spaced write"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(
        older_plain.write(b"plain").expect("plain write"),
        AtomicWriteOutcome::Written
    );
    assert_eq!(fs::read(&plain).expect("read plain"), b"plain");
    assert_eq!(fs::read(&dotted).expect("read dotted"), b"dot");
    assert_eq!(fs::read(&spaced).expect("read spaced"), b"space");
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
fn runtime_snapshot_moves_to_state_with_a_durable_backup() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let legacy = directory.path().join("data/runtime-state.json");
    let canonical = directory.path().join("state/runtime-state.json");
    let expected = RuntimeSessionSnapshot {
        active_scene_id: Some("active".to_owned()),
        global_brightness: 0.7,
        ..RuntimeSessionSnapshot::default()
    };
    save(&legacy, &expected).expect("seed legacy runtime snapshot");

    let (loaded, outcome) = load_migrated(&legacy, &canonical).expect("runtime migration succeeds");
    let MigrationOutcome::Imported {
        backup: Some(backup),
    } = outcome
    else {
        panic!("expected an imported backup, got {outcome:?}");
    };

    let loaded = loaded.expect("runtime snapshot exists");
    assert_eq!(loaded.active_scene_id, expected.active_scene_id);
    assert!((loaded.global_brightness - expected.global_brightness).abs() < f32::EPSILON);
    assert!(canonical.exists());
    assert!(!legacy.exists());
    assert!(backup.exists());

    let (_, second) = load_migrated(&legacy, &canonical).expect("restart is idempotent");
    assert_eq!(second, MigrationOutcome::AlreadyMigrated);
}

#[test]
fn invalid_legacy_runtime_snapshot_never_replaces_state() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let legacy = directory.path().join("data/runtime-state.json");
    let canonical = directory.path().join("state/runtime-state.json");
    fs::create_dir_all(legacy.parent().expect("legacy parent")).expect("legacy directory");
    fs::write(&legacy, b"not json").expect("write invalid legacy snapshot");

    let error = load_migrated(&legacy, &canonical).expect_err("invalid legacy is refused");

    assert!(error.to_string().contains("failed to parse"));
    assert_eq!(fs::read(&legacy).expect("legacy survives"), b"not json");
    assert!(!canonical.exists());
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
fn dropped_reservation_cannot_supersede_an_older_dirty_snapshot() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");
    writer.set_injected_replace_failures(usize::MAX);

    assert!(matches!(
        writer.write(b"generation-one"),
        Err(PersistenceError::Replace { .. })
    ));
    let abandoned_generation_two = writer.reserve();
    writer
        .flush(Duration::from_millis(25))
        .expect_err("generation one should remain dirty while replacement fails");
    drop(abandoned_generation_two);

    writer.set_injected_replace_failures(0);
    writer
        .flush(Duration::from_secs(5))
        .expect("generation one should converge after restoring replacement");
    assert_eq!(
        fs::read(&path).expect("read restored snapshot"),
        b"generation-one"
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn cancelled_async_snapshot_assembly_preserves_the_older_dirty_payload() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");
    writer.set_injected_replace_failures(usize::MAX);
    assert!(matches!(
        writer.write(b"generation-one"),
        Err(PersistenceError::Replace { .. })
    ));

    let reservation = writer.reserve();
    let assembly = tokio::spawn(async move {
        std::future::pending::<()>().await;
        reservation.write(b"generation-two")
    });
    tokio::task::yield_now().await;
    assembly.abort();
    assert!(
        assembly
            .await
            .expect_err("assembly should be cancelled")
            .is_cancelled()
    );

    writer.set_injected_replace_failures(0);
    writer
        .flush(Duration::from_secs(5))
        .expect("older dirty payload should survive assembly cancellation");
    assert_eq!(
        fs::read(&path).expect("read restored snapshot"),
        b"generation-one"
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[test]
fn runtime_serialization_failure_preserves_the_older_dirty_payload() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("runtime-state.json");
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");
    let older = RuntimeSessionSnapshot {
        active_scene_id: Some("generation-one".to_owned()),
        ..RuntimeSessionSnapshot::default()
    };
    writer.set_injected_replace_failures(usize::MAX);
    let older_save = reserve_save(&path).expect("older reservation");
    assert!(save_reserved(older_save, &older).is_err());

    let newer_save = reserve_save(&path).expect("newer reservation");
    hypercolor_daemon::persistence::set_injected_serialization_failures(1);
    let newer = RuntimeSessionSnapshot {
        active_scene_id: Some("generation-two".to_owned()),
        ..RuntimeSessionSnapshot::default()
    };
    assert!(matches!(
        save_reserved(newer_save, &newer),
        Err(hypercolor_daemon::runtime_state::RuntimeSessionError::Serialize(_))
    ));

    writer.set_injected_replace_failures(0);
    writer
        .flush(Duration::from_secs(5))
        .expect("older runtime snapshot should converge");
    assert_eq!(
        load(&path)
            .expect("load runtime snapshot")
            .expect("runtime snapshot exists")
            .active_scene_id
            .as_deref(),
        Some("generation-one")
    );
}

#[test]
fn dropped_admitted_payload_is_retained_by_the_shared_supervisor() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("state.json");
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");

    drop(writer.reserve().admit(b"retained".to_vec()));

    assert_eq!(
        writer
            .flush(std::time::Duration::from_secs(5))
            .expect("admitted payload should converge"),
        hypercolor_daemon::persistence::PersistenceFlushOutcome::Written
    );
    assert_eq!(fs::read(&path).expect("read retained payload"), b"retained");
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn scene_creation_rolls_back_when_serialization_fails_before_admission() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let data_dir = directory.path().join("data");
    let state = Arc::new(hypercolor_daemon::api::AppState::new_with_data_dir(
        data_dir,
    ));
    let app = hypercolor_daemon::api::build_router(Arc::clone(&state), None);
    hypercolor_daemon::persistence::set_injected_serialization_failures(1);

    let response = app
        .oneshot(
            http::Request::builder()
                .method("POST")
                .uri("/api/v1/scenes")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"must-rollback"}"#))
                .expect("scene request"),
        )
        .await
        .expect("scene response");

    assert_eq!(response.status(), http::StatusCode::INTERNAL_SERVER_ERROR);
    let manager = state.scene_manager.read().await;
    assert!(
        manager
            .list()
            .into_iter()
            .all(|scene| scene.name != "must-rollback")
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn library_mutation_rolls_back_when_serialization_fails_before_admission() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("library.json");
    let store = JsonLibraryStore::open(path).expect("library store");
    let retained = EffectId::new(uuid::Uuid::now_v7());
    let rejected = EffectId::new(uuid::Uuid::now_v7());
    store
        .upsert_favorite(retained, 1)
        .await
        .expect("seed favorite");
    hypercolor_daemon::persistence::set_injected_serialization_failures(1);

    let error = store
        .upsert_favorite(rejected, 2)
        .await
        .expect_err("serialization failure should reject mutation");

    assert!(matches!(
        error,
        hypercolor_daemon::library::LibraryStoreError::Persistence(_)
    ));
    let favorites = store.list_favorites().await;
    assert_eq!(favorites.len(), 1);
    assert_eq!(favorites[0].effect_id, retained);
}

#[cfg(feature = "persistence-test-hooks")]
#[test]
fn display_preference_rolls_back_when_serialization_fails_before_admission() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("display-preferences.json");
    let mut store = DisplayPreferencesStore::new(path).expect("display preference store");
    let device_id = DeviceId::new();
    let retained_effect = EffectId::new(uuid::Uuid::now_v7());
    store
        .set(
            device_id,
            DisplayPreference {
                effect_id: retained_effect,
                controls: HashMap::new(),
                blend_mode: hypercolor_types::scene::DisplayFaceBlendMode::Alpha,
                opacity: 1.0,
            },
        )
        .expect("seed display preference");
    hypercolor_daemon::persistence::set_injected_serialization_failures(1);

    store
        .set(
            device_id,
            DisplayPreference {
                effect_id: EffectId::new(uuid::Uuid::now_v7()),
                controls: HashMap::new(),
                blend_mode: hypercolor_types::scene::DisplayFaceBlendMode::Replace,
                opacity: 1.0,
            },
        )
        .expect_err("serialization failure should reject mutation");

    assert_eq!(
        store.get(device_id).map(|preference| preference.effect_id),
        Some(retained_effect)
    );
}

#[test]
fn writer_construction_failure_occurs_before_candidate_mutation() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let blocked_parent = directory.path().join("not-a-directory");
    fs::write(&blocked_parent, b"file").expect("blocking file");
    let live = HashMap::from([("effect".to_owned(), "current".to_owned())]);
    let mut candidate = live.clone();
    candidate.insert("effect".to_owned(), "rejected".to_owned());

    let error = AtomicFileWriter::new(&blocked_parent.join("state.json"))
        .expect_err("writer construction should fail");

    assert!(matches!(error, PersistenceError::CreateDirectory { .. }));
    assert_eq!(live.get("effect").map(String::as_str), Some("current"));
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
async fn library_failed_delete_returns_error_and_retry_converges() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("library.json");
    let store = JsonLibraryStore::open(path.clone()).expect("library store");
    let effect_id = EffectId::new(uuid::Uuid::now_v7());
    store
        .upsert_favorite(effect_id, 42)
        .await
        .expect("upsert favorite");
    let writer = AtomicFileWriter::new(&path).expect("atomic writer");
    writer.set_injected_replace_failures(usize::MAX);

    assert!(store.remove_favorite(effect_id).await.is_err());
    assert_eq!(store.list_favorites().await.len(), 1);
    let before_retry = JsonLibraryStore::open(path.clone()).expect("reload retained favorite");
    assert_eq!(before_retry.list_favorites().await.len(), 1);

    writer.set_injected_replace_failures(0);
    assert!(
        store
            .remove_favorite(effect_id)
            .await
            .expect("retry favorite removal")
    );
    assert!(
        !store
            .remove_favorite(effect_id)
            .await
            .expect("remove missing favorite after durable retry")
    );
    writer
        .flush(Duration::from_secs(5))
        .expect("library deletion should converge");
    drop(store);

    let reloaded = JsonLibraryStore::open(path).expect("reload library store");
    assert!(reloaded.list_favorites().await.is_empty());
}
