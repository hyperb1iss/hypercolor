use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use hypercolor_driver_api::DeviceBackend;
use hypercolor_driver_api::DiscoveryConnectBehavior;
use hypercolor_driver_nanoleaf::{
    NanoleafBackend, NanoleafConfig, NanoleafDiscoveredDevice, build_device_info,
};
use hypercolor_driver_support::CredentialStore;
use hypercolor_types::device::DeviceError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::time::timeout;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn backend_connect_write_brightness_and_disconnect() -> TestResult {
    let api_listener = TcpListener::bind("127.0.0.1:0").await?;
    let api_port = api_listener.local_addr()?.port();
    let api_task = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut stream, _) = api_listener.accept().await.expect("accept request");
            let request = read_http_request(&mut stream)
                .await
                .expect("read HTTP request");
            let response = if request.starts_with("GET /api/v1/test-token/panelLayout HTTP/1.1") {
                json_response(
                    r#"{"globalOrientation":{"value":0},"layout":{"positionData":[{"panelId":41,"x":0,"y":0,"o":0,"shapeType":7},{"panelId":42,"x":10,"y":10,"o":90,"shapeType":17},{"panelId":10,"x":20,"y":20,"o":180,"shapeType":12}]}}"#,
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

    let receiver = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let stream_port = receiver.local_addr()?.port();

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
    let backend = NanoleafBackend::new(config, Arc::clone(&store)).with_stream_port(stream_port);

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
            0.0,
        ),
        panel_ids: Vec::new(),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        metadata: HashMap::from([
            ("device_key".to_owned(), "living-room".to_owned()),
            ("ip".to_owned(), "127.0.0.1".to_owned()),
            ("api_port".to_owned(), api_port.to_string()),
        ]),
        claim: None,
    };
    let device_id = discovered.info.id;
    backend.adopt_device(&discovered.into_discovered())?;

    backend.connect(&device_id).await?;

    let info = backend
        .connected_device_info(&device_id)
        .await?
        .expect("connected device info");
    assert_eq!(info.total_led_count(), 2);
    assert_eq!(info.segments.len(), 2);

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
    let backend = NanoleafBackend::new(
        NanoleafConfig {
            device_ips: Vec::new(),
            transition_time: 1,
        },
        store,
    );

    let error = backend
        .connect(&hypercolor_types::device::DeviceId::new())
        .await
        .expect_err("connect without discovery should fail");
    assert!(matches!(error, DeviceError::NotAdopted { .. }));
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

#[derive(Debug)]
enum TouchObservation {
    Attached,
    Edges(Vec<hypercolor_types::device_input::DeviceInputEdge>),
    Detached,
}

struct TouchRecorder(tokio::sync::mpsc::UnboundedSender<TouchObservation>);
struct TouchPublisher(tokio::sync::mpsc::UnboundedSender<TouchObservation>);

impl hypercolor_driver_api::DeviceInputSink for TouchRecorder {
    fn attach(
        &self,
        _device_id: hypercolor_types::device::DeviceId,
        _label: &str,
    ) -> Box<dyn hypercolor_driver_api::DeviceInputPublisher> {
        let _ = self.0.send(TouchObservation::Attached);
        Box::new(TouchPublisher(self.0.clone()))
    }
}

impl hypercolor_driver_api::DeviceInputPublisher for TouchPublisher {
    fn publish(&self, edges: &[hypercolor_types::device_input::DeviceInputEdge]) -> bool {
        self.0.send(TouchObservation::Edges(edges.to_vec())).is_ok()
    }
}

impl Drop for TouchPublisher {
    fn drop(&mut self) {
        let _ = self.0.send(TouchObservation::Detached);
    }
}

#[tokio::test]
async fn touch_stream_reconnects_and_disconnect_retires_input_with_retained_frame_sink()
-> TestResult {
    touch_stream_lifecycle(TouchShutdown::Disconnect).await
}

#[tokio::test]
async fn dropping_backend_retires_touch_input_with_retained_frame_sink() -> TestResult {
    touch_stream_lifecycle(TouchShutdown::Drop).await
}

