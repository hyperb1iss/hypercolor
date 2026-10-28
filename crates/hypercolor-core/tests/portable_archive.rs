use hypercolor_core::portable_setup::{
    SetupBlobStore,
    archive::{read_archive, write_archive},
};
use hypercolor_types::portable_setup::{
    PortableAsset, PortableBackupV1, PortableOutputState, PortablePreferences, PortableSetupV1,
};
use std::io::{self, Read, Seek, SeekFrom, Write};
use uuid::Uuid;

fn backup() -> PortableBackupV1 {
    PortableBackupV1 {
        tui_preferences: None,
        schema_version: 1,
        setup: PortableSetupV1::new(
            Uuid::now_v7(),
            "Desk".into(),
            PortableOutputState {
                canvas_width: 640,
                canvas_height: 480,
                brightness: 0.8,
                enabled: true,
                paused: false,
            },
        ),
        preferences: Some(PortablePreferences::default()),
        favorites: Vec::new(),
    }
}

fn append(archive: &mut tar::Builder<Vec<u8>>, path: &str, data: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o600);
    archive
        .append_data(&mut header, path, data)
        .expect("portable backup fixture should be valid");
}

#[test]
fn large_media_streams_once_for_multiple_asset_aliases_and_round_trips() {
    let source_dir = tempfile::tempdir().expect("portable backup fixture should be valid");
    let destination_dir = tempfile::tempdir().expect("portable backup fixture should be valid");
    let source = SetupBlobStore::open(source_dir.path().join("blobs"))
        .expect("portable backup fixture should be valid");
    let destination = SetupBlobStore::open(destination_dir.path().join("blobs"))
        .expect("portable backup fixture should be valid");
    let path = source_dir.path().join("large-media");
    let mut file = std::fs::File::create(&path).expect("portable backup fixture should be valid");
    io::copy(&mut io::repeat(0x7b).take(17 * 1024 * 1024), &mut file)
        .expect("portable backup fixture should be valid");
    file.sync_all()
        .expect("portable backup fixture should be valid");
    let descriptor = source
        .capture(&path)
        .expect("portable backup fixture should be valid");
    let mut manifest = backup();
    for name in ["First alias", "Second alias"] {
        manifest.setup.assets.insert(
            Uuid::now_v7(),
            PortableAsset {
                name: name.into(),
                mime_type: "video/mp4".into(),
                tags: Vec::new(),
                content: descriptor.clone(),
            },
        );
    }
    let mut archive = tempfile::tempfile().expect("portable backup fixture should be valid");
    write_archive(&mut archive, &manifest, &source)
        .expect("portable backup fixture should be valid");
    assert!(
        archive
            .metadata()
            .expect("portable backup fixture should be valid")
            .len()
            < descriptor.byte_len + 8192
    );
    archive
        .seek(SeekFrom::Start(0))
        .expect("portable backup fixture should be valid");
    let restored = read_archive(BoundedRead(archive), &destination)
        .expect("portable backup fixture should be valid");
    assert_eq!(restored, manifest);
    assert_eq!(
        destination
            .read(&descriptor)
            .expect("portable backup fixture should be valid")
            .file
            .metadata()
            .expect("portable backup fixture should be valid")
            .len(),
        descriptor.byte_len
    );
}

struct BoundedRead<R>(R);
impl<R: Read> Read for BoundedRead<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        assert!(bytes.len() <= 64 * 1024, "unbounded media read");
        self.0.read(bytes)
    }
}

