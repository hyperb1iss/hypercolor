use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use hypercolor_driver_api::CredentialStore;
use hypercolor_driver_api::DeviceBackend;
use hypercolor_driver_api::DiscoveryConnectBehavior;
use hypercolor_driver_nanoleaf::{
    NanoleafBackend, NanoleafConfig, NanoleafDiscoveredDevice, build_device_info,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Mutex;
use tokio::time::timeout;

static UDP_PORT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn udp_test_port_lock() -> &'static Mutex<()> {
    UDP_PORT_LOCK.get_or_init(|| Mutex::new(()))
}

#[tokio::test]
async fn backend_connect_write_brightness_and_disconnect() -> TestResult {
    let _guard = udp_test_port_lock().lock().await;

    let api_listener = TcpListener::bind("127.0.0.1:0").await?;
    let api_port = api_listener.local_addr()?.port();
    let api_task = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut stream, _) = api_listener.accept().await.expect("accept request");
            let request = read_http_request(&mut stream)
                .await
                .expect("read HTTP request");
            let response = if request
                .starts_with("GET /api/v1/test-token/panelLayout/layout HTTP/1.1")
            {
                json_response(
                    r#"{"positionData":[{"panelId":41,"x":0,"y":0,"o":0,"shapeType":7},{"panelId":42,"x":10,"y":10,"o":90,"shapeType":17},{"panelId":10,"x":20,"y":20,"o":180,"shapeType":12}]}"#,
                )
            } else if request.starts_with("GET /api/v1/test-token HTTP/1.1") {
                json_response(
                    r#"{"name":"Living Room Shapes","model":"Shapes","serialNo":"SERIAL42","firmwareVersion":"12.0.0"}"#,
                )
            } else if request.starts_with("PUT /api/v1/test-token/effects HTTP/1.1") {
                json_response("{}")
            } else {
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
            };

            stream
                .write_all(response.as_slice())
                .await
                .expect("write HTTP response");
        }
    });

    let receiver = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 60_222))).await?;

    let tempdir = tempfile::tempdir()?;
    let store = Arc::new(CredentialStore::open(tempdir.path()).await?);
    store
        .store_driver_json(
            "nanoleaf",
            "living-room",
            serde_json::json!({
                "auth_token": "test-token",
            }),
        )
        .await?;

    let config = NanoleafConfig {
        device_ips: Vec::new(),
        transition_time: 1,
    };
    let mut backend = NanoleafBackend::with_mdns_enabled(config, Arc::clone(&store), false);

    let discovered = NanoleafDiscoveredDevice {
        device_key: "living-room".to_owned(),
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        api_port,
        info: build_device_info(
            "living-room",
            "Living Room Shapes",
            Some("Shapes"),
            None,
            &[],
        ),
        panel_ids: Vec::new(),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        metadata: HashMap::new(),
    };
    let device_id = discovered.info.id;
    backend.remember_device(discovered);

    backend.connect(&device_id).await?;

    let info = backend
        .connected_device_info(&device_id)
        .await?
        .expect("connected device info");
    assert_eq!(info.total_led_count(), 2);
    assert_eq!(info.zones.len(), 2);

    backend
        .write_colors(&device_id, &[[100, 120, 140], [20, 40, 60]])
        .await?;

    let mut buf = [0_u8; 64];
    let (len, _) = timeout(Duration::from_millis(500), receiver.recv_from(&mut buf)).await??;
    assert_eq!(
        &buf[..len],
        &[
            0x00, 0x02, 0x00, 0x29, 100, 120, 140, 0, 0, 1, 0x00, 0x2A, 20, 40, 60, 0, 0, 1,
        ]
    );

    backend.set_brightness(&device_id, 128).await?;
    backend.write_colors(&device_id, &[[200, 100, 50]]).await?;

    let (len, _) = timeout(Duration::from_millis(500), receiver.recv_from(&mut buf)).await??;
    assert_eq!(
        &buf[..len],
        &[
            0x00, 0x02, 0x00, 0x29, 100, 50, 25, 0, 0, 1, 0x00, 0x2A, 0, 0, 0, 0, 0, 1,
        ]
    );

    backend.disconnect(&device_id).await?;
    assert!(backend.connected_device_info(&device_id).await?.is_none());

    api_task.await?;
    Ok(())
}

#[tokio::test]
async fn backend_connect_without_discovery_fails() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        CredentialStore::open(tempdir.path())
            .await
            .expect("credential store"),
    );
    let mut backend = NanoleafBackend::with_mdns_enabled(
        NanoleafConfig {
            device_ips: Vec::new(),
            transition_time: 1,
        },
        store,
        false,
    );

    let error = backend
        .connect(&hypercolor_types::device::DeviceId::new())
        .await
        .expect_err("connect without discovery should fail");
    assert!(
        error.to_string().contains("not known"),
        "unexpected error: {error}"
    );
}

fn json_response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .into_bytes()
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
