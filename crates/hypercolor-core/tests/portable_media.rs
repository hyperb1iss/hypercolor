use std::collections::BTreeMap;
use std::io::{Cursor, Write};

use hypercolor_core::asset::{AssetLibrary, AssetUploadOptions};
use hypercolor_core::portable_setup::SetupBlobStore;
use hypercolor_persistence::{AtomicWriteWaitOutcome, PersistenceShutdown};
use hypercolor_types::asset::AssetId;
use hypercolor_types::portable_setup::{PortableAsset, PortableBlob};
use image::{ImageBuffer, ImageFormat, Rgba};
use tempfile::TempDir;
use uuid::Uuid;

fn image_bytes() -> Vec<u8> {
    let image = ImageBuffer::from_pixel(2, 2, Rgba([20_u8, 40, 80, 255]));
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode PNG");
    bytes.into_inner()
}

fn asset(bytes: &[u8], mime: &str) -> PortableAsset {
    PortableAsset {
        name: "authored asset".into(),
        mime_type: mime.into(),
        tags: vec!["custom".into()],
        content: PortableBlob {
            sha256: AssetLibrary::hash_bytes(bytes),
            byte_len: bytes.len() as u64,
        },
    }
}

#[test]
fn fresh_restore_preserves_aliases_and_waits_for_publication() {
    let root = TempDir::new().expect("tempdir");
    let blobs = SetupBlobStore::open(root.path().join("blobs")).expect("blobs");
    let mut library = AssetLibrary::open(root.path().join("assets")).expect("library");
    let media = asset(&image_bytes(), "image/png");
    blobs
        .stage(&media.content, Cursor::new(image_bytes()))
        .expect("stage");
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let desired = BTreeMap::from([(first, media.clone()), (second, media.clone())]);
    let before_index = std::fs::read(library.index_path()).expect("index");
    let mut prepared = library
        .prepare_portable_replace(&BTreeMap::new(), &desired, &blobs)
        .expect("prepare");
    assert!(library.records().is_empty());
    assert_eq!(
        std::fs::read(library.index_path()).expect("index"),
        before_index
    );
    assert!(
        !library
            .object_path_for_hash(&media.content.sha256)
            .expect("path")
            .exists()
    );
    assert_eq!(
        prepared
            .persist(&PersistenceShutdown::default())
            .expect("persist"),
        AtomicWriteWaitOutcome::Written
    );
    assert!(library.records().is_empty());
    prepared.publish(&mut library);
    assert_eq!(library.portable_snapshot(), desired);
    assert_eq!(
        library.get(AssetId(first)).expect("asset").intrinsic_width,
        Some(2)
    );
    assert!(library.thumbnail_path(AssetId(second)).exists());
    let reopened = AssetLibrary::open(library.root()).expect("reopen");
    assert_eq!(reopened.portable_snapshot(), desired);
    let remaining = BTreeMap::from([(second, media.clone())]);
    let mut deletion = library
        .prepare_portable_replace(&desired, &remaining, &blobs)
        .expect("remove one alias");
    deletion
        .persist(&PersistenceShutdown::default())
        .expect("persist alias deletion");
    deletion.publish(&mut library);
    assert!(
        library
            .object_path_for_hash(&media.content.sha256)
            .expect("shared object")
            .exists()
    );
    let mut reopened = AssetLibrary::open(library.root()).expect("reopen one alias");
    reopened.refresh_from_disk().expect("refresh");
    assert_eq!(reopened.portable_snapshot(), remaining);
}

#[test]
fn deleting_last_alias_does_not_resurrect_an_object_on_empty_index_restart() {
    let root = TempDir::new().expect("tempdir");
    let blobs = SetupBlobStore::open(root.path().join("blobs")).expect("blobs");
    let mut library = AssetLibrary::open(root.path().join("assets")).expect("library");
    let record = library
        .add_bytes(&image_bytes(), AssetUploadOptions::new("old"))
        .expect("add")
        .record;
    let before = library.portable_snapshot();
    let mut prepared = library
        .prepare_portable_replace(&before, &BTreeMap::new(), &blobs)
        .expect("prepare");
    prepared
        .persist(&PersistenceShutdown::default())
        .expect("persist");
    prepared.publish(&mut library);
    assert!(
        !library
            .object_path_for_hash(&record.hash_sha256)
            .expect("path")
            .exists()
    );
    assert!(
        AssetLibrary::open(library.root())
            .expect("reopen")
            .records()
            .is_empty()
    );
}

