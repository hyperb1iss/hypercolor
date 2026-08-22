//! The domain service layer's error and adapter conventions
//! (Spec 76 §2.1, §2.4).
//!
//! Business transactions live in this module tree as transport-free
//! `async fn`s; the four transports are thin adapters over them:
//!
//! - **REST**: parse → service → envelope. The route handler converts
//!   wire input, calls one service function, and wraps the typed
//!   outcome. `DomainError` owns the shared error projection.
//! - **MCP**: schema validation, deterministic selector resolution, and
//!   one service call. The adapter projects `DomainError` into its tool
//!   error vocabulary.
//! - **WS commands**: call services directly; versions ride in-band.
//! - **CLI**: speaks the REST wire and deserializes
//!   `hypercolor_types::api::envelope::ApiResponse<Outcome>`.
//!
//! Domain signatures never mention Axum, `serde_json::Value`, or
//! `Response`. Mutations whose canonical events carry provenance accept
//! [`MutationContext`] beside the command, never inside it. Commands
//! that cannot publish the trigger carry no ceremonial context.
//!
//! MCP selector failures are an adapter concern. The adapter returns a
//! JSON-RPC invalid-params error with the normalized query, failure kind,
//! and deterministic candidates instead of asking the domain to resolve
//! transport-facing identity.
//!
//! [`DomainError`]'s `IntoResponse` is the ONLY error rendering in the
//! daemon. There is no second factory, no per-family projection, and no
//! route that hand-builds an error body: an error either is a
//! `DomainError` or it does not reach the wire.

pub mod commit;
pub mod context;
pub mod diagnostics;
pub mod display;
pub mod effect;
pub mod input_status;
pub mod layer;
pub mod layout;
#[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
mod macos_screen_parity;
pub mod output;
pub mod scene;
pub mod scene_tree;
pub mod spatial;
pub mod zone;

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use hypercolor_types::api::envelope::{ApiErrorBody, ApiErrorDetail, ResponseMeta};
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::ChangeTrigger;

/// What kind of resource a domain error is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Scene,
    Zone,
    Layer,
    Effect,
    Device,
    Display,
    DisplayFrame,
    SimulatedDisplay,
    Driver,
    AttachmentProfile,
    Layout,
    Preset,
    Playlist,
    Favorite,
    Asset,
    AttachmentTemplate,
    AttachmentSlot,
    Control,
    ControlSurface,
    Diagnostic,
    Config,
    ConfigKey,
    Session,
    /// An address on the API surface, for the unmatched-path fallback.
    Route,
}

impl std::fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Scene => "scene",
            Self::Zone => "zone",
            Self::Layer => "layer",
            Self::Effect => "effect",
            Self::Device => "device",
            Self::Display => "display",
            Self::DisplayFrame => "display frame",
            Self::SimulatedDisplay => "simulated display",
            Self::Driver => "driver",
            Self::AttachmentProfile => "attachment profile",
            Self::Layout => "layout",
            Self::Preset => "preset",
            Self::Playlist => "playlist",
            Self::Favorite => "favorite",
            Self::Asset => "asset",
            Self::AttachmentTemplate => "attachment template",
            Self::AttachmentSlot => "attachment slot",
            Self::Control => "control",
            Self::ControlSurface => "control surface",
            Self::Diagnostic => "diagnostic",
            Self::Config => "config",
            Self::ConfigKey => "config key",
            Self::Session => "session",
            Self::Route => "route",
        };
        f.write_str(name)
    }
}

