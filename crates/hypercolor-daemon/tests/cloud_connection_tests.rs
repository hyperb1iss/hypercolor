#![cfg(feature = "cloud")]

use std::collections::HashMap;
use std::sync::Mutex;

use hypercolor_cloud_client::daemon_link::{
    HEADER_AUTHORIZATION, IdentityNonce, UpgradeNonce, WelcomeFrame,
};
use hypercolor_cloud_client::{
    CloudClient, CloudClientConfig, CloudClientError, CloudSecretKey, RefreshTokenOwner,
    SecretStore, load_or_create_identity, store_refresh_token,
};
use hypercolor_daemon::cloud_connection::{
    CloudConnectionPrepareInput, CloudConnectionPrepareResult, CloudConnectionRuntime,
    CloudConnectionRuntimeState,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Default)]
struct MemorySecretStore {
    values: Mutex<HashMap<CloudSecretKey, String>>,
}

impl SecretStore for MemorySecretStore {
    fn get_secret(&self, key: CloudSecretKey) -> Result<Option<String>, CloudClientError> {
        Ok(self
            .values
            .lock()
            .expect("memory secret store lock should not be poisoned")
            .get(&key)
            .cloned())
    }

    fn put_secret(&self, key: CloudSecretKey, value: &str) -> Result<(), CloudClientError> {
        self.values
            .lock()
            .expect("memory secret store lock should not be poisoned")
            .insert(key, value.to_owned());
        Ok(())
    }

    fn delete_secret(&self, key: CloudSecretKey) -> Result<(), CloudClientError> {
        self.values
            .lock()
            .expect("memory secret store lock should not be poisoned")
            .remove(&key);
        Ok(())
    }
}

#[test]
fn cloud_connection_runtime_records_transitions_and_clears_stale_channels() {
    let mut runtime = CloudConnectionRuntime::default();

    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.runtime_state, CloudConnectionRuntimeState::Idle);
    assert!(!snapshot.connected);

    runtime.mark_connecting();
    let snapshot = runtime.snapshot();
    assert_eq!(
        snapshot.runtime_state,
        CloudConnectionRuntimeState::Connecting
    );
    assert!(!snapshot.connected);
    assert!(snapshot.session_id.is_none());

    runtime.mark_connected(&welcome_fixture());
    let snapshot = runtime.snapshot();
    assert_eq!(
        snapshot.runtime_state,
        CloudConnectionRuntimeState::Connected
    );
    assert!(snapshot.connected);
    assert_eq!(
        snapshot.available_channels,
        vec!["control", "sync.notifications"]
    );
    assert_eq!(snapshot.denied_channels[0].name, "relay.ws");
    assert_eq!(snapshot.denied_channels[0].reason, "entitlement_missing");

    runtime.mark_backoff("cloud unavailable");
    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.runtime_state, CloudConnectionRuntimeState::Backoff);
    assert!(!snapshot.connected);
    assert!(snapshot.session_id.is_none());
    assert!(snapshot.available_channels.is_empty());
    assert!(snapshot.denied_channels.is_empty());
    assert_eq!(snapshot.last_error.as_deref(), Some("cloud unavailable"));

    runtime.mark_connecting();
    let snapshot = runtime.snapshot();
    assert_eq!(
        snapshot.runtime_state,
        CloudConnectionRuntimeState::Connecting
    );
    assert!(snapshot.last_error.is_none());

    runtime.mark_backoff("cloud unavailable");
    runtime.mark_idle();
    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.runtime_state, CloudConnectionRuntimeState::Idle);
    assert!(snapshot.last_error.is_none());
}

#[tokio::test]
async fn cloud_connection_runtime_prepares_registered_daemon_connect() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("test server should bind");
    let base_url = format!(
        "http://{}",
        listener
            .local_addr()
            .expect("test server address should resolve")
    );
    let store = MemorySecretStore::default();
    let identity = load_or_create_identity(&store).expect("identity should create");
    store_refresh_token(&store, RefreshTokenOwner::Daemon, "refresh-old")
        .expect("refresh token should seed");
    let server = spawn_connect_preparation_server(
        listener,
        "access-for-registration",
        "refresh-next",
        "daemon-connect-token",
        identity.daemon_id(),
        identity.keypair().public_key().as_str().to_owned(),
    );
    let client = CloudClient::new(
        CloudClientConfig::with_auth_base_url(&base_url, &base_url)
            .expect("base urls should parse"),
    );
    let mut runtime = CloudConnectionRuntime::default();
    runtime.mark_backoff("stale error");

    let result = runtime
        .prepare_stored_daemon_connect(
            &client,
            &store,
            prepare_input("desk-mac", [3_u8; 32], [7_u8; 16]),
        )
        .await
        .expect("stored daemon connect should prepare");

    tokio::time::timeout(std::time::Duration::from_secs(2), server)
        .await
        .expect("test server should finish")
        .expect("test server should not panic");
    let CloudConnectionPrepareResult::Prepared(request) = result else {
        panic!("runtime should return prepared connect request");
    };
    let pairs = request.headers.pairs();
    assert_eq!(
        header_value(&pairs, HEADER_AUTHORIZATION),
        "Bearer daemon-connect-token"
    );
    assert_eq!(
        request.url.as_str(),
        format!("{base_url}/v1/daemon/connect").replace("http://", "ws://")
    );

    let snapshot = runtime.snapshot();
    assert_eq!(
        snapshot.runtime_state,
        CloudConnectionRuntimeState::Connecting
    );
    assert!(!snapshot.connected);
    assert!(snapshot.last_error.is_none());
}

