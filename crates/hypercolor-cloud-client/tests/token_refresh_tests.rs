use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use hypercolor_cloud_client::{
    CloudClient, CloudClientConfig, CloudClientError, CloudSecretKey, RefreshTokenOwner,
    SecretStore, load_refresh_token, store_refresh_token,
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
