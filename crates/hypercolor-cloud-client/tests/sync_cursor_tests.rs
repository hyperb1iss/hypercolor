use chrono::{DateTime, Utc};
use hypercolor_cloud_client::SyncCursor;

#[test]
fn sync_cursor_missing_file_loads_empty() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let path = temp.path().join("cloud/cursor.toml");
    std::fs::create_dir_all(path.parent().expect("cursor should have a parent"))
        .expect("cursor parent should create");

    let cursor = SyncCursor::load(path).expect("missing cursor should not fail");

    assert!(cursor.is_none());
}

#[test]
fn sync_cursor_saves_and_round_trips() {
    let temp = tempfile::tempdir().expect("temp dir should create");
    let path = temp.path().join("cloud/cursor.toml");
    let cursor = SyncCursor::new(42, fixed_time("2026-05-15T17:00:00Z"));

    cursor.save(&path).expect("cursor should save");
    let loaded = SyncCursor::load(&path)
        .expect("cursor should load")
        .expect("cursor should exist");

    assert_eq!(loaded, cursor);
    assert!(!path.with_file_name("cursor.toml.tmp").exists());
}

#[test]
fn sync_cursor_record_sync_result_preserves_highest_seen_sequence() {
    let mut cursor = SyncCursor::new(42, fixed_time("2026-05-15T17:00:00Z"));

    cursor.record_sync_result(41, fixed_time("2026-05-15T17:05:00Z"));
    assert_eq!(cursor.last_seen_seq, 42);
    assert_eq!(cursor.last_sync_at, fixed_time("2026-05-15T17:05:00Z"));

    cursor.record_sync_result(44, fixed_time("2026-05-15T17:10:00Z"));
    assert_eq!(cursor.last_seen_seq, 44);
    assert_eq!(cursor.last_sync_at, fixed_time("2026-05-15T17:10:00Z"));
}

fn fixed_time(input: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(input)
        .expect("fixture timestamp should parse")
        .with_timezone(&Utc)
}
