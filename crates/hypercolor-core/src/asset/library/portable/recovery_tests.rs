use super::*;
use std::io::Cursor;

fn image_bytes() -> Vec<u8> {
    let image = image::ImageBuffer::from_pixel(2, 2, image::Rgba([20_u8, 40, 80, 255]));
    let mut cursor = Cursor::new(Vec::new());
    image.write_to(&mut cursor, ImageFormat::Png).expect("PNG");
    cursor.into_inner()
}

#[test]
fn pending_fresh_restore_keeps_empty_index_authoritative_until_recovery() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("assets");
    let blobs = SetupBlobStore::open(temp.path().join("blobs")).expect("blobs");
    let bytes = image_bytes();
    let asset = PortableAsset {
        name: "Recovered".into(),
        mime_type: "image/png".into(),
        tags: vec![],
        content: PortableBlob {
            sha256: AssetLibrary::hash_bytes(&bytes),
            byte_len: bytes.len() as u64,
        },
    };
    blobs
        .stage(&asset.content, Cursor::new(&bytes))
        .expect("stage");
    let desired = BTreeMap::from([(Uuid::new_v4(), asset)]);
    let operation_id = Uuid::new_v4();
    let library = AssetLibrary::open(&root).expect("open");
    let mut prepared = library
        .prepare_portable_replace(&BTreeMap::new(), &desired, &blobs, operation_id)
        .expect("prepare");
    assert_eq!(
        prepared
            .persist_files(&PersistenceShutdown::default())
            .expect("objects"),
        AtomicWriteWaitOutcome::Written
    );
    drop(prepared);
    drop(library);
    let mut restarted = AssetLibrary::open(&root).expect("restart before index");
    assert!(restarted.refresh_from_disk().is_err());
    assert!(restarted.rebuild_index_from_objects().is_err());
    assert!(
        restarted.records().is_empty(),
        "no invented random IDs from partial object publication"
    );
    assert!(
        restarted
            .prepare_portable_replace(&BTreeMap::new(), &desired, &blobs, Uuid::new_v4())
            .is_err()
    );
    let mut resumed = restarted
        .prepare_portable_recovery(&BTreeMap::new(), &desired, &blobs, operation_id)
        .expect("recover");
    resumed
        .persist(&PersistenceShutdown::default())
        .expect("persist");
    resumed.publish(&mut restarted);
    assert_eq!(restarted.portable_snapshot(), desired);
    assert!(
        finalize_portable_restore(
            &root,
            Uuid::new_v4(),
            &desired,
            &PersistenceShutdown::default()
        )
        .is_err()
    );
    finalize_portable_restore(
        &root,
        operation_id,
        &desired,
        &PersistenceShutdown::default(),
    )
    .expect("finalize receipt");
    assert!(!has_pending_restore(&root).expect("pending"));
    assert_eq!(
        AssetLibrary::open(&root)
            .expect("restart complete")
            .portable_snapshot(),
        desired
    );
}

#[test]
fn deletion_recovers_after_retirement_before_index_and_requires_exact_operation() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("assets");
    let blobs = SetupBlobStore::open(temp.path().join("blobs")).expect("blobs");
    let mut library = AssetLibrary::open(&root).expect("open");
    let record = library
        .add_bytes(&image_bytes(), AssetUploadOptions::new("Old"))
        .expect("add")
        .record;
    let before = library.portable_snapshot();
    let operation_id = Uuid::new_v4();
    let mut prepared = library
        .prepare_portable_replace(&before, &BTreeMap::new(), &blobs, operation_id)
        .expect("prepare");
    prepared
        .persist_files(&PersistenceShutdown::default())
        .expect("retire");
    assert!(
        !library
            .object_path_for_hash(&record.hash_sha256)
            .expect("path")
            .exists()
    );
    assert_eq!(
        prepared
            .persist_files(&PersistenceShutdown::default())
            .expect("repeat retirement durability barrier"),
        AtomicWriteWaitOutcome::Written
    );
    drop(prepared);
    drop(library);
    let mut restarted = AssetLibrary::open(&root).expect("restart");
    assert!(
        restarted
            .prepare_portable_recovery(&before, &BTreeMap::new(), &blobs, Uuid::new_v4())
            .is_err()
    );
    let mut resumed = restarted
        .prepare_portable_recovery(&before, &BTreeMap::new(), &blobs, operation_id)
        .expect("recover");
    resumed
        .persist(&PersistenceShutdown::default())
        .expect("persist");
    resumed.publish(&mut restarted);
    finalize_portable_restore(
        &root,
        operation_id,
        &BTreeMap::new(),
        &PersistenceShutdown::default(),
    )
    .expect("finalize");
    assert!(
        AssetLibrary::open(&root)
            .expect("reopen")
            .records()
            .is_empty()
    );
}

#[test]
fn invalid_pending_intent_never_bypasses_startup_validation() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("assets");
    let library = AssetLibrary::open(&root).expect("open");
    drop(library);
    fs::write(root.join("portable-restore.json"), b"{invalid").expect("bad intent");
    assert!(AssetLibrary::open(&root).is_err());
}

#[test]
fn global_intent_before_media_admission_can_resume_only_exact_before_image() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("assets");
    let blobs = SetupBlobStore::open(temp.path().join("blobs")).expect("blobs");
    let mut library = AssetLibrary::open(&root).expect("open");
    let operation_id = Uuid::new_v4();
    let mut prepared = library
        .prepare_portable_recovery(&BTreeMap::new(), &BTreeMap::new(), &blobs, operation_id)
        .expect("global journal precedes local participant");
    prepared
        .persist(&PersistenceShutdown::default())
        .expect("persist");
    prepared.publish(&mut library);
    finalize_portable_restore(
        &root,
        operation_id,
        &BTreeMap::new(),
        &PersistenceShutdown::default(),
    )
    .expect("finalize");
    assert!(
        library
            .prepare_portable_recovery(&BTreeMap::new(), &BTreeMap::new(), &blobs, Uuid::new_v4())
            .is_ok(),
        "completed prior operation does not block new journal recovery"
    );
    library
        .add_bytes(&image_bytes(), AssetUploadOptions::new("Concurrent edit"))
        .expect("edit");
    assert!(
        library
            .prepare_portable_recovery(&BTreeMap::new(), &BTreeMap::new(), &blobs, Uuid::new_v4())
            .is_err()
    );
}
