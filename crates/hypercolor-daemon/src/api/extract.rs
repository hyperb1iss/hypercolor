//! Request extractors whose rejections are domain errors.
//!
//! Axum's stock extractors answer a request they cannot decode with a
//! `text/plain` sentence of their own, which would make extractor
//! refusals the one family of `/api/v1` failures that never reaches
//! [`DomainError`]. Each wrapper here delegates to the stock extractor
//! and converts its typed rejection into the `DomainError` it describes,
//! so the refusal renders as the canonical error envelope with a stable
//! code, the framework's full diagnostic as the message, and the
//! offending field path when the decoder reports one.
//!
//! Every built-in handler extracts through these. Extension routes can
//! opt in by naming them in place of the Axum types; nothing intercepts a
//! route that does not.

use std::error::Error as StdError;
use std::ops::{Deref, DerefMut};

use axum::extract::multipart::MultipartRejection;
use axum::extract::rejection::{
    BytesRejection, ExtensionRejection, JsonRejection, PathRejection, QueryRejection,
};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::{
    Extension, FromRequest, FromRequestParts, Multipart, OptionalFromRequest, Path, Query, Request,
};
use axum::http::StatusCode;
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use crate::domain::DomainError;

/// The request body ceiling every `/api/v1` route enforces unless it sets
/// its own.
///
/// This equals Axum's built-in default. The API router installs it
/// explicitly so the `413` [`ApiJson`] renders names a limit the daemon
/// actually enforces. A route that installs a different
/// `DefaultBodyLimit` reports its own ceiling instead (asset upload does).
pub const API_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;

/// A JSON request body. A body that will not decode is `400
/// malformed_request` (invalid JSON), `422 validation_error` (wrong
/// shape, with `details.field` naming the path), `415
/// unsupported_media_type` (no JSON content type), or `413
/// payload_too_large`.
///
/// As `Option<ApiJson<T>>` it is absent when the request carries no
/// `Content-Type`, matching `Option<axum::Json<T>>`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = DomainError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        <axum::Json<T> as FromRequest<S>>::from_request(request, state)
            .await
            .map(|axum::Json(value)| Self(value))
            .map_err(json_rejection)
    }
}

impl<T, S> OptionalFromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = DomainError;

    async fn from_request(request: Request, state: &S) -> Result<Option<Self>, Self::Rejection> {
        <axum::Json<T> as OptionalFromRequest<S>>::from_request(request, state)
            .await
            .map(|body| body.map(|axum::Json(value)| Self(value)))
            .map_err(json_rejection)
    }
}

/// Typed path parameters. A segment that will not parse is `400
/// malformed_request` naming the key and value.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = DomainError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        <Path<T> as FromRequestParts<S>>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(path_rejection)
    }
}

/// A typed query string. A parameter that will not deserialize is `422
/// validation_error` with `details.field` naming the key, the convention
/// the effect catalog's filters established.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = DomainError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        <Query<T> as FromRequestParts<S>>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(query_rejection)
    }
}

/// A `multipart/form-data` body. A request without a multipart boundary
/// is `400 malformed_request`; field-level read failures stay with the
/// handler, which knows its own size ceiling.
#[derive(Debug)]
pub struct ApiMultipart(pub Multipart);

impl<S> FromRequest<S> for ApiMultipart
where
    S: Send + Sync,
{
    type Rejection = DomainError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        <Multipart as FromRequest<S>>::from_request(request, state)
            .await
            .map(Self)
            .map_err(multipart_rejection)
    }
}

/// A request extension the daemon's own middleware installs. Its absence
/// is a wiring defect, so it renders as `500 internal_error`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiExtension<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiExtension<T>
where
    T: Clone + Send + Sync + 'static,
    S: Send + Sync,
{
    type Rejection = DomainError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        <Extension<T> as FromRequestParts<S>>::from_request_parts(parts, state)
            .await
            .map(|Extension(value)| Self(value))
            .map_err(extension_rejection)
    }
}

/// A WebSocket upgrade. A request that is not a usable upgrade is `400
/// malformed_request` carrying the handshake diagnostic.
#[derive(Debug)]
pub struct ApiWebSocketUpgrade(pub WebSocketUpgrade);

