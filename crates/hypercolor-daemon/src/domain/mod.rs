//! The domain service layer's error and adapter conventions
//! (Spec 76 §2.1, §2.4).
//!
//! Business transactions live in this module tree as transport-free
//! `async fn`s; the four transports are thin adapters over them:
//!
//! - **REST**: parse → service → envelope. The route handler converts
//!   wire input, calls one service function, and wraps the typed
//!   outcome via [`respond`]. `DomainError` renders itself — the first
//!   `IntoResponse` error type in the codebase. ETags attach in one
//!   layer keyed on [`Versioned`].
//! - **MCP**: argument parse + one service call; fuzzy name matching
//!   stays an MCP-adapter concern. `DomainError` converts to
//!   [`ToolError`](crate::mcp::tools::ToolError) via `From`.
//! - **WS commands**: call services directly; versions ride in-band
//!   via [`Versioned`].
//! - **CLI**: speaks the REST wire and deserializes
//!   `hypercolor_types::api::envelope::ApiResponse<Outcome>`.
//!
//! Domain signatures never mention Axum, `serde_json::Value`, or
//! `Response`. Transport provenance rides in [`MutationContext`],
//! never inside command payloads. WS/session/startup provenance
//! variants arrive with the WS-command wave — `ChangeTrigger` rides
//! serialized events, so new variants ship under the §0 dual-accept
//! process, not as a side effect here.
//!
//! MCP fuzzy-lookup misses are an ADAPTER concern: today they return
//! structured success payloads ("did you mean …"), and migrating
//! workers must keep that behavior rather than projecting them
//! through [`DomainError::NotFound`] (which would surface as a
//! JSON-RPC error the client never saw before).
//!
//! [`DomainError`]'s `IntoResponse` is the ONLY error rendering in the
//! daemon. There is no second factory, no per-family projection, and no
//! route that hand-builds an error body: an error either is a
//! `DomainError` or it does not reach the wire.

pub mod commit;
pub mod display;
pub mod effect;
pub mod layer;
pub mod output;
pub mod scene;
pub mod zone;

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use hypercolor_types::api::envelope::{ApiErrorBody, ApiErrorDetail, ApiResponse, ResponseMeta};
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::ChangeTrigger;

use crate::mcp::tools::ToolError;

/// What kind of resource a domain error is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Scene,
    Zone,
    Layer,
    Effect,
    Device,
    LogicalDevice,
    Display,
    DisplayPreview,
    SimulatedDisplay,
    Driver,
    Profile,
    Layout,
    Preset,
    Playlist,
    Favorite,
    Asset,
    AttachmentTemplate,
    AttachmentSlot,
    Control,
    ControlSurface,
    Sensor,
    Diagnostic,
    Config,
    ConfigKey,
    Session,
}

