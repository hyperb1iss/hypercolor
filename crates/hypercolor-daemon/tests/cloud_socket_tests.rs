#![cfg(feature = "cloud")]

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use hypercolor_cloud_client::DaemonConnectRequest;
use hypercolor_cloud_client::daemon_link::{
    HEADER_AUTHORIZATION, HEADER_DAEMON_ID, HEADER_DAEMON_NONCE, HEADER_DAEMON_SIG,
    HEADER_DAEMON_TS, HEADER_DAEMON_VERSION, HEADER_WEBSOCKET_PROTOCOL, IdentityKeypair,
    PROTOCOL_VERSION, UpgradeHeaderInput, UpgradeNonce, WEBSOCKET_PROTOCOL, frame::TunnelResume,
};
use hypercolor_daemon::cloud_connection::{CloudConnectionRuntime, CloudConnectionRuntimeState};
use hypercolor_daemon::cloud_socket::{
    CloudSocketError, CloudSocketHelloInput, CloudSocketRuntime, connect_prepared_once, hello_frame,
};
use tokio::sync::{RwLock, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse as WsErrorResponse, Request as WsRequest, Response as WsResponse,
};
use uuid::Uuid;

#[tokio::test]
async fn cloud_socket_consumes_prepared_request_and_records_welcome() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("test server should bind");
    let addr = listener
        .local_addr()
        .expect("test server address should resolve");
    let server = tokio::spawn(async move {
        let (stream, _) = listener
            .accept()
            .await
            .expect("client should connect to test cloud");
        let mut socket = tokio_tungstenite::accept_hdr_async(stream, assert_handshake)
            .await
            .expect("test cloud should accept websocket");

        let hello = socket
            .next()
            .await
            .expect("daemon should send hello")
            .expect("hello frame should read");
        let Message::Text(hello) = hello else {
            panic!("hello should be text");
        };
        let hello: hypercolor_cloud_client::daemon_link::HelloFrame =
            serde_json::from_str(&hello).expect("hello should deserialize");
        assert!(hello.daemon_capabilities.sync);
        assert!(hello.daemon_capabilities.relay);
        assert!(hello.daemon_capabilities.entitlement_refresh);
        assert!(!hello.daemon_capabilities.telemetry);
        assert!(!hello.daemon_capabilities.studio_preview);
        assert_eq!(hello.protocol_version, PROTOCOL_VERSION);
        assert_eq!(hello.entitlement_jwt.as_deref(), Some("entitlement.jwt"));
        assert!(hello.tunnel_resume.is_none());

        socket
            .send(Message::Text(welcome_fixture().into()))
            .await
            .expect("welcome should send");
    });
    let runtime = Arc::new(RwLock::new(CloudConnectionRuntime::default()));
    runtime.write().await.mark_prepared(connect_request(addr));

    let session = connect_prepared_once(&runtime, hello_input())
        .await
        .expect("prepared connection should complete welcome");

    server.await.expect("test cloud task should join");
    assert_eq!(
        session.welcome().session_id.to_string(),
        "00000000000000000000000000"
    );
    let snapshot = runtime.read().await.snapshot();
    assert_eq!(
        snapshot.runtime_state,
        CloudConnectionRuntimeState::Connected
    );
    assert!(snapshot.connected);
    assert_eq!(
        snapshot.session_id.as_deref(),
        Some("00000000000000000000000000")
    );
    assert!(runtime.write().await.take_prepared_connect().is_none());
}

