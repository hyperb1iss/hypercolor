use hypercolor_cloud_client::daemon_link::{
    HEADER_AUTHORIZATION, HEADER_DAEMON_ID, HEADER_DAEMON_NONCE, HEADER_DAEMON_SIG,
    HEADER_DAEMON_TS, HEADER_DAEMON_VERSION, HEADER_WEBSOCKET_PROTOCOL, IdentityKeypair,
    UpgradeNonce, WEBSOCKET_PROTOCOL, verify_identity_signature,
};
use hypercolor_cloud_client::{
    CloudClient, CloudClientConfig, DaemonConnectInput, connect_authority,
};
use reqwest::Url;
use uuid::Uuid;

#[test]
fn prepare_daemon_connect_builds_signed_request_without_leaking_debug_tokens() {
    let client = CloudClient::new(
        CloudClientConfig::new("https://api.hypercolor.lighting/").expect("base url should parse"),
    );
    let daemon_id =
        Uuid::parse_str("018f4c36-4a44-7cc9-9f57-0d2e9224d2f1").expect("fixture uuid should parse");
    let keypair = IdentityKeypair::generate();
    let nonce = UpgradeNonce::from_bytes([4_u8; 16]);

    let input = DaemonConnectInput {
        daemon_id,
        keypair: &keypair,
        bearer_token: "secret.jwt.value",
        daemon_version: "1.4.2",
        timestamp: "2026-05-15T17:00:00Z",
        nonce,
    };
    let input_debug = format!("{input:?}");
    assert!(!input_debug.contains("secret.jwt.value"));
    assert!(!input_debug.contains(keypair.private_key().as_str()));

    let request = client
        .prepare_daemon_connect(input)
        .expect("daemon connect request should prepare");
    let pairs = request.headers.pairs();

    assert_eq!(
        request.url.as_str(),
        "wss://api.hypercolor.lighting/v1/daemon/connect"
    );
    assert_eq!(
        header_value(&pairs, HEADER_AUTHORIZATION),
        "Bearer secret.jwt.value"
    );
    assert_eq!(
        header_value(&pairs, HEADER_WEBSOCKET_PROTOCOL),
        WEBSOCKET_PROTOCOL
    );
    assert_eq!(
        header_value(&pairs, HEADER_DAEMON_ID),
        daemon_id.to_string()
    );
    assert_eq!(header_value(&pairs, HEADER_DAEMON_VERSION), "1.4.2");
    assert_eq!(
        header_value(&pairs, HEADER_DAEMON_TS),
        "2026-05-15T17:00:00Z"
    );
    assert!(!header_value(&pairs, HEADER_DAEMON_NONCE).is_empty());
    assert!(!header_value(&pairs, HEADER_DAEMON_SIG).is_empty());
    assert!(!format!("{request:?}").contains("secret.jwt.value"));

    verify_identity_signature(
        &keypair.public_key(),
        hypercolor_cloud_client::daemon_link::UpgradeSignatureInput {
            method: "GET",
            host: "api.hypercolor.lighting",
            path: "/v1/daemon/connect",
            websocket_protocol: WEBSOCKET_PROTOCOL,
            daemon_id,
            daemon_version: "1.4.2",
            timestamp: "2026-05-15T17:00:00Z",
            nonce: header_value(&pairs, HEADER_DAEMON_NONCE),
            authorization_jwt: "secret.jwt.value",
        }
        .canonicalize()
        .as_bytes(),
        &request.headers.signature,
    )
    .expect("connect signature should verify");
}

#[test]
fn connect_authority_preserves_explicit_ports() {
    let local =
        Url::parse("ws://127.0.0.1:9421/v1/daemon/connect").expect("local url should parse");
    let ipv6 = Url::parse("ws://[::1]:9421/v1/daemon/connect").expect("ipv6 url should parse");
    let production = Url::parse("wss://api.hypercolor.lighting/v1/daemon/connect")
        .expect("production url should parse");

    assert_eq!(
        connect_authority(&local).expect("local authority should build"),
        "127.0.0.1:9421"
    );
    assert_eq!(
        connect_authority(&ipv6).expect("ipv6 authority should build"),
        "[::1]:9421"
    );
    assert_eq!(
        connect_authority(&production).expect("production authority should build"),
        "api.hypercolor.lighting"
    );
}

fn header_value<'a>(pairs: &'a [(&'static str, String)], name: &str) -> &'a str {
    pairs
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, value)| value.as_str())
        .expect("header should be present")
}
