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
//! JSON-RPC error the client never saw before). Canonical routes render the
//! canonical error envelope; legacy v1 paths keep their frozen error
//! projections via the [`legacy`] shims.

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
    Display,
    Profile,
    Layout,
    Preset,
    Playlist,
    Asset,
    Config,
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
            Self::Display => "display",
            Self::Profile => "profile",
            Self::Layout => "layout",
            Self::Preset => "preset",
            Self::Playlist => "playlist",
            Self::Asset => "asset",
            Self::Config => "config",
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
    },
    /// Current state rejects the mutation.
    #[error("{message}")]
    Conflict {
        /// Human-readable description.
        message: String,
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
        }
    }

    /// A semantic validation failure naming its field.
    #[must_use]
    pub fn validation_field(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
            field: Some(field.into()),
        }
    }

    /// A state conflict.
    #[must_use]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict {
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
            Self::Conflict { .. } => "conflict",
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
            Self::Conflict { .. } => StatusCode::CONFLICT,
            Self::PreconditionFailed { .. } => StatusCode::PRECONDITION_FAILED,
            Self::DeviceUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn detail(&self) -> ApiErrorDetail {
        let details = match self {
            Self::Validation {
                field: Some(field), ..
            } => Some(json!({ "field": field })),
            Self::PreconditionFailed {
                expected, current, ..
            } => Some(json!({ "expected": expected, "current": current })),
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
            DomainError::Validation { message, field } => ToolError::InvalidParam {
                param: field.unwrap_or_else(|| "request".to_owned()),
                reason: message,
            },
            DomainError::Conflict { message } => ToolError::Conflict(message),
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

/// Frozen legacy error projections for v1 paths (Spec 76 §0).
///
/// The v1 compat matrix pins these shapes; canonical routes never use
/// them. The three byte-identical hand copies in the scene/zone
/// handlers collapse onto this module as waves 2.2/2.3 migrate.
pub mod legacy {
    use super::{HeaderValue, IntoResponse, Json, Response, StatusCode, header, json};

    /// The frozen v1 412 body: `{ "error": "<label> mismatch",
    /// "current": N }` with the current version as `ETag` — top-level
    /// `current`, no envelope, exactly as the compat matrix pins it.
    #[must_use]
    pub fn revision_mismatch_response(label: &str, current: u64) -> Response {
        let body = json!({
            "error": format!("{label} mismatch"),
            "current": current,
        });
        let mut response = (StatusCode::PRECONDITION_FAILED, Json(body)).into_response();
        if let Ok(etag) = HeaderValue::from_str(&format!("\"{current}\"")) {
            response.headers_mut().insert(header::ETAG, etag);
        }
        response
    }
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
    async fn legacy_shim_matches_the_frozen_v1_shape_byte_for_byte() {
        let response = legacy::revision_mismatch_response("groups_revision", 9);
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
        assert_eq!(
            response
                .headers()
                .get(header::ETAG)
                .and_then(|value| value.to_str().ok()),
            Some("\"9\"")
        );
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        // Expected bytes come from the same json! construction the
        // in-tree builders use, so this pins the shim to them under
        // whatever serde_json map-order policy the build graph
        // resolves — a hard-coded string would encode the local
        // feature graph instead. The real wire bytes are frozen by
        // the v1 compat matrix.
        let expected = serde_json::to_string(&json!({
            "error": "groups_revision mismatch",
            "current": 9,
        }))
        .expect("expected body serializes");
        assert_eq!(
            std::str::from_utf8(&bytes).expect("utf8"),
            expected,
            "the frozen v1 412 body, byte for byte against the builder construction"
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

        let conflict: ToolError = DomainError::Conflict {
            message: "busy".to_owned(),
        }
        .into();
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
