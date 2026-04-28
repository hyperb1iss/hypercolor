use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use hypercolor_driver_api::CredentialStore;
use hypercolor_driver_api::DeviceBackend;
use hypercolor_driver_api::DiscoveryConnectBehavior;
use hypercolor_driver_hue::{
    GAMUT_C, HueBackend, HueConfig, HueDiscoveredBridge, build_device_info, rgb_to_cie_xyb,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::timeout;
use webrtc_dtls::cipher_suite::CipherSuiteId;
use webrtc_dtls::config::Config as DtlsConfig;
use webrtc_dtls::conn::DTLSConn;
use webrtc_util::conn::Listener;

static HUE_STREAM_LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn hue_stream_lock() -> &'static AsyncMutex<()> {
    HUE_STREAM_LOCK.get_or_init(|| AsyncMutex::new(()))
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the integration test inlines the mock bridge, DTLS server, and assertions for readability"
)]
async fn backend_connects_streams_and_disconnects() -> TestResult {
    let _guard = hue_stream_lock().lock().await;

    let http_listener = TcpListener::bind("127.0.0.1:0").await?;
    let api_port = http_listener.local_addr()?.port();
    let actions = Arc::new(Mutex::new(Vec::<String>::new()));
    let actions_for_server = Arc::clone(&actions);
    let config_id = "12345678-1234-1234-1234-123456789abc";
    let http_task = tokio::spawn(async move {
        for _ in 0..5 {
            let (mut stream, _) = http_listener.accept().await.expect("accept request");
            let request = read_http_request(&mut stream)
                .await
                .expect("read HTTP request");
            let response = if request.starts_with("GET /api/config HTTP/1.1") {
                json_response(
                    r#"{"bridgeid":"test-bridge","name":"Living Room Bridge","modelid":"BSB002","swversion":"1968096020"}"#,
                )
            } else if request.starts_with("GET /clip/v2/resource/light HTTP/1.1") {
                json_response(
                    r#"{"data":[{"id":"light-left","metadata":{"name":"Left Bulb"},"product_data":{"model_id":"LCA001"},"color":{"gamut_type":"C"}},{"id":"light-right","metadata":{"name":"Right Bulb"},"product_data":{"model_id":"LCA001"},"color":{"gamut_type":"C"}}]}"#,
                )
            } else if request
                .starts_with("GET /clip/v2/resource/entertainment_configuration HTTP/1.1")
            {
                json_response(&format!(
                    r#"{{"data":[{{"id":"{config_id}","metadata":{{"name":"Living Room"}},"configuration_type":"screen","channels":[{{"channel_id":1,"position":{{"x":-0.5,"y":0.0,"z":0.0}},"members":[{{"service":{{"rid":"light-left","rtype":"light"}}}}]}},{{"channel_id":2,"position":{{"x":0.5,"y":0.0,"z":0.0}},"members":[{{"service":{{"rid":"light-right","rtype":"light"}}}}]}}]}}]}}"#
                ))
            } else if request.starts_with(&format!(
                "PUT /clip/v2/resource/entertainment_configuration/{config_id} HTTP/1.1"
            )) {
                let action = if request.contains(r#""action":"start""#) {
                    "start"
                } else if request.contains(r#""action":"stop""#) {
                    "stop"
                } else {
                    "unknown"
                };
                actions_for_server
                    .lock()
                    .expect("lock actions")
                    .push(action.to_owned());
                json_response(r#"{"data":[{"success":true}]}"#)
            } else {
                not_found_response()
            };

            stream
                .write_all(response.as_slice())
                .await
                .expect("write HTTP response");
        }
    });

    let dtls_task = tokio::spawn(async move {
        let listener = webrtc_util::conn::conn_udp_listener::listen("127.0.0.1:2100")
            .await
            .expect("bind DTLS listener");
        let (conn, _) = listener.accept().await.expect("accept DTLS connection");
        let psk = Arc::new(hex_decode("00112233445566778899aabbccddeeff"));
        let config = DtlsConfig {
            psk: Some(Arc::new(move |_| Ok(psk.as_ref().clone()))),
            cipher_suites: vec![CipherSuiteId::Tls_Psk_With_Aes_128_Gcm_Sha256],
            ..Default::default()
        };
        let dtls = DTLSConn::new(conn, config, false, None)
            .await
            .expect("handshake DTLS");
        let mut buf = [0_u8; 256];
        let len = dtls
            .read(&mut buf, Some(Duration::from_secs(5)))
            .await
            .expect("read HueStream packet");
        dtls.close().await.expect("close DTLS server");
        buf[..len].to_vec()
    });

    let tempdir = tempfile::tempdir()?;
    let store = Arc::new(CredentialStore::open(tempdir.path()).await?);
    store
        .store_json(
            "hue:test-bridge",
            serde_json::json!({
                "api_key": "test-api-key",
                "client_key": "00112233445566778899aabbccddeeff",
            }),
        )
        .await?;

    let mut backend =
        HueBackend::with_mdns_enabled(HueConfig::default(), Arc::clone(&store), false);
    let discovered = HueDiscoveredBridge {
        bridge_id: "test-bridge".to_owned(),
        ip: "127.0.0.1".parse()?,
        api_port,
        info: build_device_info(
            "test-bridge",
            "Living Room Bridge",
            Some("BSB002"),
            None,
            None,
            &[],
        ),
        entertainment_config: None,
        lights: Vec::new(),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        metadata: HashMap::new(),
    };
    let device_id = discovered.info.id;
    backend.remember_bridge(discovered);

    backend.connect(&device_id).await?;
    let info = backend
        .connected_device_info(&device_id)
        .await?
        .expect("connected device info");
    assert_eq!(info.total_led_count(), 2);
    assert_eq!(info.zones.len(), 2);

    backend
        .write_colors(&device_id, &[[255, 0, 0], [0, 0, 255]])
        .await?;

    let packet = timeout(Duration::from_secs(10), dtls_task).await??;
    assert_eq!(&packet[..9], b"HueStream");
    assert_eq!(packet[9], 0x02);
    assert_eq!(packet[10], 0x00);
    assert_eq!(packet[11], 0);
    assert_eq!(&packet[16..52], config_id.as_bytes());

    let red = rgb_to_cie_xyb(255, 0, 0, &GAMUT_C);
    let blue = rgb_to_cie_xyb(0, 0, 255, &GAMUT_C);
    assert_eq!(packet[52], 1);
    assert_eq!(&packet[53..55], &encode_unit(red.x).to_be_bytes());
    assert_eq!(&packet[55..57], &encode_unit(red.y).to_be_bytes());
    assert_eq!(&packet[57..59], &encode_unit(red.brightness).to_be_bytes());
    assert_eq!(packet[59], 2);
    assert_eq!(&packet[60..62], &encode_unit(blue.x).to_be_bytes());
    assert_eq!(&packet[62..64], &encode_unit(blue.y).to_be_bytes());
    assert_eq!(&packet[64..66], &encode_unit(blue.brightness).to_be_bytes());

    backend.disconnect(&device_id).await?;
    assert_eq!(
        actions.lock().expect("lock actions").as_slice(),
        &["start".to_owned(), "stop".to_owned()]
    );

    http_task.await?;
    Ok(())
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "Hue wire assertions quantize unit floats into u16 values"
)]
fn encode_unit(value: f64) -> u16 {
    ((value.clamp(0.0, 1.0) * f64::from(u16::MAX)).round()) as u16
}

fn hex_decode(raw: &str) -> Vec<u8> {
    raw.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).expect("valid hex");
            u8::from_str_radix(pair, 16).expect("parse hex")
        })
        .collect()
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
