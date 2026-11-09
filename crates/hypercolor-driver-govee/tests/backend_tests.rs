use std::sync::Arc;

use hypercolor_driver_api::{DeviceBackend, DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_driver_conformance::{
    DeliveryLog, SessionRig, assert_session_contract, assert_unchanged, assert_written,
};
use hypercolor_driver_govee::backend::GoveeBackend;
use hypercolor_driver_govee::cloud::{CloudClient, V1Device};
use hypercolor_driver_govee::lan::razer::encode_razer_mode_base64;
use hypercolor_driver_govee::{
    GoveeConfig, GoveeLanDevice, build_cloud_discovered_device, build_device_info,
};
use hypercolor_types::device::{DeviceFingerprint, DeviceId, FingerprintNamespace};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::net::UdpSocket;
use tokio::time::{Duration, timeout};

fn adopt_lan_device(backend: &GoveeBackend, device: GoveeLanDevice, target: std::net::SocketAddr) {
    let info = build_device_info(&device);
    let fingerprint = DeviceFingerprint::mint(
        FingerprintNamespace::Net,
        "govee",
        &device
            .mac
            .chars()
            .filter(char::is_ascii_hexdigit)
            .collect::<String>()
            .to_ascii_lowercase(),
    );
    let discovered = DiscoveredDevice {
        fingerprint,
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        info,
        metadata: std::collections::HashMap::from([
            ("ip".to_owned(), target.ip().to_string()),
            ("port".to_owned(), target.port().to_string()),
            ("sku".to_owned(), device.sku),
            ("mac".to_owned(), device.mac),
        ]),
        claim: None,
    };
    backend
        .adopt_device(&discovered)
        .expect("Govee backend should adopt LAN discovery metadata");
}

fn adopt_cloud_device(backend: &GoveeBackend, device: V1Device) {
    backend
        .adopt_device(&build_cloud_discovered_device(device))
        .expect("Govee backend should adopt cloud discovery metadata");
}

#[tokio::test]
async fn write_colors_dedups_and_paces_lan_state_frames() {
    let socket = UdpSocket::bind(("127.0.0.1", 0))
        .await
        .expect("test UDP socket should bind");
    let target = socket.local_addr().expect("test socket has local addr");
    let device = test_device("H6163");
    let device_id = build_device_info(&device).id;
    let backend = GoveeBackend::new(GoveeConfig {
        lan_state_fps: 10,
        ..GoveeConfig::default()
    });
    adopt_lan_device(&backend, device, target);
    backend.connect(&device_id).await.expect("connect");
    assert!(recv_payload(&socket).await.contains("\"turn\""));

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
    let h619a_backend = GoveeBackend::new(GoveeConfig::default());
    adopt_lan_device(&h619a_backend, h619a, h619a_target);

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
    let h6163_backend = GoveeBackend::new(GoveeConfig::default());
    adopt_lan_device(&h6163_backend, h6163, h6163_target);

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
    let backend = GoveeBackend::new(GoveeConfig {
        razer_fps: 25,
        ..GoveeConfig::default()
    });
    adopt_lan_device(&backend, device, target);
    backend.connect(&device_id).await.expect("connect");
    assert!(recv_payload(&socket).await.contains("\"turn\""));
    assert!(recv_payload(&socket).await.contains("\"razer\""));

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
    let backend = GoveeBackend::new(GoveeConfig::default()).with_cloud_client(client);
    adopt_cloud_device(&backend, cloud_device);

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
async fn cloud_only_write_reports_duplicate_suppression_without_counting_a_send() {
    let cloud_device = test_cloud_device();
    let device_id = build_cloud_discovered_device(cloud_device.clone()).info.id;
    let (base_url, requests) =
        serve_http_requests(2, r#"{"code":200,"message":"Success","data":{}}"#).await;
    let client = CloudClient::with_base_url("test-key", base_url).expect("base URL should parse");
    let backend = GoveeBackend::new(GoveeConfig::default()).with_cloud_client(client);
    adopt_cloud_device(&backend, cloud_device);
    backend.connect(&device_id).await.expect("connect");

    let colors = vec![[255, 0, 0], [0, 0, 255]];
    let log = Arc::new(DeliveryLog::default());
    let _ = backend
        .submit_frame(&device_id, log.frame(device_id, 1, colors.clone()))
        .await;
    let _ = backend
        .submit_frame(&device_id, log.frame(device_id, 2, colors))
        .await;

    let receipts = log.settled(2).await;
    assert_written(&receipts[0]);
    assert_unchanged(&receipts[1]);
    assert_eq!(log.started(), [1]);
    assert_eq!(requests.await.expect("server task should join").len(), 2);
}

#[test]
fn cloud_only_output_policy_preserves_sub_hz_interval() {
    let cloud_device = test_cloud_device();
    let device_id = build_cloud_discovered_device(cloud_device.clone()).info.id;
    let backend = GoveeBackend::new(GoveeConfig::default());
    adopt_cloud_device(&backend, cloud_device);

    let cadence = backend
        .output_policy(&device_id)
        .expect("remembered cloud device should expose cadence");

    assert_eq!(cadence.display_fps(), 0);
    assert_eq!(cadence.interval_ms(), Some(6_000));
}

#[tokio::test]
async fn cloud_only_device_uses_v1_control_for_brightness() {
    let cloud_device = test_cloud_device();
    let device_id = build_cloud_discovered_device(cloud_device.clone()).info.id;
    let (base_url, requests) =
        serve_http_requests(2, r#"{"code":200,"message":"Success","data":{}}"#).await;
    let client = CloudClient::with_base_url("test-key", base_url).expect("base URL should parse");
    let backend = GoveeBackend::new(GoveeConfig::default()).with_cloud_client(client);
    adopt_cloud_device(&backend, cloud_device);
    backend.connect(&device_id).await.expect("connect");

    backend
        .set_brightness(&device_id, 250)
        .await
        .expect("cloud brightness should send");

    let requests = requests.await.expect("server task should join");
    let body = request_json_body(&requests[1]);
    assert_eq!(body["cmd"]["name"], "brightness");
    assert_eq!(body["cmd"]["value"], 100);
}

struct GoveeRig {
    backend: GoveeBackend,
    id: DeviceId,
    receiver: UdpSocket,
    /// Razer mode switches the device received: on is true, off false.
    razer_modes: Vec<bool>,
}

impl GoveeRig {
    /// Read what arrived, remembering the Razer mode switches among it.
    fn drain(&mut self) -> usize {
        let on = encode_razer_mode_base64(true);
        let off = encode_razer_mode_base64(false);
        let mut buf = [0_u8; 2048];
        let mut count = 0;
        while let Ok((len, _)) = self.receiver.try_recv_from(&mut buf) {
            count += 1;
            let payload = String::from_utf8_lossy(&buf[..len]);
            if payload.contains(&on) {
                self.razer_modes.push(true);
            } else if payload.contains(&off) {
                self.razer_modes.push(false);
            }
        }
        count
    }
}

#[async_trait::async_trait]
impl SessionRig for GoveeRig {
    fn backend(&self) -> &dyn DeviceBackend {
        &self.backend
    }

    fn device_id(&self) -> DeviceId {
        self.id
    }

    fn frame(&self, shade: u8) -> Vec<[u8; 3]> {
        vec![[shade, 0, 255]]
    }

    async fn saw_traffic(&mut self, window: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + window;
        while tokio::time::Instant::now() < deadline {
            if self.drain() > 0 {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        false
    }

    async fn discard_traffic(&mut self) {
        self.drain();
    }

    /// Every Govee session sends from the backend's one LAN socket, so
    /// there is nothing private to release, and sharing is proven by
    /// `closing_one_session_leaves_the_shared_socket_running`. What a close
    /// leaves is its goodbye datagram, which nothing acknowledges, so wait
    /// for it to land before anything counts teardowns.
    async fn assert_transport_released(&mut self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            self.drain();
            if self.razer_modes.last() != Some(&true) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "a closed session left the device in Razer mode"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn teardowns(&mut self) -> usize {
        self.drain();
        self.razer_modes.iter().filter(|on| !**on).count()
    }

    async fn engaged(&mut self) -> bool {
        self.drain();
        self.razer_modes.last() == Some(&true)
    }
}

#[tokio::test]
async fn govee_sessions_keep_the_contract() {
    let receiver = UdpSocket::bind(("127.0.0.1", 0))
        .await
        .expect("test UDP socket should bind");
    let target = receiver.local_addr().expect("test socket has local addr");
    let device = test_device("H619A");
    let id = build_device_info(&device).id;
    let backend = GoveeBackend::new(GoveeConfig::default());
    adopt_lan_device(&backend, device, target);

    let mut rig = GoveeRig {
        backend,
        id,
        receiver,
        razer_modes: Vec::new(),
    };
    assert_session_contract(&mut rig).await;
}

#[tokio::test]
async fn closing_one_session_leaves_the_shared_socket_running() {
    let receiver = UdpSocket::bind(("127.0.0.1", 0))
        .await
        .expect("test UDP socket should bind");
    let target = receiver.local_addr().expect("test socket has local addr");
    let first = test_device("H6163");
    let second = GoveeLanDevice {
        mac: "ffeeddccbbaa".to_owned(),
        ..test_device("H6163")
    };
    let (first_id, second_id) = (build_device_info(&first).id, build_device_info(&second).id);
    let backend = GoveeBackend::new(GoveeConfig::default());
    adopt_lan_device(&backend, first, target);
    adopt_lan_device(&backend, second, target);
    backend.connect(&first_id).await.expect("connect first");
    backend.connect(&second_id).await.expect("connect second");
    recv_payload(&receiver).await;
    recv_payload(&receiver).await;

    backend.disconnect(&first_id).await.expect("disconnect");

    backend
        .write_colors(&second_id, &[[0, 255, 0]])
        .await
        .expect("the other session keeps its socket");
    assert!(recv_payload(&receiver).await.contains("\"colorwc\""));
}

#[tokio::test]
async fn retained_sink_goes_dark_when_the_device_disconnects() {
    let receiver = UdpSocket::bind(("127.0.0.1", 0))
        .await
        .expect("test UDP socket should bind");
    let target = receiver.local_addr().expect("test socket has local addr");
    let device = test_device("H6163");
    let id = build_device_info(&device).id;
    let backend = GoveeBackend::new(GoveeConfig {
        power_off_on_disconnect: true,
        ..GoveeConfig::default()
    });
    adopt_lan_device(&backend, device, target);
    backend.connect(&id).await.expect("connect");
    recv_payload(&receiver).await;
    let sink = backend.frame_sink(&id).expect("sink");

    backend.disconnect(&id).await.expect("disconnect");

    assert!(recv_payload(&receiver).await.contains("\"turn\""));
    assert!(sink.is_closed());
    assert!(
        sink.write_direct(id, Arc::new(vec![[9, 9, 9]]))
            .await
            .is_err()
    );
    assert_no_payload(&receiver).await;
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
