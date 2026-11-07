//! The shared session contract against a loopback panel controller.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use hypercolor_driver_api::{DeviceBackend, DiscoveryConnectBehavior};
use hypercolor_driver_conformance::{SessionRig, assert_session_contract, assert_udp_port_released};
use hypercolor_driver_nanoleaf::{
    NanoleafBackend, NanoleafConfig, NanoleafDiscoveredDevice, build_device_info,
};
use hypercolor_driver_support::CredentialStore;
use hypercolor_types::device::DeviceId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct NanoleafRig {
    backend: NanoleafBackend,
    id: DeviceId,
    receiver: UdpSocket,
    last_source: Option<SocketAddr>,
}

#[async_trait]
impl SessionRig for NanoleafRig {
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
        let mut buffer = [0_u8; 256];
        match tokio::time::timeout(window, self.receiver.recv_from(&mut buffer)).await {
            Ok(Ok((_, source))) => {
                self.last_source = Some(source);
                true
            }
            _ => false,
        }
    }

    async fn discard_traffic(&mut self) {
        let mut buffer = [0_u8; 256];
        while self.receiver.try_recv_from(&mut buffer).is_ok() {}
    }

    async fn assert_transport_released(&mut self) {
        let source = self.last_source.expect("a frame arrived");
        assert_udp_port_released(source.port()).await;
    }
}

async fn serve_controller(listener: TcpListener) {
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(answer(stream));
    }
}

async fn answer(mut stream: TcpStream) {
    let mut buffer = vec![0_u8; 4096];
    let mut read = 0;
    while !buffer[..read].windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut buffer[read..]).await {
            Ok(0) | Err(_) => return,
            Ok(count) => read += count,
        }
    }
    let request = String::from_utf8_lossy(&buffer[..read]);
    let body = if request.starts_with("GET /api/v1/test-token/panelLayout/layout") {
        r#"{"positionData":[{"panelId":41,"x":0,"y":0,"o":0,"shapeType":7},{"panelId":42,"x":10,"y":10,"o":90,"shapeType":17}]}"#
    } else if request.starts_with("GET /api/v1/test-token") {
        r#"{"name":"Shapes","model":"Shapes","serialNo":"SERIAL42","firmwareVersion":"12.0.0"}"#
    } else {
        "{}"
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

#[tokio::test]
async fn nanoleaf_sessions_keep_the_contract() -> TestResult {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let api_port = listener.local_addr()?.port();
    let controller = tokio::spawn(serve_controller(listener));

    let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let stream_port = receiver.local_addr()?.port();

    let tempdir = tempfile::tempdir()?;
    let store = Arc::new(CredentialStore::open(tempdir.path()).await?);
    store
        .store_driver_json(
            "nanoleaf",
            "shapes",
            serde_json::json!({ "auth_token": "test-token" }),
        )
        .await?;
    let backend = NanoleafBackend::new(NanoleafConfig::default(), store).with_stream_port(stream_port);

    let discovered = NanoleafDiscoveredDevice {
        device_key: "shapes".to_owned(),
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        api_port,
        info: build_device_info("shapes", "Shapes", Some("Shapes"), None, &[]),
        panel_ids: Vec::new(),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        metadata: HashMap::from([
            ("device_key".to_owned(), "shapes".to_owned()),
            ("ip".to_owned(), "127.0.0.1".to_owned()),
            ("api_port".to_owned(), api_port.to_string()),
        ]),
        claim: None,
    };
    let id = discovered.info.id;
    backend.adopt_device(&discovered.into_discovered())?;

    let mut rig = NanoleafRig {
        backend,
        id,
        receiver,
        last_source: None,
    };
    assert_session_contract(&mut rig).await;

    controller.abort();
    Ok(())
}