impl std::fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Scene => "scene",
            Self::Zone => "zone",
            Self::Layer => "layer",
            Self::Effect => "effect",
            Self::Device => "device",
            Self::LogicalDevice => "logical device",
            Self::Display => "display",
            Self::DisplayPreview => "display preview",
            Self::SimulatedDisplay => "simulated display",
            Self::Driver => "driver",
            Self::Profile => "profile",
            Self::Layout => "layout",
            Self::Preset => "preset",
            Self::Playlist => "playlist",
            Self::Favorite => "favorite",
            Self::Asset => "asset",
            Self::AttachmentTemplate => "attachment template",
            Self::AttachmentSlot => "attachment slot",
            Self::Control => "control",
            Self::ControlSurface => "control surface",
            Self::Sensor => "sensor",
            Self::Diagnostic => "diagnostic",
            Self::Config => "config",
            Self::ConfigKey => "config key",
            Self::Session => "session",
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

    /// Stable machine-readable code (snake_case) for the canonical
    /// error envelope.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "not_found",
            Self::Validation { .. } => "validation_error",
            Self::Malformed { .. } => "malformed_request",
            Self::Conflict { .. } => "conflict",
            Self::Unauthorized { .. } => "unauthorized",
            Self::Forbidden { .. } => "forbidden",
            Self::PayloadTooLarge { .. } => "payload_too_large",
            Self::UnsupportedMediaType { .. } => "unsupported_media_type",
            Self::RateLimited { .. } => "rate_limited",
            Self::PreconditionFailed { .. } => "precondition_failed",
            Self::DeviceUnavailable { .. } => "device_unavailable",
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
            Self::Conflict { .. } => StatusCode::CONFLICT,
            Self::Unauthorized { .. } => StatusCode::UNAUTHORIZED,
            Self::Forbidden { .. } => StatusCode::FORBIDDEN,
            Self::PayloadTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnsupportedMediaType { .. } => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::PreconditionFailed { .. } => StatusCode::PRECONDITION_FAILED,
            Self::DeviceUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn detail(&self) -> ApiErrorDetail {
        let details = match self {
            Self::Validation { field, details, .. } => merge_field(details.clone(), field.as_ref()),
            Self::Conflict { details, .. } | Self::Forbidden { details, .. } => details.clone(),
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

impl From<DomainError> for ToolError {
    fn from(error: DomainError) -> Self {
        match error {
            DomainError::NotFound { kind, id } => ToolError::InvalidParam {
                param: kind.to_string(),
                reason: format!("not found: {id}"),
            },
            DomainError::Validation { message, field, .. } => ToolError::InvalidParam {
                param: field.unwrap_or_else(|| "request".to_owned()),
                reason: message,
            },
            DomainError::Malformed { message } => ToolError::InvalidParam {
                param: "request".to_owned(),
                reason: message,
            },
            DomainError::Conflict { message, .. }
            | DomainError::Unauthorized { message }
            | DomainError::Forbidden { message, .. }
            | DomainError::UnsupportedMediaType { message }
            | DomainError::RateLimited { message, .. } => ToolError::Conflict(message),
            DomainError::PayloadTooLarge { limit_bytes } => ToolError::InvalidParam {
                param: "payload".to_owned(),
                reason: format!("exceeds the {limit_bytes} byte limit"),
            },
            DomainError::PreconditionFailed {
                resource,
                expected,
                current,
            } => ToolError::Conflict(format!(
                "{resource} version mismatch: expected {expected}, current {current}"
            )),
            DomainError::DeviceUnavailable { device_id, reason } => {
                ToolError::Conflict(format!("device {device_id} unavailable: {reason}"))
            }
            DomainError::Internal(error) => {
                tracing::error!(chain = format!("{error:#}"), "domain internal error (mcp)");
                ToolError::Internal("internal error".to_owned())
            }
        }
    }
}

/// Versioned resources expose one `u64` the ETag layer keys on and WS
/// results carry in-band.
pub trait Versioned {
    /// The optimistic-concurrency version of this resource.
    fn version(&self) -> u64;
}

/// Transport provenance for a mutation. Rides beside the command,
/// never inside it — command payloads stay transport-free.
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

    /// Provenance for a CLI-initiated mutation.
    #[must_use]
    pub const fn cli() -> Self {
        Self {
            trigger: ChangeTrigger::Cli,
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
        timestamp: crate::api::envelope::iso8601_system_time(std::time::SystemTime::now()),
    }
}

/// Wrap a typed service outcome in the canonical success envelope.
pub fn respond<T: serde::Serialize>(status: StatusCode, data: T) -> Response {
    let body = ApiResponse {
        data,
        meta: response_meta(),
    };
    (status, Json(body)).into_response()
}

/// Wrap a versioned outcome and attach its ETag in one step, so call
/// sites cannot forget the header or fight ownership.
pub fn respond_versioned<T: serde::Serialize + Versioned>(status: StatusCode, data: T) -> Response {
    let version = data.version();
    let response = respond(status, data);
    attach_version_etag(response, version)
}

fn attach_version_etag(mut response: Response, version: u64) -> Response {
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{version}\"")) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}

