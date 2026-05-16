use std::sync::Arc;
use std::time::Duration;

use hypercolor_driver_api::CredentialStore;
use hypercolor_driver_api::DiscoveryConnectBehavior;
use hypercolor_driver_hue::{HueKnownBridge, HueScanner};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn scanner_enriches_known_bridge_and_marks_authenticated_bridge_autoconnect() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let api_port = listener.local_addr()?.port();
    let config_id = "12345678-1234-1234-1234-123456789abc";
    let server_task = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let request = read_http_request(&mut stream)
                .await
                .expect("read HTTP request");
            let response = if request.starts_with("GET /api/config HTTP/1.1") {
                json_response(
                    r#"{"bridgeid":"test-bridge","name":"Studio Bridge","modelid":"BSB002","swversion":"1968096020"}"#,
                )
            } else if request.starts_with("GET /clip/v2/resource/light HTTP/1.1") {
                json_response(
                    r#"{"data":[{"id":"light-left","metadata":{"name":"Left Bulb"},"product_data":{"model_id":"LCA001"},"color":{"gamut_type":"C"}}]}"#,
                )
            } else if request
                .starts_with("GET /clip/v2/resource/entertainment_configuration HTTP/1.1")
            {
                json_response(&format!(
                    r#"{{"data":[{{"id":"{config_id}","metadata":{{"name":"Studio"}},"configuration_type":"screen","channels":[{{"channel_id":1,"position":{{"x":0.0,"y":0.0,"z":0.0}},"members":[{{"service":{{"rid":"light-left","rtype":"light"}}}}]}}]}}]}}"#
                ))
            } else {
                not_found_response()
            };

            stream
                .write_all(response.as_slice())
                .await
                .expect("write HTTP response");
        }
    });

    let tempdir = tempfile::tempdir()?;
    let store = Arc::new(CredentialStore::open(tempdir.path()).await?);
    store
        .store_driver_json(
            "hue",
            "test-bridge",
            serde_json::json!({
                "api_key": "test-api-key",
                "client_key": "00112233445566778899aabbccddeeff",
            }),
        )
        .await?;

    let mut scanner = HueScanner::with_options(
        vec![HueKnownBridge {
            bridge_id: String::new(),
            ip: "127.0.0.1".parse()?,
            api_port,
            name: String::new(),
            model_id: String::new(),
            sw_version: String::new(),
        }],
        store,
        Duration::from_secs(1),
        false,
        Some("Studio".to_owned()),
    )
    .with_nupnp_url(format!("http://127.0.0.1:{api_port}/nupnp"));
    let bridges = scanner.scan_bridges().await?;

    assert_eq!(bridges.len(), 1);
    let bridge = &bridges[0];
    assert_eq!(bridge.bridge_id, "test-bridge");
    assert_eq!(bridge.info.name, "Studio");
    assert_eq!(bridge.info.zones.len(), 1);
    assert_eq!(bridge.info.total_led_count(), 1);
    assert_eq!(
        bridge.connect_behavior,
        DiscoveryConnectBehavior::AutoConnect
    );
    assert_eq!(
        bridge
            .metadata
            .get("entertainment_config_name")
            .map(String::as_str),
        Some("Studio")
    );
    assert_eq!(
        bridge.metadata.get("bridge_name").map(String::as_str),
        Some("Studio Bridge")
    );

    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn scanner_uses_nupnp_only_when_no_local_candidates_exist() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let api_port = listener.local_addr()?.port();
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept request");
        let request = read_http_request(&mut stream)
            .await
            .expect("read HTTP request");
        assert!(request.starts_with("GET /nupnp HTTP/1.1"));
        stream
            .write_all(json_response("[]").as_slice())
            .await
            .expect("write HTTP response");
    });

    let tempdir = tempfile::tempdir()?;
    let store = Arc::new(CredentialStore::open(tempdir.path()).await?);
    let mut scanner =
        HueScanner::with_options(Vec::new(), store, Duration::from_secs(1), false, None)
            .with_nupnp_url(format!("http://127.0.0.1:{api_port}/nupnp"));
    let bridges = scanner.scan_bridges().await?;

    assert!(bridges.is_empty());
    server_task.await?;
    Ok(())
}

fn json_response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .into_bytes()
}

fn not_found_response() -> Vec<u8> {
    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
}

async fn read_http_request(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = vec![0_u8; 4096];
    let mut total = 0_usize;
    let mut header_end = None;

    loop {
        let read = stream.read(&mut buf[total..]).await?;
        if read == 0 {
            break;
        }
        total += read;

        if header_end.is_none() {
            header_end = buf[..total]
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4);
        }

        if let Some(header_end) = header_end {
            let headers = String::from_utf8_lossy(&buf[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if total >= header_end + content_length {
                break;
            }
        }

        if total == buf.len() {
            buf.resize(buf.len() * 2, 0);
        }
    }

    Ok(String::from_utf8_lossy(&buf[..total]).into_owned())
}
