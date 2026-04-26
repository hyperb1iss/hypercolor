use hypercolor_core::device::DeviceBackend;
use hypercolor_driver_govee::backend::GoveeBackend;
use hypercolor_driver_govee::cloud::{CloudClient, V1Device};
use hypercolor_driver_govee::{GoveeLanDevice, build_cloud_discovered_device, build_device_info};
use hypercolor_types::config::GoveeConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::net::UdpSocket;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn write_colors_dedups_and_paces_lan_state_frames() {
    let socket = UdpSocket::bind(("127.0.0.1", 0))
        .await
        .expect("test UDP socket should bind");
    let target = socket.local_addr().expect("test socket has local addr");
    let device = test_device("H6163");
    let device_id = build_device_info(&device).id;
    let mut backend = GoveeBackend::new(GoveeConfig {
        lan_state_fps: 10,
        ..GoveeConfig::default()
    });
    backend.remember_device_at(device, target);

    backend
        .write_colors(&device_id, &[[255, 0, 0], [0, 0, 255]])
        .await
        .expect("first frame should send");
    let first = recv_payload(&socket).await;
    assert!(first.contains("\"colorwc\""));
    assert!(first.contains("\"r\":127"));
    assert!(first.contains("\"b\":127"));

    backend
        .write_colors(&device_id, &[[255, 0, 0], [0, 0, 255]])
        .await
        .expect("duplicate frame should be skipped");
    assert_no_payload(&socket).await;

    backend
        .write_colors(&device_id, &[[0, 255, 0]])
        .await
        .expect("paced frame should be skipped");
    assert_no_payload(&socket).await;

    tokio::time::sleep(Duration::from_millis(110)).await;
    backend
        .write_colors(&device_id, &[[0, 255, 0]])
        .await
        .expect("frame after pacing window should send");
    let second = recv_payload(&socket).await;
    assert!(second.contains("\"g\":255"));
}

#[tokio::test]
async fn connect_enables_razer_only_for_validated_profiles() {
    let h619a_socket = UdpSocket::bind(("127.0.0.1", 0))
        .await
        .expect("test UDP socket should bind");
    let h619a_target = h619a_socket
        .local_addr()
        .expect("test socket has local addr");
    let h619a = test_device("H619A");
    let h619a_id = build_device_info(&h619a).id;
    let mut h619a_backend = GoveeBackend::new(GoveeConfig::default());
    h619a_backend.remember_device_at(h619a, h619a_target);

    h619a_backend
        .connect(&h619a_id)
        .await
        .expect("validated Razer SKU should connect");
    let turn = recv_payload(&h619a_socket).await;
    let razer = recv_payload(&h619a_socket).await;
    assert!(turn.contains("\"turn\""));
    assert!(razer.contains("\"razer\""));
    assert!(razer.contains("uwABsQEK"));

    let h6163_socket = UdpSocket::bind(("127.0.0.1", 0))
        .await
        .expect("test UDP socket should bind");
    let h6163_target = h6163_socket
        .local_addr()
        .expect("test socket has local addr");
    let h6163 = test_device("H6163");
    let h6163_id = build_device_info(&h6163).id;
    let mut h6163_backend = GoveeBackend::new(GoveeConfig::default());
    h6163_backend.remember_device_at(h6163, h6163_target);

    h6163_backend
        .connect(&h6163_id)
        .await
        .expect("basic LAN SKU should connect");
    let turn = recv_payload(&h6163_socket).await;
    assert!(turn.contains("\"turn\""));
    assert_no_payload(&h6163_socket).await;
}

#[tokio::test]
async fn write_colors_uses_razer_only_when_led_count_matches() {
    let socket = UdpSocket::bind(("127.0.0.1", 0))
        .await
        .expect("test UDP socket should bind");
    let target = socket.local_addr().expect("test socket has local addr");
    let device = test_device("H619A");
    let device_id = build_device_info(&device).id;
    let mut backend = GoveeBackend::new(GoveeConfig {
        razer_fps: 25,
        ..GoveeConfig::default()
    });
    backend.remember_device_at(device, target);

    backend
        .write_colors(&device_id, &[[1, 2, 3]; 20])
        .await
        .expect("matching Razer frame should send");
    let razer = recv_payload(&socket).await;
    assert!(razer.contains("\"razer\""));

    tokio::time::sleep(Duration::from_millis(45)).await;
    backend
        .write_colors(&device_id, &[[0, 255, 0]])
        .await
        .expect("mismatched Razer frame should fall back");
    let colorwc = recv_payload(&socket).await;
    assert!(colorwc.contains("\"colorwc\""));
    assert!(colorwc.contains("\"g\":255"));
}