/// Attach a [`Versioned`] resource's ETag to a response — the one
/// ETag layer, replacing the three hand-rolled implementations as
/// waves 2.2/2.3 migrate call sites.
#[must_use]
pub fn with_etag<R: Versioned>(response: Response, resource: &R) -> Response {
    attach_version_etag(response, resource.version())
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
                | ResourceKind::LogicalDevice
                | ResourceKind::Display
                | ResourceKind::DisplayPreview
                | ResourceKind::SimulatedDisplay
                | ResourceKind::Driver
                | ResourceKind::Profile
                | ResourceKind::Layout
                | ResourceKind::Preset
                | ResourceKind::Playlist
                | ResourceKind::Favorite
                | ResourceKind::Asset
                | ResourceKind::AttachmentTemplate
                | ResourceKind::AttachmentSlot
                | ResourceKind::Control
                | ResourceKind::ControlSurface
                | ResourceKind::Sensor
                | ResourceKind::Diagnostic
                | ResourceKind::Config
                | ResourceKind::ConfigKey
                | ResourceKind::Session => true,
            }
        }

        const ALL: [ResourceKind; 25] = [
            ResourceKind::Scene,
            ResourceKind::Zone,
            ResourceKind::Layer,
            ResourceKind::Effect,
            ResourceKind::Device,
            ResourceKind::LogicalDevice,
            ResourceKind::Display,
            ResourceKind::DisplayPreview,
            ResourceKind::SimulatedDisplay,
            ResourceKind::Driver,
            ResourceKind::Profile,
            ResourceKind::Layout,
            ResourceKind::Preset,
            ResourceKind::Playlist,
            ResourceKind::Favorite,
            ResourceKind::Asset,
            ResourceKind::AttachmentTemplate,
            ResourceKind::AttachmentSlot,
            ResourceKind::Control,
            ResourceKind::ControlSurface,
            ResourceKind::Sensor,
            ResourceKind::Diagnostic,
            ResourceKind::Config,
            ResourceKind::ConfigKey,
            ResourceKind::Session,
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

    #[tokio::test]
    async fn respond_versioned_wraps_and_tags_in_one_step() {
        #[derive(serde::Serialize)]
        struct Doc {
            version: u64,
        }
        impl Versioned for Doc {
            fn version(&self) -> u64 {
                self.version
            }
        }
        let response = respond_versioned(StatusCode::OK, Doc { version: 5 });
        assert_eq!(
            response
                .headers()
                .get(header::ETAG)
                .and_then(|value| value.to_str().ok()),
            Some("\"5\"")
        );
        let json = body_json(response).await;
        assert_eq!(json["data"]["version"], 5);
    }

    #[test]
    fn tool_error_projection_keeps_codes_sane() {
        let not_found: ToolError = DomainError::NotFound {
            kind: ResourceKind::Effect,
            id: "fx".to_owned(),
        }
        .into();
        assert_eq!(not_found.error_code(), -32602);

        let conflict: ToolError = DomainError::conflict("busy").into();
        assert_eq!(conflict.error_code(), -32000);

        let precondition: ToolError = DomainError::PreconditionFailed {
            resource: ResourceKind::Scene,
            expected: 1,
            current: 2,
        }
        .into();
        assert_eq!(precondition.error_code(), -32000);
    }

    #[test]
    fn versioned_etag_attaches_quoted() {
        struct Doc(u64);
        impl Versioned for Doc {
            fn version(&self) -> u64 {
                self.0
            }
        }
        let response = with_etag(respond(StatusCode::OK, serde_json::json!({})), &Doc(12));
        assert_eq!(
            response
                .headers()
                .get(header::ETAG)
                .and_then(|value| value.to_str().ok()),
            Some("\"12\"")
        );
    }
}