#[tokio::test]
async fn cloud_socket_runtime_shutdown_aborts_live_session_and_marks_idle() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("test server should bind");
    let addr = listener
        .local_addr()
        .expect("test server address should resolve");
    let (welcome_tx, welcome_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener
            .accept()
            .await
            .expect("client should connect to test cloud");
        let mut socket = tokio_tungstenite::accept_hdr_async(stream, assert_handshake)
            .await
            .expect("test cloud should accept websocket");
        let _ = socket
            .next()
            .await
            .expect("daemon should send hello")
            .expect("hello frame should read");

        socket
            .send(Message::Text(welcome_fixture().into()))
            .await
            .expect("welcome should send");
        let _ = welcome_tx.send(());
        let _ = release_rx.await;
    });
    let runtime = Arc::new(RwLock::new(CloudConnectionRuntime::default()));
    runtime.write().await.mark_prepared(connect_request(addr));
    let mut socket_runtime = CloudSocketRuntime::default();

    socket_runtime
        .spawn_prepared_session(Arc::clone(&runtime), hello_input())
        .await
        .expect("prepared session should spawn");
    tokio::time::timeout(std::time::Duration::from_secs(2), welcome_rx)
        .await
        .expect("welcome should arrive")
        .expect("welcome signal should send");
    wait_for_runtime_state(&runtime, CloudConnectionRuntimeState::Connected).await;

    socket_runtime.shutdown(&runtime).await;
    assert_eq!(
        runtime.read().await.snapshot().runtime_state,
        CloudConnectionRuntimeState::Idle
    );
    let _ = release_tx.send(());
    server.await.expect("test cloud task should join");
}

#[tokio::test]
async fn cloud_socket_runtime_shutdown_without_task_preserves_state() {
    let runtime = Arc::new(RwLock::new(CloudConnectionRuntime::default()));
    runtime.write().await.mark_backoff("previous failure");
    let mut socket_runtime = CloudSocketRuntime::default();

    socket_runtime.shutdown(&runtime).await;

    let snapshot = runtime.read().await.snapshot();
    assert_eq!(snapshot.runtime_state, CloudConnectionRuntimeState::Backoff);
    assert_eq!(snapshot.last_error.as_deref(), Some("previous failure"));
}

#[tokio::test]
async fn cloud_socket_requires_prepared_request() {
    let runtime = Arc::new(RwLock::new(CloudConnectionRuntime::default()));

    let error = connect_prepared_once(&runtime, hello_input())
        .await
        .expect_err("missing prepared request should fail");

    assert!(matches!(error, CloudSocketError::MissingPreparedRequest));
    assert_eq!(
        runtime.read().await.snapshot().runtime_state,
        CloudConnectionRuntimeState::Idle
    );
}

#[tokio::test]
async fn cloud_socket_records_backoff_on_non_text_welcome() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("test server should bind");
    let addr = listener
        .local_addr()
        .expect("test server address should resolve");
    let server = tokio::spawn(async move {
        let (stream, _) = listener
            .accept()
            .await
            .expect("client should connect to test cloud");
        let mut socket = tokio_tungstenite::accept_hdr_async(stream, assert_handshake)
            .await
            .expect("test cloud should accept websocket");
        let _ = socket
            .next()
            .await
            .expect("daemon should send hello")
            .expect("hello frame should read");
        socket
            .send(Message::Binary(vec![0x7f].into()))
            .await
            .expect("binary frame should send");
    });
    let runtime = Arc::new(RwLock::new(CloudConnectionRuntime::default()));
    runtime.write().await.mark_prepared(connect_request(addr));

    let error = connect_prepared_once(&runtime, hello_input())
        .await
        .expect_err("binary welcome should fail");

    server.await.expect("test cloud task should join");
    assert!(matches!(error, CloudSocketError::NonTextWelcome));
    let snapshot = runtime.read().await.snapshot();
    assert_eq!(snapshot.runtime_state, CloudConnectionRuntimeState::Backoff);
    assert_eq!(
        snapshot.last_error.as_deref(),
        Some("cloud sent non-text welcome frame")
    );
}

