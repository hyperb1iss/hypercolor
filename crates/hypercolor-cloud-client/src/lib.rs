#![forbid(unsafe_code)]

pub mod auth;
pub mod config;
pub mod connect;
pub mod devices;
pub mod entitlements;
pub mod error;
pub mod secrets;

pub use auth::{
    DEFAULT_DEVICE_AUTHORIZATION_POLL_INTERVAL, DeviceAuthorizationSession,
    DeviceAuthorizationStatus, DeviceTokenPoll, SLOW_DOWN_POLL_INTERVAL_STEP, persist_device_token,
};
pub use config::{CloudClient, CloudClientConfig, DAEMON_CONNECT_PATH};
pub use connect::{DaemonConnectInput, DaemonConnectRequest, connect_authority};
pub use devices::{DEVICE_REGISTRATION_PATH, DeviceRegistrationInput, signed_device_registration};
pub use entitlements::ENTITLEMENTS_PATH;
pub use error::CloudClientError;
pub use secrets::{
    CloudIdentity, CloudSecretKey, KEYRING_SERVICE, KeyringSecretStore, RefreshTokenOwner,
    SecretStore, delete_daemon_identity, delete_refresh_token, load_identity,
    load_or_create_identity, load_refresh_token, persist_identity, store_refresh_token,
};

pub use hypercolor_cloud_api as api;
pub use hypercolor_daemon_link as daemon_link;
