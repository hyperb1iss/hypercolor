//! REST projection for transport-neutral domain failures.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hypercolor_types::api::envelope::{ApiErrorBody, ApiErrorDetail};

use super::envelope::response_meta;
use crate::domain::DomainError;

const fn status(error: &DomainError) -> StatusCode {
    match error {
        DomainError::NotFound { .. } => StatusCode::NOT_FOUND,
        DomainError::Validation { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        DomainError::Malformed { .. } => StatusCode::BAD_REQUEST,
        DomainError::Conflict { .. } | DomainError::ControlBound { .. } => StatusCode::CONFLICT,
        DomainError::Unauthorized { .. } => StatusCode::UNAUTHORIZED,
        DomainError::Forbidden { .. } => StatusCode::FORBIDDEN,
        DomainError::PayloadTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
        DomainError::UnsupportedMediaType { .. } => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        DomainError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
        DomainError::PreconditionFailed { .. } => StatusCode::PRECONDITION_FAILED,
        DomainError::DeviceUnavailable { .. } | DomainError::ServiceUnavailable { .. } => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        DomainError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Structured recovery context every transport projection shares.
pub(crate) fn client_details(error: &DomainError) -> Option<serde_json::Value> {
    match error {
        DomainError::Validation { field, details, .. } => {
            merge_field(details.clone(), field.as_ref())
        }
        DomainError::Conflict { details, .. }
        | DomainError::Forbidden { details, .. }
        | DomainError::ServiceUnavailable { details, .. } => details.clone(),
        DomainError::ControlBound { keys } => Some(serde_json::json!({ "bound": keys })),
        DomainError::PreconditionFailed {
            expected, current, ..
        } => Some(serde_json::json!({ "expected": expected, "current": current })),
        DomainError::PayloadTooLarge { limit_bytes } => {
            Some(serde_json::json!({ "limit_bytes": limit_bytes }))
        }
        DomainError::RateLimited {
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

fn detail(error: &DomainError) -> ApiErrorDetail {
    ApiErrorDetail {
        code: error.code().to_owned(),
        message: error.client_message(),
        details: client_details(error),
    }
}

impl IntoResponse for DomainError {
    fn into_response(self) -> Response {
        if let Self::Internal(error) = &self {
            tracing::error!(chain = format!("{error:#}"), "domain internal error");
        }
        let status = status(&self);
        let body = ApiErrorBody {
            error: detail(&self),
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

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use serde_json::json;

    use super::*;
    use crate::domain::ResourceKind;

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        serde_json::from_slice(&bytes).expect("body is JSON")
    }

    #[tokio::test]
    async fn canonical_error_envelope_carries_code_message_meta() {
        let response = DomainError::not_found(ResourceKind::Scene, "sc_123").into_response();
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
        let response = DomainError::PreconditionFailed {
            resource: ResourceKind::Zone,
            expected: 4,
            current: 7,
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
        assert_eq!(
            response
                .headers()
                .get(header::ETAG)
                .and_then(|value| value.to_str().ok()),
            Some("\"7\"")
        );
        let json = body_json(response).await;
        assert_eq!(json["error"]["details"]["expected"], 4);
        assert_eq!(json["error"]["details"]["current"], 7);
    }

    #[tokio::test]
    async fn internal_errors_render_generically() {
        let response =
            DomainError::Internal(anyhow::anyhow!("secret path /home/user leaked")).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let json = body_json(response).await;
        assert_eq!(json["error"]["message"], "internal error");
        assert_eq!(json["error"]["code"], "internal_error");
    }

    #[tokio::test]
    async fn absent_details_are_omitted() {
        let response = DomainError::validation("nope").into_response();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let text = std::str::from_utf8(&bytes).expect("utf8");
        assert!(!text.contains("\"details\""), "unexpected details: {text}");
    }

    #[tokio::test]
    async fn validation_details_keep_the_field() {
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
    async fn structured_context_survives_projection() {
        let validation = DomainError::validation_field("name", "must not be blank");
        let json = body_json(validation.into_response()).await;
        assert_eq!(json["error"]["details"]["field"], "name");

        let forbidden =
            DomainError::forbidden_details("read-only key", json!({ "required_tier": "control" }));
        let json = body_json(forbidden.into_response()).await;
        assert_eq!(json["error"]["details"]["required_tier"], "control");

        let conflict = DomainError::conflict_details(
            "stale control surface",
            json!({ "current_revision": 4 }),
        );
        let json = body_json(conflict.into_response()).await;
        assert_eq!(json["error"]["details"]["current_revision"], 4);
    }

    #[tokio::test]
    async fn every_status_family_keeps_its_rest_projection() {
        let cases = [
            (
                DomainError::malformed("bad header"),
                StatusCode::BAD_REQUEST,
                "malformed_request",
            ),
            (
                DomainError::unauthorized("missing key"),
                StatusCode::UNAUTHORIZED,
                "unauthorized",
            ),
            (
                DomainError::forbidden("denied"),
                StatusCode::FORBIDDEN,
                "forbidden",
            ),
            (
                DomainError::unsupported_media_type("bad type"),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
            ),
            (
                DomainError::conflict("stale state"),
                StatusCode::CONFLICT,
                "conflict",
            ),
        ];

        for (error, expected_status, expected_code) in cases {
            let response = error.into_response();
            assert_eq!(response.status(), expected_status);
            assert_eq!(body_json(response).await["error"]["code"], expected_code);
        }
    }

    #[tokio::test]
    async fn limit_details_survive_projection() {
        let payload = DomainError::PayloadTooLarge { limit_bytes: 64 };
        let json = body_json(payload.into_response()).await;
        assert_eq!(json["error"]["details"]["limit_bytes"], 64);

        let rate = DomainError::RateLimited {
            message: "slow down".to_owned(),
            limit: 60,
            window_seconds: 60,
            retry_after_secs: 12,
        };
        let response = rate.into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let json = body_json(response).await;
        assert_eq!(json["error"]["details"]["limit"], 60);
        assert_eq!(json["error"]["details"]["retry_after"], 12);
    }
}
