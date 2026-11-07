//! The shared session contract against a loopback bridge.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use hypercolor_driver_api::{DeviceBackend, DiscoveryConnectBehavior};
use hypercolor_driver_conformance::{SessionRig, assert_session_contract};
use hypercolor_driver_hue::{HueBackend, HueConfig, HueDiscoveredBridge, build_device_info};
use hypercolor_driver_support::CredentialStore;
use hypercolor_types::device::DeviceId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use webrtc_dtls::cipher_suite::CipherSuiteId;
use webrtc_dtls::config::Config as DtlsConfig;
use webrtc_dtls::conn::DTLSConn;
use webrtc_util::conn::Listener;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const CONFIG_ID: &str = "12345678-1234-1234-1234-123456789abc";
const SLOW_TEARDOWN: Duration = Duration::from_millis(50);
const CLIENT_KEY: [u8; 16] = [
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];

#[derive(Default)]
struct Bridge {
    streams_opened: AtomicUsize,
    streams_closed: AtomicUsize,
    actions: Mutex<Vec<String>>,
}

impl Bridge {
    fn count(&self, action: &str) -> usize {
        let actions = self.actions.lock().expect("actions lock");
        actions.iter().filter(|entry| *entry == action).count()
    }
}

struct HueRig {
    backend: HueBackend,
    id: DeviceId,
    packets: mpsc::UnboundedReceiver<Vec<u8>>,
    bridge: Arc<Bridge>,
}

#[async_trait]
impl SessionRig for HueRig {
    const TEARDOWN_ANSWERED: bool = true;

    fn backend(&self) -> &dyn DeviceBackend {
        &self.backend
    }

    fn device_id(&self) -> DeviceId {
        self.id
    }

    fn frame(&self, shade: u8) -> Vec<[u8; 3]> {
        vec![[shade, 0, 255]; 2]
    }

    async fn saw_traffic(&mut self, window: Duration) -> bool {
        tokio::time::timeout(window, self.packets.recv())
            .await
            .is_ok_and(|packet| packet.is_some())
    }

    async fn discard_traffic(&mut self) {
        while self.packets.try_recv().is_ok() {}
    }

