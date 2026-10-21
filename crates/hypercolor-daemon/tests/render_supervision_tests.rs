//! Render supervision reports the failure an earlier daemon run left in the
//! state directory, and its status projection gates private detail.

use hypercolor_daemon::render_thread::supervision::{
    RENDER_FAILURE_FILE, RENDER_RUNTIME_THREAD_NAME, RENDER_THREAD_NAME, is_render_thread_name,
};
use hypercolor_daemon::render_thread::{RenderSupervision, RenderThreadPhase};

/// A record as an earlier run wrote it, before records carried a process
/// instance id.
fn earlier_run_record() -> serde_json::Value {
    serde_json::json!({
        "kind": "panic",
        "message": "frame panicked in the earlier run",
        "thread": "hypercolor-render",
        "location": "crates/hypercolor-daemon/src/render_thread/frame_executor.rs:120:9",
        "occurred_at": "2026-10-19T22:30:00.000Z",
        "occurred_at_unix_ms": 1_792_449_000_000_u64,
        "pid": 1,
        "daemon_version": "0.7.1",
        "restart_attempt": 0,
        "restarts_total": 0,
        "action": "process_abort",
        "backtrace": "  4: hypercolor_daemon::render_thread::frame_executor::execute_frame\n",
    })
}

#[test]
fn a_failure_persisted_by_an_earlier_run_is_reported_with_private_detail_gated() {
    let state_dir = tempfile::tempdir().expect("state tempdir should be created");
    let record_path = state_dir.path().join(RENDER_FAILURE_FILE);
    std::fs::write(
        &record_path,
        serde_json::to_vec(&earlier_run_record()).expect("record should serialize"),
    )
    .expect("record should be written");

    let supervision = RenderSupervision::for_state_dir(state_dir.path());

    assert_eq!(
        supervision.failure_record_path(),
        Some(record_path.as_path())
    );
    let snapshot = supervision.snapshot();
    assert_eq!(snapshot.phase, RenderThreadPhase::NotStarted);
    assert!(!snapshot.last_failure_in_this_process);
    let failure = snapshot
        .last_failure
        .clone()
        .expect("the earlier run's failure should be reported");
    assert_eq!(
        failure.pid, 1,
        "a container's pid 1 must not hide the record"
    );
    assert!(failure.process_instance.is_empty());

    let private = snapshot.status(true);
    assert_eq!(private.phase, "not_started");
    assert_eq!(private.restarts_total, 0);
    let reported = private
        .last_failure
        .expect("status should carry the earlier failure");
    assert_eq!(reported.kind, "panic");
    assert_eq!(reported.action, "process_abort");
    assert_eq!(reported.message, "frame panicked in the earlier run");
    assert!(reported.location.is_some());
    assert!(!reported.current_process);

    let public = snapshot
        .status(false)
        .last_failure
        .expect("kind stays public");
    assert_eq!(public.kind, "panic");
    assert!(public.message.is_empty());
    assert!(public.location.is_none());
    assert!(snapshot.status(false).gpu_errors.last_error.is_none());
}

#[test]
fn an_unreadable_failure_record_is_ignored() {
    let state_dir = tempfile::tempdir().expect("state tempdir should be created");
    std::fs::write(state_dir.path().join(RENDER_FAILURE_FILE), b"not a record")
        .expect("record should be written");

    let staged = state_dir
        .path()
        .join(".render-failure.left-by-a-killed-run.tmp");
    std::fs::write(&staged, b"half written").expect("staged record should be written");

    let supervision = RenderSupervision::for_state_dir(state_dir.path());

    assert!(supervision.snapshot().last_failure.is_none());
    assert!(supervision.termination().is_none());
    assert!(supervision.watchdog_healthy());
    assert!(!staged.exists(), "a stale staged record should be removed");
}

#[test]
fn render_thread_names_cover_the_thread_and_its_runtime_workers() {
    assert!(is_render_thread_name(RENDER_THREAD_NAME));
    assert!(is_render_thread_name(RENDER_RUNTIME_THREAD_NAME));
    assert!(!is_render_thread_name("hypercolor-daemon-runtime"));
    assert!(!is_render_thread_name("servo-worker"));
}