#[tokio::test]
async fn retiring_backend_retires_touch_input_and_prevents_reopening() -> TestResult {
    touch_stream_lifecycle(TouchShutdown::Retire).await
}

enum TouchShutdown {
    Disconnect,
    Retire,
    Drop,
}

async fn touch_stream_lifecycle(shutdown: TouchShutdown) -> TestResult {
    use hypercolor_types::device_input::DeviceInputEdge;
    let api = TcpListener::bind("127.0.0.1:0").await?;
    let api_port = api.local_addr()?.port();
    let (close_tx, close_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        answer_touch_setup(&api).await?;
        let mut first = open_touch_stream(&api).await?;
        close_rx.await?;
        first.shutdown().await?;
        drop(first);
        let mut second = open_touch_stream(&api).await?;
        let mut byte = [0];
        assert_eq!(second.read(&mut byte).await?, 0, "disconnect closes SSE");
        TestResult::Ok(())
    });
    let output = UdpSocket::bind("127.0.0.1:0").await?;
    let tempdir = tempfile::tempdir()?;
    let store = Arc::new(CredentialStore::open(tempdir.path()).await?);
    store
        .store_driver_json(
            "nanoleaf",
            "touch",
            serde_json::json!({"auth_token":"touch-secret"}),
        )
        .await?;
    let (tx, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let backend = NanoleafBackend::new(NanoleafConfig::default(), store)
        .with_stream_port(output.local_addr()?.port())
        .with_device_input(Arc::new(TouchRecorder(tx)));
    let discovered = NanoleafDiscoveredDevice {
        device_key: "touch".to_owned(),
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        api_port,
        info: build_device_info("touch", "Touch Shapes", Some("NL42"), None, &[], 0.0),
        panel_ids: Vec::new(),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        metadata: HashMap::from([
            ("device_key".to_owned(), "touch".to_owned()),
            ("ip".to_owned(), "127.0.0.1".to_owned()),
            ("api_port".to_owned(), api_port.to_string()),
        ]),
        claim: None,
    };
    let id = discovered.info.id;
    backend.adopt_device(&discovered.into_discovered())?;
    backend.connect(&id).await?;
    let retained = backend.frame_sink(&id).expect("frame sink");
    assert!(matches!(
        next_touch(&mut observations).await?,
        TouchObservation::Attached
    ));
    let TouchObservation::Edges(edges) = next_touch(&mut observations).await? else {
        panic!("touch edge expected");
    };
    assert!(matches!(
        &edges[..],
        [DeviceInputEdge::TouchBegan { contact: 41, .. }]
    ));
    close_tx.send(()).expect("close first stream");
    assert!(matches!(
        next_touch(&mut observations).await?,
        TouchObservation::Detached
    ));
    assert!(matches!(
        next_touch(&mut observations).await?,
        TouchObservation::Attached
    ));
    let TouchObservation::Edges(edges) = next_touch(&mut observations).await? else {
        panic!("touch edge expected");
    };
    assert!(matches!(
        &edges[..],
        [DeviceInputEdge::TouchBegan { contact: 41, .. }]
    ));
    backend.write_colors(&id, &[[1, 2, 3]]).await?;
    let mut frame = [0; 32];
    assert_eq!(
        timeout(Duration::from_secs(2), output.recv(&mut frame)).await??,
        10
    );
    match shutdown {
        TouchShutdown::Disconnect => backend.disconnect(&id).await?,
        TouchShutdown::Retire => {
            backend.retire().await?;
            assert!(backend.connect(&id).await.is_err());
        }
        TouchShutdown::Drop => drop(backend),
    }
    assert!(retained.is_closed());
    assert!(
        retained
            .write_direct(id, Arc::new(vec![[1, 2, 3]]))
            .await
            .is_err()
    );
    assert!(matches!(
        next_touch(&mut observations).await?,
        TouchObservation::Detached
    ));
    timeout(Duration::from_secs(2), server).await???;
    drop(retained);
    Ok(())
}

