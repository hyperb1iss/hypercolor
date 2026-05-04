use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use hypercolor_cloud_client::{
    CloudClient, CloudClientConfig, CloudClientError, CloudSecretKey, RefreshTokenOwner,
    SecretStore, StoredDaemonConnect, StoredDaemonConnectInput, connect_authority,
    daemon_link::{
        HEADER_AUTHORIZATION, HEADER_DAEMON_ID, HEADER_DAEMON_NONCE, HEADER_DAEMON_SIG,
        HEADER_DAEMON_TS, HEADER_DAEMON_VERSION, HEADER_WEBSOCKET_PROTOCOL, UpgradeNonce,
        WEBSOCKET_PROTOCOL, verify_identity_signature,
    },
    load_identity, load_or_create_identity, load_refresh_token, store_refresh_token,
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

#[tokio::test]
async fn refresh_stored_device_token_rotates_refresh_token() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("test server should bind");
    let base_url = format!(
        "http://{}",
        listener
            .local_addr()
            .expect("test server address should resolve")
    );
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("request should connect");
        let mut buffer = vec![0_u8; 4096];
        let read = socket.read(&mut buffer).await.expect("request should read");
        let request = String::from_utf8_lossy(&buffer[..read]);

        assert!(request.starts_with("POST /api/auth/oauth2/token HTTP/1.1"));
        assert!(request.contains(r#""grant_type":"refresh_token""#));
        assert!(request.contains(r#""refresh_token":"refresh-old""#));
        assert!(request.contains(r#""client_id":"hypercolor-daemon""#));

        let body = serde_json::json!({
            "access_token": "access-new",
            "token_type": "Bearer",
            "refresh_token": "refresh-new",
            "expires_in": 900,
            "scope": "openid profile email"
        });
        let body = serde_json::to_string(&body).expect("response should serialize");
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("response should write");
    });
    let client = CloudClient::new(
        CloudClientConfig::with_auth_base_url(&base_url, &base_url)
            .expect("base urls should parse"),
    );
    let store = MemorySecretStore::default();
    store_refresh_token(&store, RefreshTokenOwner::Daemon, "refresh-old")
        .expect("refresh token should seed");

    let response = client
        .refresh_stored_device_token(&store, RefreshTokenOwner::Daemon)
        .await
        .expect("refresh token should exchange")
        .expect("stored refresh token should exist");

    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("test server should finish")
        .expect("test server should not panic");
    assert_eq!(response.access_token, "access-new");
    assert_eq!(
        load_refresh_token(&store, RefreshTokenOwner::Daemon).expect("refresh token should load"),
        Some("refresh-new".to_owned())
    );
}

#[tokio::test]
async fn refresh_stored_device_token_skips_missing_refresh_token() {
    let client = CloudClient::new(
        CloudClientConfig::with_auth_base_url(
            "https://api.hypercolor.lighting/",
            "https://hypercolor.lighting/",
        )
        .expect("base urls should parse"),
    );
    let store = MemorySecretStore::default();

    let response = client
        .refresh_stored_device_token(&store, RefreshTokenOwner::Daemon)
        .await
        .expect("missing refresh token should not fail");

    assert!(response.is_none());
}

#[tokio::test]
async fn prepare_stored_daemon_connect_refreshes_and_signs_upgrade() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("test server should bind");
    let base_url = format!(
        "http://{}",
        listener
            .local_addr()
            .expect("test server address should resolve")
    );
    let server = spawn_refresh_server(listener, "access-for-connect", "refresh-next");
    let client = CloudClient::new(
        CloudClientConfig::with_auth_base_url(&base_url, &base_url)
            .expect("base urls should parse"),
    );
    let store = MemorySecretStore::default();
    let identity = load_or_create_identity(&store).expect("identity should create");
    store_refresh_token(&store, RefreshTokenOwner::Daemon, "refresh-old")
        .expect("refresh token should seed");

    let connect = client
        .prepare_stored_daemon_connect(
            &store,
            StoredDaemonConnectInput {
                token_owner: RefreshTokenOwner::Daemon,
                daemon_version: "1.4.2",
                timestamp: "2026-05-15T17:00:00Z",
                nonce: UpgradeNonce::from_bytes([7_u8; 16]),
            },
        )
        .await
        .expect("stored connect request should prepare");

    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("test server should finish")
        .expect("test server should not panic");
    let StoredDaemonConnect::Prepared(request) = connect else {
        panic!("stored connect should be prepared");
    };
    let pairs = request.headers.pairs();
    assert_eq!(
        request.url.as_str(),
        format!("{base_url}/v1/daemon/connect").replace("http://", "ws://")
    );
    assert_eq!(
        header_value(&pairs, HEADER_AUTHORIZATION),
        "Bearer access-for-connect"
    );
    assert_eq!(
        header_value(&pairs, HEADER_WEBSOCKET_PROTOCOL),
        WEBSOCKET_PROTOCOL
    );
    assert_eq!(
        header_value(&pairs, HEADER_DAEMON_ID),
        identity.daemon_id().to_string()
    );
    assert_eq!(header_value(&pairs, HEADER_DAEMON_VERSION), "1.4.2");
    assert_eq!(
        header_value(&pairs, HEADER_DAEMON_TS),
        "2026-05-15T17:00:00Z"
    );
    assert!(!header_value(&pairs, HEADER_DAEMON_SIG).is_empty());
    assert_eq!(
        load_refresh_token(&store, RefreshTokenOwner::Daemon).expect("refresh token should load"),
        Some("refresh-next".to_owned())
    );
    assert!(!format!("{request:?}").contains("access-for-connect"));

    let reloaded_identity = load_identity(&store)
        .expect("identity should load")
        .expect("identity should exist");
    let host = connect_authority(&request.url).expect("connect authority should build");
    verify_identity_signature(
        &reloaded_identity.keypair().public_key(),
        hypercolor_cloud_client::daemon_link::UpgradeSignatureInput {
            method: "GET",
            host: &host,
            path: "/v1/daemon/connect",
            websocket_protocol: WEBSOCKET_PROTOCOL,
            daemon_id: reloaded_identity.daemon_id(),
            daemon_version: "1.4.2",
            timestamp: "2026-05-15T17:00:00Z",
            nonce: header_value(&pairs, HEADER_DAEMON_NONCE),
            authorization_jwt: "access-for-connect",
        }
        .canonicalize()
        .as_bytes(),
        &request.headers.signature,
    )
    .expect("stored connect signature should verify");
}

#[tokio::test]
async fn prepare_stored_daemon_connect_reports_missing_prerequisites() {
    let client = CloudClient::new(
        CloudClientConfig::with_auth_base_url(
            "https://api.hypercolor.lighting/",
            "https://hypercolor.lighting/",
        )
        .expect("base urls should parse"),
    );
    let store = MemorySecretStore::default();
    let missing_identity = client
        .prepare_stored_daemon_connect(
            &store,
            StoredDaemonConnectInput {
                token_owner: RefreshTokenOwner::Daemon,
                daemon_version: "1.4.2",
                timestamp: "2026-05-15T17:00:00Z",
                nonce: UpgradeNonce::from_bytes([7_u8; 16]),
            },
        )
        .await
        .expect("missing identity should not fail");
    assert_eq!(missing_identity, StoredDaemonConnect::MissingIdentity);

    load_or_create_identity(&store).expect("identity should create");
    let missing_refresh = client
        .prepare_stored_daemon_connect(
            &store,
            StoredDaemonConnectInput {
                token_owner: RefreshTokenOwner::Daemon,
                daemon_version: "1.4.2",
                timestamp: "2026-05-15T17:00:00Z",
                nonce: UpgradeNonce::from_bytes([8_u8; 16]),
            },
        )
        .await
        .expect("missing refresh token should not fail");
    assert_eq!(missing_refresh, StoredDaemonConnect::MissingRefreshToken);
}

fn spawn_refresh_server(
    listener: tokio::net::TcpListener,
    access_token: &'static str,
    refresh_token: &'static str,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("request should connect");
        let mut buffer = vec![0_u8; 4096];
        let read = socket.read(&mut buffer).await.expect("request should read");
        let request = String::from_utf8_lossy(&buffer[..read]);

        assert!(request.starts_with("POST /api/auth/oauth2/token HTTP/1.1"));
        assert!(request.contains(r#""grant_type":"refresh_token""#));
        assert!(request.contains(r#""refresh_token":"refresh-old""#));
        assert!(request.contains(r#""client_id":"hypercolor-daemon""#));

        let body = serde_json::json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "refresh_token": refresh_token,
            "expires_in": 900,
            "scope": "openid profile email"
        });
        let body = serde_json::to_string(&body).expect("response should serialize");
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("response should write");
    })
}

fn header_value<'a>(pairs: &'a [(&'static str, String)], name: &str) -> &'a str {
    pairs
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, value)| value.as_str())
        .expect("header should be present")
}
