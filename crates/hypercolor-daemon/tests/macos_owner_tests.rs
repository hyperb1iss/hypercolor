use std::fs::{self, OpenOptions};
use std::sync::{Arc, Barrier};
use std::thread;

#[cfg(target_os = "macos")]
use hypercolor_daemon::macos_owner::try_acquire_macos_daemon_guard;
use hypercolor_daemon::macos_owner::{
    MACOS_HANDOVER_JOURNAL_SCHEMA_VERSION, MACOS_OWNER_RECORD_SCHEMA_VERSION,
    MAX_MACOS_HANDOVER_OPERATIONS, MAX_MACOS_OWNER_ARTIFACT_BYTES, MacosAutostartStates,
    MacosConflictUpdate, MacosDaemonOwner, MacosExternalOwnerMode, MacosHandoverJournal,
    MacosHandoverOperation, MacosHandoverPhase, MacosHandoverTransactionId, MacosOwnerIdentity,
    MacosOwnerStore, MacosOwnerStoreError,
};
use serde_json::{Value, json};

fn transaction_id(value: &str) -> MacosHandoverTransactionId {
    MacosHandoverTransactionId::new(value).expect("fixture transaction ID should be valid")
}

fn identity(label: &str, pid: u32) -> MacosOwnerIdentity {
    MacosOwnerIdentity::new(
        format!("audit-token-{label}"),
        format!("/Applications/{label}/hypercolor-daemon"),
        format!("sha256-{label}"),
        pid,
    )
    .expect("fixture owner identity should be valid")
}

fn all_operations() -> Vec<MacosHandoverOperation> {
    vec![
        MacosHandoverOperation::SetAppSidecarAutostart { enabled: false },
        MacosHandoverOperation::FlushAndStopAppSidecar {},
        MacosHandoverOperation::StartAppSidecar {},
        MacosHandoverOperation::SetDirectLaunchdAutostart { enabled: true },
        MacosHandoverOperation::FlushAndStopDirectLaunchd {},
        MacosHandoverOperation::StartDirectLaunchd {},
        MacosHandoverOperation::SetHomebrewAutostart { enabled: false },
        MacosHandoverOperation::FlushAndStopHomebrew {},
        MacosHandoverOperation::StartHomebrew {},
        MacosHandoverOperation::AwaitStandaloneExit { pid: 4242 },
    ]
}

fn journal(id: &str) -> MacosHandoverJournal {
    MacosHandoverJournal::new(
        transaction_id(id),
        MacosDaemonOwner::DirectLaunchd,
        MacosDaemonOwner::Standalone,
        MacosAutostartStates::new(true, false, false),
        all_operations(),
        7,
        Some(8),
        Some(4242),
    )
}

#[test]
fn owner_publication_advances_monotonic_epochs() {
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let store = MacosOwnerStore::new(directory.path());

    let first = store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
        .expect("first owner should publish");
    store
        .set_external_owner_mode(Some(MacosExternalOwnerMode::DirectLaunchd))
        .expect("external owner mode should publish");
    let second = store
        .publish_owner(MacosDaemonOwner::DirectLaunchd, identity("launchd", 102))
        .expect("second owner should publish");
    store
        .set_external_owner_mode(Some(MacosExternalOwnerMode::Homebrew))
        .expect("external owner mode should update");
    let third = store
        .publish_owner(MacosDaemonOwner::Homebrew, identity("homebrew", 103))
        .expect("third owner should publish");

    assert_eq!(
        [first.owner_epoch, second.owner_epoch, third.owner_epoch],
        [1, 2, 3]
    );
    assert_eq!(
        second.selected_external_owner,
        Some(MacosExternalOwnerMode::DirectLaunchd)
    );
    assert_eq!(
        third.selected_external_owner,
        Some(MacosExternalOwnerMode::Homebrew)
    );
    assert_eq!(third.schema_version, MACOS_OWNER_RECORD_SCHEMA_VERSION);
    assert_eq!(
        store
            .load_owner_record()
            .expect("owner record should load")
            .expect("owner record should exist"),
        third
    );
}

