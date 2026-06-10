//! Shared HTTP plumbing for the REST API module.
//!
//! Every function in `api/*.rs` should be a thin wrapper over one of these
//! helpers. They handle: request construction, serialization, error mapping,
//! status-code checks, envelope unwrapping, and response deserialization — so
//! domain modules only specify the URL, request body type, and response type.
//!
//! Helpers return [`ApiError`]. Domain functions that still return
//! `Result<T, String>` can convert via `?` (see `From<ApiError> for String`)
//! or `map_err(Into::into)`.

use std::fmt;

use gloo_net::http::{Method, Request, RequestBuilder, Response};
use serde::{Serialize, de::DeserializeOwned};

use super::ApiEnvelope;

#[cfg(target_arch = "wasm32")]
const API_KEY_STORAGE_KEY: &str = "hypercolor.api_key";

// ── Error type ──────────────────────────────────────────────────────────────

/// Typed error surface for HTTP operations.
///
/// Preserves the failure mode (network vs status vs parse vs serialize) so
/// callers can make informed decisions later (retry on network/5xx, surface
/// parse errors as bugs, etc.) without re-parsing `String` messages.
#[derive(Debug, Clone)]
pub enum ApiError {
    /// Transport-layer failure (socket, CORS, abort, DNS, etc.).
    Network(String),
    /// Non-2xx response from the server.
    Http {
        status: u16,
        message: Option<String>,
    },
    /// Response body couldn't be deserialized into the expected envelope.
    Parse(String),
    /// Request body couldn't be serialized.
    Serialize(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(msg) => write!(f, "Network error: {msg}"),
            Self::Http {
                status,
                message: Some(message),
            } => write!(f, "{message} (HTTP {status})"),
            Self::Http {
                status,
                message: None,
            } => write!(f, "HTTP {status}"),
            Self::Parse(msg) => write!(f, "Parse error: {msg}"),
            Self::Serialize(msg) => write!(f, "Serialize error: {msg}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<ApiError> for String {
    fn from(err: ApiError) -> Self {
        err.to_string()
    }
}

// ── Versioned-mutation outcome ──────────────────────────────────────────────

/// Outcome of a mutation guarded by an `If-Match` version precondition.
///
/// The daemon honors `If-Match: "<version>"` on its optimistic-concurrency
/// routes (zone, layer, and effect-control mutations): it applies the
/// mutation only when the version still matches, otherwise replies `412`
/// with the authoritative `current` version. Modeled as a real type — not
/// an HTTP string match — so callers can drive a clean rebase/refetch path
/// off `Stale`.
#[derive(Debug, Clone, PartialEq)]
pub enum MutationOutcome<T> {
    /// The mutation applied; carries whatever the route returned.
    Applied(T),
    /// The `If-Match` precondition failed. `current` is the daemon's
    /// authoritative version token to rebase on before retrying.
    Stale { current: u64 },
}

impl<T> MutationOutcome<T> {
    /// Transform the `Applied` payload, passing `Stale` through unchanged.
    pub fn map<U>(self, transform: impl FnOnce(T) -> U) -> MutationOutcome<U> {
        match self {
            Self::Applied(value) => MutationOutcome::Applied(transform(value)),
            Self::Stale { current } => MutationOutcome::Stale { current },
        }
    }
}

async fn ensure_success(resp: Response) -> Result<Response, ApiError> {
    let status = resp.status();
    if (200..300).contains(&status) {
        Ok(resp)
    } else {
        Err(http_error(resp).await)
    }
}

async fn http_error(resp: Response) -> ApiError {
    let status = resp.status();
    let message = resp
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|body| extract_error_message(&body));
    ApiError::Http { status, message }
}

fn extract_error_message(body: &serde_json::Value) -> Option<String> {
    body.pointer("/error/message")
        .and_then(serde_json::Value::as_str)
        .or_else(|| body.get("message").and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(ToOwned::to_owned)
}

pub async fn response_error_string(resp: Response) -> String {
    http_error(resp).await.to_string()
}

/// Return the browser-stored API key, if one has been configured.
#[must_use]
pub fn stored_api_key() -> Option<String> {
    stored_api_key_impl()
}

/// Persist the browser API key used for REST and WebSocket requests.
pub fn save_api_key(api_key: &str) {
    save_api_key_impl(api_key);
}

pub(crate) fn with_auth(request: RequestBuilder) -> RequestBuilder {
    if let Some(api_key) = stored_api_key() {
        request.header("Authorization", &format!("Bearer {api_key}"))
    } else {
        request
    }
}

#[cfg(target_arch = "wasm32")]
fn stored_api_key_impl() -> Option<String> {
    let storage = web_sys::window().and_then(|window| window.local_storage().ok().flatten())?;
    storage
        .get_item(API_KEY_STORAGE_KEY)
        .ok()
        .flatten()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
}

#[cfg(not(target_arch = "wasm32"))]
fn stored_api_key_impl() -> Option<String> {
    None
}

#[cfg(target_arch = "wasm32")]
fn save_api_key_impl(api_key: &str) {
    let trimmed = api_key.trim();
    let Some(storage) = web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    else {
        return;
    };

    if trimmed.is_empty() {
        let _ = storage.remove_item(API_KEY_STORAGE_KEY);
    } else {
        let _ = storage.set_item(API_KEY_STORAGE_KEY, trimmed);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn save_api_key_impl(_api_key: &str) {}

// ── Request core ────────────────────────────────────────────────────────────
// Every JSON helper below is a thin wrapper over this single sender, so the
// auth header, optional `If-Match` precondition, serialization, and error
// mapping live in exactly one place.

/// Build and send one JSON request. Attaches the stored API key, an
/// optional `If-Match` version precondition, and — when a body is supplied —
/// serializes it with a `Content-Type: application/json` header. Performs
/// no status-code handling; callers classify the response.
async fn send_request<Req>(
    method: Method,
    url: &str,
    body: Option<&Req>,
    if_match: Option<u64>,
) -> Result<Response, ApiError>
where
    Req: Serialize + ?Sized,
{
    let mut builder = with_auth(RequestBuilder::new(url).method(method));
    if let Some(version) = if_match {
        builder = builder.header("If-Match", &version.to_string());
    }
    match body {
        Some(body) => {
            let body_str =
                serde_json::to_string(body).map_err(|e| ApiError::Serialize(e.to_string()))?;
            builder
                .header("Content-Type", "application/json")
                .body(body_str)
                .map_err(|e| ApiError::Network(e.to_string()))?
                .send()
                .await
                .map_err(|e| ApiError::Network(e.to_string()))
        }
        None => builder
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string())),
    }
}

/// Unwrap the [`ApiEnvelope`] from a successful response.
async fn parse_envelope<Res>(resp: Response) -> Result<Res, ApiError>
where
    Res: DeserializeOwned,
{
    let envelope: ApiEnvelope<Res> = resp
        .json()
        .await
        .map_err(|e| ApiError::Parse(e.to_string()))?;
    Ok(envelope.data)
}

/// Send a JSON request, require success, parse the envelope.
async fn send_json<Req, Res>(method: Method, url: &str, body: Option<&Req>) -> Result<Res, ApiError>
where
    Req: Serialize + ?Sized,
    Res: DeserializeOwned,
{
    let resp = send_request(method, url, body, None).await?;
    let resp = ensure_success(resp).await?;
    parse_envelope(resp).await
}

/// Send a JSON request, require success, discard the response body.
async fn send_json_discard<Req>(
    method: Method,
    url: &str,
    body: Option<&Req>,
) -> Result<(), ApiError>
where
    Req: Serialize + ?Sized,
{
    let resp = send_request(method, url, body, None).await?;
    ensure_success(resp).await?;
    Ok(())
}

/// Send a JSON mutation guarded by an optional `If-Match` version
/// precondition, classifying a `412` reply as [`MutationOutcome::Stale`].
///
/// On success the envelope's inner data is returned as
/// [`MutationOutcome::Applied`]. On `412` the daemon's authoritative
/// `current` version is parsed from the response body. Pass `None` for
/// `if_match` to apply unconditionally (the daemon then skips the
/// precondition check); a `412` is still classified if one arrives.
pub async fn send_json_versioned<Req, Res>(
    method: Method,
    url: &str,
    body: Option<&Req>,
    if_match: Option<u64>,
) -> Result<MutationOutcome<Res>, ApiError>
where
    Req: Serialize + ?Sized,
    Res: DeserializeOwned,
{
    let resp = send_request(method, url, body, if_match).await?;
    match resp.status() {
        200..=299 => Ok(MutationOutcome::Applied(parse_envelope(resp).await?)),
        412 => {
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| ApiError::Parse(e.to_string()))?;
            let current = stale_current_version(&body).ok_or_else(|| {
                ApiError::Parse("412 response missing `current` version token".to_owned())
            })?;
            Ok(MutationOutcome::Stale { current })
        }
        _ => Err(http_error(resp).await),
    }
}

/// Extract the authoritative version token from a `412` response body.
fn stale_current_version(body: &serde_json::Value) -> Option<u64> {
    body.get("current").and_then(serde_json::Value::as_u64)
}

// ── GET helpers ─────────────────────────────────────────────────────────────

/// GET `url`, unwrap the [`ApiEnvelope`], return the inner data.
pub async fn fetch_json<T>(url: &str) -> Result<T, ApiError>
where
    T: DeserializeOwned,
{
    send_json::<(), T>(Method::GET, url, None).await
}

/// GET `url`, returning `Ok(None)` on HTTP 404 and `Ok(Some(data))` on success.
/// All other non-2xx responses return `Err`. Used for endpoints where absence
/// is a normal state (e.g., "no active effect").
pub async fn fetch_json_optional<T>(url: &str) -> Result<Option<T>, ApiError>
where
    T: DeserializeOwned,
{
    let resp = with_auth(Request::get(url))
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    if resp.status() == 404 {
        return Ok(None);
    }
    let resp = ensure_success(resp).await?;
    let envelope: ApiEnvelope<T> = resp
        .json()
        .await
        .map_err(|e| ApiError::Parse(e.to_string()))?;
    Ok(Some(envelope.data))
}

// ── Write helpers that return a parsed response ─────────────────────────────

/// POST JSON body, parse envelope, return inner data.
pub async fn post_json<Req, Res>(url: &str, body: &Req) -> Result<Res, ApiError>
where
    Req: Serialize + ?Sized,
    Res: DeserializeOwned,
{
    send_json(Method::POST, url, Some(body)).await
}

/// PATCH JSON body, parse envelope, return inner data.
pub async fn patch_json<Req, Res>(url: &str, body: &Req) -> Result<Res, ApiError>
where
    Req: Serialize + ?Sized,
    Res: DeserializeOwned,
{
    send_json(Method::PATCH, url, Some(body)).await
}

/// PUT JSON body, parse envelope, return inner data.
pub async fn put_json<Req, Res>(url: &str, body: &Req) -> Result<Res, ApiError>
where
    Req: Serialize + ?Sized,
    Res: DeserializeOwned,
{
    send_json(Method::PUT, url, Some(body)).await
}

// ── Write helpers that discard the response body ────────────────────────────

/// POST with no request body, discard the response. Used for trigger actions
/// like `apply_effect` or `discover_devices`.
pub async fn post_empty(url: &str) -> Result<(), ApiError> {
    send_json_discard::<()>(Method::POST, url, None).await
}

/// POST JSON body, discard the response. Used for actions that send a payload
/// but don't return anything meaningful (e.g., `identify_device`, `add_favorite`).
pub async fn post_json_discard<Req>(url: &str, body: &Req) -> Result<(), ApiError>
where
    Req: Serialize + ?Sized,
{
    send_json_discard(Method::POST, url, Some(body)).await
}

/// PUT JSON body, discard the response. Used for idempotent actions that
/// send a payload but don't return anything (e.g., `preview_layout`).
pub async fn put_json_discard<Req>(url: &str, body: &Req) -> Result<(), ApiError>
where
    Req: Serialize + ?Sized,
{
    send_json_discard(Method::PUT, url, Some(body)).await
}

/// PATCH JSON body, discard the response. Used for partial updates that
/// don't echo the updated resource (e.g., `update_controls`).
pub async fn patch_json_discard<Req>(url: &str, body: &Req) -> Result<(), ApiError>
where
    Req: Serialize + ?Sized,
{
    send_json_discard(Method::PATCH, url, Some(body)).await
}

/// DELETE `url`, parse envelope, return inner data. Used for deletes that
/// echo a confirmation payload (e.g., `unpair_device`).
pub async fn delete_json<Res>(url: &str) -> Result<Res, ApiError>
where
    Res: DeserializeOwned,
{
    send_json::<(), Res>(Method::DELETE, url, None).await
}

/// DELETE `url`, discard the response body.
pub async fn delete_empty(url: &str) -> Result<(), ApiError> {
    send_json_discard::<()>(Method::DELETE, url, None).await
}

#[cfg(test)]
mod tests {
    use super::{ApiError, MutationOutcome, extract_error_message, stale_current_version};

