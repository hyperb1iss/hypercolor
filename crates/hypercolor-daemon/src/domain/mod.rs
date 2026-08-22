//! The domain service layer's error and adapter conventions
//! (Spec 76 §2.1, §2.4).
//!
//! Business transactions live in this module tree as transport-free
//! `async fn`s; the four transports are thin adapters over them:
//!
//! - **REST**: parse → service → envelope. The route handler converts
//!   wire input, calls one service function, and wraps the typed
//!   outcome. The REST adapter projects `DomainError` into HTTP.
//! - **MCP**: schema validation, deterministic selector resolution, and
//!   one service call. The adapter projects `DomainError` into its tool
//!   error vocabulary.
//! - **WS commands**: call services directly; versions ride in-band.
//! - **CLI**: speaks the REST wire and deserializes
//!   `hypercolor_types::api::envelope::ApiResponse<Outcome>`.
//!
//! Domain signatures never mention Axum or `Response`. Structured JSON
//! remains only in error details and media admission diagnostics until
//! their typed replacements land. Mutations whose canonical events carry
//! provenance accept [`MutationContext`] beside the command, never inside
//! it. Commands that cannot publish the trigger carry no ceremonial context.
//!
//! MCP selector failures are an adapter concern. The adapter returns a
//! JSON-RPC invalid-params error with the normalized query, failure kind,
//! and deterministic candidates instead of asking the domain to resolve
//! transport-facing identity.
//!
//! Each transport owns exactly one `DomainError` projection. Domain code
//! never selects an HTTP status, builds an envelope, or emits a header.

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

    /// Client-safe message shared by every transport projection.
    #[must_use]
    pub(crate) fn client_message(&self) -> String {
        match self {
            Self::Internal(_) => "internal error".to_owned(),
            other => other.to_string(),
        }
    }

    /// Structured recovery context shared by every transport projection.
    #[must_use]
    pub(crate) fn client_details(&self) -> Option<serde_json::Value> {
        match self {
            Self::Validation { field, details, .. } => merge_field(details.clone(), field.as_ref()),
            Self::Conflict { details, .. }
            | Self::Forbidden { details, .. }
            | Self::ServiceUnavailable { details, .. } => details.clone(),
            Self::ControlBound { keys } => Some(serde_json::json!({ "bound": keys })),
            Self::PreconditionFailed {
                expected, current, ..
            } => Some(serde_json::json!({ "expected": expected, "current": current })),
            Self::PayloadTooLarge { limit_bytes } => {
                Some(serde_json::json!({ "limit_bytes": limit_bytes }))
            }
            Self::RateLimited {
                limit,
                window_seconds,
                retry_after_secs,
                ..
            } => Some(serde_json::json!({
                "limit": limit,
                "window_seconds": window_seconds,
                "retry_after": retry_after_secs,
            })),
            _ => None,
        }
    }
}

fn merge_field(
    details: Option<serde_json::Value>,
    field: Option<&String>,
) -> Option<serde_json::Value> {
    let Some(field) = field else {
        return details;
    };
    match details {
        Some(serde_json::Value::Object(mut map)) => {
            map.insert("field".to_owned(), serde_json::json!(field));
            Some(serde_json::Value::Object(map))
        }
        Some(other) => Some(serde_json::json!({ "field": field, "context": other })),
        None => Some(serde_json::json!({ "field": field })),
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

#[cfg(test)]
mod tests {
    use super::*;

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
