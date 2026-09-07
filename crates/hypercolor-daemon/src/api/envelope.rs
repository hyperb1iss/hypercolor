//! The standard API success envelope.
//!
//! Every successful response flows through [`ApiResponse`] so the JSON
//! shape is consistent across endpoints. Request IDs use UUID v7 for
//! time-ordered traceability. Domain failures stay transport neutral;
//! the REST adapter beside this module owns their HTTP rendering.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

pub use hypercolor_types::api::envelope::ApiResponse;
use hypercolor_types::api::envelope::ResponseMeta;

// ── Success Envelope ─────────────────────────────────────────────────────

fn respond<T: Serialize>(status: StatusCode, data: T) -> Response {
    let body = ApiResponse {
        data,
        meta: response_meta(),
    };
    (status, axum::Json(body)).into_response()
}

/// Wrap data in a 200 OK envelope.
pub fn ok<T: Serialize>(data: T) -> Response {
    respond(StatusCode::OK, data)
}

/// Wrap data in a 201 Created envelope.
pub fn created<T: Serialize>(data: T) -> Response {
    respond(StatusCode::CREATED, data)
}

/// Wrap data in a 202 Accepted envelope.
pub fn accepted<T: Serialize>(data: T) -> Response {
    respond(StatusCode::ACCEPTED, data)
}

/// Fresh canonical metadata for every REST envelope.
#[must_use]
pub(super) fn response_meta() -> ResponseMeta {
    ResponseMeta {
        api_version: "1.0".to_owned(),
        request_id: format!("req_{}", uuid::Uuid::now_v7()),
        timestamp: iso8601_system_time(std::time::SystemTime::now()),
    }
}

fn iso8601_system_time(now: std::time::SystemTime) -> String {
    let duration = now
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();

    let total_secs = duration.as_secs();
    let millis = duration.subsec_millis();
    let (year, month, day, hour, minute, second) = epoch_to_utc(total_secs);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

#[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
fn epoch_to_utc(epoch_secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let secs_per_day: u64 = 86_400;
    let days = epoch_secs / secs_per_day;
    let day_secs = epoch_secs % secs_per_day;

    let hour = (day_secs / 3_600) as u32;
    let minute = ((day_secs % 3_600) / 60) as u32;
    let second = (day_secs % 60) as u32;

    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    (y as u32, m as u32, d as u32, hour, minute, second)
}