    #[test]
    fn stale_current_version_parses_daemon_412_body() {
        let body = serde_json::json!({ "current": 17 });
        assert_eq!(stale_current_version(&body), Some(17));
    }

    #[test]
    fn stale_current_version_rejects_missing_or_nonnumeric_token() {
        assert_eq!(stale_current_version(&serde_json::json!({})), None);
        assert_eq!(
            stale_current_version(&serde_json::json!({ "current": "7" })),
            None
        );
    }

    #[test]
    fn mutation_outcome_map_transforms_applied_payload() {
        let outcome = MutationOutcome::Applied(21_u64).map(|value| value * 2);
        assert_eq!(outcome, MutationOutcome::Applied(42));
    }

    #[test]
    fn mutation_outcome_map_passes_stale_through() {
        let outcome = MutationOutcome::<u64>::Stale { current: 9 }.map(|value| value * 2);
        assert_eq!(outcome, MutationOutcome::Stale { current: 9 });
    }

    #[test]
    fn extracts_daemon_error_message_from_envelope() {
        let body = serde_json::json!({
            "error": {
                "message": "Active scene changed elsewhere"
            }
        });

        assert_eq!(
            extract_error_message(&body),
            Some("Active scene changed elsewhere".to_owned())
        );
    }

    #[test]
    fn http_error_display_includes_server_message() {
        let error = ApiError::Http {
            status: 409,
            message: Some("Scene is snapshot locked".to_owned()),
        };

        assert_eq!(error.to_string(), "Scene is snapshot locked (HTTP 409)");
    }
}