#[test]
fn owner_publication_cannot_overwrite_a_concurrent_external_mode_update() {
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let store = Arc::new(MacosOwnerStore::new(directory.path()));
    store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
        .expect("initial owner should publish");
    let barrier = Arc::new(Barrier::new(3));

    let publisher = {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            store
                .publish_owner(MacosDaemonOwner::Homebrew, identity("homebrew", 103))
                .expect("concurrent owner should publish");
        })
    };
    let selector = {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            store
                .set_external_owner_mode(Some(MacosExternalOwnerMode::Homebrew))
                .expect("concurrent external mode should publish");
        })
    };

    barrier.wait();
    publisher.join().expect("publisher should join");
    selector.join().expect("selector should join");
    assert_eq!(
        store
            .load_owner_record()
            .expect("owner record should load")
            .expect("owner record should exist")
            .selected_external_owner,
        Some(MacosExternalOwnerMode::Homebrew)
    );
}

#[test]
fn owner_publication_rebases_a_prepublication_contender() {
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let store = MacosOwnerStore::new(directory.path());
    store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("old-sidecar", 101))
        .expect("prior owner should publish");
    store
        .record_conflict(
            MacosDaemonOwner::Homebrew,
            identity("homebrew-contender", 201),
            100,
        )
        .expect("prepublication contender should publish");

    let owner = store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("new-sidecar", 102))
        .expect("new owner should publish");
    let conflict = owner
        .conflict
        .expect("contender should survive publication");

    assert_eq!(conflict.active_owner, MacosDaemonOwner::AppSidecar);
    assert_eq!(conflict.active_epoch, owner.owner_epoch);
    assert_eq!(conflict.contender_owner, MacosDaemonOwner::Homebrew);
}

#[test]
fn owner_publication_preserves_a_distinct_same_topology_contender() {
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let store = MacosOwnerStore::new(directory.path());
    store
        .publish_owner(MacosDaemonOwner::Homebrew, identity("old-homebrew", 101))
        .expect("prior owner should publish");
    store
        .record_conflict(
            MacosDaemonOwner::AppSidecar,
            identity("losing-sidecar", 201),
            100,
        )
        .expect("prepublication contender should publish");

    let owner = store
        .publish_owner(
            MacosDaemonOwner::AppSidecar,
            identity("winning-sidecar", 202),
        )
        .expect("new owner should publish");
    let conflict = owner
        .conflict
        .expect("distinct same-topology contender should survive publication");

    assert_eq!(conflict.active_owner, MacosDaemonOwner::AppSidecar);
    assert_eq!(conflict.active_epoch, owner.owner_epoch);
    assert_eq!(conflict.contender_owner, MacosDaemonOwner::AppSidecar);
    assert_eq!(conflict.contender_identity.pid, 201);
}

#[test]
fn owner_publication_clears_the_contender_that_became_active() {
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let store = MacosOwnerStore::new(directory.path());
    store
        .publish_owner(MacosDaemonOwner::Standalone, identity("standalone", 101))
        .expect("prior owner should publish");
    store
        .record_conflict(MacosDaemonOwner::AppSidecar, identity("sidecar", 201), 100)
        .expect("prepublication contender should publish");

    let owner = store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 202))
        .expect("contender should become the active owner");

    assert!(owner.conflict.is_none());
}

#[test]
fn identical_conflicts_coalesce_with_the_original_observation() {
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let store = MacosOwnerStore::new(directory.path());
    store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
        .expect("owner should publish");

    let first = store
        .record_conflict(
            MacosDaemonOwner::DirectLaunchd,
            identity("launchd-contender", 201),
            100,
        )
        .expect("first conflict should publish");
    let duplicate = store
        .record_conflict(
            MacosDaemonOwner::DirectLaunchd,
            identity("launchd-contender", 201),
            999,
        )
        .expect("duplicate conflict should coalesce");
    let restarted_contender = store
        .record_conflict(
            MacosDaemonOwner::DirectLaunchd,
            MacosOwnerIdentity::new(
                "audit-token-after-restart",
                "/Applications/launchd-contender/hypercolor-daemon",
                "sha256-launchd-contender",
                303,
            )
            .expect("restarted contender identity should be valid"),
            1_000,
        )
        .expect("same executable and requirement should coalesce");
    let changed_identity = store
        .record_conflict(
            MacosDaemonOwner::DirectLaunchd,
            identity("different-launchd-contender", 202),
            2_000,
        )
        .expect("new contender identity should publish");

    assert!(matches!(first, MacosConflictUpdate::Recorded(_)));
    assert!(matches!(duplicate, MacosConflictUpdate::Coalesced(_)));
    assert!(matches!(
        restarted_contender,
        MacosConflictUpdate::Coalesced(_)
    ));
    assert!(matches!(changed_identity, MacosConflictUpdate::Recorded(_)));
    assert_eq!(first.snapshot(), duplicate.snapshot());
    assert_eq!(first.snapshot(), restarted_contender.snapshot());
    assert_eq!(
        duplicate
            .snapshot()
            .conflict
            .expect("conflict should remain present")
            .observed_at_ms,
        100
    );
    assert_eq!(
        changed_identity
            .snapshot()
            .conflict
            .expect("changed conflict should remain present")
            .observed_at_ms,
        2_000
    );
}