/// The one domain error type (Spec 76 §2.1). Services return this;
/// every transport renders it through its own projection.
#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    /// The addressed resource does not exist.
    #[error("{kind} not found: {id}")]
    NotFound {
        /// What kind of resource.
        kind: ResourceKind,
        /// The identifier that missed.
        id: String,
    },
    /// The request is well-formed but semantically invalid.
    #[error("{message}")]
    Validation {
        /// Human-readable description.
        message: String,
        /// The offending field, when one names itself.
        field: Option<String>,
        /// Caller-actionable structured context (rejected control ids,
        /// cap violations, per-layer diagnostics). Rides the envelope's
        /// `error.details`; merges with `field` when both are present.
        details: Option<serde_json::Value>,
    },
    /// The request cannot be parsed at all — a header, path segment, or
    /// body fragment whose syntax is wrong, as distinct from a
    /// well-formed request the domain rejects.
    #[error("{message}")]
    Malformed {
        /// Human-readable description.
        message: String,
    },
    /// Current state rejects the mutation.
    #[error("{message}")]
    Conflict {
        /// Human-readable description.
        message: String,
        /// Caller-actionable structured context.
        details: Option<serde_json::Value>,
    },
    /// A control write names keys an input binding already drives
    /// (Spec 78 §1.6). Recoverable in the same shape: the caller either
    /// drops the key or clears its binding in the same request.
    #[error("controls are driven by an input binding: {}", keys.join(", "))]
    ControlBound {
        /// The bound keys the request tried to write, sorted.
        keys: Vec<String>,
    },
    /// Credentials are missing or unrecognized.
    #[error("{message}")]
    Unauthorized {
        /// Human-readable description.
        message: String,
    },
    /// Credentials are valid but the operation is not permitted, or the
    /// caller's origin is not allowed to reach this surface.
    #[error("{message}")]
    Forbidden {
        /// Human-readable description.
        message: String,
        /// Caller-actionable structured context (required tier,
        /// rejected client address, invalid allow-list rules).
        details: Option<serde_json::Value>,
    },
    /// The payload exceeds what the route accepts.
    #[error("payload exceeds the {limit_bytes} byte limit")]
    PayloadTooLarge {
        /// The accepting limit, in bytes.
        limit_bytes: u64,
    },
    /// The payload's media type is not one this route can decode.
    #[error("{message}")]
    UnsupportedMediaType {
        /// Human-readable description.
        message: String,
    },
    /// The caller exhausted its request budget for the current window.
    #[error("{message}")]
    RateLimited {
        /// Human-readable description.
        message: String,
        /// Requests permitted per window.
        limit: u32,
        /// Window length, in seconds.
        window_seconds: u64,
        /// Seconds until the budget refills.
        retry_after_secs: u64,
    },
    /// An If-Match / version precondition failed.
    #[error("version mismatch: expected {expected}, current {current}")]
    PreconditionFailed {
        /// What kind of resource carries the version.
        resource: ResourceKind,
        /// The version the caller expected.
        expected: u64,
        /// The version the resource holds.
        current: u64,
    },
    /// The device exists but cannot serve the request.
    #[error("device {device_id} unavailable: {reason}")]
    DeviceUnavailable {
        /// Which device.
        device_id: DeviceId,
        /// Why it cannot serve.
        reason: String,
    },
    /// A daemon capability is not available in the current runtime.
    #[error("{message}")]
    ServiceUnavailable {
        /// Human-readable description.
        message: String,
        /// Caller-actionable structured context.
        details: Option<serde_json::Value>,
    },
    /// Unexpected internal failure. Renders generically on the wire;
    /// the full chain goes to tracing.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl DomainError {
    /// A missing resource.
    #[must_use]
    pub fn not_found(kind: ResourceKind, id: impl std::fmt::Display) -> Self {
        Self::NotFound {
            kind,
            id: id.to_string(),
        }
    }

    /// A semantic validation failure with no single offending field.
    #[must_use]
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
            field: None,
            details: None,
        }
    }

    /// A semantic validation failure naming its field.
    #[must_use]
    pub fn validation_field(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
            field: Some(field.into()),
            details: None,
        }
    }

    /// A semantic validation failure carrying structured context the
    /// caller needs in order to correct the request.
    #[must_use]
    pub fn validation_details(message: impl Into<String>, details: serde_json::Value) -> Self {
        Self::Validation {
            message: message.into(),
            field: None,
            details: Some(details),
        }
    }

    /// Input whose syntax the parser cannot read.
    #[must_use]
    pub fn malformed(message: impl Into<String>) -> Self {
        Self::Malformed {
            message: message.into(),
        }
    }

    /// A state conflict.
    #[must_use]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict {
            message: message.into(),
            details: None,
        }
    }

    /// A state conflict carrying structured context.
    #[must_use]
    pub fn conflict_details(message: impl Into<String>, details: serde_json::Value) -> Self {
        Self::Conflict {
            message: message.into(),
            details: Some(details),
        }
    }

    /// Missing or unrecognized credentials.
    #[must_use]
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::Unauthorized {
            message: message.into(),
        }
    }

    /// A permitted caller reaching an operation it may not perform.
    #[must_use]
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden {
            message: message.into(),
            details: None,
        }
    }

    /// A refusal carrying structured context.
    #[must_use]
    pub fn forbidden_details(message: impl Into<String>, details: serde_json::Value) -> Self {
        Self::Forbidden {
            message: message.into(),
            details: Some(details),
        }
    }

    /// A media type this route cannot decode.
    #[must_use]
    pub fn unsupported_media_type(message: impl Into<String>) -> Self {
        Self::UnsupportedMediaType {
            message: message.into(),
        }
    }

    /// A daemon capability that is absent in the current runtime.
    #[must_use]
    pub fn service_unavailable_details(
        message: impl Into<String>,
        details: serde_json::Value,
    ) -> Self {
        Self::ServiceUnavailable {
            message: message.into(),
            details: Some(details),
        }
    }

    /// Stable machine-readable code (snake_case) for the canonical
    /// error envelope.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "not_found",
            Self::Validation { .. } => "validation_error",
            Self::Malformed { .. } => "malformed_request",
            Self::Conflict { .. } => "conflict",
            Self::ControlBound { .. } => "control_bound",
            Self::Unauthorized { .. } => "unauthorized",
            Self::Forbidden { .. } => "forbidden",
            Self::PayloadTooLarge { .. } => "payload_too_large",
            Self::UnsupportedMediaType { .. } => "unsupported_media_type",
            Self::RateLimited { .. } => "rate_limited",
            Self::PreconditionFailed { .. } => "precondition_failed",
            Self::DeviceUnavailable { .. } => "device_unavailable",
            Self::ServiceUnavailable { .. } => "service_unavailable",
            Self::Internal(_) => "internal_error",
        }
    }

    /// HTTP status for the canonical rendering.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::NotFound { .. } => StatusCode::NOT_FOUND,
            Self::Validation { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Malformed { .. } => StatusCode::BAD_REQUEST,
            Self::Conflict { .. } | Self::ControlBound { .. } => StatusCode::CONFLICT,
            Self::Unauthorized { .. } => StatusCode::UNAUTHORIZED,
            Self::Forbidden { .. } => StatusCode::FORBIDDEN,
            Self::PayloadTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnsupportedMediaType { .. } => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::PreconditionFailed { .. } => StatusCode::PRECONDITION_FAILED,
            Self::DeviceUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::ServiceUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub(crate) fn detail(&self) -> ApiErrorDetail {
        let details = match self {
            Self::Validation { field, details, .. } => merge_field(details.clone(), field.as_ref()),
            Self::Conflict { details, .. }
            | Self::Forbidden { details, .. }
            | Self::ServiceUnavailable { details, .. } => details.clone(),
            Self::ControlBound { keys } => Some(json!({ "bound": keys })),
            Self::PreconditionFailed {
                expected, current, ..
            } => Some(json!({ "expected": expected, "current": current })),
            Self::PayloadTooLarge { limit_bytes } => Some(json!({ "limit_bytes": limit_bytes })),
            Self::RateLimited {
                limit,
                window_seconds,
                retry_after_secs,
                ..
            } => Some(json!({
                "limit": limit,
                "window_seconds": window_seconds,
                "retry_after": retry_after_secs,
            })),
            _ => None,
        };
        let message = match self {
            // Internal chains carry paths, addresses, and other
            // internals that don't belong on the wire.
            Self::Internal(_) => "internal error".to_owned(),
            other => other.to_string(),
        };
        ApiErrorDetail {
            code: self.code().to_owned(),
            message,
            details,
        }
    }
}