async fn next_touch(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<TouchObservation>,
) -> TestResult<TouchObservation> {
    Ok(timeout(Duration::from_secs(3), receiver.recv())
        .await?
        .expect("touch observation"))
}

async fn answer_touch_setup(api: &TcpListener) -> TestResult {
    for _ in 0..3 {
        let (mut stream, _) = api.accept().await?;
        let request = read_http_request(&mut stream).await?;
        let body = if request.starts_with("GET /api/v1/touch-secret/panelLayout ") {
            r#"{"globalOrientation":{"value":0},"layout":{"positionData":[{"panelId":41,"x":0,"y":0,"o":0,"shapeType":7}]}}"#
        } else if request.starts_with("GET /api/v1/touch-secret HTTP") {
            r#"{"name":"Touch Shapes","model":"NL42","serialNo":"TOUCH","firmwareVersion":"12.0.0"}"#
        } else {
            assert!(request.starts_with("PUT /api/v1/touch-secret/effects HTTP"));
            "{}"
        };
        stream.write_all(&json_response(body)).await?;
    }
    Ok(())
}

async fn open_touch_stream(api: &TcpListener) -> TestResult<TcpStream> {
    let (mut stream, peer) = api.accept().await?;
    let request = read_http_request(&mut stream).await?;
    assert!(request.starts_with("GET /api/v1/touch-secret/events?id=4 HTTP"));
    let port = request
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("TouchEventsPort")
                .then(|| value.trim().parse::<u16>().expect("touch UDP port"))
        })
        .expect("TouchEventsPort header");
    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n:\n\n\r\n").await?;
    let udp = UdpSocket::bind("127.0.0.1:0").await?;
    udp.send_to(
        &[0, 1, 0, 41, 0x1f, 0xff, 0xff],
        SocketAddr::new(peer.ip(), port),
    )
    .await?;
    Ok(stream)
}

#[tokio::test]
async fn unsupported_touch_subscription_leaves_lighting_available() -> TestResult {
    let api = TcpListener::bind("127.0.0.1:0").await?;
    let api_port = api.local_addr()?.port();
    let server = tokio::spawn(async move {
        answer_touch_setup(&api).await?;
        let (mut stream, _) = api.accept().await?;
        let request = read_http_request(&mut stream).await?;
        assert!(request.starts_with("GET /api/v1/touch-secret/events?id=4 HTTP"));
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
        TestResult::Ok(())
    });
    let output = UdpSocket::bind("127.0.0.1:0").await?;
    let tempdir = tempfile::tempdir()?;
    let store = Arc::new(CredentialStore::open(tempdir.path()).await?);
    store
        .store_driver_json(
            "nanoleaf",
            "touch",
            serde_json::json!({"auth_token":"touch-secret"}),
        )
        .await?;
    let (tx, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let backend = NanoleafBackend::new(NanoleafConfig::default(), store)
        .with_stream_port(output.local_addr()?.port())
        .with_device_input(Arc::new(TouchRecorder(tx)));
    let discovered = NanoleafDiscoveredDevice {
        device_key: "touch".to_owned(),
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        api_port,
        info: build_device_info("touch", "Touch Shapes", Some("NL42"), None, &[], 0.0),
        panel_ids: Vec::new(),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        metadata: HashMap::from([
            ("device_key".to_owned(), "touch".to_owned()),
            ("ip".to_owned(), "127.0.0.1".to_owned()),
            ("api_port".to_owned(), api_port.to_string()),
        ]),
        claim: None,
    };
    let id = discovered.info.id;
    backend.adopt_device(&discovered.into_discovered())?;
    backend.connect(&id).await?;
    timeout(Duration::from_secs(2), server).await???;
    backend.write_colors(&id, &[[3, 4, 5]]).await?;
    let mut frame = [0; 32];
    let length = timeout(Duration::from_secs(2), output.recv(&mut frame)).await??;
    assert_eq!(&frame[..length], &[0, 1, 0, 41, 3, 4, 5, 0, 0, 1]);
    backend.disconnect(&id).await?;
    assert!(matches!(
        observations.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    Ok(())
}
