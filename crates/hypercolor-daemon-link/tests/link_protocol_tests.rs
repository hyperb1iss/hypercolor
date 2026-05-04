use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hypercolor_daemon_link::frame::DenialReason;
use hypercolor_daemon_link::{
    AdmissionError, AdmissionSet, ChannelName, DeniedChannel, Frame, FrameKind, IdentityKeypair,
    IdentityNonce, IdentityPrivateKey, IdentityPublicKey, ServerCapabilities, UpgradeHeaderInput,
    UpgradeNonce, UpgradeSignatureInput, WEBSOCKET_PROTOCOL, WelcomeFrame,
    registration_proof_message, verify_identity_signature,
};
use serde_json::json;
use ulid::Ulid;
use uuid::Uuid;

#[test]
fn channel_names_use_wire_strings() {
    let value =
        serde_json::to_value(ChannelName::SyncNotifications).expect("serialize channel name");
    assert_eq!(value, json!("sync.notifications"));
    assert_eq!(ChannelName::RelayHttp.required_feature(), Some("hc.remote"));
}

#[test]
fn admission_reports_denied_feature() {
    let welcome = WelcomeFrame {
        session_id: Ulid::nil(),
        available_channels: vec![ChannelName::Control, ChannelName::SyncNotifications],
        denied_channels: vec![DeniedChannel {
            name: ChannelName::RelayHttp,
            reason: DenialReason::EntitlementMissing,
            feature: Some("hc.remote".into()),
        }],
        server_capabilities: ServerCapabilities {
            tunnel_resume: true,
            compression: vec!["zstd".into()],
            max_frame_bytes: Some(1_048_576),
        },
        heartbeat_interval_s: 25,
    };
    let admission = AdmissionSet::from_welcome(&welcome);

    assert!(admission.check(ChannelName::Control).is_ok());
    assert_eq!(
        admission.check(ChannelName::RelayHttp),
        Err(AdmissionError::ChannelDenied {
            channel: ChannelName::RelayHttp,
            feature: Some("hc.remote".into())
        })
    );
    assert!(matches!(
        admission.check_name("not.real"),
        Err(AdmissionError::UnknownChannel { .. })
    ));
}

#[test]
fn frame_round_trips_with_generic_payload() {
    let frame = Frame::new(
        ChannelName::Control,
        FrameKind::Msg,
        Ulid::nil(),
        json!({ "kind": "version.report" }),
    );

    let raw = serde_json::to_string(&frame).expect("serialize frame");
    let decoded: Frame = serde_json::from_str(&raw).expect("deserialize frame");

    assert_eq!(decoded.channel, ChannelName::Control);
    assert_eq!(decoded.kind, FrameKind::Msg);
    assert_eq!(decoded.payload["kind"], "version.report");
}

#[test]
fn upgrade_canonicalization_binds_jwt_without_exposing_it() {
    let daemon_id =
        Uuid::parse_str("018f4c36-4a44-7cc9-9f57-0d2e9224d2f1").expect("fixture uuid should parse");
    let input = UpgradeSignatureInput {
        method: "GET",
        host: "api.hypercolor.lighting",
        path: "/v1/daemon/connect",
        websocket_protocol: WEBSOCKET_PROTOCOL,
        daemon_id,
        daemon_version: "1.4.2",
        timestamp: "2026-05-15T17:00:00Z",
        nonce: "nonce-1",
        authorization_jwt: "secret.jwt.value",
    };
    let canonical = input.canonicalize();
    let changed = UpgradeSignatureInput {
        authorization_jwt: "different.jwt.value",
        ..input
    }
    .canonicalize();

    assert_ne!(canonical.sha256(), changed.sha256());
    let canonical_text = std::str::from_utf8(canonical.as_bytes()).expect("utf8 canonical bytes");
    assert!(!canonical_text.contains("secret.jwt.value"));
}

#[test]
fn signed_upgrade_headers_match_rfc51_handshake() {
    let daemon_id =
        Uuid::parse_str("018f4c36-4a44-7cc9-9f57-0d2e9224d2f1").expect("fixture uuid should parse");
    let keypair = IdentityKeypair::generate();
    let nonce = UpgradeNonce::from_bytes([3_u8; 16]);
    let input = UpgradeHeaderInput {
        host: "api.hypercolor.lighting",
        daemon_id,
        daemon_version: "1.4.2",
        timestamp: "2026-05-15T17:00:00Z",
        nonce: &nonce,
        authorization_jwt: "secret.jwt.value",
    };

    let headers = input.signed_headers(&keypair);
    let canonical = UpgradeSignatureInput {
        method: "GET",
        host: "api.hypercolor.lighting",
        path: "/v1/daemon/connect",
        websocket_protocol: WEBSOCKET_PROTOCOL,
        daemon_id,
        daemon_version: "1.4.2",
        timestamp: "2026-05-15T17:00:00Z",
        nonce: nonce.as_str(),
        authorization_jwt: "secret.jwt.value",
    }
    .canonicalize();
    let pairs = headers.pairs();

    assert_eq!(headers.authorization, "Bearer secret.jwt.value");
    assert_eq!(headers.websocket_protocol, WEBSOCKET_PROTOCOL);
    assert!(
        pairs
            .iter()
            .any(|(name, _)| *name == "X-Hypercolor-Daemon-Sig")
    );
    verify_identity_signature(
        &keypair.public_key(),
        canonical.as_bytes(),
        &headers.signature,
    )
    .expect("upgrade signature should verify");
}

