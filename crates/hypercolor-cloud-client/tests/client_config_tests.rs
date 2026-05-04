use hypercolor_cloud_client::config::{DAEMON_CONNECT_PATH, DEVICE_CODE_PATH, DEVICE_TOKEN_PATH};
use hypercolor_cloud_client::{
    CloudClientConfig, DeviceRegistrationInput, ENTITLEMENTS_PATH, api, signed_device_registration,
};
use hypercolor_daemon_link::{
    IdentityKeypair, IdentityNonce, registration_proof_message, verify_identity_signature,
};
use hypercolor_types::config::CloudConfig;
use uuid::Uuid;

#[test]
fn cloud_client_config_builds_api_urls() {
    let config =
        CloudClientConfig::new("https://api.hypercolor.lighting/").expect("base url should parse");

    let devices = config
        .api_url("/v1/me/devices")
        .expect("api url should build");

    assert_eq!(
        devices.as_str(),
        "https://api.hypercolor.lighting/v1/me/devices"
    );
    let entitlements = config
        .api_url(ENTITLEMENTS_PATH)
        .expect("entitlements url should build");
    assert_eq!(
        entitlements.as_str(),
        "https://api.hypercolor.lighting/v1/me/entitlements"
    );
    assert_eq!(DAEMON_CONNECT_PATH, "/v1/daemon/connect");
}

#[test]
fn cloud_client_config_builds_auth_urls() {
    let config = CloudClientConfig::with_auth_base_url(
        "https://api.hypercolor.lighting/",
        "https://hypercolor.lighting/",
    )
    .expect("base urls should parse")
    .with_device_client("hypercolor-daemon-dev", "openid profile email cloud");

    let code_url = config
        .auth_url(DEVICE_CODE_PATH)
        .expect("device code url should build");
    let token_url = config
        .auth_url(DEVICE_TOKEN_PATH)
        .expect("device token url should build");

    assert_eq!(
        code_url.as_str(),
        "https://hypercolor.lighting/api/auth/device/code"
    );
    assert_eq!(
        token_url.as_str(),
        "https://hypercolor.lighting/api/auth/device/token"
    );
    assert_eq!(config.device_client_id(), "hypercolor-daemon-dev");
    assert_eq!(config.device_scope(), "openid profile email cloud");
}

#[test]
fn cloud_client_config_builds_daemon_connect_websocket_url() {
    let config =
        CloudClientConfig::new("https://api.hypercolor.lighting/").expect("base url should parse");
    let local =
        CloudClientConfig::new("http://127.0.0.1:9421/").expect("local base url should parse");

    assert_eq!(
        config
            .daemon_connect_url()
            .expect("daemon connect url should build")
            .as_str(),
        "wss://api.hypercolor.lighting/v1/daemon/connect"
    );
    assert_eq!(
        local
            .daemon_connect_url()
            .expect("local daemon connect url should build")
            .as_str(),
        "ws://127.0.0.1:9421/v1/daemon/connect"
    );
}

#[test]
fn cloud_client_config_maps_from_shared_cloud_config() {
    let config = CloudConfig {
        enabled: true,
        base_url: "https://api.staging.hypercolor.lighting".into(),
        auth_base_url: "https://staging.hypercolor.lighting".into(),
        app_base_url: "https://app.staging.hypercolor.lighting".into(),
        device_client_id: "hypercolor-daemon-dev".into(),
        device_scope: "openid profile email cloud".into(),
        connect_on_start: false,
    };

    let client_config =
        CloudClientConfig::try_from(&config).expect("cloud config should map to client config");

    assert_eq!(
        client_config.base_url().as_str(),
        "https://api.staging.hypercolor.lighting/"
    );
    assert_eq!(
        client_config.auth_base_url().as_str(),
        "https://staging.hypercolor.lighting/"
    );
    assert_eq!(client_config.device_client_id(), "hypercolor-daemon-dev");
    assert_eq!(client_config.device_scope(), "openid profile email cloud");
}

#[test]
fn cloud_client_config_rejects_invalid_urls() {
    assert!(CloudClientConfig::new("not a url").is_err());
}

#[test]
fn device_token_poll_reports_retryable_waiting_states() {
    let pending = hypercolor_cloud_client::DeviceTokenPoll::Waiting(api::DeviceTokenError {
        error: api::DeviceTokenErrorCode::AuthorizationPending,
        error_description: None,
    });

    assert!(pending.is_retryable());
}

#[test]
fn signed_device_registration_builds_verifiable_identity_proof() {
    let daemon_id =
        Uuid::parse_str("018f4c36-4a44-7cc9-9f57-0d2e9224d2f1").expect("fixture uuid should parse");
    let keypair = IdentityKeypair::generate();
    let nonce = IdentityNonce::from_bytes([9_u8; 32]);
    let request = signed_device_registration(
        DeviceRegistrationInput {
            daemon_id,
            install_name: "desk-mac".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            daemon_version: "1.4.2".into(),
            nonce,
        },
        &keypair,
    );
    let public_key =
        hypercolor_daemon_link::IdentityPublicKey::new(request.identity_pubkey.clone())
            .expect("public key should validate");
    let signature = hypercolor_daemon_link::IdentitySignature::new(request.identity_proof.clone())
        .expect("identity proof should validate");

    assert_eq!(request.daemon_id, daemon_id);
    assert_eq!(request.install_name, "desk-mac");
    verify_identity_signature(
        &public_key,
        &registration_proof_message(daemon_id, &public_key, &request.nonce),
        &signature,
    )
    .expect("registration proof should verify");
}
