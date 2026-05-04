use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use hypercolor_cloud_client::{
    CloudClientError, CloudSecretKey, DEFAULT_DEVICE_AUTHORIZATION_POLL_INTERVAL,
    DeviceAuthorizationSession, DeviceAuthorizationStatus, DeviceTokenPoll, RefreshTokenOwner,
    SLOW_DOWN_POLL_INTERVAL_STEP, SecretStore, api, load_refresh_token, persist_device_token,
};

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

fn device_code_response(interval: Option<u64>) -> api::DeviceCodeResponse {
    api::DeviceCodeResponse {
        device_code: "device-code".to_owned(),
        user_code: "HC-1234".to_owned(),
        verification_uri: "https://hypercolor.lighting/activate".to_owned(),
        verification_uri_complete: Some("https://hypercolor.lighting/activate?code=HC-1234".into()),
        expires_in: 900,
        interval,
    }
}

fn token_error(error: api::DeviceTokenErrorCode) -> api::DeviceTokenError {
    api::DeviceTokenError {
        error,
        error_description: None,
    }
}

#[test]
fn device_authorization_session_uses_default_poll_interval_and_expiry() {
    let session = DeviceAuthorizationSession::new(device_code_response(None));

    assert_eq!(
        session.poll_interval(),
        DEFAULT_DEVICE_AUTHORIZATION_POLL_INTERVAL
    );
    assert_eq!(session.device_code(), "device-code");
    assert_eq!(session.user_code(), "HC-1234");
    assert_eq!(
        session.verification_uri(),
        "https://hypercolor.lighting/activate"
    );
    assert_eq!(
        session.verification_uri_complete(),
        Some("https://hypercolor.lighting/activate?code=HC-1234")
    );
    assert!(!session.is_expired_at(session.started_at()));
    assert!(session.is_expired_at(session.expires_at()));
}

#[test]
fn device_authorization_session_tracks_slow_down_interval() {
    let mut session = DeviceAuthorizationSession::new(device_code_response(Some(2)));

    let pending = session.record_poll_result(DeviceTokenPoll::Waiting(token_error(
        api::DeviceTokenErrorCode::AuthorizationPending,
    )));

    assert!(matches!(pending, DeviceAuthorizationStatus::Pending { .. }));
    assert_eq!(pending.retry_after(), Some(Duration::from_secs(2)));
    assert!(!pending.is_terminal());

    let slowed = session.record_poll_result(DeviceTokenPoll::Waiting(token_error(
        api::DeviceTokenErrorCode::SlowDown,
    )));

    assert_eq!(
        slowed.retry_after(),
        Some(Duration::from_secs(2) + SLOW_DOWN_POLL_INTERVAL_STEP)
    );
    assert_eq!(session.poll_interval(), Duration::from_secs(7));

    let pending_after_slow_down = session.record_poll_result(DeviceTokenPoll::Waiting(
        token_error(api::DeviceTokenErrorCode::AuthorizationPending),
    ));

    assert_eq!(
        pending_after_slow_down.retry_after(),
        Some(Duration::from_secs(7))
    );
}

#[test]
fn device_authorization_session_maps_terminal_states() {
    let mut session = DeviceAuthorizationSession::new(device_code_response(Some(5)));
    let token = api::DeviceTokenResponse {
        access_token: "access".to_owned(),
        token_type: "Bearer".to_owned(),
        refresh_token: Some("refresh".to_owned()),
        expires_in: Some(900),
        scope: None,
    };

    let authorized = session.record_poll_result(DeviceTokenPoll::Authorized(token.clone()));
    let expired = session.record_poll_result(DeviceTokenPoll::Waiting(token_error(
        api::DeviceTokenErrorCode::ExpiredToken,
    )));
    let rejected = session.record_poll_result(DeviceTokenPoll::Waiting(token_error(
        api::DeviceTokenErrorCode::AccessDenied,
    )));

    assert_eq!(authorized, DeviceAuthorizationStatus::Authorized(token));
    assert!(matches!(expired, DeviceAuthorizationStatus::Expired(_)));
    assert!(matches!(rejected, DeviceAuthorizationStatus::Rejected(_)));
    assert!(authorized.is_terminal());
    assert!(expired.is_terminal());
    assert!(rejected.is_terminal());
}

#[test]
fn persist_device_token_stores_refresh_token_when_present() {
    let store = MemorySecretStore::default();
    let token = api::DeviceTokenResponse {
        access_token: "access".to_owned(),
        token_type: "Bearer".to_owned(),
        refresh_token: Some("refresh".to_owned()),
        expires_in: Some(900),
        scope: None,
    };

    assert!(
        persist_device_token(&store, RefreshTokenOwner::Daemon, &token)
            .expect("refresh token should persist")
    );
    assert_eq!(
        load_refresh_token(&store, RefreshTokenOwner::Daemon).expect("refresh token should load"),
        Some("refresh".to_owned())
    );
}

#[test]
fn persist_device_token_leaves_store_unchanged_without_refresh_token() {
    let store = MemorySecretStore::default();
    let token = api::DeviceTokenResponse {
        access_token: "access".to_owned(),
        token_type: "Bearer".to_owned(),
        refresh_token: None,
        expires_in: Some(900),
        scope: None,
    };

    assert!(
        !persist_device_token(&store, RefreshTokenOwner::Daemon, &token)
            .expect("missing refresh token should be handled")
    );
    assert_eq!(
        load_refresh_token(&store, RefreshTokenOwner::Daemon).expect("refresh token should load"),
        None
    );
}
