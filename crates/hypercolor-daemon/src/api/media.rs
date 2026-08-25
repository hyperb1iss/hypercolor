//! Media authorization endpoint.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, State};
use axum::response::{IntoResponse, Response};
use hypercolor_core::input::media::{
    MediaAuthorizationTarget, MediaProviderError, MediaProviderErrorKind,
    request_media_authorization,
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use utoipa::ToSchema;

use crate::api::envelope;
use crate::api::security::RequestAuthContext;
use crate::app_state::AppState;
use crate::domain::DomainError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaAuthorizationAdapter {
    /// Apple Music.
    Music,
    /// Spotify.
    Spotify,
}

/// Explicit media Automation authorization request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MediaAuthorizationRequest {
    /// Already-running media application to authorize.
    pub adapter: MediaAuthorizationAdapter,
}

/// Result of one explicit media Automation authorization request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MediaAuthorizationResponse {
    /// Whether Automation access is authorized.
    pub authorized: bool,
    /// Application whose access was authorized.
    pub adapter: MediaAuthorizationAdapter,
}

/// Request macOS Automation authorization for one already-running media app.
#[utoipa::path(
    post,
    path = "/api/v1/media/authorize",
    request_body = MediaAuthorizationRequest,
    responses(
        (
            status = 200,
            description = "Media Automation authorization result",
            body = crate::api::envelope::ApiResponse<MediaAuthorizationResponse>
        ),
        (
            status = 403,
            description = "Control credential required",
            body = hypercolor_types::api::envelope::ApiErrorBody
        ),
        (
            status = 409,
            description = "Media Automation is unavailable or denied",
            body = hypercolor_types::api::envelope::ApiErrorBody
        ),
        (
            status = 422,
            description = "The requested media application is not running",
            body = hypercolor_types::api::envelope::ApiErrorBody
        )
    ),
    tag = "media"
)]
pub(crate) async fn authorize_media(
    State(_state): State<Arc<AppState>>,
    Extension(auth_context): Extension<RequestAuthContext>,
    Json(request): Json<MediaAuthorizationRequest>,
) -> Response {
    if let Some(rejection) = protected_control_rejection(auth_context) {
        return rejection;
    }

    let adapter = request.adapter;
    let result =
        tokio::task::spawn_blocking(move || request_media_authorization(adapter.into())).await;
    match result {
        Ok(Ok(())) => {
            info!(?adapter, "Media Automation authorization requested");
            envelope::ok(MediaAuthorizationResponse {
                authorized: true,
                adapter,
            })
        }
        Ok(Err(error)) => {
            warn!(?adapter, kind = ?error.kind(), %error, "Media Automation authorization failed");
            authorization_error(&error)
        }
        Err(error) => DomainError::Internal(anyhow::anyhow!(
            "Media Automation authorization task failed: {error}"
        ))
        .into_response(),
    }
}

fn protected_control_rejection(auth_context: RequestAuthContext) -> Option<Response> {
    (!auth_context.can_protected_control()).then(|| {
        DomainError::forbidden("Protected media access requires a control credential")
            .into_response()
    })
}

impl From<MediaAuthorizationAdapter> for MediaAuthorizationTarget {
    fn from(adapter: MediaAuthorizationAdapter) -> Self {
        match adapter {
            MediaAuthorizationAdapter::Music => Self::Music,
            MediaAuthorizationAdapter::Spotify => Self::Spotify,
        }
    }
}

fn authorization_error(error: &MediaProviderError) -> Response {
    match error.kind() {
        MediaProviderErrorKind::NoRunningPlayer => {
            DomainError::validation(error.to_string()).into_response()
        }
        MediaProviderErrorKind::UnsupportedCapability
        | MediaProviderErrorKind::AuthorizationRequired
        | MediaProviderErrorKind::AuthorizationDenied
        | MediaProviderErrorKind::StaleTarget => {
            DomainError::conflict(error.to_string()).into_response()
        }
        MediaProviderErrorKind::BackendFailure
        | MediaProviderErrorKind::TimedOut
        | MediaProviderErrorKind::AdapterFailure
        | MediaProviderErrorKind::Disconnected => {
            DomainError::Internal(anyhow::anyhow!(error.to_string())).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::*;

    #[test]
    fn media_authorization_contract_uses_stable_adapter_names() {
        let music = serde_json::to_value(MediaAuthorizationRequest {
            adapter: MediaAuthorizationAdapter::Music,
        })
        .expect("media authorization request serializes");
        let spotify: MediaAuthorizationRequest = serde_json::from_value(serde_json::json!({
            "adapter": "spotify"
        }))
        .expect("media authorization request deserializes");

        assert_eq!(music, serde_json::json!({ "adapter": "music" }));
        assert_eq!(spotify.adapter, MediaAuthorizationAdapter::Spotify);
    }

    #[test]
    fn media_authorization_requires_protected_control() {
        let rejection = protected_control_rejection(RequestAuthContext::read_only())
            .expect("read-only credentials must be rejected");

        assert_eq!(rejection.status(), StatusCode::FORBIDDEN);
        assert!(protected_control_rejection(RequestAuthContext::control()).is_none());
    }

    #[test]
    fn media_authorization_maps_provider_errors_without_prompting() {
        let not_running = authorization_error(&MediaProviderError::classified(
            MediaProviderErrorKind::NoRunningPlayer,
            "Music is not running",
        ));
        let denied = authorization_error(&MediaProviderError::classified(
            MediaProviderErrorKind::AuthorizationDenied,
            "Automation access was denied",
        ));
        let unsupported = authorization_error(&MediaProviderError::classified(
            MediaProviderErrorKind::UnsupportedCapability,
            "no provider here",
        ));

        assert_eq!(not_running.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(denied.status(), StatusCode::CONFLICT);
        assert_eq!(unsupported.status(), StatusCode::CONFLICT);
    }
}