#[test]
fn owner_identity_rejects_empty_oversized_relative_and_zero_pid_fields() {
    let valid_path = "/Applications/Hypercolor.app/Contents/MacOS/hypercolor-daemon";
    assert!(matches!(
        MacosOwnerIdentity::new("", valid_path, "sha256-valid", 1),
        Err(MacosOwnerStoreError::InvalidOwnerIdentity {
            field: "audit_token_identity",
            ..
        })
    ));
    assert!(matches!(
        MacosOwnerIdentity::new("audit", "relative/daemon", "sha256-valid", 1),
        Err(MacosOwnerStoreError::InvalidOwnerIdentity {
            field: "executable_path",
            ..
        })
    ));
    assert!(matches!(
        MacosOwnerIdentity::new("audit", valid_path, "", 1),
        Err(MacosOwnerStoreError::InvalidOwnerIdentity {
            field: "designated_requirement_hash",
            ..
        })
    ));
    assert!(matches!(
        MacosOwnerIdentity::new("audit", valid_path, "sha256-valid", 0),
        Err(MacosOwnerStoreError::InvalidOwnerIdentity { field: "pid", .. })
    ));
    assert!(matches!(
        MacosOwnerIdentity::new("a".repeat(257), valid_path, "sha256-valid", 1),
        Err(MacosOwnerStoreError::InvalidOwnerIdentity {
            field: "audit_token_identity",
            ..
        })
    ));
    assert!(matches!(
        MacosOwnerIdentity::new("audit", format!("/{}", "p".repeat(4_096)), "hash", 1),
        Err(MacosOwnerStoreError::InvalidOwnerIdentity {
            field: "executable_path",
            ..
        })
    ));
    assert!(matches!(
        MacosOwnerIdentity::new("audit", valid_path, "h".repeat(257), 1),
        Err(MacosOwnerStoreError::InvalidOwnerIdentity {
            field: "designated_requirement_hash",
            ..
        })
    ));
}

#[test]
fn record_and_journal_writers_interleave_without_lost_updates() {
    const THREADS_PER_ARTIFACT: usize = 3;
    const WRITES_PER_THREAD: usize = 24;

    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let store = Arc::new(MacosOwnerStore::new(directory.path()));
    store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
        .expect("initial owner should publish");
    let id = transaction_id("concurrent-handover");
    store
        .begin_handover(journal(id.as_str()))
        .expect("journal should begin");
    let barrier = Arc::new(Barrier::new(THREADS_PER_ARTIFACT * 2));
    let mut handles = Vec::new();

    for _ in 0..THREADS_PER_ARTIFACT {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for _ in 0..WRITES_PER_THREAD {
                store
                    .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
                    .expect("concurrent owner publication should succeed");
            }
        }));
    }
    for thread_index in 0..THREADS_PER_ARTIFACT {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        let id = id.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            for write_index in 0..WRITES_PER_THREAD {
                let phase = if (thread_index + write_index) % 2 == 0 {
                    MacosHandoverPhase::StopRequested
                } else {
                    MacosHandoverPhase::RollbackPending
                };
                store
                    .advance_handover(&id, phase)
                    .expect("concurrent journal publication should succeed");
            }
        }));
    }
    for handle in handles {
        handle.join().expect("writer thread should finish");
    }

    let expected_updates = (THREADS_PER_ARTIFACT * WRITES_PER_THREAD) as u64;
    assert_eq!(
        store
            .load_owner_record()
            .expect("owner record should load")
            .expect("owner record should exist")
            .owner_epoch,
        expected_updates + 1
    );
    assert_eq!(
        store
            .load_handover_journal()
            .expect("journal should load")
            .expect("journal should exist")
            .journal_revision,
        expected_updates + 1
    );
}