/// Fold a validation error's offending field into its structured
/// details, so a caller reads one `details` object rather than two
/// competing context channels.
fn merge_field(
    details: Option<serde_json::Value>,
    field: Option<&String>,
) -> Option<serde_json::Value> {
    let Some(field) = field else {
        return details;
    };
    match details {
        Some(serde_json::Value::Object(mut map)) => {
            map.insert("field".to_owned(), json!(field));
            Some(serde_json::Value::Object(map))
        }
        Some(other) => Some(json!({ "field": field, "context": other })),
        None => Some(json!({ "field": field })),
    }
}

impl IntoResponse for DomainError {
    /// Canonical envelope rendering: `{ error: { code, message,
    /// details }, meta }` with the variant's status code. A 412 also
    /// carries the current version as its `ETag` so clients can
    /// re-sync without a second read.
    fn into_response(self) -> Response {
        if let Self::Internal(error) = &self {
            tracing::error!(chain = format!("{error:#}"), "domain internal error");
        }
        let status = self.status();
        let body = ApiErrorBody {
            error: self.detail(),
            meta: response_meta(),
        };
        let mut response = (status, Json(body)).into_response();
        if let Self::PreconditionFailed { current, .. } = &self
            && let Ok(etag) = HeaderValue::from_str(&format!("\"{current}\""))
        {
            response.headers_mut().insert(header::ETAG, etag);
        }
        response
    }
}

