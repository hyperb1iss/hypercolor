//! The standard API success envelope.
//!
//! Every successful response flows through [`ApiResponse`] so the JSON
//! shape is consistent across endpoints. Request IDs use UUID v7 for
//! time-ordered traceability. Errors are not built here: they are
//! [`DomainError`](crate::domain::DomainError) values that render
//! themselves, which is the daemon's only error rendering.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

// ── Meta ─────────────────────────────────────────────────────────────────

/// Response metadata included in every envelope.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, ToSchema)]
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

// ── Helpers ──────────────────────────────────────────────────────────────

/// Format the current wall-clock time as ISO 8601 UTC with millisecond precision.
fn iso8601_now() -> String {
    use std::time::SystemTime;

    iso8601_system_time(SystemTime::now())
}

pub(crate) fn iso8601_system_time(now: std::time::SystemTime) -> String {
    let duration = now
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
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