#[test]
fn every_handover_phase_round_trips() {
    for (index, phase) in MacosHandoverPhase::ALL.into_iter().enumerate() {
        let directory = tempfile::tempdir().expect("temporary directory should be available");
        let store = MacosOwnerStore::new(directory.path());
        let id = transaction_id(&format!("phase-round-trip-{index}"));
        let initial = store
            .begin_handover(journal(id.as_str()))
            .expect("journal should begin");
        assert_eq!(
            initial.schema_version,
            MACOS_HANDOVER_JOURNAL_SCHEMA_VERSION
        );
        let advanced = store
            .advance_handover(&id, phase)
            .expect("phase should persist");
        let loaded = store
            .load_handover_journal()
            .expect("journal should load")
            .expect("journal should exist");
        assert_eq!(advanced, loaded);
        assert_eq!(loaded.phase, phase);
    }
}

#[test]
fn terminal_handover_cannot_be_revived() {
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let store = MacosOwnerStore::new(directory.path());
    let id = transaction_id("terminal-handover");
    store
        .begin_handover(journal(id.as_str()))
        .expect("journal should begin");
    store
        .advance_handover(&id, MacosHandoverPhase::Committed)
        .expect("handover should commit");

    assert!(matches!(
        store.advance_handover(&id, MacosHandoverPhase::StartRequested),
        Err(MacosOwnerStoreError::TerminalHandover { .. })
    ));
    assert_eq!(
        store
            .load_handover_journal()
            .expect("journal should load")
            .expect("journal should exist")
            .phase,
        MacosHandoverPhase::Committed
    );
}

#[test]
fn malformed_and_unknown_artifacts_reject_without_replacement() {
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let store = MacosOwnerStore::new(directory.path());
    store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
        .expect("owner should publish");
    let valid_owner = fs::read(store.owner_record_path()).expect("owner bytes should exist");

    let malformed = b"{ malformed owner record\n";
    fs::write(store.owner_record_path(), malformed).expect("fixture corruption should write");
    assert!(matches!(
        store.publish_owner(MacosDaemonOwner::Homebrew, identity("homebrew", 103)),
        Err(MacosOwnerStoreError::Decode {
            artifact: "owner record",
            ..
        })
    ));
    assert_eq!(
        fs::read(store.owner_record_path()).expect("malformed bytes should remain"),
        malformed
    );

    let mut unknown_version: Value =
        serde_json::from_slice(&valid_owner).expect("valid owner should decode as JSON");
    unknown_version["schema_version"] = json!(99);
    let unknown_version = serde_json::to_vec_pretty(&unknown_version)
        .expect("unknown-version fixture should serialize");
    fs::write(store.owner_record_path(), &unknown_version)
        .expect("unknown-version fixture should write");
    assert!(matches!(
        store.set_external_owner_mode(Some(MacosExternalOwnerMode::Homebrew)),
        Err(MacosOwnerStoreError::UnsupportedVersion {
            artifact: "owner record",
            found: 99,
            ..
        })
    ));
    assert_eq!(
        fs::read(store.owner_record_path()).expect("unknown-version bytes should remain"),
        unknown_version
    );

    let mut invalid_identity: Value =
        serde_json::from_slice(&valid_owner).expect("valid owner should decode as JSON");
    invalid_identity["active_identity"]["executable_path"] = json!("relative/daemon");
    let invalid_identity = serde_json::to_vec_pretty(&invalid_identity)
        .expect("invalid-identity fixture should serialize");
    fs::write(store.owner_record_path(), &invalid_identity)
        .expect("invalid-identity fixture should write");
    assert!(matches!(
        store.set_external_owner_mode(Some(MacosExternalOwnerMode::Homebrew)),
        Err(MacosOwnerStoreError::Decode {
            artifact: "owner record",
            ..
        })
    ));
    assert_eq!(
        fs::read(store.owner_record_path()).expect("invalid-identity bytes should remain"),
        invalid_identity
    );

    store
        .begin_handover(journal("unknown-operation"))
        .expect("journal should begin");
    let mut unknown_operation: Value = serde_json::from_slice(
        &fs::read(store.handover_journal_path()).expect("journal bytes should exist"),
    )
    .expect("valid journal should decode as JSON");
    unknown_operation["allowed_rollback_operations"] =
        json!([{ "kind": "run_command", "command": "forbidden" }]);
    let unknown_operation = serde_json::to_vec_pretty(&unknown_operation)
        .expect("unknown-operation fixture should serialize");
    fs::write(store.handover_journal_path(), &unknown_operation)
        .expect("unknown-operation fixture should write");
    assert!(matches!(
        store.advance_handover(
            &transaction_id("unknown-operation"),
            MacosHandoverPhase::StopRequested
        ),
        Err(MacosOwnerStoreError::Decode {
            artifact: "handover journal",
            ..
        })
    ));
    assert_eq!(
        fs::read(store.handover_journal_path()).expect("unknown-operation bytes should remain"),
        unknown_operation
    );

    let mut known_operation_with_payload: Value =
        serde_json::to_value(journal("known-operation")).expect("journal fixture should serialize");
    known_operation_with_payload["journal_revision"] = json!(1);
    known_operation_with_payload["allowed_rollback_operations"] = json!([{
        "kind": "flush_and_stop_app_sidecar",
        "command": "/bin/sh",
        "argv": ["-c", "forbidden"],
        "executable_path": "/tmp/forbidden"
    }]);
    let known_operation_with_payload = serde_json::to_vec_pretty(&known_operation_with_payload)
        .expect("known-operation payload fixture should serialize");
    fs::write(store.handover_journal_path(), &known_operation_with_payload)
        .expect("known-operation payload fixture should write");
    assert!(matches!(
        store.advance_handover(
            &transaction_id("known-operation"),
            MacosHandoverPhase::StopRequested
        ),
        Err(MacosOwnerStoreError::Decode {
            artifact: "handover journal",
            ..
        })
    ));
    assert_eq!(
        fs::read(store.handover_journal_path())
            .expect("known-operation payload bytes should remain"),
        known_operation_with_payload
    );
}

