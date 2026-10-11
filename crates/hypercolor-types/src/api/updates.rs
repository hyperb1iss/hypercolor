//! Optional OSS GitHub release notification contracts.
use serde::{Deserialize, Serialize};

/// A newer stable release and its upstream download information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ReleaseUpdate {
    pub version: String,
    pub name: String,
    pub notes: String,
    pub release_url: String,
    pub download_url: Option<String>,
}

/// Cached release discovery and notification preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct UpdateStatus {
    pub current_version: String,
    pub automatic_checks: bool,
    pub skipped_version: Option<String>,
    pub last_checked: Option<String>,
    pub release: Option<ReleaseUpdate>,
}

/// Partial preference update. Clearing and setting a skip are mutually exclusive.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct UpdatePreferencesPatch {
    pub automatic_checks: Option<bool>,
    pub skipped_version: Option<String>,
    #[serde(default)]
    pub clear_skipped_version: bool,
}
