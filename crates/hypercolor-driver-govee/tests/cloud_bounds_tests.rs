//! Reply bounds on the cloud API: a large account fits, and oversized or
//! dripped replies fail at the bound.

use std::time::{Duration, Instant};

use hypercolor_driver_govee::cloud::CloudClient;
use hypercolor_driver_support::http::HttpError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const HEAD: &[u8] = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";

/// Answer one request with `HEAD`, then `body`, then a drip of spaces when
/// `drip` is set.
async fn serve(body: Vec<u8>, drip: bool) -> CloudClient {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("test listener should bind");
    let address = listener.local_addr().expect("local address");
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("test connection");
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).await;
        let _ = stream.write_all(HEAD).await;
        let _ = stream.write_all(&body).await;
        while drip && stream.write_all(b" ").await.is_ok() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    CloudClient::with_base_url("test-key", format!("http://{address}/v1"))
        .expect("base URL should parse")
}

fn http_cause(error: &anyhow::Error) -> Option<&HttpError> {
    error.chain().find_map(|cause| cause.downcast_ref())
}

#[tokio::test]
async fn a_large_account_inventory_fits() {
    let devices: Vec<serde_json::Value> = (0..5_000)
        .map(|index| {
            serde_json::json!({
                "device": format!("99:E5:A4:C1:38:29:{:02X}:{:02X}", index / 256, index % 256),
                "model": "H6159",
                "deviceName": format!("Light Strip {index}"),
                "controllable": true,
                "retrievable": true,
                "supportCmds": ["turn", "brightness", "color", "colorTem"],
                "properties": { "colorTem": { "range": { "min": 2000, "max": 9000 } } },
            })
        })
        .collect();
    let body = serde_json::json!({
        "code": 200,
        "message": "Success",
        "data": { "devices": devices },
    });
    let client = serve(body.to_string().into_bytes(), false).await;
    let devices = client.list_v1_devices().await.expect("large inventory");
    assert_eq!(devices.len(), 5_000);
}

#[tokio::test]
async fn an_oversized_device_list_is_refused_while_it_streams() {
    let client = serve(vec![b' '; 8 * 1024 * 1024], false).await;
    let error = client
        .list_v1_devices()
        .await
        .expect_err("oversized device list");
    assert!(matches!(
        http_cause(&error),
        Some(HttpError::BodyTooLarge { .. })
    ));
}

#[tokio::test]
async fn an_oversized_device_state_is_refused_while_it_streams() {
    let client = serve(vec![b' '; 128 * 1024], false).await;
    let error = client
        .v1_state("H6159", "99:E5:A4:C1:38:29:DA:7B")
        .await
        .expect_err("oversized state");
    assert!(matches!(
        http_cause(&error),
        Some(HttpError::BodyTooLarge { .. })
    ));
}

#[tokio::test]
async fn a_slow_drip_fails_at_the_request_deadline() {
    let client = serve(Vec::new(), true).await;
    let started = Instant::now();
    let error = client.list_v1_devices().await.expect_err("dripped reply");
    assert!(matches!(
        http_cause(&error),
        Some(HttpError::DeadlineExceeded)
    ));
    let elapsed = started.elapsed();
    assert!(
        (Duration::from_secs(9)..Duration::from_secs(14)).contains(&elapsed),
        "the cloud deadline holds: {elapsed:?}"
    );
}