impl<S> FromRequestParts<S> for ApiWebSocketUpgrade
where
    S: Send + Sync,
{
    type Rejection = DomainError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        <WebSocketUpgrade as FromRequestParts<S>>::from_request_parts(parts, state)
            .await
            .map(Self)
            .map_err(websocket_upgrade_rejection)
    }
}

/// Transparent access to the extracted value, as the Axum extractors
/// offer, so a handler can bind the wrapper and read fields through it.
macro_rules! deref_to_inner {
    ($($wrapper:ident),+) => {$(
        impl<T> Deref for $wrapper<T> {
            type Target = T;

            fn deref(&self) -> &T {
                &self.0
            }
        }

        impl<T> DerefMut for $wrapper<T> {
            fn deref_mut(&mut self) -> &mut T {
                &mut self.0
            }
        }
    )+};
}

deref_to_inner!(ApiJson, ApiPath, ApiQuery, ApiExtension);

fn json_rejection(rejection: JsonRejection) -> DomainError {
    match rejection {
        JsonRejection::JsonDataError(error) => DomainError::Validation {
            field: deserialize_field::<serde_json::Error>(&error),
            message: error.body_text(),
            details: None,
        },
        JsonRejection::JsonSyntaxError(error) => DomainError::malformed(error.body_text()),
        JsonRejection::MissingJsonContentType(error) => {
            DomainError::unsupported_media_type(error.body_text())
        }
        JsonRejection::BytesRejection(error) => bytes_rejection(&error),
        other => status_rejection(other.status(), other.body_text()),
    }
}

fn bytes_rejection(rejection: &BytesRejection) -> DomainError {
    status_rejection(rejection.status(), rejection.body_text())
}

fn path_rejection(rejection: PathRejection) -> DomainError {
    status_rejection(rejection.status(), rejection.body_text())
}

fn query_rejection(rejection: QueryRejection) -> DomainError {
    match rejection {
        QueryRejection::FailedToDeserializeQueryString(error) => DomainError::Validation {
            field: deserialize_field::<serde::de::value::Error>(&error),
            message: error.body_text(),
            details: None,
        },
        other => status_rejection(other.status(), other.body_text()),
    }
}

fn multipart_rejection(rejection: MultipartRejection) -> DomainError {
    status_rejection(rejection.status(), rejection.body_text())
}

fn extension_rejection(rejection: ExtensionRejection) -> DomainError {
    DomainError::Internal(anyhow::anyhow!(rejection.body_text()))
}

fn websocket_upgrade_rejection(rejection: WebSocketUpgradeRejection) -> DomainError {
    DomainError::malformed(rejection.body_text())
}

/// The domain failure a rejection with no richer mapping stands for,
/// keyed by the status Axum assigned it.
fn status_rejection(status: StatusCode, message: String) -> DomainError {
    match status {
        StatusCode::PAYLOAD_TOO_LARGE => DomainError::PayloadTooLarge {
            limit_bytes: u64::try_from(API_BODY_LIMIT_BYTES).unwrap_or(u64::MAX),
        },
        StatusCode::UNSUPPORTED_MEDIA_TYPE => DomainError::unsupported_media_type(message),
        StatusCode::UNPROCESSABLE_ENTITY => DomainError::validation(message),
        status if status.is_server_error() => {
            DomainError::Internal(anyhow::anyhow!("request extraction failed: {message}"))
        }
        _ => DomainError::malformed(message),
    }
}

/// The field path a deserialization failure names, when it names one.
///
/// Axum deserializes JSON bodies and query strings through
/// `serde_path_to_error` and keeps that error as the rejection's source,
/// so the path is read from the error chain rather than reparsed from the
/// message. A failure at the document root has no field.
fn deserialize_field<E>(rejection: &(dyn StdError + 'static)) -> Option<String>
where
    E: StdError + 'static,
{
    let mut current = Some(rejection);
    while let Some(error) = current {
        if let Some(located) = error.downcast_ref::<serde_path_to_error::Error<E>>() {
            let path = located.path().to_string();
            return (path != ".").then_some(path);
        }
        current = error.source();
    }
    None
}
