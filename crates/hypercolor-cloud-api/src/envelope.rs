use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiEnvelope<T> {
    pub data: T,
    pub meta: ApiMeta,
}

impl<T> ApiEnvelope<T> {
    #[must_use]
    pub const fn new(data: T, meta: ApiMeta) -> Self {
        Self { data, meta }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiMeta {
    pub api_version: String,
    pub request_id: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProblemDetails {
    #[serde(rename = "type")]
    pub type_url: String,
    pub title: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}
