use chrono::{DateTime, Utc};
use hypercolor_cloud_api::{
    ChangesResponse, DEVICE_CODE_GRANT_TYPE, DeviceCodeRequest, DeviceTokenError,
    DeviceTokenErrorCode, DeviceTokenRequest, DeviceTokenResponse, EntitlementClaims,
    EntitlementTokenResponse, Etag, FeatureKey, REFRESH_TOKEN_GRANT_TYPE, RateLimits,
    RefreshTokenRequest, ReleaseChannel, SyncChange, SyncEntityKind, SyncOp,
};
use serde_json::json;
use uuid::Uuid;

fn fixed_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-05-15T17:00:00Z")
        .expect("fixture timestamp should parse")
        .with_timezone(&Utc)
}

#[test]
fn feature_keys_use_canonical_hc_prefix() {
    let value = serde_json::to_value(FeatureKey::Remote).expect("serialize feature key");
    assert_eq!(value, json!("hc.remote"));
    assert_eq!(FeatureKey::SignedBuilds.as_str(), "hc.signed_builds");
}

#[test]
fn entitlement_claims_keep_update_channel_scope() {
    let claims = EntitlementClaims {
        iss: "https://api.hypercolor.lighting".into(),
        sub: Uuid::nil().to_string(),
        aud: vec!["hypercolor-daemon".into(), "hypercolor-updater".into()],
        iat: 1_714_780_000,
        exp: 1_714_783_600,
        jti: "01JTEST".into(),
        kid: "ent-2026-01".into(),
        token_version: 1,
        device_install_id: Uuid::nil(),
        tier: "free".into(),
        features: vec![FeatureKey::CloudSync, FeatureKey::SignedBuilds],
        channels: vec![ReleaseChannel::Stable],
        rate_limits: RateLimits {
            remote_bandwidth_gb_month: 10,
            remote_concurrent_tunnels: 5,
            studio_sessions_month: 5,
            studio_max_session_seconds: 30,
            studio_max_session_tokens: 100_000,
            studio_default_model: "claude-haiku-4-5".into(),
        },
        update_until: 1_746_319_600,
    };

    assert!(claims.has_feature(FeatureKey::CloudSync));
    assert!(!claims.has_feature(FeatureKey::Remote));
    assert!(claims.allows_channel(ReleaseChannel::Stable));
    assert!(!claims.allows_channel(ReleaseChannel::Nightly));
    assert_eq!(ReleaseChannel::Stable.as_str(), "stable");
}

#[test]
fn entitlement_token_response_wraps_jwt_and_claims() {
    let claims = EntitlementClaims {
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
        features: vec![FeatureKey::CloudSync],
        channels: vec![ReleaseChannel::Stable],
        rate_limits: RateLimits {
            remote_bandwidth_gb_month: 10,
            remote_concurrent_tunnels: 5,
            studio_sessions_month: 5,
            studio_max_session_seconds: 30,
            studio_max_session_tokens: 100_000,
            studio_default_model: "claude-haiku-4-5".into(),
        },
        update_until: 1_746_319_600,
    };
    let token = EntitlementTokenResponse {
        jwt: "header.payload.signature".into(),
        claims,
    };
    let value = serde_json::to_value(&token).expect("serialize entitlement token");

    assert_eq!(value["jwt"], json!("header.payload.signature"));
    assert_eq!(value["claims"]["tier"], json!("free"));
    assert_eq!(value["claims"]["features"][0], json!("hc.cloud_sync"));
}

#[test]
fn sync_contract_serializes_snake_case_entities() {
    let payload = serde_json::to_value((SyncEntityKind::InstalledEffect, Etag(42), fixed_time()))
        .expect("serialize sync payload");

    assert_eq!(payload[0], json!("installed_effect"));
    assert_eq!(payload[1], json!(42));
}

#[test]
fn sync_changes_response_serializes_data_key() {
    let response = ChangesResponse {
        changes: vec![SyncChange {
            seq: 42,
            op: SyncOp::Delete,
            entity_kind: SyncEntityKind::Favorite,
            entity_id: "effect-xyz".into(),
            entity: None,
        }],
        next_seq: 42,
        has_more: false,
    };
    let payload = serde_json::to_value(response).expect("serialize changes response");

    assert!(payload.get("changes").is_none());
    assert_eq!(payload["data"][0]["entity_kind"], json!("favorite"));
    assert_eq!(payload["next_seq"], json!(42));
}

#[test]
fn device_code_request_omits_empty_scope() {
    let request = DeviceCodeRequest::new("hypercolor-daemon", "");
    let value = serde_json::to_value(request).expect("serialize device code request");

    assert_eq!(value["client_id"], "hypercolor-daemon");
    assert!(value.get("scope").is_none());
}

#[test]
fn device_token_contract_matches_rfc8628_grant() {
    let request = DeviceTokenRequest::new("device-code", "hypercolor-daemon");
    let value = serde_json::to_value(&request).expect("serialize device token request");

    assert_eq!(value["grant_type"], DEVICE_CODE_GRANT_TYPE);
    assert_eq!(value["device_code"], "device-code");
    assert_eq!(value["client_id"], "hypercolor-daemon");
    assert!(!format!("{request:?}").contains("device-code"));
}

#[test]
fn refresh_token_contract_matches_oauth_grant() {
    let request = RefreshTokenRequest::new("refresh-token", "hypercolor-daemon");
    let value = serde_json::to_value(&request).expect("serialize refresh token request");

    assert_eq!(value["grant_type"], REFRESH_TOKEN_GRANT_TYPE);
    assert_eq!(value["refresh_token"], "refresh-token");
    assert_eq!(value["client_id"], "hypercolor-daemon");
    assert!(!format!("{request:?}").contains("refresh-token"));
}

#[test]
fn token_response_debug_redacts_bearer_material() {
    let response = DeviceTokenResponse {
        access_token: "access-secret".to_owned(),
        token_type: "Bearer".to_owned(),
        refresh_token: Some("refresh-secret".to_owned()),
        expires_in: Some(900),
        scope: Some("openid profile email".to_owned()),
    };
    let debug = format!("{response:?}");

    assert!(!debug.contains("access-secret"));
    assert!(!debug.contains("refresh-secret"));
    assert!(debug.contains("Bearer"));
}

#[test]
fn device_token_errors_use_better_auth_codes() {
    let pending: DeviceTokenError = serde_json::from_str(r#"{"error":"authorization_pending"}"#)
        .expect("deserialize pending device token error");
    let slowdown: DeviceTokenError = serde_json::from_str(r#"{"error":"slow_down"}"#)
        .expect("deserialize slow_down device token error");
    let unknown: DeviceTokenError = serde_json::from_str(r#"{"error":"new_code"}"#)
        .expect("deserialize future device token error");

    assert_eq!(pending.error, DeviceTokenErrorCode::AuthorizationPending);
    assert!(pending.error.is_retryable());
    assert_eq!(slowdown.error, DeviceTokenErrorCode::SlowDown);
    assert_eq!(unknown.error, DeviceTokenErrorCode::Unknown);
}