#[test]
fn missing_corrupt_duplicate_and_unreferenced_objects_never_return_a_manifest() {
    let dir = tempfile::tempdir().expect("portable backup fixture should be valid");
    let blobs = SetupBlobStore::open(dir.path()).expect("portable backup fixture should be valid");
    let path = dir.path().join("source");
    std::fs::write(&path, b"abc").expect("portable backup fixture should be valid");
    let descriptor = blobs
        .capture(&path)
        .expect("portable backup fixture should be valid");
    let mut manifest = backup();
    manifest.setup.assets.insert(
        Uuid::now_v7(),
        PortableAsset {
            name: "Image".into(),
            mime_type: "image/png".into(),
            tags: Vec::new(),
            content: descriptor.clone(),
        },
    );
    let json = serde_json::to_vec(&manifest).expect("portable backup fixture should be valid");
    for entries in [
        vec![],
        vec![b"bad".as_slice()],
        vec![b"abc".as_slice(), b"abc".as_slice()],
    ] {
        let mut archive = tar::Builder::new(Vec::new());
        append(&mut archive, "manifest.json", &json);
        for bytes in entries {
            append(&mut archive, &format!("blobs/{}", descriptor.sha256), bytes);
        }
        let bytes = archive
            .into_inner()
            .expect("portable backup fixture should be valid");
        assert!(read_archive(bytes.as_slice(), &blobs).is_err());
    }
    let mut archive = tar::Builder::new(Vec::new());
    append(
        &mut archive,
        "manifest.json",
        &serde_json::to_vec(&backup()).expect("portable backup fixture should be valid"),
    );
    append(&mut archive, "unexpected-file", b"x");
    assert!(
        read_archive(
            archive
                .into_inner()
                .expect("portable backup fixture should be valid")
                .as_slice(),
            &blobs
        )
        .is_err()
    );
}

#[test]
fn archive_rejects_links_truncation_appended_payload_and_wrong_version() {
    let dir = tempfile::tempdir().expect("portable backup fixture should be valid");
    let blobs = SetupBlobStore::open(dir.path()).expect("portable backup fixture should be valid");
    let manifest = backup();
    let mut valid = Vec::new();
    write_archive(&mut valid, &manifest, &blobs).expect("portable backup fixture should be valid");
    assert_eq!(
        read_archive(valid.as_slice(), &blobs).expect("portable backup fixture should be valid"),
        manifest
    );
    for length in [
        0,
        511,
        valid.len() - 1,
        valid.len() - 512,
        valid.len() - 1024,
    ] {
        assert!(
            read_archive(&valid[..length], &blobs).is_err(),
            "accepted truncation at {length}"
        );
    }
    valid.extend_from_slice(b"hidden payload");
    assert!(read_archive(valid.as_slice(), &blobs).is_err());
    let mut archive = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o600);
    archive
        .append_link(&mut header, "manifest.json", "outside")
        .expect("portable backup fixture should be valid");
    assert!(
        read_archive(
            archive
                .into_inner()
                .expect("portable backup fixture should be valid")
                .as_slice(),
            &blobs
        )
        .is_err()
    );
    let mut unsupported = manifest;
    unsupported.schema_version = 2;
    assert!(write_archive(Vec::new(), &unsupported, &blobs).is_err());
}

#[test]
fn failed_output_is_reported_and_a_damaged_source_is_not_exported() {
    struct FailingWriter;
    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("disk full"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let dir = tempfile::tempdir().expect("portable backup fixture should be valid");
    let blobs = SetupBlobStore::open(dir.path()).expect("portable backup fixture should be valid");
    assert!(write_archive(FailingWriter, &backup(), &blobs).is_err());
    let path = dir.path().join("input");
    std::fs::write(&path, b"original").expect("portable backup fixture should be valid");
    let descriptor = blobs
        .capture(&path)
        .expect("portable backup fixture should be valid");
    let mut manifest = backup();
    manifest.setup.assets.insert(
        Uuid::now_v7(),
        PortableAsset {
            name: "Media".into(),
            mime_type: "video/mp4".into(),
            tags: Vec::new(),
            content: descriptor.clone(),
        },
    );
    std::fs::write(dir.path().join(&descriptor.sha256), b"corrupted")
        .expect("portable backup fixture should be valid");
    assert!(write_archive(Vec::new(), &manifest, &blobs).is_err());
}
