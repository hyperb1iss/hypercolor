#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::PermissionsExt;

use hypercolor_macos_owner::{
    MACOS_DAEMON_SESSION_ATTESTATION_SCHEMA_VERSION, MAX_MACOS_OWNER_ARTIFACT_BYTES,
    MacosDaemonOwner, MacosOwnerIdentity, MacosOwnerStore, MacosProtectedControlCredential,
    MacosServerSessionId, try_acquire_macos_daemon_guard,
};
use serde_json::{Value, json};

fn identity(label: &str, pid: u32) -> MacosOwnerIdentity {
    MacosOwnerIdentity::new(
        format!("audit-{label}"),
        format!("/Applications/{label}/hypercolor-daemon"),
        format!("requirement-{label}"),
        pid,
    )
    .expect("fixture identity should be valid")
}

fn publish_fixture(
    directory: &tempfile::TempDir,
    label: &str,
) -> (
    MacosOwnerStore,
    hypercolor_macos_owner::MacosDaemonGuard,
    hypercolor_macos_owner::MacosOwnerRecord,
    hypercolor_macos_owner::MacosDaemonSessionAttestation,
) {
    let store = MacosOwnerStore::new(directory.path().join("state"));
    let guard_path = directory.path().join("daemon.lock");
    let guard = try_acquire_macos_daemon_guard(&guard_path.to_string_lossy())
        .expect("guard lookup should succeed")
        .expect("fixture should acquire the canonical guard");
    let record = store
        .publish_guard_winner(
            &guard,
            MacosDaemonOwner::AppSidecar,
            identity(label, std::process::id()),
        )
        .expect("guard winner should publish");
    let attestation = store
        .publish_daemon_session_attestation(&guard, &record.incarnation())
        .expect("exact guard winner should publish a session");
    (store, guard, record, attestation)
}

#[test]
fn session_attestation_is_private_exact_and_not_an_owner_lease() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let guard_path = directory.path().join("daemon.lock");
    let (store, guard, record, attestation) = publish_fixture(&directory, "first");
    let path = store.daemon_session_attestation_path();

    assert_eq!(
        fs::metadata(&path)
            .expect("attestation metadata should load")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(attestation.owner_incarnation(), record.incarnation());
    assert_eq!(
        store
            .load_daemon_session_attestation()
            .expect("attestation should load"),
        Some(attestation.clone())
    );
    assert!(
        try_acquire_macos_daemon_guard(&guard_path.to_string_lossy())
            .expect("contending guard lookup should succeed")
            .is_none(),
        "session publication must not create or replace the canonical owner lease"
    );

    let secret = attestation
        .protected_control_credential
        .expose_secret()
        .to_owned();
    let debug = format!("{attestation:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains(&secret));

    assert!(
        store
            .clear_daemon_session_attestation(
                &record.incarnation(),
                &attestation.server_session_id,
            )
            .expect("matching session should clear")
    );
    assert!(!path.exists());
    drop(guard);
}

#[test]
fn load_rejects_wrong_mode_topology_identity_and_epoch() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let (store, _guard, _record, _attestation) = publish_fixture(&directory, "validation");
    let path = store.daemon_session_attestation_path();
    let valid = fs::read(&path).expect("valid attestation should read");

    fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
        .expect("fixture mode should change");
    assert!(
        store
            .load_daemon_session_attestation()
            .expect_err("public mode must fail closed")
            .to_string()
            .contains("mode must be 0600")
    );

    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .expect("fixture mode should restore");
    let mut wrong_topology: Value =
        serde_json::from_slice(&valid).expect("valid attestation should decode as JSON");
    wrong_topology["owner"] = json!("standalone");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&wrong_topology).expect("fixture should encode"),
    )
    .expect("wrong topology fixture should write");
    assert!(
        store
            .load_daemon_session_attestation()
            .expect_err("wrong topology must fail closed")
            .to_string()
            .contains("not current")
    );

    let mut wrong_identity: Value =
        serde_json::from_slice(&valid).expect("valid attestation should decode as JSON");
    wrong_identity["owner_identity"]["pid"] = json!(std::process::id().saturating_add(1));
    fs::write(
        &path,
        serde_json::to_vec_pretty(&wrong_identity).expect("fixture should encode"),
    )
    .expect("wrong identity fixture should write");
    assert!(
        store
            .load_daemon_session_attestation()
            .expect_err("wrong identity must fail closed")
            .to_string()
            .contains("not current")
    );

    let mut wrong_epoch: Value =
        serde_json::from_slice(&valid).expect("valid attestation should decode as JSON");
    wrong_epoch["owner_epoch"] = json!(99);
    fs::write(
        &path,
        serde_json::to_vec_pretty(&wrong_epoch).expect("fixture should encode"),
    )
    .expect("wrong epoch fixture should write");
    assert!(
        store
            .load_daemon_session_attestation()
            .expect_err("wrong epoch must fail closed")
            .to_string()
            .contains("not current")
    );
}

