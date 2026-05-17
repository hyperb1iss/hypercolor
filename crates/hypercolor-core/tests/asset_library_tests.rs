use std::io::Cursor;

use hypercolor_core::asset::{
    AssetEvent, AssetLibrary, AssetLibraryLimits, AssetScanStatus, AssetTypeHint,
    AssetUploadOptions,
};
use image::{ImageBuffer, ImageFormat, Rgba};
use serde_json::json;
use tempfile::TempDir;

fn png_bytes(color: [u8; 4]) -> Vec<u8> {
    let image = ImageBuffer::from_pixel(2, 2, Rgba(color));
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode test png");
    bytes.into_inner()
}

#[test]
fn duplicate_uploads_dedupe_without_mutating_name() {
    let tempdir = TempDir::new().expect("tempdir");
    let mut library = AssetLibrary::open(tempdir.path().join("assets")).expect("open library");
    let bytes = png_bytes([255, 0, 128, 255]);

    let first = library
        .add_bytes(&bytes, AssetUploadOptions::new("../cat?.png"))
        .expect("add first asset");
    let second = library
        .add_bytes(&bytes, AssetUploadOptions::new("renamed.png"))
        .expect("add duplicate asset");

    assert!(!first.duplicate);
    assert!(second.duplicate);
    assert_eq!(first.record.id, second.record.id);
    assert_eq!(second.record.name, "cat_.png");
    assert!(second.events.is_empty());
    assert_eq!(library.records().len(), 1);
    assert!(library.thumbnail_path(first.record.id).exists());
}

#[test]
fn corrupt_index_never_deletes_objects() {
    let tempdir = TempDir::new().expect("tempdir");
    let root = tempdir.path().join("assets");
    let mut library = AssetLibrary::open(&root).expect("open library");
    let bytes = png_bytes([80, 120, 255, 255]);
    let added = library
        .add_bytes(&bytes, AssetUploadOptions::new("logo.png"))
        .expect("add asset");
    let object_path = library
        .object_path_for_hash(&added.record.hash_sha256)
        .expect("object path");

    std::fs::write(library.index_path(), b"not valid json").expect("corrupt index");

    let rebuilt = AssetLibrary::open(&root).expect("reopen library");
    assert!(object_path.exists());
    assert_eq!(rebuilt.records().len(), 1);
    assert_eq!(rebuilt.records()[0].hash_sha256, added.record.hash_sha256);
    assert_eq!(rebuilt.records()[0].scan_status, AssetScanStatus::Unscanned);
}

#[test]
fn watcher_rebuild_preserves_existing_asset_id_to_hash() {
    let tempdir = TempDir::new().expect("tempdir");
    let mut library = AssetLibrary::open(tempdir.path().join("assets")).expect("open library");
    let original_bytes = png_bytes([10, 20, 30, 255]);
    let changed_bytes = png_bytes([200, 30, 10, 255]);

    let added = library
        .add_bytes(&original_bytes, AssetUploadOptions::new("panel.png"))
        .expect("add asset");
    let original_id = added.record.id;
    let original_hash = added.record.hash_sha256.clone();
    let object_path = library
        .object_path_for_hash(&original_hash)
        .expect("object path");
    std::fs::write(&object_path, &changed_bytes).expect("mutate object in place");

    let events = library.refresh_from_disk().expect("refresh from disk");
    let original = library.get(original_id).expect("original record retained");
    let changed_hash = AssetLibrary::hash_bytes(&changed_bytes);

    assert_eq!(original.hash_sha256, original_hash);
    assert!(
        library
            .records()
            .iter()
            .any(|record| { record.id != original_id && record.hash_sha256 == changed_hash })
    );
    assert!(events.iter().any(
        |event| matches!(event, AssetEvent::Added { record } if record.hash_sha256 == changed_hash)
    ));
}