#[cfg(target_os = "macos")]
#[test]
fn proven_guard_winner_repairs_invalid_diagnostic_owner_records() {
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let store = MacosOwnerStore::new(directory.path());
    let guard_name = directory
        .path()
        .join("daemon-instance.lock")
        .to_string_lossy()
        .into_owned();
    let guard = try_acquire_macos_daemon_guard(&guard_name)
        .expect("guard inspection should succeed")
        .expect("fixture winner should acquire the guard");

    for invalid in [
        b"{ malformed owner record".to_vec(),
        serde_json::to_vec(&json!({
            "schema_version": 99,
            "owner_epoch": 1,
            "active_owner": "app_sidecar",
            "active_identity": {
                "audit_token_identity": "audit-old",
                "executable_path": "/Applications/old/hypercolor-daemon",
                "designated_requirement_hash": "requirement-old",
                "pid": 100
            },
            "conflict": null,
            "selected_external_owner": null
        }))
        .expect("future-version fixture should serialize"),
        serde_json::to_vec(&json!({
            "schema_version": MACOS_OWNER_RECORD_SCHEMA_VERSION,
            "owner_epoch": 0,
            "active_owner": "app_sidecar",
            "active_identity": {
                "audit_token_identity": "audit-old",
                "executable_path": "/Applications/old/hypercolor-daemon",
                "designated_requirement_hash": "requirement-old",
                "pid": 100
            },
            "conflict": null,
            "selected_external_owner": null
        }))
        .expect("semantically-invalid fixture should serialize"),
    ] {
        fs::write(store.owner_record_path(), &invalid).expect("invalid fixture should write");
        assert!(
            store
                .publish_owner(MacosDaemonOwner::Homebrew, identity("ordinary", 200))
                .is_err(),
            "ordinary publication must remain fail-closed"
        );
        assert_eq!(
            fs::read(store.owner_record_path()).expect("invalid bytes should remain"),
            invalid
        );

        let repaired = store
            .publish_guard_winner(
                &guard,
                MacosDaemonOwner::Homebrew,
                identity("guard-winner", 201),
            )
            .expect("guard winner should atomically replace invalid diagnostics");
        assert_eq!(repaired.owner_epoch, 1);
        assert_eq!(repaired.active_owner, MacosDaemonOwner::Homebrew);
        assert_eq!(
            store
                .load_owner_record()
                .expect("repaired owner should load")
                .expect("repaired owner should exist"),
            repaired
        );
    }
}

