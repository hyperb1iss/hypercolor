use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::portable_setup::{SetupBlobError, SetupBlobStore};
use hypercolor_types::portable_setup::PortableBlob;
use std::io::{Cursor, Read};

#[test]
fn streamed_blob_round_trip_and_reopen_use_verified_content() {
    let dir = tempfile::tempdir().expect("temporary root");
    let bytes = vec![42; 200_000];
    let descriptor = PortableBlob {
        sha256: AssetLibrary::hash_bytes(&bytes),
        byte_len: bytes.len() as u64,
    };
    let store = SetupBlobStore::open(dir.path()).expect("store");
    store
        .stage(&descriptor, Cursor::new(&bytes))
        .expect("stage");
    let reopened = SetupBlobStore::open(dir.path()).expect("reopen");
    let mut blob = reopened.read(&descriptor).expect("verified reader");
    let mut actual = Vec::new();
    blob.file.read_to_end(&mut actual).expect("read");
    assert_eq!(actual, bytes);
}

#[test]
fn incomplete_wrong_hash_and_oversize_transfers_never_publish() {
    let dir = tempfile::tempdir().expect("temporary root");
    let store = SetupBlobStore::open(dir.path()).expect("store");
    let descriptor = PortableBlob {
        sha256: AssetLibrary::hash_bytes(b"good"),
        byte_len: 4,
    };
    for bytes in [
        b"bad".as_slice(),
        b"evil".as_slice(),
        b"too long".as_slice(),
    ] {
        assert!(matches!(
            store.stage(&descriptor, Cursor::new(bytes)),
            Err(SetupBlobError::ContentMismatch)
        ));
        assert!(store.read(&descriptor).is_err());
    }
    assert_eq!(std::fs::read_dir(dir.path()).expect("directory").count(), 0);
}

#[test]
fn corruption_is_detected_before_restore_reads_a_blob() {
    let dir = tempfile::tempdir().expect("temporary root");
    let store = SetupBlobStore::open(dir.path()).expect("store");
    let descriptor = PortableBlob {
        sha256: AssetLibrary::hash_bytes(b"good"),
        byte_len: 4,
    };
    store
        .stage(&descriptor, Cursor::new(b"good"))
        .expect("stage");
    std::fs::write(dir.path().join(&descriptor.sha256), b"evil").expect("corrupt object");
    assert!(matches!(
        store.read(&descriptor),
        Err(SetupBlobError::ContentMismatch)
    ));
}

#[test]
fn capture_cache_reuses_unchanged_object_and_detects_source_and_cache_changes() {
    let source_root = tempfile::tempdir().expect("source root");
    let cache_root = tempfile::tempdir().expect("cache root");
    let source = source_root.path().join("media");
    std::fs::write(&source, b"first").expect("source");
    let store = SetupBlobStore::open(cache_root.path()).expect("store");
    let first = store.capture(&source).expect("initial capture");
    let object = cache_root.path().join(&first.sha256);
    let stamp = hypercolor_platform_fs::path_change_stamp(&object).expect("stamp");
    assert_eq!(store.capture(&source).expect("cached capture"), first);
    assert_eq!(
        hypercolor_platform_fs::path_change_stamp(&object).expect("stamp"),
        stamp,
        "unchanged content must not be replaced"
    );
    std::fs::write(&object, b"wrong").expect("corrupt cache");
    assert_eq!(
        store
            .capture(&source)
            .expect("repair from unchanged source"),
        first
    );
    let mut restored = Vec::new();
    store
        .read(&first)
        .expect("verified")
        .file
        .read_to_end(&mut restored)
        .expect("read");
    assert_eq!(restored, b"first");
    std::fs::write(&source, b"other").expect("change same-size source");
    let second = store.capture(&source).expect("capture changed file");
    assert_ne!(first.sha256, second.sha256);
}
