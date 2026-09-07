use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use hypercolor_core::asset::{AssetLibrary, AssetUploadOptions};
use hypercolor_daemon::persistence::{AtomicFileWriter, flush_all};
use hypercolor_driver_support::CredentialStore;
use serde_json::json;

/// `flush_all` observes every live destination in the process, so tests that
/// inject failures must not overlap.
static FLUSH_GATE: Mutex<()> = Mutex::new(());

#[test]
fn all_destinations_share_one_bounded_flush_deadline() {
    let _gate = FLUSH_GATE.lock().unwrap_or_else(PoisonError::into_inner);
    let directory = tempfile::tempdir().expect("temporary directory");
    let writers = (0..3)
        .map(|index| {
            let path = directory.path().join(format!("state-{index}.json"));
            let writer = AtomicFileWriter::new(&path).expect("atomic writer");
            writer.set_injected_replace_failures(usize::MAX);
            writer.write(b"dirty").expect_err("injected failure");
            writer
        })
        .collect::<Vec<_>>();
    let started = Instant::now();

    let report = flush_all(Duration::from_millis(200));

    assert_eq!(report.errors().len(), writers.len());
    assert!(started.elapsed() < Duration::from_millis(450));

    for writer in &writers {
        writer.set_injected_replace_failures(0);
        writer.kick();
    }
    assert!(flush_all(Duration::from_secs(5)).is_complete());
}

#[test]
fn asset_index_and_credential_store_join_the_flush_report() {
    let _gate = FLUSH_GATE.lock().unwrap_or_else(PoisonError::into_inner);
    let directory = tempfile::tempdir().expect("temporary directory");
    let assets_root = directory.path().join("assets");
    let credentials_root = directory.path().join("credentials");

    let mut library = AssetLibrary::open(&assets_root).expect("asset library");
    let index_writer = AtomicFileWriter::new(library.index_path()).expect("index writer");
    let credential_store =
        CredentialStore::open_blocking(&credentials_root).expect("credential store");
    let credential_writer = AtomicFileWriter::new(&credentials_root.join("credentials.json.enc"))
        .expect("credential writer");

    index_writer.set_injected_replace_failures(usize::MAX);
    credential_writer.set_injected_replace_failures(usize::MAX);

    library
        .add_bytes(&png_bytes(), AssetUploadOptions::new("swatch.png"))
        .expect_err("injected index failure");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    runtime
        .block_on(credential_store.store_driver_json("hue", "bridge", json!({"token": "secret"})))
        .expect_err("injected credential failure");

    let report = flush_all(Duration::from_millis(200));
    let paths = report
        .errors()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        paths.contains("index.json"),
        "asset index missing from flush report: {paths}"
    );
    assert!(
        paths.contains("credentials.json.enc"),
        "credential store missing from flush report: {paths}"
    );

    index_writer.set_injected_replace_failures(0);
    credential_writer.set_injected_replace_failures(0);
    index_writer.kick();
    credential_writer.kick();
    assert!(flush_all(Duration::from_secs(5)).is_complete());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let mode = std::fs::metadata(credentials_root.join("credentials.json.enc"))
            .expect("credential store metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

fn png_bytes() -> Vec<u8> {
    let image = image::ImageBuffer::from_pixel(2, 2, image::Rgba([255_u8, 0, 128, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode test png");
    bytes.into_inner()
}
