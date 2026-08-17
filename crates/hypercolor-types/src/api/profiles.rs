//! Profile API contracts — `/api/v1/profiles/*`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Request body for `POST /api/v1/profiles`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateProfileRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<u8>,
    /// Overwrite an existing profile with the same name.
    #[serde(default)]
    pub force: bool,
}

/// Request body for `PUT /api/v1/profiles/{id}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateProfileRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<u8>,
}

/// Optional body for `POST /api/v1/profiles/{id}/apply`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ApplyProfileRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_ms: Option<u32>,
}