#[tokio::test]
async fn cloud_only_device_uses_v1_control_for_connect_and_color() {
    let cloud_device = test_cloud_device();
    let device_id = build_cloud_discovered_device(cloud_device.clone()).info.id;
    let (base_url, requests) =
        serve_http_requests(2, r#"{"code":200,"message":"Success","data":{}}"#).await;
    let client = CloudClient::with_base_url("test-key", base_url).expect("base URL should parse");
    let mut backend = GoveeBackend::new(GoveeConfig::default()).with_cloud_client(client);
    backend.remember_cloud_device(cloud_device);

    backend
        .connect(&device_id)
        .await
        .expect("cloud-only connect should turn the device on");
    backend
        .write_colors(&device_id, &[[255, 0, 0], [0, 0, 255]])
        .await
        .expect("cloud-only color frame should send mean color");

    let requests = requests.await.expect("server task should join");
    assert_eq!(requests.len(), 2);
    let turn = request_json_body(&requests[0]);
    assert_eq!(turn["cmd"]["name"], "turn");
    assert_eq!(turn["cmd"]["value"], "on");
    let color = request_json_body(&requests[1]);
    assert_eq!(color["cmd"]["name"], "color");
    assert_eq!(
        color["cmd"]["value"],
        serde_json::json!({ "r": 127, "g": 0, "b": 127 })
    );
}

#[tokio::test]
async fn cloud_only_device_uses_v1_control_for_brightness() {
    let cloud_device = test_cloud_device();
    let device_id = build_cloud_discovered_device(cloud_device.clone()).info.id;
    let (base_url, requests) =
        serve_http_requests(1, r#"{"code":200,"message":"Success","data":{}}"#).await;
    let client = CloudClient::with_base_url("test-key", base_url).expect("base URL should parse");
    let mut backend = GoveeBackend::new(GoveeConfig::default()).with_cloud_client(client);
    backend.remember_cloud_device(cloud_device);

    backend
        .set_brightness(&device_id, 250)
        .await
        .expect("cloud brightness should send");

    let requests = requests.await.expect("server task should join");
    let body = request_json_body(&requests[0]);
    assert_eq!(body["cmd"]["name"], "brightness");
    assert_eq!(body["cmd"]["value"], 100);
}

fn test_device(sku: &str) -> GoveeLanDevice {
    GoveeLanDevice {
        ip: "127.0.0.1".parse().expect("valid test IP"),
        sku: sku.to_owned(),
        mac: "aabbccddeeff".to_owned(),
        name: "Test Govee".to_owned(),
        firmware_version: None,
    }
}

fn test_cloud_device() -> V1Device {
    V1Device {
        device: "AA:BB:CC:DD:EE:FF".to_owned(),
        model: "H6163".to_owned(),
        device_name: "Cloud Strip".to_owned(),
        controllable: true,
        retrievable: true,
        support_cmds: vec![
            "turn".to_owned(),
            "brightness".to_owned(),
            "color".to_owned(),
        ],
        properties: None,
    }
}

async fn recv_payload(socket: &UdpSocket) -> String {
    let mut buf = [0_u8; 2048];
    let (len, _) = timeout(Duration::from_millis(200), socket.recv_from(&mut buf))
        .await
        .expect("payload should arrive")
        .expect("UDP receive should succeed");
    String::from_utf8(buf[..len].to_vec()).expect("payload should be UTF-8")
}

async fn assert_no_payload(socket: &UdpSocket) {
    let mut buf = [0_u8; 2048];
    assert!(
        timeout(Duration::from_millis(50), socket.recv_from(&mut buf))
            .await
            .is_err(),
        "unexpected UDP payload"
    );
}

async fn serve_http_requests(
    count: usize,
    body: &'static str,
) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("test HTTP listener should bind");
    let address = listener
        .local_addr()
        .expect("test listener should have local address");
    let task = tokio::spawn(async move {
        let mut requests = Vec::with_capacity(count);
        for _ in 0..count {
            let (mut stream, _) = listener
                .accept()
                .await
                .expect("test HTTP connection should arrive");
            let request = read_http_request(&mut stream)
                .await
                .expect("test HTTP request should read");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("test HTTP response should write");
            requests.push(request);
        }
        requests
    });

    (format!("http://{address}/v1"), task)
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> std::io::Result<String> {
    let mut request = Vec::new();
    let mut buf = [0_u8; 4096];
    loop {
        let len = stream.read(&mut buf).await?;
        if len == 0 {
            break;
        }
        request.extend_from_slice(&buf[..len]);
        if request_complete(&request) {
            break;
        }
    }

    Ok(String::from_utf8(request).expect("request should be UTF-8"))
}

fn request_complete(request: &[u8]) -> bool {
    let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length: "))
        .or_else(|| {
            headers
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
        })
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);

    request.len() >= header_end + 4 + content_length
}

fn request_json_body(request: &str) -> serde_json::Value {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .expect("HTTP request should contain a header/body separator");
    serde_json::from_str(body).expect("HTTP request body should be JSON")
}