#[test]
fn upgrade_debug_output_redacts_bearer_material() {
    let daemon_id =
        Uuid::parse_str("018f4c36-4a44-7cc9-9f57-0d2e9224d2f1").expect("fixture uuid should parse");
    let keypair = IdentityKeypair::generate();
    let nonce = UpgradeNonce::from_bytes([3_u8; 16]);
    let signature_input = UpgradeSignatureInput {
        method: "GET",
        host: "api.hypercolor.lighting",
        path: "/v1/daemon/connect",
        websocket_protocol: WEBSOCKET_PROTOCOL,
        daemon_id,
        daemon_version: "1.4.2",
        timestamp: "2026-05-15T17:00:00Z",
        nonce: nonce.as_str(),
        authorization_jwt: "secret.jwt.value",
    };
    let header_input = UpgradeHeaderInput {
        host: "api.hypercolor.lighting",
        daemon_id,
        daemon_version: "1.4.2",
        timestamp: "2026-05-15T17:00:00Z",
        nonce: &nonce,
        authorization_jwt: "secret.jwt.value",
    };
    let headers = header_input.signed_headers(&keypair);

    for debug_output in [
        format!("{signature_input:?}"),
        format!("{header_input:?}"),
        format!("{headers:?}"),
    ] {
        assert!(!debug_output.contains("secret.jwt.value"));
        assert!(!debug_output.contains("Bearer"));
        assert!(debug_output.contains("<redacted>"));
    }
}

#[test]
fn upgrade_nonce_validates_wire_length() {
    let nonce = UpgradeNonce::generate();

    assert!(UpgradeNonce::new(nonce.as_str()).is_ok());
    assert!(UpgradeNonce::new(STANDARD.encode([1_u8; 15])).is_err());
}

#[test]
fn identity_public_key_validates_base64_length() {
    let encoded = STANDARD.encode([7_u8; 32]);
    let public_key = IdentityPublicKey::new(encoded).expect("public key should validate");

    assert_eq!(public_key.decode().expect("decode key"), [7_u8; 32]);
    assert!(IdentityPublicKey::new(STANDARD.encode([1_u8; 31])).is_err());
}

#[test]
fn identity_keypair_signs_and_verifies_messages() {
    let keypair = IdentityKeypair::generate();
    let message = b"hypercolor identity test";
    let signature = keypair.sign(message);

    verify_identity_signature(&keypair.public_key(), message, &signature)
        .expect("generated signature should verify");
    assert!(verify_identity_signature(&keypair.public_key(), b"changed", &signature).is_err());
}

#[test]
fn identity_keypair_round_trips_private_key_material() {
    let keypair = IdentityKeypair::generate();
    let private_key = IdentityPrivateKey::new(keypair.private_key().as_str())
        .expect("private key should validate");
    let restored =
        IdentityKeypair::from_private_key(&private_key).expect("private key should restore");

    assert_eq!(restored.public_key(), keypair.public_key());
}

#[test]
fn identity_nonce_validates_registration_nonce_length() {
    let nonce = IdentityNonce::generate();

    assert_eq!(nonce.decode().expect("decode nonce").len(), 32);
    assert!(IdentityNonce::new(STANDARD.encode([1_u8; 31])).is_err());
}

#[test]
fn registration_proof_message_binds_daemon_key_and_nonce() {
    let daemon_id =
        Uuid::parse_str("018f4c36-4a44-7cc9-9f57-0d2e9224d2f1").expect("fixture uuid should parse");
    let keypair = IdentityKeypair::generate();
    let public_key = keypair.public_key();
    let proof = registration_proof_message(daemon_id, &public_key, "nonce-a");
    let changed = registration_proof_message(daemon_id, &public_key, "nonce-b");
    let signature = keypair.sign(&proof);

    assert_ne!(proof, changed);
    verify_identity_signature(&public_key, &proof, &signature)
        .expect("registration proof should verify");
    assert!(verify_identity_signature(&public_key, &changed, &signature).is_err());
}