    /// The bridge saw every stream end and was told to leave entertainment
    /// mode each time, the two things that free it for the next client.
    async fn assert_transport_released(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let starts = self.bridge.count("start");
            let stops = self.bridge.count("stop");
            let opened = self.bridge.streams_opened.load(Ordering::SeqCst);
            let closed = self.bridge.streams_closed.load(Ordering::SeqCst);
            if starts == stops && opened == closed {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "a closed session left the bridge streaming: {starts} starts, {stops} stops, \
                 {opened} streams opened, {closed} closed"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn teardowns(&mut self) -> usize {
        self.bridge.count("stop")
    }

    async fn engaged(&mut self) -> bool {
        let actions = self.bridge.actions.lock().expect("actions lock");
        actions.last().is_some_and(|action| action == "start")
    }
}

async fn serve_http(listener: TcpListener, bridge: Arc<Bridge>) {
    while let Ok((mut stream, _)) = listener.accept().await {
        let bridge = Arc::clone(&bridge);
        tokio::spawn(async move {
            let Ok(request) = read_http_request(&mut stream).await else {
                return;
            };
            // A slow stop lands after any session that raced ahead of it.
            if request.contains(r#""action":"stop""#) {
                tokio::time::sleep(SLOW_TEARDOWN).await;
            }
            let _ = stream.write_all(&respond(&request, &bridge)).await;
        });
    }
}

fn respond(request: &str, bridge: &Bridge) -> Vec<u8> {
    if request.starts_with("GET /api/config HTTP/1.1") {
        json_response(
            r#"{"bridgeid":"test-bridge","name":"Bridge","modelid":"BSB002","swversion":"1968096020"}"#,
        )
    } else if request.starts_with("GET /clip/v2/resource/light HTTP/1.1") {
        json_response(
            r#"{"data":[{"id":"light-left","metadata":{"name":"Left"},"product_data":{"model_id":"LCA001"},"color":{"gamut_type":"C"}},{"id":"light-right","metadata":{"name":"Right"},"product_data":{"model_id":"LCA001"},"color":{"gamut_type":"C"}}]}"#,
        )
    } else if request.starts_with("GET /clip/v2/resource/entertainment_configuration HTTP/1.1") {
        json_response(&format!(
            r#"{{"data":[{{"id":"{CONFIG_ID}","metadata":{{"name":"Room"}},"configuration_type":"screen","channels":[{{"channel_id":1,"position":{{"x":-0.5,"y":0.0,"z":0.0}},"members":[{{"service":{{"rid":"light-left","rtype":"light"}}}}]}},{{"channel_id":2,"position":{{"x":0.5,"y":0.0,"z":0.0}},"members":[{{"service":{{"rid":"light-right","rtype":"light"}}}}]}}]}}]}}"#
        ))
    } else if request.starts_with("PUT /clip/v2/resource/entertainment_configuration/") {
        let action = if request.contains(r#""action":"stop""#) {
            "stop"
        } else {
            "start"
        };
        bridge
            .actions
            .lock()
            .expect("actions lock")
            .push(action.to_owned());
        json_response(r#"{"data":[{"success":true}]}"#)
    } else {
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
    }
}

fn json_response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

async fn read_http_request(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buffer = vec![0_u8; 4096];
    let mut total = 0;
    loop {
        let read = stream.read(&mut buffer[total..]).await?;
        if read == 0 {
            break;
        }
        total += read;
        let text = String::from_utf8_lossy(&buffer[..total]);
        if let Some((headers, body)) = text.split_once("\r\n\r\n") {
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if body.len() >= length {
                break;
            }
        }
    }
    Ok(String::from_utf8_lossy(&buffer[..total]).into_owned())
}

async fn serve_dtls(
    listener: impl Listener + Send + Sync + 'static,
    bridge: Arc<Bridge>,
    packets: mpsc::UnboundedSender<Vec<u8>>,
) {
    while let Ok((conn, _)) = listener.accept().await {
        let bridge = Arc::clone(&bridge);
        let packets = packets.clone();
        tokio::spawn(async move {
            let config = DtlsConfig {
                psk: Some(Arc::new(|_| Ok(CLIENT_KEY.to_vec()))),
                cipher_suites: vec![CipherSuiteId::Tls_Psk_With_Aes_128_Gcm_Sha256],
                ..Default::default()
            };
            let dtls = DTLSConn::new(conn, config, false, None)
                .await
                .expect("handshake DTLS");
            bridge.streams_opened.fetch_add(1, Ordering::SeqCst);
            let mut buffer = [0_u8; 256];
            while let Ok(len) = dtls.read(&mut buffer, None).await {
                let _ = packets.send(buffer[..len].to_vec());
            }
            bridge.streams_closed.fetch_add(1, Ordering::SeqCst);
        });
    }
}

#[tokio::test]
async fn hue_sessions_keep_the_contract() -> TestResult {
    let bridge_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let http_listener = TcpListener::bind(SocketAddr::new(bridge_ip, 0)).await?;
    let api_port = http_listener.local_addr()?.port();
    let dtls_listener =
        webrtc_util::conn::conn_udp_listener::listen(SocketAddr::new(bridge_ip, 2_100)).await?;

    let bridge = Arc::new(Bridge::default());
    let (packet_tx, packet_rx) = mpsc::unbounded_channel();
    let http = tokio::spawn(serve_http(http_listener, Arc::clone(&bridge)));
    let dtls = tokio::spawn(serve_dtls(dtls_listener, Arc::clone(&bridge), packet_tx));

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
    let backend = HueBackend::new(HueConfig::default(), store);
    let discovered = HueDiscoveredBridge {
        bridge_id: "test-bridge".to_owned(),
        ip: bridge_ip,
        api_port,
        info: build_device_info("test-bridge", "Bridge", Some("BSB002"), None, None, &[]),
        entertainment_config: None,
        lights: Vec::new(),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        metadata: HashMap::from([
            ("bridge_id".to_owned(), "test-bridge".to_owned()),
            ("ip".to_owned(), bridge_ip.to_string()),
            ("api_port".to_owned(), api_port.to_string()),
        ]),
    };
    let id = discovered.info.id;
    backend.adopt_device(&discovered.into_discovered())?;

    let mut rig = HueRig {
        backend,
        id,
        packets: packet_rx,
        bridge: Arc::clone(&bridge),
    };
    assert_session_contract(&mut rig).await;
    http.abort();
    dtls.abort();
    Ok(())
}
