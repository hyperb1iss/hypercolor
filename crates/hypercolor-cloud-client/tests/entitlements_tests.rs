use std::time::Duration;

use hypercolor_cloud_client::{CloudClient, CloudClientConfig, api};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

#[tokio::test]
async fn fetch_entitlement_token_uses_bearer_auth() {
    let token = entitlement_token_fixture();
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

        assert!(request.starts_with("GET /v1/me/entitlements HTTP/1.1"));
        assert!(request.contains("authorization: Bearer access-token"));

        let body = serde_json::to_string(&token).expect("response should serialize");
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
    let client = CloudClient::new(CloudClientConfig::new(base_url).expect("base url should parse"));

    let response = client
        .fetch_entitlement_token("access-token")
        .await
        .expect("entitlement token should fetch");

    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("test server should finish")
        .expect("test server should not panic");
    assert_eq!(response.jwt, "header.payload.signature");
    assert!(response.claims.has_feature(api::FeatureKey::CloudSync));
}

fn entitlement_token_fixture() -> api::EntitlementTokenResponse {
    api::EntitlementTokenResponse {
        jwt: "header.payload.signature".into(),
        claims: api::EntitlementClaims {
            iss: "https://api.hypercolor.lighting".into(),
            sub: Uuid::nil().to_string(),
            aud: vec!["hypercolor-daemon".into()],
            iat: 1_714_780_000,
            exp: 1_714_783_600,
            jti: "01JTEST".into(),
            kid: "ent-2026-01".into(),
            token_version: 1,
            device_install_id: Uuid::nil(),
            tier: "free".into(),
            features: vec![api::FeatureKey::CloudSync],
            channels: vec![api::ReleaseChannel::Stable],
            rate_limits: api::RateLimits {
                remote_bandwidth_gb_month: 10,
                remote_concurrent_tunnels: 5,
                studio_sessions_month: 5,
                studio_max_session_seconds: 30,
                studio_max_session_tokens: 100_000,
                studio_default_model: "claude-haiku-4-5".into(),
            },
            update_until: 1_746_319_600,
        },
    }
}
