//! Diagnostics API contracts — `/api/v1/diagnose`.

use serde::{Deserialize, Serialize};

/// Optional body for `POST /api/v1/diagnose`.
///
/// Omitting `checks` runs the full check set; `system` adds the host
/// environment section to the report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DiagnoseRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<bool>,
}
