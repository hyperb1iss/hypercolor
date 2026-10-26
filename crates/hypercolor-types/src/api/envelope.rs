//! Canonical response envelope conventions (Spec 76 §4.3).
//!
//! Pure data with no transport dependencies. The daemon's REST adapter
//! wraps payloads in these; MCP and WS command results project the same
//! error body.

use serde::{Deserialize, Serialize};

/// Response header naming the daemon run that answered, on every HTTP
/// response the daemon serves: the same opaque value as
/// [`SceneDocument::run_epoch`](crate::api::scene::SceneDocument::run_epoch).
///
/// Scene revisions are comparable only within one run. A client that
/// sees this change re-reads before it sends another revision fence.
/// Spelled lowercase, the form HTTP/2 puts on the wire; HTTP/1.1 header
/// names compare case-insensitively.
pub const RUN_EPOCH_HEADER: &str = "hypercolor-run-epoch";

/// Request header naming one logical write, so a retry of a request whose
/// answer was lost is never applied twice (IETF httpapi idempotency-key
/// draft). A client-generated opaque string, typically a UUID, sent bare
/// or as a quoted structured-field string. Routes that honor it declare it
/// in the OpenAPI document; elsewhere it is ignored.
pub const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

/// Response header set to `true` on an answer replayed for a repeated
/// [`IDEMPOTENCY_KEY_HEADER`] instead of produced by applying the request.
pub const IDEMPOTENCY_REPLAYED_HEADER: &str = "idempotency-replayed";

/// Error code for a key reused on a request whose method, path,
/// `If-Match`, or body differs from the request it first named (`422`).
pub const IDEMPOTENCY_KEY_MISMATCH_CODE: &str = "idempotency_key_mismatch";

/// Response metadata included in every envelope.
///
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ResponseMeta {
    /// API version string.
    pub api_version: String,
    /// Per-request correlation ID, prefixed `req_`.
    pub request_id: String,
    /// ISO 8601 UTC timestamp of response generation.
    pub timestamp: String,
}

/// Standard success envelope: `{ data, meta }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ApiResponse<T> {
    /// The response payload.
    pub data: T,
    /// Response metadata.
    pub meta: ResponseMeta,
}

/// Standard error envelope: `{ error: { code, message, details }, meta }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ApiErrorBody {
    /// The error payload.
    pub error: ApiErrorDetail,
    /// Response metadata.
    pub meta: ResponseMeta,
}

/// The error payload inside [`ApiErrorBody`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ApiErrorDetail {
    /// Stable machine-readable error code (snake_case).
    pub code: String,
    /// Human-readable message.
    pub message: String,
    /// Optional structured detail (validation fields, current
    /// versions on precondition failures).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// Canonical list payload: honest pagination or none at all.
///
/// `page: None` means the response is complete — no fabricated
/// `limit`/`has_more` block pretending a paging contract that doesn't
/// exist. `page: Some` means the endpoint genuinely pages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ListResponse<T> {
    /// The items.
    pub items: Vec<T>,
    /// Total matching items (across all pages when paged).
    pub total: u64,
    /// Paging state; `None` = this response is complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<PageInfo>,
}

/// Paging state for endpoints that genuinely page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct PageInfo {
    /// Offset of the first item in this page.
    pub offset: u64,
    /// Requested page size.
    pub limit: u64,
    /// Whether more items exist past this page.
    pub has_more: bool,
}
