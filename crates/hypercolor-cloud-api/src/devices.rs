use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRegistrationRequest {
    pub daemon_id: Uuid,
    pub install_name: String,
    pub os: String,
    pub arch: String,
    pub daemon_version: String,
    pub identity_pubkey: String,
    pub identity_proof: String,
    pub nonce: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRegistrationResponse {
    pub device: DeviceInstallation,
    pub registration_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInstallation {
    pub id: Uuid,
    pub user_id: Uuid,
    pub daemon_id: Uuid,
    pub install_name: String,
    pub os: String,
    pub arch: String,
    pub daemon_version: String,
    pub identity_pubkey: String,
    pub last_seen_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}