#[tokio::test]
async fn cloud_connection_runtime_records_missing_connect_prerequisites() {
    let client = CloudClient::new(
        CloudClientConfig::with_auth_base_url("http://127.0.0.1:1/", "http://127.0.0.1:1/")
            .expect("base urls should parse"),
    );
    let store = MemorySecretStore::default();
    let mut runtime = CloudConnectionRuntime::default();

    let missing_identity = runtime
        .prepare_stored_daemon_connect(
            &client,
            &store,
            prepare_input("desk-mac", [4_u8; 32], [8_u8; 16]),
        )
        .await
        .expect("missing identity should not fail");
    assert_eq!(
        missing_identity,
        CloudConnectionPrepareResult::MissingIdentity
    );
    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.runtime_state, CloudConnectionRuntimeState::Backoff);
    assert_eq!(
        snapshot.last_error.as_deref(),
        Some("missing cloud identity")
    );

    load_or_create_identity(&store).expect("identity should create");
    let missing_refresh = runtime
        .prepare_stored_daemon_connect(
            &client,
            &store,
            prepare_input("desk-mac", [5_u8; 32], [9_u8; 16]),
        )
        .await
        .expect("missing refresh token should not fail");
    assert_eq!(
        missing_refresh,
        CloudConnectionPrepareResult::MissingRefreshToken
    );
    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.runtime_state, CloudConnectionRuntimeState::Backoff);
    assert_eq!(
        snapshot.last_error.as_deref(),
        Some("missing cloud refresh token")
    );
}

fn prepare_input(
    install_name: &str,
    identity_nonce: [u8; 32],
    upgrade_nonce: [u8; 16],
) -> CloudConnectionPrepareInput<'_> {
    CloudConnectionPrepareInput {
        install_name,
        os: "macos",
        arch: "aarch64",
        daemon_version: "1.4.2",
        identity_nonce: IdentityNonce::from_bytes(identity_nonce),
        timestamp: "2026-05-15T17:00:00Z",
        upgrade_nonce: UpgradeNonce::from_bytes(upgrade_nonce),
    }
}

fn spawn_connect_preparation_server(
    listener: tokio::net::TcpListener,
    access_token: &'static str,
    refresh_token: &'static str,
    registration_token: &'static str,
    daemon_id: uuid::Uuid,
    identity_pubkey: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (mut token_socket, _) = listener
            .accept()
            .await
            .expect("token request should connect");
        let mut buffer = vec![0_u8; 4096];
        let read = token_socket
            .read(&mut buffer)
            .await
            .expect("token request should read");
        let token_request = String::from_utf8_lossy(&buffer[..read]);

        assert!(token_request.starts_with("POST /api/auth/oauth2/token HTTP/1.1"));
        assert!(token_request.contains(r#""refresh_token":"refresh-old""#));
        assert!(token_request.contains(r#""client_id":"hypercolor-daemon""#));

        let body = serde_json::json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "refresh_token": refresh_token,
            "expires_in": 900,
            "scope": "openid profile email"
        });
        write_json_response(&mut token_socket, &body).await;
        drop(token_socket);

        let (mut registration_socket, _) = listener
            .accept()
            .await
            .expect("registration request should connect");
        let mut buffer = vec![0_u8; 8192];
        let read = registration_socket
            .read(&mut buffer)
            .await
            .expect("registration request should read");
        let registration_request = String::from_utf8_lossy(&buffer[..read]);

        assert!(registration_request.starts_with("POST /v1/me/devices HTTP/1.1"));
        assert!(registration_request.contains(&format!("authorization: Bearer {access_token}")));
        assert!(registration_request.contains(&format!(r#""daemon_id":"{daemon_id}""#)));
        assert!(registration_request.contains(r#""install_name":"desk-mac""#));
        assert!(
            registration_request.contains(&format!(r#""identity_pubkey":"{identity_pubkey}""#))
        );

        let body = serde_json::json!({
            "device": {
                "id": "018f4c36-4a44-7cc9-9f57-0d2e9224d2f1",
                "user_id": "018f4c36-4a44-7cc9-9f57-0d2e9224d2f2",
                "daemon_id": daemon_id,
                "install_name": "desk-mac",
                "os": "macos",
                "arch": "aarch64",
                "daemon_version": "1.4.2",
                "identity_pubkey": identity_pubkey,
                "last_seen_at": "2026-05-15T17:00:00Z",
                "created_at": "2026-05-15T17:00:00Z"
            },
            "registration_token": registration_token
        });
        write_json_response(&mut registration_socket, &body).await;
        drop(registration_socket);
    })
}

async fn write_json_response(socket: &mut tokio::net::TcpStream, body: &serde_json::Value) {
    let body = serde_json::to_string(body).expect("response should serialize");
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    socket
        .write_all(response.as_bytes())
        .await
        .expect("response should write");
}

fn header_value<'a>(pairs: &'a [(&'static str, String)], name: &str) -> &'a str {
    pairs
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, value)| value.as_str())
        .expect("header should be present")
}

fn welcome_fixture() -> WelcomeFrame {
    serde_json::from_value(serde_json::json!({
        "session_id": "00000000000000000000000000",
        "available_channels": ["control", "sync.notifications"],
        "denied_channels": [
            {
                "name": "relay.ws",
                "reason": "entitlement_missing",
                "feature": "hc.remote"
            }
        ],
        "server_capabilities": {
            "tunnel_resume": false,
            "compression": [],
            "max_frame_bytes": 65536
        },
        "heartbeat_interval_s": 25
    }))
    .expect("welcome fixture should deserialize")
}
