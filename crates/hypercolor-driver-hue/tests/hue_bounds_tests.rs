//! Reply bounds on the bridge API: a Bridge Pro at its hard limit fits, and
//! oversized or dripped replies fail at the bound.

use std::time::{Duration, Instant};

use hypercolor_driver_hue::HueBridgeClient;
use hypercolor_driver_support::http::HttpError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const HEAD: &[u8] = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";
/// Past every limit the client sets.
const OVERSIZED: usize = 8 * 1024 * 1024;

/// Answer one request with `HEAD`, then `body`, then a drip of spaces when
/// `drip` is set.
async fn serve(body: Vec<u8>, drip: bool) -> TestResult<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept request");
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).await;
        let _ = stream.write_all(HEAD).await;
        let _ = stream.write_all(&body).await;
        while drip && stream.write_all(b" ").await.is_ok() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    Ok(port)
}

fn authenticated(port: u16) -> TestResult<HueBridgeClient> {
    Ok(HueBridgeClient::authenticated_with_port(
        "127.0.0.1".parse()?,
        port,
        "test-key".to_owned(),
    ))
}

fn http_cause(error: &anyhow::Error) -> Option<&HttpError> {
    error.chain().find_map(|cause| cause.downcast_ref())
}

#[tokio::test]
async fn a_bridge_pro_at_its_hard_limit_fits() -> TestResult {
    let lights: Vec<serde_json::Value> = (0..186)
        .map(|index| {
            serde_json::json!({
                "id": format!("light-{index}"),
                "metadata": { "name": format!("Light {index}") },
                "product_data": { "model_id": "LCA001" },
                // Stands in for the effect, gradient, and powerup objects
                // a real resource carries.
                "padding": "x".repeat(4096),
            })
        })
        .collect();
    let body = serde_json::json!({ "data": lights }).to_string();
    let port = serve(body.into_bytes(), false).await?;
    let lights = authenticated(port)?.lights().await?;
    assert_eq!(lights.len(), 186);
    Ok(())
}

#[tokio::test]
async fn an_oversized_light_inventory_is_refused_while_it_streams() -> TestResult {
    let port = serve(vec![b' '; OVERSIZED], false).await?;
    let error = authenticated(port)?
        .lights()
        .await
        .expect_err("oversized inventory");
    assert!(matches!(
        http_cause(&error),
        Some(HttpError::BodyTooLarge { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn an_oversized_nupnp_listing_is_refused_while_it_streams() -> TestResult {
    let port = serve(vec![b' '; 128 * 1024], false).await?;
    let error = HueBridgeClient::discover_bridges_with_url(&format!("http://127.0.0.1:{port}/"))
        .await
        .expect_err("oversized listing");
    assert!(matches!(
        http_cause(&error),
        Some(HttpError::BodyTooLarge { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_slow_drip_fails_at_the_request_deadline() -> TestResult {
    let port = serve(Vec::new(), true).await?;
    let started = Instant::now();
    let error = authenticated(port)?
        .lights()
        .await
        .expect_err("dripped reply");
    assert!(matches!(
        http_cause(&error),
        Some(HttpError::DeadlineExceeded)
    ));
    assert!(started.elapsed() < Duration::from_secs(9));
    Ok(())
}