#[test]
fn oversized_artifacts_and_operation_lists_reject_without_mutation() {
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let store = MacosOwnerStore::new(directory.path());
    let oversized = vec![b'x'; MAX_MACOS_OWNER_ARTIFACT_BYTES + 1];
    fs::write(store.owner_record_path(), &oversized).expect("oversized fixture should write");

    assert!(matches!(
        store.publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101)),
        Err(MacosOwnerStoreError::ArtifactTooLarge {
            artifact: "owner record",
            ..
        })
    ));
    assert_eq!(
        fs::read(store.owner_record_path()).expect("oversized bytes should remain"),
        oversized
    );

    let mut excessive_operations = journal("excessive-operations");
    excessive_operations.allowed_rollback_operations =
        vec![MacosHandoverOperation::StartAppSidecar {}; MAX_MACOS_HANDOVER_OPERATIONS + 1];
    assert!(matches!(
        store.begin_handover(excessive_operations),
        Err(MacosOwnerStoreError::InvalidArtifact {
            artifact: "handover journal",
            ..
        })
    ));
    assert!(
        !store.handover_journal_path().exists(),
        "invalid journal must not create durable bytes"
    );
}

#[test]
fn failed_mutation_releases_the_stable_coordination_lock() {
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let store = MacosOwnerStore::new(directory.path());
    store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
        .expect("owner should publish");
    fs::write(store.owner_record_path(), b"not json").expect("fixture corruption should write");

    assert!(matches!(
        store.record_conflict(
            MacosDaemonOwner::DirectLaunchd,
            identity("launchd-contender", 201),
            1
        ),
        Err(MacosOwnerStoreError::Decode { .. })
    ));
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.coordination_lock_path())
        .expect("stable coordination lock should exist");
    lock.try_lock()
        .expect("failed mutation should release its lock");
    lock.unlock().expect("test lock should release");
}

#[test]
fn diagnostic_owner_path_is_bounded_while_the_journal_stays_path_free() {
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let store = MacosOwnerStore::new(directory.path());
    let owner = store
        .publish_owner(MacosDaemonOwner::DirectLaunchd, identity("launchd", 102))
        .expect("owner should publish");
    let journal = store
        .begin_handover(journal("path-free-shape"))
        .expect("journal should begin");

    let owner_value = serde_json::to_value(owner).expect("owner record should serialize");
    assert_eq!(
        owner_value["active_identity"]["executable_path"],
        "/Applications/launchd/hypercolor-daemon"
    );
    assert_path_free(&serde_json::to_value(journal).expect("handover journal should serialize"));
}

#[cfg(unix)]
#[test]
fn durable_owner_artifacts_are_user_read_write_only() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let store = MacosOwnerStore::new(directory.path());
    store
        .publish_owner(MacosDaemonOwner::AppSidecar, identity("sidecar", 101))
        .expect("owner should publish");
    store
        .begin_handover(journal("mode-check"))
        .expect("journal should begin");

    for path in [
        store.owner_record_path(),
        store.handover_journal_path(),
        store.coordination_lock_path(),
    ] {
        let mode = fs::metadata(path)
            .expect("durable artifact metadata should load")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

fn assert_path_free(value: &Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                let normalized = key.to_ascii_lowercase();
                assert!(!normalized.contains("path"), "path key leaked: {key}");
                assert!(!normalized.contains("command"), "command key leaked: {key}");
                assert!(
                    !normalized.contains("argument"),
                    "argument key leaked: {key}"
                );
                assert!(!normalized.contains("argv"), "argv key leaked: {key}");
                assert!(
                    !normalized.contains("executable"),
                    "executable key leaked: {key}"
                );
                assert_path_free(value);
            }
        }
        Value::Array(values) => values.iter().for_each(assert_path_free),
        Value::String(value) => {
            assert!(!value.contains('/'), "path-like value leaked: {value}");
            assert!(!value.contains('\\'), "path-like value leaked: {value}");
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}
