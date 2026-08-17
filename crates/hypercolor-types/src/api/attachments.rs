//! Component-template catalog API contracts — `/api/v1/attachments/*`.

use serde::{Deserialize, Serialize};

/// Query parameters for `GET /api/v1/attachments/templates`.
///
/// Every field narrows the catalog; an empty query lists everything the
/// registry knows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListTemplatesQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Free-text filter over template name and description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// Restrict to templates a given controller can host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_min: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_max: Option<u32>,
}
