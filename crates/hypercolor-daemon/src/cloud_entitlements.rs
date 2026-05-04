use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hypercolor_cloud_client::api::{EntitlementClaims, EntitlementTokenResponse};
use hypercolor_core::config::ConfigManager;
use serde::{Deserialize, Serialize};

pub const ENTITLEMENT_CACHE_FILE: &str = "entitlement.json";

#[derive(Debug, thiserror::Error)]
pub enum EntitlementCacheError {
    #[error("failed entitlement cache I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("failed to process entitlement cache JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedCloudEntitlement {
    pub jwt: String,
    pub claims: EntitlementClaims,
    pub cached_at: String,
    pub expires_at: String,
}

impl CachedCloudEntitlement {
    #[must_use]
    pub fn from_response(response: &EntitlementTokenResponse, now: SystemTime) -> Self {
        Self {
            jwt: response.jwt.clone(),
            claims: response.claims.clone(),
            cached_at: iso8601_from_system_time(now),
            expires_at: iso8601_from_unix_seconds(response.claims.exp),
        }
    }

    #[must_use]
    pub fn is_stale_at_unix(&self, now_unix: i64) -> bool {
        self.claims.exp <= now_unix
    }
}

#[must_use]
pub fn entitlement_cache_path() -> PathBuf {
    ConfigManager::data_dir().join(ENTITLEMENT_CACHE_FILE)
}

pub async fn store_entitlement_response(
    path: impl AsRef<Path>,
    response: &EntitlementTokenResponse,
) -> Result<CachedCloudEntitlement, EntitlementCacheError> {
    let entitlement = CachedCloudEntitlement::from_response(response, SystemTime::now());
    let path = path.as_ref();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    let bytes = serde_json::to_vec_pretty(&entitlement)?;
    tokio::fs::write(path, bytes).await?;
    Ok(entitlement)
}

pub async fn load_cached_entitlement(
    path: impl AsRef<Path>,
) -> Result<Option<CachedCloudEntitlement>, EntitlementCacheError> {
    let path = path.as_ref();
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub async fn delete_cached_entitlement(
    path: impl AsRef<Path>,
) -> Result<bool, EntitlementCacheError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[must_use]
pub fn unix_now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

#[must_use]
pub fn iso8601_from_unix_seconds(seconds: i64) -> String {
    let seconds = u64::try_from(seconds).unwrap_or_default();
    iso8601_from_system_time(UNIX_EPOCH + Duration::from_secs(seconds))
}

fn iso8601_from_system_time(time: SystemTime) -> String {
    crate::api::envelope::iso8601_system_time(time)
}