/// Transport provenance for a trigger-bearing mutation.
///
/// Rides beside the command rather than inside it so command payloads
/// stay transport-free. Mutations without a canonical trigger-bearing
/// event do not accept this context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationContext {
    /// Which surface initiated the mutation.
    pub trigger: ChangeTrigger,
}

impl MutationContext {
    /// Provenance for a REST-initiated mutation.
    #[must_use]
    pub const fn api() -> Self {
        Self {
            trigger: ChangeTrigger::Api,
        }
    }

    /// Provenance for an MCP-initiated mutation.
    #[must_use]
    pub const fn mcp() -> Self {
        Self {
            trigger: ChangeTrigger::Mcp,
        }
    }
}

/// Fresh canonical metadata under the same emission policy as the v1
/// envelope (version string, `req_` UUIDv7 correlation id, ISO 8601
/// timestamp).
#[must_use]
pub fn response_meta() -> ResponseMeta {
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        serde_json::from_slice(&bytes).expect("body is JSON")
    }

    #[tokio::test]
    async fn canonical_error_envelope_carries_code_message_meta() {
        let error = DomainError::NotFound {
            kind: ResourceKind::Scene,
            id: "sc_123".to_owned(),
        };
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "not_found");
        assert_eq!(json["error"]["message"], "scene not found: sc_123");
        for key in ["api_version", "request_id", "timestamp"] {
            assert!(json["meta"][key].is_string(), "meta.{key} must be present");
        }
    }

    #[tokio::test]
    async fn precondition_failure_renders_412_with_etag_and_details() {
        let error = DomainError::PreconditionFailed {
            resource: ResourceKind::Zone,
            expected: 4,
            current: 7,
        };
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
        assert_eq!(
            response
                .headers()
                .get(header::ETAG)
                .and_then(|value| value.to_str().ok()),
            Some("\"7\"")
        );
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "precondition_failed");
        assert_eq!(json["error"]["details"]["expected"], 4);
        assert_eq!(json["error"]["details"]["current"], 7);
    }

    #[tokio::test]
    async fn internal_errors_render_generically() {
        let error = DomainError::Internal(anyhow::anyhow!("secret path /home/user leaked"));
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let json = body_json(response).await;
        assert_eq!(json["error"]["message"], "internal error");
        assert_eq!(json["error"]["code"], "internal_error");
    }

    #[tokio::test]
    async fn absent_details_are_omitted_rather_than_serialized_as_null() {
        let response = DomainError::validation("nope").into_response();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let text = std::str::from_utf8(&bytes).expect("utf8");
        assert!(
            !text.contains("\"details\""),
            "the canonical envelope skips the key entirely when there is no context: {text}"
        );
    }

    #[tokio::test]
    async fn a_validation_field_rides_the_details_object() {
        let response = DomainError::validation_field("name", "must not be blank").into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "validation_error");
        assert_eq!(json["error"]["details"]["field"], "name");
    }

    #[tokio::test]
    async fn structured_validation_details_survive_beside_the_field() {
        let error = DomainError::Validation {
            message: "no valid controls to apply".to_owned(),
            field: Some("controls".to_owned()),
            details: Some(json!({ "rejected": ["speed"] })),
        };
        let json = body_json(error.into_response()).await;
        assert_eq!(json["error"]["details"]["field"], "controls");
        assert_eq!(json["error"]["details"]["rejected"][0], "speed");
    }

    #[tokio::test]
    async fn malformed_input_is_a_400_distinct_from_validation() {
        let response =
            DomainError::malformed("If-Match must be a non-negative integer").into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "malformed_request");
        assert_eq!(
            json["error"]["message"],
            "If-Match must be a non-negative integer"
        );
    }

    #[tokio::test]
    async fn unauthorized_and_forbidden_keep_their_own_statuses() {
        let response = DomainError::unauthorized("Missing API key").into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(response).await["error"]["code"], "unauthorized");

        let response = DomainError::forbidden_details(
            "Read-only API key cannot perform write operations",
            json!({ "required_tier": "control" }),
        )
        .into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "forbidden");
        assert_eq!(json["error"]["details"]["required_tier"], "control");
    }

    #[tokio::test]
    async fn payload_too_large_names_the_limit_it_enforced() {
        let response = DomainError::PayloadTooLarge {
            limit_bytes: 64 * 1024 * 1024,
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "payload_too_large");
        assert_eq!(json["error"]["details"]["limit_bytes"], 64 * 1024 * 1024);
    }

    #[tokio::test]
    async fn unsupported_media_type_and_rate_limits_render_canonically() {
        let response =
            DomainError::unsupported_media_type("audio/flac is not decodable").into_response();
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(
            body_json(response).await["error"]["code"],
            "unsupported_media_type"
        );

        let response = DomainError::RateLimited {
            message: "Too many mutations".to_owned(),
            limit: 60,
            window_seconds: 60,
            retry_after_secs: 12,
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "rate_limited");
        assert_eq!(json["error"]["details"]["limit"], 60);
        assert_eq!(json["error"]["details"]["retry_after"], 12);
    }

    #[tokio::test]
    async fn conflict_details_reach_the_envelope() {
        let response = DomainError::conflict_details(
            "Control surface revision conflict",
            json!({ "kind": "control_surface_revision_conflict", "current_revision": 4 }),
        )
        .into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "conflict");
        assert_eq!(json["error"]["details"]["current_revision"], 4);
    }

    #[test]
    fn every_resource_kind_renders_a_distinct_label() {
        // Not-found prose is derived from the kind, so a kind without a
        // label, or two kinds sharing one, is a wire defect. The match
        // below is exhaustive: a new variant stops compiling here until
        // it also joins `ALL`.
        const fn is_listed(kind: ResourceKind) -> bool {
            match kind {
                ResourceKind::Scene
                | ResourceKind::Zone
                | ResourceKind::Layer
                | ResourceKind::Effect
                | ResourceKind::Device
                | ResourceKind::Display
                | ResourceKind::DisplayFrame
                | ResourceKind::SimulatedDisplay
                | ResourceKind::Driver
                | ResourceKind::AttachmentProfile
                | ResourceKind::Layout
                | ResourceKind::Preset
                | ResourceKind::Playlist
                | ResourceKind::Favorite
                | ResourceKind::Asset
                | ResourceKind::AttachmentTemplate
                | ResourceKind::AttachmentSlot
                | ResourceKind::Control
                | ResourceKind::ControlSurface
                | ResourceKind::Diagnostic
                | ResourceKind::Config
                | ResourceKind::ConfigKey
                | ResourceKind::Session
                | ResourceKind::Route => true,
            }
        }

        const ALL: [ResourceKind; 24] = [
            ResourceKind::Scene,
            ResourceKind::Zone,
            ResourceKind::Layer,
            ResourceKind::Effect,
            ResourceKind::Device,
            ResourceKind::Display,
            ResourceKind::DisplayFrame,
            ResourceKind::SimulatedDisplay,
            ResourceKind::Driver,
            ResourceKind::AttachmentProfile,
            ResourceKind::Layout,
            ResourceKind::Preset,
            ResourceKind::Playlist,
            ResourceKind::Favorite,
            ResourceKind::Asset,
            ResourceKind::AttachmentTemplate,
            ResourceKind::AttachmentSlot,
            ResourceKind::Control,
            ResourceKind::ControlSurface,
            ResourceKind::Diagnostic,
            ResourceKind::Config,
            ResourceKind::ConfigKey,
            ResourceKind::Session,
            ResourceKind::Route,
        ];

        let mut seen: Vec<String> = Vec::new();
        for kind in ALL {
            assert!(is_listed(kind));
            let label = kind.to_string();
            assert!(!label.is_empty(), "{kind:?} renders an empty label");
            assert_eq!(
                label,
                label.to_lowercase(),
                "{kind:?} must render lowercase so derived prose reads as a sentence"
            );
            assert!(
                !seen.contains(&label),
                "two resource kinds share the label {label:?}"
            );
            seen.push(label);
        }
        assert_eq!(
            DomainError::not_found(ResourceKind::Scene, "sc_1").to_string(),
            "scene not found: sc_1",
            "the not-found message is derived from the kind, never hand-written"
        );
    }
}