#[test]
fn corruption_wrong_mime_and_stale_snapshot_are_rejected_before_admission() {
    let root = TempDir::new().expect("tempdir");
    let blobs = SetupBlobStore::open(root.path().join("blobs")).expect("blobs");
    let library = AssetLibrary::open(root.path().join("assets")).expect("library");
    let bytes = image_bytes();
    let mut media = asset(&bytes, "video/mp4");
    blobs
        .stage(&media.content, Cursor::new(&bytes))
        .expect("stage");
    let id = Uuid::new_v4();
    let desired = BTreeMap::from([(id, media.clone())]);
    assert!(
        library
            .prepare_portable_replace(&BTreeMap::new(), &desired, &blobs)
            .is_err()
    );
    media.mime_type = "image/png".into();
    let desired = BTreeMap::from([(id, media.clone())]);
    assert!(
        library
            .prepare_portable_replace(&desired, &desired, &blobs)
            .is_err()
    );
    std::fs::write(
        root.path().join("blobs").join(&media.content.sha256),
        b"corrupt",
    )
    .expect("corrupt");
    assert!(
        library
            .prepare_portable_replace(&BTreeMap::new(), &desired, &blobs)
            .is_err()
    );
    assert!(library.records().is_empty());
}

#[test]
fn interrupted_file_step_retries_and_shutdown_never_publishes() {
    let root = TempDir::new().expect("tempdir");
    let blobs = SetupBlobStore::open(root.path().join("blobs")).expect("blobs");
    let mut library = AssetLibrary::open(root.path().join("assets")).expect("library");
    let media = asset(&image_bytes(), "image/png");
    blobs
        .stage(&media.content, Cursor::new(image_bytes()))
        .expect("stage");
    let desired = BTreeMap::from([(Uuid::new_v4(), media.clone())]);
    let mut prepared = library
        .prepare_portable_replace(&BTreeMap::new(), &desired, &blobs)
        .expect("prepare");
    let path = library
        .object_path_for_hash(&media.content.sha256)
        .expect("path");
    let shard = path.parent().expect("shard");
    std::fs::write(shard, b"block directory creation").expect("inject fault");
    assert!(prepared.persist(&PersistenceShutdown::default()).is_err());
    std::fs::remove_file(shard).expect("clear fault");
    assert_eq!(
        prepared
            .persist(&PersistenceShutdown::default())
            .expect("retry"),
        AtomicWriteWaitOutcome::Written
    );
    prepared.publish(&mut library);
    let mut deletion = library
        .prepare_portable_replace(&desired, &BTreeMap::new(), &blobs)
        .expect("deletion");
    let shutdown = PersistenceShutdown::default();
    shutdown.writers_stopped();
    assert_eq!(
        deletion.persist(&shutdown).expect("shutdown"),
        AtomicWriteWaitOutcome::ShutdownPending
    );
    assert!(path.exists());
    assert_eq!(library.portable_snapshot(), desired);
}

#[test]
fn large_video_is_scanned_and_copied_from_a_file() {
    let root = TempDir::new().expect("tempdir");
    let path = root.path().join("video.mp4");
    let mut file = std::fs::File::create(&path).expect("video");
    file.write_all(b"\0\0\0\x18ftypmp42").expect("header");
    file.set_len(136 * 1024 * 1024).expect("length");
    let blobs = SetupBlobStore::open(root.path().join("blobs")).expect("blobs");
    let content = blobs.capture(&path).expect("capture");
    let media = PortableAsset {
        name: "video".into(),
        mime_type: "video/mp4".into(),
        tags: vec![],
        content,
    };
    let desired = BTreeMap::from([(Uuid::new_v4(), media)]);
    let mut library = AssetLibrary::open(root.path().join("assets")).expect("library");
    let mut prepared = library
        .prepare_portable_replace(&BTreeMap::new(), &desired, &blobs)
        .expect("prepare");
    prepared
        .persist(&PersistenceShutdown::default())
        .expect("persist");
    prepared.publish(&mut library);
    assert_eq!(library.records()[0].byte_len, 136 * 1024 * 1024);
}