#[test]
fn publication_requires_the_exact_current_incarnation() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let (store, guard, record, _attestation) = publish_fixture(&directory, "publication");
    let mut stale = record.incarnation();
    stale.owner_epoch = stale.owner_epoch.saturating_add(1);

    assert!(
        store
            .publish_daemon_session_attestation(&guard, &stale)
            .expect_err("noncurrent incarnation must not publish")
            .to_string()
            .contains("guard-winning incarnation")
    );
}

#[test]
fn load_rejects_an_oversized_session_artifact() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let (store, _guard, _record, _attestation) = publish_fixture(&directory, "bounded");
    fs::write(
        store.daemon_session_attestation_path(),
        vec![b'x'; MAX_MACOS_OWNER_ARTIFACT_BYTES + 1],
    )
    .expect("oversized fixture should write");

    assert!(
        store
            .load_daemon_session_attestation()
            .expect_err("oversized session must fail before decoding")
            .to_string()
            .contains("exceeds the 262144-byte limit")
    );
}

#[test]
fn clear_requires_the_exact_owner_epoch_identity_and_session() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let (store, _guard, record, attestation) = publish_fixture(&directory, "clear");
    let wrong_session = MacosServerSessionId::from_bytes([0x55; 16]);
    assert!(
        store
            .clear_daemon_session_attestation(&record.incarnation(), &wrong_session)
            .expect_err("wrong session must not clear")
            .to_string()
            .contains("does not match")
    );
    assert!(store.daemon_session_attestation_path().exists());

    let replacement = store
        .publish_owner(
            MacosDaemonOwner::AppSidecar,
            identity("replacement", std::process::id()),
        )
        .expect("new owner epoch should publish");
    assert!(
        store
            .clear_daemon_session_attestation(
                &record.incarnation(),
                &attestation.server_session_id,
            )
            .expect_err("stale owner must not clear")
            .to_string()
            .contains("clearing incarnation")
    );
    assert!(replacement.owner_epoch > record.owner_epoch);
    assert!(store.daemon_session_attestation_path().exists());
}

#[test]
fn next_guard_winner_replaces_a_stale_crash_session() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let guard_path = directory.path().join("daemon.lock");
    let (store, first_guard, first_record, first) = publish_fixture(&directory, "crashed");
    drop(first_guard);

    let second_guard = try_acquire_macos_daemon_guard(&guard_path.to_string_lossy())
        .expect("second guard lookup should succeed")
        .expect("next process should acquire the released guard");
    let second_record = store
        .publish_guard_winner(
            &second_guard,
            MacosDaemonOwner::DirectLaunchd,
            identity("replacement", std::process::id()),
        )
        .expect("next guard winner should publish");
    let second = store
        .publish_daemon_session_attestation(&second_guard, &second_record.incarnation())
        .expect("next guard winner should replace the stale session");

    assert!(second_record.owner_epoch > first_record.owner_epoch);
    assert_ne!(second.server_session_id, first.server_session_id);
    assert_ne!(
        second.protected_control_credential,
        first.protected_control_credential
    );
    assert_eq!(
        store
            .load_daemon_session_attestation()
            .expect("replacement should load"),
        Some(second)
    );
}

#[test]
fn schema_v1_shape_is_separate_and_credential_has_256_random_bits() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let (_store, _guard, _record, attestation) = publish_fixture(&directory, "shape");
    let value = serde_json::to_value(&attestation).expect("attestation should encode");
    let mut keys = value
        .as_object()
        .expect("attestation should be an object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();

    assert_eq!(
        keys,
        [
            "owner",
            "owner_epoch",
            "owner_identity",
            "protected_control_credential",
            "schema_version",
            "server_session_id",
        ]
    );
    assert_eq!(
        attestation.schema_version,
        MACOS_DAEMON_SESSION_ATTESTATION_SCHEMA_VERSION
    );
    assert_eq!(
        attestation
            .protected_control_credential
            .expose_secret()
            .strip_prefix("hc_pc_")
            .expect("credential should have its type prefix")
            .len(),
        64
    );
    assert_ne!(
        attestation.protected_control_credential,
        MacosProtectedControlCredential::from_bytes([0; 32])
    );
}
