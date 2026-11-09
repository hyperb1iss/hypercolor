//! Reply bounds on the Nanoleaf API: a full 500-panel install fits, and
//! oversized or dripped replies fail at the bound.

use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use hypercolor_driver_support::http::HttpError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::{
    NANOLEAF_HTTP_TIMEOUT, NANOLEAF_LAYOUT_REPLY_BYTES, NANOLEAF_SMALL_REPLY_BYTES,
    fetch_device_info, fetch_panel_layout, pair_device_with_status,
};

const HEAD: &[u8] = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";
const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Answer one request with `HEAD`, then `body`, then a drip of spaces when
/// `drip` is set.
async fn serve(body: Vec<u8>, drip: bool) -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind loopback");
    let port = listener.local_addr().expect("local address").port();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).await;
        let _ = stream.write_all(HEAD).await;
        let _ = stream.write_all(&body).await;
        while drip && stream.write_all(b" ").await.is_ok() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    port
}

fn http_cause(error: &anyhow::Error) -> Option<&HttpError> {
    error.chain().find_map(|cause| cause.downcast_ref())
}

fn panels(count: u16) -> Vec<serde_json::Value> {
    (0..count)
        .map(|index| {
            serde_json::json!({
                "panelId": 10_000 + index,
                "x": i32::from(index) * 100,
                "y": i32::from(index) * 50,
                "o": 300,
                "shapeType": 7,
            })
        })
        .collect()
}

#[tokio::test]
async fn a_full_install_layout_fits() {
    let body = serde_json::json!({ "globalOrientation": {"value": 60}, "layout": { "numPanels": 500, "positionData": panels(500) } });
    let port = serve(body.to_string().into_bytes(), false).await;
    let layout = fetch_panel_layout(LOOPBACK, port, "token")
        .await
        .expect("full install layout");
    assert_eq!(layout.layout.position_data.len(), 500);
}

#[tokio::test]
async fn device_info_with_a_full_install_and_a_long_effect_list_fits() {
    let effects: Vec<String> = (0..2000)
        .map(|index| format!("Northern Lights Variation {index}"))
        .collect();
    let body = serde_json::json!({
        "name": "Shapes",
        "model": "NL42",
        "serialNo": "S0000000",
        "firmwareVersion": "12.0.0",
        "effects": { "effectsList": effects },
        "panelLayout": { "layout": { "positionData": panels(500) } },
    });
    let port = serve(body.to_string().into_bytes(), false).await;
    let info = fetch_device_info(LOOPBACK, port, "token")
        .await
        .expect("device info");
    assert_eq!(info.model, "NL42");
}

#[tokio::test]
async fn an_oversized_layout_is_refused_while_it_streams() {
    let port = serve(vec![b' '; NANOLEAF_LAYOUT_REPLY_BYTES + 1], false).await;
    let error = fetch_panel_layout(LOOPBACK, port, "token")
        .await
        .expect_err("oversized layout");
    assert!(matches!(
        http_cause(&error),
        Some(HttpError::BodyTooLarge { .. })
    ));
}

#[tokio::test]
async fn an_oversized_pairing_reply_is_refused_while_it_streams() {
    let port = serve(vec![b' '; NANOLEAF_SMALL_REPLY_BYTES + 1], false).await;
    let error = pair_device_with_status(LOOPBACK, port)
        .await
        .expect_err("oversized pairing reply");
    assert!(matches!(
        http_cause(&error),
        Some(HttpError::BodyTooLarge { .. })
    ));
}

#[tokio::test]
async fn a_slow_drip_fails_at_the_request_deadline() {
    let port = serve(Vec::new(), true).await;
    let started = Instant::now();
    let error = fetch_device_info(LOOPBACK, port, "token")
        .await
        .expect_err("dripped reply");
    assert!(matches!(
        http_cause(&error),
        Some(HttpError::DeadlineExceeded)
    ));
    assert!(started.elapsed() < NANOLEAF_HTTP_TIMEOUT + Duration::from_secs(4));
}
