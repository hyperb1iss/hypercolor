//! Protected input and screen-capture REST contracts.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct ProtectedSourceGrantOwner(String);

impl ProtectedSourceGrantOwner {
    #[must_use]
    pub fn new(owner: impl Into<String>) -> Self {
        Self(owner.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CaptureAuthorizationResponse {
    pub authorized: bool,
    pub grant_owner: ProtectedSourceGrantOwner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CapturePickerResponse {
    pub picking: bool,
    pub grant_owner: ProtectedSourceGrantOwner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CaptureMonitor {
    pub index: usize,
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    pub value: String,
}
