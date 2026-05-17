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

use gloo_net::http::{Request, RequestBuilder, Response};
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
    Http { status: u16 },
    /// Response body couldn't be deserialized into the expected envelope.
    Parse(String),
    /// Request body couldn't be serialized.
    Serialize(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(msg) => write!(f, "Network error: {msg}"),
            Self::Http { status } => write!(f, "HTTP {status}"),
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

fn ensure_success(resp: &Response) -> Result<(), ApiError> {
    let status = resp.status();
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(ApiError::Http { status })
    }
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

fn with_auth(request: RequestBuilder) -> RequestBuilder {
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

// ── GET helpers ─────────────────────────────────────────────────────────────

/// GET `url`, unwrap the [`ApiEnvelope`], return the inner data.
pub async fn fetch_json<T>(url: &str) -> Result<T, ApiError>
where
    T: DeserializeOwned,
{
    let resp = with_auth(Request::get(url))
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    ensure_success(&resp)?;
    let envelope: ApiEnvelope<T> = resp
        .json()
        .await
        .map_err(|e| ApiError::Parse(e.to_string()))?;
    Ok(envelope.data)
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
    ensure_success(&resp)?;
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
    let body_str = serde_json::to_string(body).map_err(|e| ApiError::Serialize(e.to_string()))?;
    let resp = with_auth(Request::post(url))
        .header("Content-Type", "application/json")
        .body(body_str)
        .map_err(|e| ApiError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    ensure_success(&resp)?;
    let envelope: ApiEnvelope<Res> = resp
        .json()
        .await
        .map_err(|e| ApiError::Parse(e.to_string()))?;
    Ok(envelope.data)
}

/// PATCH JSON body, parse envelope, return inner data.
pub async fn patch_json<Req, Res>(url: &str, body: &Req) -> Result<Res, ApiError>
where
    Req: Serialize + ?Sized,
    Res: DeserializeOwned,
{
    let body_str = serde_json::to_string(body).map_err(|e| ApiError::Serialize(e.to_string()))?;
    let resp = with_auth(Request::patch(url))
        .header("Content-Type", "application/json")
        .body(body_str)
        .map_err(|e| ApiError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    ensure_success(&resp)?;
    let envelope: ApiEnvelope<Res> = resp
        .json()
        .await
        .map_err(|e| ApiError::Parse(e.to_string()))?;
    Ok(envelope.data)
}

/// PUT JSON body, parse envelope, return inner data.
pub async fn put_json<Req, Res>(url: &str, body: &Req) -> Result<Res, ApiError>
where
    Req: Serialize + ?Sized,
    Res: DeserializeOwned,
{
    let body_str = serde_json::to_string(body).map_err(|e| ApiError::Serialize(e.to_string()))?;
    let resp = with_auth(Request::put(url))
        .header("Content-Type", "application/json")
        .body(body_str)
        .map_err(|e| ApiError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    ensure_success(&resp)?;
    let envelope: ApiEnvelope<Res> = resp
        .json()
        .await
        .map_err(|e| ApiError::Parse(e.to_string()))?;
    Ok(envelope.data)
}

// ── Write helpers that discard the response body ────────────────────────────

/// POST with no request body, discard the response. Used for trigger actions
/// like `apply_effect` or `discover_devices`.
pub async fn post_empty(url: &str) -> Result<(), ApiError> {
    let resp = with_auth(Request::post(url))
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    ensure_success(&resp)?;
    Ok(())
}

/// POST JSON body, discard the response. Used for actions that send a payload
/// but don't return anything meaningful (e.g., `identify_device`, `add_favorite`).
pub async fn post_json_discard<Req>(url: &str, body: &Req) -> Result<(), ApiError>
where
    Req: Serialize + ?Sized,
{
    let body_str = serde_json::to_string(body).map_err(|e| ApiError::Serialize(e.to_string()))?;
    let resp = with_auth(Request::post(url))
        .header("Content-Type", "application/json")
        .body(body_str)
        .map_err(|e| ApiError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    ensure_success(&resp)?;
    Ok(())
}

/// PUT JSON body, discard the response. Used for idempotent actions that
/// send a payload but don't return anything (e.g., `preview_layout`).
pub async fn put_json_discard<Req>(url: &str, body: &Req) -> Result<(), ApiError>
where
    Req: Serialize + ?Sized,
{
    let body_str = serde_json::to_string(body).map_err(|e| ApiError::Serialize(e.to_string()))?;
    let resp = with_auth(Request::put(url))
        .header("Content-Type", "application/json")
        .body(body_str)
        .map_err(|e| ApiError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    ensure_success(&resp)?;
    Ok(())
}

/// PATCH JSON body, discard the response. Used for partial updates that
/// don't echo the updated resource (e.g., `update_controls`).
pub async fn patch_json_discard<Req>(url: &str, body: &Req) -> Result<(), ApiError>
where
    Req: Serialize + ?Sized,
{
    let body_str = serde_json::to_string(body).map_err(|e| ApiError::Serialize(e.to_string()))?;
    let resp = with_auth(Request::patch(url))
        .header("Content-Type", "application/json")
        .body(body_str)
        .map_err(|e| ApiError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    ensure_success(&resp)?;
    Ok(())
}

/// DELETE `url`, discard the response body.
pub async fn delete_empty(url: &str) -> Result<(), ApiError> {
    let resp = with_auth(Request::delete(url))
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    ensure_success(&resp)?;
    Ok(())
}
