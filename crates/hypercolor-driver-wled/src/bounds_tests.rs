//! Reply bounds on the WLED JSON API: real replies fit, and oversized or
//! dripped replies fail at the bound.

use std::net::Ipv4Addr;
use std::time::Instant;

use hypercolor_driver_support::http::HttpError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::{BoundedHttpClient, Duration, WLED_JSON_REPLY_BYTES, fetch_json};

const HEAD: &[u8] = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";

/// Answer one request with `HEAD`, then `body`, then a drip of spaces
/// when `drip` is set.
async fn serve(body: Vec<u8>, drip: bool) -> String {
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
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    format!("http://127.0.0.1:{port}/json/info")
}

fn client() -> BoundedHttpClient {
    BoundedHttpClient::new().expect("HTTP client")
}

fn http_cause(error: &anyhow::Error) -> Option<&HttpError> {
    error.chain().find_map(|cause| cause.downcast_ref())
}

#[tokio::test]
async fn the_largest_stock_reply_fits() {
    // WLED's JSON buffer tops out at 32767 bytes.
    let padding = "x".repeat(32_767);
    let body = serde_json::json!({ "name": padding }).to_string();
    let url = serve(body.into_bytes(), false).await;
    let value = fetch_json(&client(), &url, Duration::from_secs(5))
        .await
        .expect("reply inside the limit");
    assert!(value["name"].is_string());
}

#[tokio::test]
async fn an_oversized_reply_is_refused_while_it_streams() {
    let url = serve(vec![b' '; WLED_JSON_REPLY_BYTES + 1], false).await;
    let error = fetch_json(&client(), &url, Duration::from_secs(5))
        .await
        .expect_err("oversized reply");
    assert!(matches!(
        http_cause(&error),
        Some(HttpError::BodyTooLarge { .. })
    ));
}

#[tokio::test]
async fn a_slow_drip_fails_at_the_deadline() {
    let url = serve(Vec::new(), true).await;
    let started = Instant::now();
    let error = fetch_json(&client(), &url, Duration::from_millis(300))
        .await
        .expect_err("dripped reply");
    assert!(matches!(
        http_cause(&error),
        Some(HttpError::DeadlineExceeded)
    ));
    assert!(started.elapsed() < Duration::from_secs(2));
}
