//! Media authorization endpoint.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use tracing::{info, warn};
use utoipa::ToSchema;

use crate::api::AppState;
#[cfg(target_os = "macos")]
use crate::api::envelope::ApiResponse;
use crate::api::security::RequestAuthContext;
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

    #[cfg(target_os = "macos")]
    {
        let adapter = request.adapter;
        let result = tokio::task::spawn_blocking(move || {
            let mut provider = hypercolor_macos_media::MediaProvider::new();
            provider.request_authorization(native_adapter(adapter))
        })
        .await;
        return match result {
            Ok(Ok(())) => {
                info!(?adapter, "Media Automation authorization requested");
                ApiResponse::ok(MediaAuthorizationResponse {
                    authorized: true,
                    adapter,
                })
            }
            Ok(Err(error)) => {
                warn!(?adapter, kind = ?error.kind(), %error, "Media Automation authorization failed");
                authorization_error(error)
            }
            Err(error) => DomainError::Internal(anyhow::anyhow!(
                "Media Automation authorization task failed: {error}"
            ))
            .into_response(),
        };
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = request;
        DomainError::conflict("Media Automation authorization is available only on macOS")
            .into_response()
    }
}

fn protected_control_rejection(auth_context: RequestAuthContext) -> Option<Response> {
    (!auth_context.can_protected_control()).then(|| {
        DomainError::forbidden("Protected media access requires a control credential")
            .into_response()
    })
}

#[cfg(target_os = "macos")]
const fn native_adapter(
    adapter: MediaAuthorizationAdapter,
) -> hypercolor_macos_media::MediaAdapter {
    match adapter {
        MediaAuthorizationAdapter::Music => hypercolor_macos_media::MediaAdapter::Music,
        MediaAuthorizationAdapter::Spotify => hypercolor_macos_media::MediaAdapter::Spotify,
    }
}

#[cfg(target_os = "macos")]
fn authorization_error(error: hypercolor_macos_media::MediaError) -> Response {
    use hypercolor_macos_media::MediaErrorKind;

    match error.kind() {
        MediaErrorKind::NoRunningCapablePlayer => {
            DomainError::validation(error.to_string()).into_response()
        }
        MediaErrorKind::UnsupportedCapability
        | MediaErrorKind::AuthorizationRequired
        | MediaErrorKind::AuthorizationDenied
        | MediaErrorKind::StaleTarget => DomainError::conflict(error.to_string()).into_response(),
        MediaErrorKind::TimedOut
        | MediaErrorKind::AdapterFailure
        | MediaErrorKind::Disconnected => {
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

    #[cfg(target_os = "macos")]
    #[test]
    fn media_authorization_maps_platform_errors_without_prompting() {
        use hypercolor_macos_media::{MediaError, MediaErrorKind};

        let not_running = authorization_error(MediaError::new(
            MediaErrorKind::NoRunningCapablePlayer,
            Some(hypercolor_macos_media::MediaAdapter::Music),
            "Music is not running",
        ));
        let denied = authorization_error(MediaError::new(
            MediaErrorKind::AuthorizationDenied,
            Some(hypercolor_macos_media::MediaAdapter::Music),
            "Automation access was denied",
        ));

        assert_eq!(not_running.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(denied.status(), StatusCode::CONFLICT);
    }
}