#[test]
fn refresh_from_disk_emits_modified_and_removed_events() {
    let tempdir = TempDir::new().expect("tempdir");
    let mut library = AssetLibrary::open(tempdir.path().join("assets")).expect("open library");
    let bytes = png_bytes([80, 10, 220, 255]);
    let added = library
        .add_bytes(&bytes, AssetUploadOptions::new("source.png"))
        .expect("add asset");

    let raw_index = std::fs::read_to_string(library.index_path()).expect("read index");
    let mut index_json: serde_json::Value =
        serde_json::from_str(&raw_index).expect("parse index json");
    index_json["records"][0]["name"] = json!("renamed.png");
    std::fs::write(
        library.index_path(),
        serde_json::to_vec_pretty(&index_json).expect("serialize index json"),
    )
    .expect("write modified index");

    let events = library.refresh_from_disk().expect("refresh modified index");
    assert!(events.iter().any(
        |event| matches!(event, AssetEvent::Modified { record } if record.id == added.record.id && record.name == "renamed.png")
    ));

    let object_path = library
        .object_path_for_hash(&added.record.hash_sha256)
        .expect("object path");
    std::fs::remove_file(object_path).expect("remove object");
    let events = library.refresh_from_disk().expect("refresh removed object");
    assert!(events.iter().any(
        |event| matches!(event, AssetEvent::Removed { asset_id } if *asset_id == added.record.id)
    ));
}

#[test]
fn size_policy_enforces_hard_cap_and_flags_soft_caps() {
    let tempdir = TempDir::new().expect("tempdir");
    let limits = AssetLibraryLimits {
        per_asset_soft_cap_bytes: 8,
        library_soft_cap_bytes: 8,
        hard_file_cap_bytes: 16,
        thumbnail_size: 32,
    };
    let mut library =
        AssetLibrary::open_with_limits(tempdir.path().join("assets"), limits).expect("open");
    let bytes = png_bytes([1, 2, 3, 255]);

    let error = library
        .add_bytes(&bytes, AssetUploadOptions::new("too-large.png"))
        .expect_err("hard cap rejects");
    assert!(format!("{error}").contains("hard cap"));

    let limits = AssetLibraryLimits {
        hard_file_cap_bytes: 4096,
        ..limits
    };
    let mut library =
        AssetLibrary::open_with_limits(tempdir.path().join("assets-soft"), limits).expect("open");
    let added = library
        .add_bytes(&bytes, AssetUploadOptions::new("soft.png"))
        .expect("soft cap accepts");
    assert_eq!(added.record.warnings.len(), 2);
}

#[test]
fn stream_url_hint_accepts_public_http_sources() {
    let tempdir = TempDir::new().expect("tempdir");
    let mut library = AssetLibrary::open(tempdir.path().join("assets")).expect("open library");
    let mut options = AssetUploadOptions::new("camera.stream");
    options.type_hint = Some(AssetTypeHint::Stream);

    let added = library
        .add_bytes(b"https://media.example.test/live.m3u8\n", options)
        .expect("stream URL asset should upload");

    assert_eq!(
        added.record.mime_type,
        "application/vnd.hypercolor.stream-url"
    );
}

#[test]
fn stream_url_hint_rejects_private_network_sources() {
    let tempdir = TempDir::new().expect("tempdir");
    let mut library = AssetLibrary::open(tempdir.path().join("assets")).expect("open library");
    let mut options = AssetUploadOptions::new("local.stream");
    options.type_hint = Some(AssetTypeHint::Stream);

    let error = library
        .add_bytes(b"http://192.168.1.10/live.m3u8\n", options)
        .expect_err("private stream URL should reject");

    assert!(format!("{error}").contains("unsupported"));
}

#[test]
fn stream_url_hint_rejects_localhost_sources() {
    let tempdir = TempDir::new().expect("tempdir");
    let mut library = AssetLibrary::open(tempdir.path().join("assets")).expect("open library");
    let mut options = AssetUploadOptions::new("localhost.stream");
    options.type_hint = Some(AssetTypeHint::Stream);

    let error = library
        .add_bytes(b"http://localhost:8080/live.m3u8\n", options)
        .expect_err("localhost stream URL should reject");

    assert!(format!("{error}").contains("unsupported"));
}
