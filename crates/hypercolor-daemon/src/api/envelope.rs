//! Standard API response envelope and error types.
//!
//! Every response flows through [`ApiResponse`] or [`ApiError`] to guarantee
//! a consistent JSON shape across all endpoints. Request IDs use UUID v7
//! for time-ordered traceability.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Meta ─────────────────────────────────────────────────────────────────

/// Response metadata included in every envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    /// API version string.
    pub api_version: String,
    /// Per-request correlation ID, prefixed `req_`.
    pub request_id: String,
    /// ISO 8601 UTC timestamp of response generation.
    pub timestamp: String,
}

impl Meta {
    /// Generate a fresh metadata block with a new request ID.
    fn now() -> Self {
        Self {
            api_version: "1.0".to_owned(),
            request_id: format!("req_{}", Uuid::now_v7()),
            timestamp: iso8601_now(),
        }
    }
}

// ── Success Envelope ─────────────────────────────────────────────────────

/// Standard success response wrapper.
#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    /// The response payload.
    pub data: T,
    /// Response metadata.
    pub meta: Meta,
}

impl<T: Serialize> ApiResponse<T> {
    /// Wrap data in a 200 OK envelope.
    pub fn ok(data: T) -> Response {
        let body = Self {
            data,
            meta: Meta::now(),
        };
        (StatusCode::OK, axum::Json(body)).into_response()
    }

    /// Wrap data in a 201 Created envelope.
    pub fn created(data: T) -> Response {
        let body = Self {
            data,
            meta: Meta::now(),
        };
        (StatusCode::CREATED, axum::Json(body)).into_response()
    }

    /// Wrap data in a 202 Accepted envelope.
    pub fn accepted(data: T) -> Response {
        let body = Self {
            data,
            meta: Meta::now(),
        };
        (StatusCode::ACCEPTED, axum::Json(body)).into_response()
    }
}

// ── Error Envelope ───────────────────────────────────────────────────────

/// Machine-readable error codes matching the spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// 400 — Malformed request.
    BadRequest,
    /// 401 — Missing or invalid credentials.
    Unauthorized,
    /// 403 — Credentials valid but insufficient permissions.
    Forbidden,
    /// 404 — Resource does not exist.
    NotFound,
    /// 409 — State conflict.
    Conflict,
    /// 422 — Validation failure.
    ValidationError,
    /// 429 — Request throttled by rate limiter.
    RateLimited,
    /// 500 — Internal daemon error.
    InternalError,
}

impl ErrorCode {
    /// Map error code to HTTP status.
    const fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict => StatusCode::CONFLICT,
            Self::ValidationError => StatusCode::UNPROCESSABLE_ENTITY,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// Error detail payload.
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    /// Machine-readable error code.
    pub code: ErrorCode,
    /// Human-readable description.
    pub message: String,
    /// Additional context (validation errors, conflicting IDs, etc.).
    pub details: Option<serde_json::Value>,
}

/// Standard error response wrapper.
#[derive(Debug, Serialize)]
pub struct ApiErrorResponse {
    /// The error detail.
    pub error: ErrorBody,
    /// Response metadata.
    pub meta: Meta,
}

/// Convenience builder for API errors.
pub struct ApiError;

impl ApiError {
    /// 404 Not Found.
    pub fn not_found(message: impl Into<String>) -> Response {
        Self::build(ErrorCode::NotFound, message.into(), None)
    }

    /// 400 Bad Request.
    pub fn bad_request(message: impl Into<String>) -> Response {
        Self::build(ErrorCode::BadRequest, message.into(), None)
    }

    /// 401 Unauthorized.
    pub fn unauthorized(message: impl Into<String>) -> Response {
        Self::build(ErrorCode::Unauthorized, message.into(), None)
    }

    /// 403 Forbidden.
    pub fn forbidden(message: impl Into<String>) -> Response {
        Self::build(ErrorCode::Forbidden, message.into(), None)
    }

    /// 403 Forbidden with details.
    pub fn forbidden_with_details(
        message: impl Into<String>,
        details: serde_json::Value,
    ) -> Response {
        Self::build(ErrorCode::Forbidden, message.into(), Some(details))
    }

    /// 409 Conflict.
    pub fn conflict(message: impl Into<String>) -> Response {
        Self::build(ErrorCode::Conflict, message.into(), None)
    }

    /// 422 Validation Error.
    pub fn validation(message: impl Into<String>) -> Response {
        Self::build(ErrorCode::ValidationError, message.into(), None)
    }

    /// 500 Internal Error.
    pub fn internal(message: impl Into<String>) -> Response {
        Self::build(ErrorCode::InternalError, message.into(), None)
    }

    /// 429 Rate Limited.
    pub fn rate_limited(message: impl Into<String>) -> Response {
        Self::build(ErrorCode::RateLimited, message.into(), None)
    }

    /// 429 Rate Limited with details.
    pub fn rate_limited_with_details(
        message: impl Into<String>,
        details: serde_json::Value,
    ) -> Response {
        Self::build(ErrorCode::RateLimited, message.into(), Some(details))
    }

    fn build(code: ErrorCode, message: String, details: Option<serde_json::Value>) -> Response {
        let status = code.status();
        let body = ApiErrorResponse {
            error: ErrorBody {
                code,
                message,
                details,
            },
            meta: Meta::now(),
        };
        (status, axum::Json(body)).into_response()
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Format the current wall-clock time as ISO 8601 UTC with millisecond precision.
fn iso8601_now() -> String {
    use std::time::SystemTime;

    let now = SystemTime::now();
    let duration = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();

    let total_secs = duration.as_secs();
    let millis = duration.subsec_millis();
    let (year, month, day, hour, minute, second) = epoch_to_utc(total_secs);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Convert Unix epoch seconds to (year, month, day, hour, minute, second) in UTC.
#[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
fn epoch_to_utc(epoch_secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let secs_per_day: u64 = 86400;
    let days = epoch_secs / secs_per_day;
    let day_secs = epoch_secs % secs_per_day;

    let hour = (day_secs / 3600) as u32;
    let minute = ((day_secs % 3600) / 60) as u32;
    let second = (day_secs % 60) as u32;

    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    (y as u32, m as u32, d as u32, hour, minute, second)
}
