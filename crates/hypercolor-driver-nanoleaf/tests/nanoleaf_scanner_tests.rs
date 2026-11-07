//! What a scan concludes from a controller it can and cannot read.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use hypercolor_driver_api::DiscoveryConnectBehavior;
use hypercolor_driver_nanoleaf::{NanoleafDiscoveredDevice, NanoleafKnownDevice, NanoleafScanner};
use hypercolor_driver_support::CredentialStore;
use hypercolor_driver_support::discovery::ScanOutcome;
use hypercolor_types::device::DeviceFingerprint;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const LOOPBACK: &str = "127.0.0.1";

fn http_reply(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A controller that serves its device info and fails its panel layout.
async fn spawn_layoutless_controller() -> TestResult<u16> {
    let listener = TcpListener::bind((LOOPBACK, 0)).await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut buffer = [0_u8; 1024];
            let read = stream.read(&mut buffer).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]);
            let reply = if request.contains("panelLayout") {
                http_reply("500 Internal Server Error", "")
            } else {
                http_reply(
                    "200 OK",
                    r#"{"name":"Shapes","model":"NL42","serialNo":"S123","firmwareVersion":"9.0"}"#,
                )
            };
            let _ = stream.write_all(reply.as_bytes()).await;
        }
    });
    Ok(port)
}

async fn paired_store(tempdir: &tempfile::TempDir) -> TestResult<Arc<CredentialStore>> {
    let store = Arc::new(CredentialStore::open(tempdir.path()).await?);
    store
        .store_driver_json(
            "nanoleaf",
            &format!("ip:{LOOPBACK}"),
            serde_json::json!({ "auth_token": "token" }),
        )
        .await?;
    Ok(store)
}

fn known(port: u16) -> TestResult<NanoleafKnownDevice> {
    Ok(NanoleafKnownDevice {
        port,
        ..NanoleafKnownDevice::from_ip(LOOPBACK.parse()?)
    })
}

async fn scan(
    device: NanoleafKnownDevice,
    store: Arc<CredentialStore>,
    tracked: HashMap<IpAddr, DeviceFingerprint>,
) -> TestResult<ScanOutcome<NanoleafDiscoveredDevice>> {
    Ok(
        NanoleafScanner::with_options(vec![device], store, Duration::from_secs(1), false)
            .with_tracked(tracked)
            .scan_devices()
            .await?,
    )
}

#[tokio::test]
async fn a_tracked_controller_that_cannot_be_read_is_renewed_not_rewritten() -> TestResult {
    let port = spawn_layoutless_controller().await?;
    let tempdir = tempfile::tempdir()?;
    let fingerprint = DeviceFingerprint::from_persisted("net:nanoleaf:s123".to_owned());

    let outcome = scan(
        known(port)?,
        paired_store(&tempdir).await?,
        HashMap::from([(LOOPBACK.parse()?, fingerprint.clone())]),
    )
    .await?;

    assert!(
        outcome.found.is_empty(),
        "a record with no panels must not replace what the host verified"
    );
    assert_eq!(outcome.present, vec![fingerprint]);
    assert!(outcome.gaps.is_empty(), "{:?}", outcome.gaps);
    Ok(())
}

#[tokio::test]
async fn an_untracked_controller_that_cannot_be_read_stays_visible_and_deferred() -> TestResult {
    let port = spawn_layoutless_controller().await?;
    let tempdir = tempfile::tempdir()?;

    let outcome = scan(known(port)?, paired_store(&tempdir).await?, HashMap::new()).await?;

    assert_eq!(outcome.found.len(), 1);
    assert_eq!(
        outcome.found[0].connect_behavior,
        DiscoveryConnectBehavior::Deferred
    );
    Ok(())
}

fn closed_port() -> TestResult<u16> {
    let closed = std::net::TcpListener::bind((LOOPBACK, 0))?;
    Ok(closed.local_addr()?.port())
}

#[tokio::test]
async fn a_tracked_controller_that_never_answers_holds_only_itself() -> TestResult {
    let tempdir = tempfile::tempdir()?;
    let fingerprint = DeviceFingerprint::from_persisted("net:nanoleaf:dead".to_owned());

    let outcome = scan(
        known(closed_port()?)?,
        paired_store(&tempdir).await?,
        HashMap::from([(LOOPBACK.parse()?, fingerprint.clone())]),
    )
    .await?;

    assert!(outcome.found.is_empty(), "no record is rebuilt for it");
    assert!(outcome.present.is_empty(), "silence is not presence");
    let [gap] = outcome.gaps.as_slice() else {
        panic!("expected one gap, got {:?}", outcome.gaps);
    };
    assert_eq!(gap.device(), Some(&fingerprint));
    assert!(
        !gap.to_string().contains("token"),
        "gaps reach the REST API, so they must not carry the auth token"
    );
    Ok(())
}

#[tokio::test]
async fn a_paired_controller_nobody_tracks_that_never_answers_holds_nothing() -> TestResult {
    let tempdir = tempfile::tempdir()?;

    let outcome = scan(
        known(closed_port()?)?,
        paired_store(&tempdir).await?,
        HashMap::new(),
    )
    .await?;

    assert!(outcome.found.is_empty());
    assert!(outcome.present.is_empty());
    assert!(
        outcome.gaps.is_empty(),
        "a dead address with no device behind it must not hold the driver: {:?}",
        outcome.gaps
    );
    Ok(())
}

#[tokio::test]
async fn an_unpaired_controller_is_listed_without_contact_so_it_can_pair() -> TestResult {
    let closed = std::net::TcpListener::bind((LOOPBACK, 0))?;
    let port = closed.local_addr()?.port();
    drop(closed);
    let tempdir = tempfile::tempdir()?;
    let store = Arc::new(CredentialStore::open(tempdir.path()).await?);

    let outcome = scan(known(port)?, store, HashMap::new()).await?;

    assert_eq!(outcome.found.len(), 1);
    assert_eq!(
        outcome.found[0].connect_behavior,
        DiscoveryConnectBehavior::Deferred
    );
    assert!(outcome.gaps.is_empty(), "{:?}", outcome.gaps);
    Ok(())
}