#[test]
fn cloud_socket_hello_frame_carries_resume_and_capabilities() {
    let resume: TunnelResume = serde_json::from_value(serde_json::json!({
        "session_id": "00000000000000000000000000",
        "last_seq": 42
    }))
    .expect("resume should deserialize");

    let hello = hello_frame(CloudSocketHelloInput {
        entitlement_jwt: Some("fresh.entitlement.jwt".into()),
        tunnel_resume: Some(resume.clone()),
        studio_preview: true,
    });

    assert_eq!(hello.protocol_version, PROTOCOL_VERSION);
    assert!(hello.daemon_capabilities.studio_preview);
    assert_eq!(
        hello.entitlement_jwt.as_deref(),
        Some("fresh.entitlement.jwt")
    );
    assert_eq!(hello.tunnel_resume, Some(resume));
}

fn connect_request(addr: std::net::SocketAddr) -> DaemonConnectRequest {
    let daemon_id =
        Uuid::parse_str("018f4c36-4a44-7cc9-9f57-0d2e9224d2f1").expect("daemon id should parse");
    let keypair = IdentityKeypair::generate();
    let nonce = UpgradeNonce::from_bytes([9_u8; 16]);
    let host = addr.to_string();
    let url = format!("ws://{host}/v1/daemon/connect")
        .parse()
        .expect("connect url should parse");
    let headers = UpgradeHeaderInput {
        host: &host,
        daemon_id,
        daemon_version: "1.4.2",
        timestamp: "2026-05-15T17:00:00Z",
        nonce: &nonce,
        authorization_jwt: "daemon-connect-token",
    }
    .signed_headers(&keypair);

    DaemonConnectRequest { url, headers }
}

fn hello_input() -> CloudSocketHelloInput {
    CloudSocketHelloInput {
        entitlement_jwt: Some("entitlement.jwt".into()),
        tunnel_resume: None,
        studio_preview: false,
    }
}

fn welcome_fixture() -> String {
    serde_json::to_string(&serde_json::json!({
        "session_id": "00000000000000000000000000",
        "available_channels": ["control", "sync.notifications"],
        "denied_channels": [],
        "server_capabilities": {
            "tunnel_resume": true,
            "compression": [],
            "max_frame_bytes": 1_048_576
        },
        "heartbeat_interval_s": 25
    }))
    .expect("welcome should serialize")
}

async fn wait_for_runtime_state(
    runtime: &Arc<RwLock<CloudConnectionRuntime>>,
    expected: CloudConnectionRuntimeState,
) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if runtime.read().await.snapshot().runtime_state == expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for cloud runtime state {expected:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[expect(
    clippy::result_large_err,
    clippy::unnecessary_wraps,
    reason = "Tungstenite's Callback trait fixes the response error type"
)]
fn assert_handshake(
    request: &WsRequest,
    mut response: WsResponse,
) -> Result<WsResponse, WsErrorResponse> {
    assert_eq!(
        request
            .headers()
            .get(HEADER_AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer daemon-connect-token")
    );
    assert_eq!(
        request
            .headers()
            .get(HEADER_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok()),
        Some(WEBSOCKET_PROTOCOL)
    );
    assert_eq!(
        request
            .headers()
            .get(HEADER_DAEMON_ID)
            .and_then(|value| value.to_str().ok()),
        Some("018f4c36-4a44-7cc9-9f57-0d2e9224d2f1")
    );
    assert_eq!(
        request
            .headers()
            .get(HEADER_DAEMON_VERSION)
            .and_then(|value| value.to_str().ok()),
        Some("1.4.2")
    );
    assert_eq!(
        request
            .headers()
            .get(HEADER_DAEMON_TS)
            .and_then(|value| value.to_str().ok()),
        Some("2026-05-15T17:00:00Z")
    );
    assert_eq!(
        request
            .headers()
            .get(HEADER_DAEMON_NONCE)
            .and_then(|value| value.to_str().ok()),
        Some(UpgradeNonce::from_bytes([9_u8; 16]).as_str())
    );
    assert!(
        request
            .headers()
            .get(HEADER_DAEMON_SIG)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| !value.is_empty())
    );
    response.headers_mut().insert(
        HEADER_WEBSOCKET_PROTOCOL,
        WEBSOCKET_PROTOCOL
            .parse()
            .expect("websocket protocol should parse"),
    );

    Ok(response)
}
