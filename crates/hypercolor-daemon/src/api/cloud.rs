//! Hypercolor Cloud endpoints.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;
use hypercolor_cloud_client::{KeyringSecretStore, load_or_create_identity};
use hypercolor_types::config::CloudConfig;
use serde::Serialize;
use utoipa::ToSchema;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CloudStatus {
    pub compiled: bool,
    pub enabled: bool,
    pub connect_on_start: bool,
    pub base_url: String,
    pub auth_base_url: String,
    pub app_base_url: String,
    pub device_client_id: String,
    pub device_scope: String,
    pub identity_storage: String,
}

impl CloudStatus {
    #[must_use]
    pub fn from_config(config: &CloudConfig) -> Self {
        Self {
            compiled: true,
            enabled: config.enabled,
            connect_on_start: config.connect_on_start,
            base_url: config.base_url.clone(),
            auth_base_url: config.auth_base_url.clone(),
            app_base_url: config.app_base_url.clone(),
            device_client_id: config.device_client_id.clone(),
            device_scope: config.device_scope.clone(),
            identity_storage: "os_keyring".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CloudIdentityStatus {
    pub daemon_id: String,
    pub identity_pubkey: String,
}

pub async fn get_status(State(state): State<Arc<AppState>>) -> Response {
    let cloud_config = state
        .config_manager
        .as_ref()
        .map(|manager| manager.get().cloud.clone())
        .unwrap_or_default();

    ApiResponse::ok(CloudStatus::from_config(&cloud_config))
}

pub async fn ensure_identity() -> Response {
    match KeyringSecretStore::new_native().and_then(|store| load_or_create_identity(&store)) {
        Ok(identity) => ApiResponse::ok(CloudIdentityStatus {
            daemon_id: identity.daemon_id().hyphenated().to_string(),
            identity_pubkey: identity.keypair().public_key().as_str().to_owned(),
        }),
        Err(error) => ApiError::internal(format!("failed to initialize cloud identity: {error}")),
    }
}
