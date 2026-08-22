//! Shared HTTP plumbing for the REST API module.
//!
//! Every function in `api/*.rs` should be a thin wrapper over one of these
//! helpers. They handle: request construction, serialization, error mapping,
//! status-code checks, envelope unwrapping, and response deserialization so
//! domain modules only specify the URL, request body type, and response type.

use std::{cell::RefCell, fmt, rc::Rc};

use gloo_net::http::{Method, RequestBuilder};
use hypercolor_types::api::{ApiErrorBody, ApiResponse, ListResponse};
#[cfg(any(test, all(debug_assertions, target_arch = "wasm32")))]
use hypercolor_types::service::ProtectedControlCredential;
use js_sys::{Array, Uint8Array};
use serde::{Serialize, de::DeserializeOwned};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, BlobPropertyBag, File, FormData};

use super::http_transport::{
    HttpHeader, HttpMethod, HttpMultipartPart, HttpRequest, HttpRequestBody, HttpResponse,
    HttpTransport,
};

#[cfg(target_arch = "wasm32")]
const API_KEY_STORAGE_KEY: &str = "hypercolor.api_key";

thread_local! {
    static DAEMON_TRANSPORT: RefCell<DaemonTransport> = RefCell::new(DaemonTransport::default());
    static HTTP_TRANSPORT: RefCell<HttpTransportState> = RefCell::new(HttpTransportState::default());
}

#[derive(Default)]
struct HttpTransportState {
    provider: Option<Rc<dyn HttpTransport>>,
    used: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpTransportInstallError {
    AlreadyInstalled,
    AlreadyUsed,
}

impl fmt::Display for HttpTransportInstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInstalled => write!(f, "an HTTP transport is already installed"),
            Self::AlreadyUsed => write!(f, "the HTTP transport has already been used"),
        }
    }
}

impl std::error::Error for HttpTransportInstallError {}

pub fn install_http_transport(
    transport: Rc<dyn HttpTransport>,
) -> Result<(), HttpTransportInstallError> {
    HTTP_TRANSPORT.with_borrow_mut(|state| {
        if state.used {
            return Err(HttpTransportInstallError::AlreadyUsed);
        }
        if state.provider.is_some() {
            return Err(HttpTransportInstallError::AlreadyInstalled);
        }
        state.provider = Some(transport);
        Ok(())
    })
}

#[derive(Clone, PartialEq, Eq)]
struct DaemonTransport {
    native_app: bool,
    base_url: Option<String>,
    protected_control_credential: Option<String>,
}

impl Default for DaemonTransport {
    fn default() -> Self {
        Self {
            native_app: false,
            base_url: None,
            protected_control_credential: fragment_dev_control_credential(),
        }
    }
}

#[cfg(any(test, all(debug_assertions, target_arch = "wasm32")))]
fn parse_dev_control_credential_fragment(fragment: &str) -> Option<String> {
    let credential = fragment.strip_prefix('#')?;
    ProtectedControlCredential::parse(credential)
        .ok()
        .map(|credential| credential.expose_secret().to_owned())
}

fn fragment_dev_control_credential() -> Option<String> {
    #[cfg(all(debug_assertions, target_arch = "wasm32"))]
    {
        let window = web_sys::window()?;
        let location = window.location();
        let credential = parse_dev_control_credential_fragment(&location.hash().ok()?)?;
        let clean_url = format!("{}{}", location.pathname().ok()?, location.search().ok()?);
        window
            .history()
            .ok()?
            .replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&clean_url))
            .ok()?;
        Some(credential)
    }

    #[cfg(not(all(debug_assertions, target_arch = "wasm32")))]
    {
        None
    }
}

impl DaemonTransport {
    fn resolve_url(&self, url: &str) -> Option<String> {
        if !url.starts_with('/') {
            return Some(url.to_owned());
        }
        self.base_url
            .as_ref()
            .map(|base| format!("{}{url}", base.trim_end_matches('/')))
            .or_else(|| (!self.native_app).then(|| url.to_owned()))
    }

    fn authorization_token(&self, stored_api_key: Option<String>) -> Option<String> {
        self.protected_control_credential.clone().or(stored_api_key)
    }
}

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
    ///
    /// `code` and `details` come from the daemon's canonical error
    /// envelope, so callers can branch on the stable machine-readable code
    /// instead of pattern-matching human prose.
    Http {
        status: u16,
        code: Option<String>,
        message: Option<String>,
        details: Option<serde_json::Value>,
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
                ..
            } => write!(f, "{message} (HTTP {status})"),
            Self::Http {
                status,
                message: None,
                ..
            } => write!(f, "HTTP {status}"),
            Self::Parse(msg) => write!(f, "Parse error: {msg}"),
            Self::Serialize(msg) => write!(f, "Serialize error: {msg}"),
        }
    }
}

impl ApiError {
    /// A status failure the client itself raised, with no daemon error
    /// envelope behind it.
    #[must_use]
    pub const fn http(status: u16, message: Option<String>) -> Self {
        Self::Http {
            status,
            code: None,
            message,
            details: None,
        }
    }

    /// A locally detected stale-revision failure, coded like the daemon's.
    #[must_use]
    pub fn precondition_failed(message: impl Into<String>) -> Self {
        Self::Http {
            status: 412,
            code: Some("precondition_failed".to_owned()),
            message: Some(message.into()),
            details: None,
        }
    }

    /// The daemon's stable `snake_case` error code, when the failure came
    /// back as the canonical error envelope.
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Http { code, .. } => code.as_deref(),
            _ => None,
        }
    }

    /// Control keys the daemon refused because an input binding owns them.
    ///
    /// Empty for every failure that is not `control_bound`.
    #[must_use]
    pub fn bound_control_keys(&self) -> Vec<String> {
        let Self::Http {
            code: Some(code),
            details: Some(details),
            ..
        } = self
        else {
            return Vec::new();
        };
        if code != "control_bound" {
            return Vec::new();
        }
        details
            .get("bound")
            .and_then(serde_json::Value::as_array)
            .map(|keys| {
                keys.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl std::error::Error for ApiError {}

/// Canonical result type for daemon REST operations.
pub type ApiResult<T> = Result<T, ApiError>;

// ── Versioned-mutation outcome ──────────────────────────────────────────────

/// Outcome of a mutation guarded by an `If-Match` version precondition.
///
/// The daemon honors `If-Match: "<version>"` on its optimistic-concurrency
/// routes (zone, layer, and effect-control mutations): it applies the
/// mutation only when the version still matches, otherwise replies `412`
/// carrying the authoritative version in `error.details.current`. Modeled
/// as a real type — not an HTTP string match — so callers can drive a clean
/// rebase/refetch path off `Stale`.
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

fn ensure_success(resp: HttpResponse) -> ApiResult<HttpResponse> {
    let status = resp.status;
    if (200..300).contains(&status) {
        Ok(resp)
    } else {
        Err(http_error(&resp))
    }
}

fn http_error(resp: &HttpResponse) -> ApiError {
    let status = resp.status;
    let Ok(body) = serde_json::from_slice::<serde_json::Value>(&resp.body) else {
        return ApiError::Http {
            status,
            code: None,
            message: None,
            details: None,
        };
    };
    http_error_from_body(status, &body)
}

/// Project the daemon's canonical error envelope onto [`ApiError::Http`].
///
/// Routes outside the envelope (Axum's own rejections, binary responses)
/// still land here, so a body that does not deserialize degrades to the
/// message pointer rather than losing the status.
#[must_use]
pub fn http_error_from_body(status: u16, body: &serde_json::Value) -> ApiError {
    if let Ok(envelope) = serde_json::from_value::<ApiErrorBody>(body.clone()) {
        let message = Some(envelope.error.message)
            .map(|message| message.trim().to_owned())
            .filter(|message| !message.is_empty());
        return ApiError::Http {
            status,
            code: Some(envelope.error.code),
            message,
            details: envelope.error.details,
        };
    }
    ApiError::Http {
        status,
        code: body
            .pointer("/error/code")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        message: extract_error_message(body),
        details: body.pointer("/error/details").cloned(),
    }
}

fn extract_error_message(body: &serde_json::Value) -> Option<String> {
    body.pointer("/error/message")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(ToOwned::to_owned)
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

/// Configure the daemon base URL held only for this browser process.
pub fn begin_native_daemon_verification() {
    DAEMON_TRANSPORT.with_borrow_mut(|transport| {
        transport.native_app = true;
        transport.base_url = None;
        transport.protected_control_credential = None;
    });
}

/// Install an exact verified daemon connection without persistent storage.
pub fn install_verified_daemon_connection(base_url: &str, credential: Option<&str>) {
    let base_url = base_url.trim().trim_end_matches('/');
    let credential = credential
        .map(str::trim)
        .filter(|credential| !credential.is_empty());
    DAEMON_TRANSPORT.with_borrow_mut(|transport| {
        transport.native_app = true;
        transport.base_url = (!base_url.is_empty()).then(|| base_url.to_owned());
        transport.protected_control_credential = credential.map(str::to_owned);
    });
}

/// Remove both parts of the verified native daemon connection.
pub fn clear_verified_daemon_connection() {
    DAEMON_TRANSPORT.with_borrow_mut(|transport| {
        transport.base_url = None;
        transport.protected_control_credential = None;
    });
}

#[cfg(test)]
pub(crate) fn reset_daemon_transport_for_test() {
    DAEMON_TRANSPORT.with_borrow_mut(|transport| *transport = DaemonTransport::default());
}

/// Resolve a daemon-relative URL against the in-memory native base route.
#[must_use]
pub fn daemon_url(url: &str) -> Option<String> {
    DAEMON_TRANSPORT.with_borrow(|transport| transport.resolve_url(url))
}

/// Select the in-memory protected credential before any stored public key.
#[must_use]
pub fn authorization_token() -> Option<String> {
    DAEMON_TRANSPORT.with_borrow(|transport| transport.authorization_token(stored_api_key()))
}

/// Fence every daemon call to a logical, unambiguous relative path.
///
/// A path that is absolute, protocol-relative, or otherwise browser-normalized
/// must never reach a transport, because the default browser adapter would
/// attach the local bearer credential to it.
fn validate_relative_path(path: &str) -> ApiResult<()> {
    if !path.starts_with('/') || path.starts_with("//") {
        return Err(ApiError::Network(
            "authenticated daemon API URLs must be relative".to_owned(),
        ));
    }
    Ok(())
}

async fn dispatch(request: HttpRequest) -> Result<HttpResponse, ApiError> {
    validate_relative_path(&request.path)?;
    let transport = HTTP_TRANSPORT.with_borrow_mut(|state| {
        state.used = true;
        state
            .provider
            .clone()
            .unwrap_or_else(|| Rc::new(BrowserHttpTransport))
    });
    transport
        .send(request)
        .await
        .map_err(|error| ApiError::Network(error.message))
}

struct BrowserHttpTransport;

impl HttpTransport for BrowserHttpTransport {
    fn send(&self, request: HttpRequest) -> super::http_transport::HttpTransportFuture<'_> {
        Box::pin(async move { browser_send(request).await })
    }
}

async fn browser_send(
    request: HttpRequest,
) -> Result<HttpResponse, super::http_transport::HttpTransportError> {
    let url =
        daemon_url(&request.path).ok_or_else(|| super::http_transport::HttpTransportError {
            message: "verified daemon connection is unavailable".to_owned(),
        })?;
    let mut builder = RequestBuilder::new(&url).method(browser_method(request.method));
    if let Some(token) = authorization_token() {
        builder = builder.header("Authorization", &format!("Bearer {token}"));
    }
    for header in request.headers {
        builder = builder.header(&header.name, &header.value);
    }
    let response = match request.body {
        HttpRequestBody::Empty => builder.send().await,
        HttpRequestBody::Bytes(body) => {
            builder
                .body(Uint8Array::from(body.as_slice()))
                .map_err(|error| super::http_transport::HttpTransportError {
                    message: error.to_string(),
                })?
                .send()
                .await
        }
        HttpRequestBody::Multipart(parts) => {
            builder
                .body(browser_form_data(parts)?)
                .map_err(|error| super::http_transport::HttpTransportError {
                    message: error.to_string(),
                })?
                .send()
                .await
        }
    }
    .map_err(|error| super::http_transport::HttpTransportError {
        message: error.to_string(),
    })?;
    let status = response.status();
    let headers = response
        .headers()
        .entries()
        .map(|(name, value)| HttpHeader { name, value })
        .collect();
    let body =
        response
            .binary()
            .await
            .map_err(|error| super::http_transport::HttpTransportError {
                message: error.to_string(),
            })?;
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn browser_method(method: HttpMethod) -> Method {
    match method {
        HttpMethod::Get => Method::GET,
        HttpMethod::Head => Method::HEAD,
        HttpMethod::Post => Method::POST,
        HttpMethod::Put => Method::PUT,
        HttpMethod::Patch => Method::PATCH,
        HttpMethod::Delete => Method::DELETE,
    }
}

fn browser_form_data(
    parts: Vec<HttpMultipartPart>,
) -> Result<FormData, super::http_transport::HttpTransportError> {
    let form = FormData::new().map_err(|error| super::http_transport::HttpTransportError {
        message: format!("{error:?}"),
    })?;
    for part in parts {
        let sequence = Array::new();
        sequence.push(&Uint8Array::from(part.body.as_slice()));
        let options = BlobPropertyBag::new();
        if let Some(content_type) = part.content_type.as_deref() {
            options.set_type(content_type);
        }
        let blob =
            Blob::new_with_u8_array_sequence_and_options(&sequence, &options).map_err(|error| {
                super::http_transport::HttpTransportError {
                    message: format!("{error:?}"),
                }
            })?;
        let result = match part.file_name {
            Some(file_name) => form.append_with_blob_and_filename(&part.name, &blob, &file_name),
            None => form.append_with_blob(&part.name, &blob),
        };
        result.map_err(|error| super::http_transport::HttpTransportError {
            message: format!("{error:?}"),
        })?;
    }
    Ok(form)
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
    method: HttpMethod,
    url: &str,
    body: Option<&Req>,
    if_match: Option<u64>,
) -> ApiResult<HttpResponse>
where
    Req: Serialize + ?Sized,
{
    let mut headers = Vec::new();
    if let Some(version) = if_match {
        headers.push(HttpHeader {
            name: "If-Match".to_owned(),
            value: version.to_string(),
        });
    }
    let body = match body {
        Some(body) => {
            headers.push(HttpHeader {
                name: "Content-Type".to_owned(),
                value: "application/json".to_owned(),
            });
            HttpRequestBody::Bytes(
                serde_json::to_vec(body).map_err(|e| ApiError::Serialize(e.to_string()))?,
            )
        }
        None => HttpRequestBody::Empty,
    };
    dispatch(HttpRequest {
        method,
        path: url.to_owned(),
        headers,
        body,
    })
    .await
}

/// Unwrap the canonical [`ApiResponse`] from a successful response.
pub(crate) fn parse_envelope<Res>(resp: &HttpResponse) -> ApiResult<Res>
where
    Res: DeserializeOwned,
{
    let envelope: ApiResponse<Res> =
        serde_json::from_slice(&resp.body).map_err(|e| ApiError::Parse(e.to_string()))?;
    Ok(envelope.data)
}

/// Send a JSON request, require success, parse the envelope.
async fn send_json<Req, Res>(method: HttpMethod, url: &str, body: Option<&Req>) -> ApiResult<Res>
where
    Req: Serialize + ?Sized,
    Res: DeserializeOwned,
{
    let resp = send_request(method, url, body, None).await?;
    let resp = ensure_success(resp)?;
    parse_envelope(&resp)
}

/// Send a JSON request, require success, discard the response body.
async fn send_json_discard<Req>(method: HttpMethod, url: &str, body: Option<&Req>) -> ApiResult<()>
where
    Req: Serialize + ?Sized,
{
    let resp = send_request(method, url, body, None).await?;
    ensure_success(resp)?;
    Ok(())
}

/// Send a JSON mutation guarded by an optional `If-Match` version
/// precondition, classifying a `412` reply as [`MutationOutcome::Stale`].
///
/// On success the envelope's inner data is returned as
/// [`MutationOutcome::Applied`]. On `412` the daemon's authoritative
/// `current` version is read from the error envelope's details. Pass `None` for
/// `if_match` to apply unconditionally (the daemon then skips the
/// precondition check); a `412` is still classified if one arrives.
pub async fn send_json_versioned<Req, Res>(
    method: HttpMethod,
    url: &str,
    body: Option<&Req>,
    if_match: Option<u64>,
) -> ApiResult<MutationOutcome<Res>>
where
    Req: Serialize + ?Sized,
    Res: DeserializeOwned,
{
    let resp = send_request(method, url, body, if_match).await?;
    match resp.status {
        200..=299 => Ok(MutationOutcome::Applied(parse_envelope(&resp)?)),
        412 => {
            let body: serde_json::Value =
                serde_json::from_slice(&resp.body).map_err(|e| ApiError::Parse(e.to_string()))?;
            let current = stale_current_version(&body).ok_or_else(|| {
                ApiError::Parse(
                    "412 response missing error.details.current version token".to_owned(),
                )
            })?;
            Ok(MutationOutcome::Stale { current })
        }
        _ => Err(http_error(&resp)),
    }
}

/// Extract the authoritative version token from a `412` response body.
///
/// The daemon serves the canonical error envelope, so the version the
/// caller must rebase onto rides `error.details.current` beside the
/// `expected` version the request carried.
fn stale_current_version(body: &serde_json::Value) -> Option<u64> {
    body.pointer("/error/details/current")
        .and_then(serde_json::Value::as_u64)
}

// ── GET helpers ─────────────────────────────────────────────────────────────

/// GET `url`, unwrap the canonical [`ApiResponse`], return the inner data.
pub async fn fetch_json<T>(url: &str) -> ApiResult<T>
where
    T: DeserializeOwned,
{
    send_json::<(), T>(HttpMethod::Get, url, None).await
}

/// Page size requested from every list route. The daemon defaults to 50 and
/// rejects anything above 200, so asking for the ceiling keeps the number of
/// round trips down without tripping validation.
pub const LIST_PAGE_LIMIT: u64 = 200;

/// Build the URL for one page of a list route.
#[must_use]
pub fn paged_list_url(base_url: &str, offset: u64) -> String {
    let separator = if base_url.contains('?') { '&' } else { '?' };
    format!("{base_url}{separator}limit={LIST_PAGE_LIMIT}&offset={offset}")
}

/// GET every page of a list route, following `page.has_more` until the daemon
/// says the listing is complete.
///
/// A route that reports `page: None` is already complete, so this costs one
/// request there.
pub async fn fetch_all_pages<T>(base_url: &str) -> ApiResult<Vec<T>>
where
    T: DeserializeOwned,
{
    let mut items: Vec<T> = Vec::new();
    let mut offset = 0_u64;
    loop {
        let page: ListResponse<T> = fetch_json(&paged_list_url(base_url, offset)).await?;
        let fetched = page.items.len() as u64;
        items.extend(page.items);
        let has_more = page.page.is_some_and(|info| info.has_more);
        if !has_more || fetched == 0 {
            return Ok(items);
        }
        offset += fetched;
    }
}

/// GET `url`, returning `Ok(None)` on HTTP 404 and `Ok(Some(data))` on success.
/// All other non-2xx responses return `Err`. Used for endpoints where absence
/// is a normal state (e.g., "no active effect").
pub async fn fetch_json_optional<T>(url: &str) -> ApiResult<Option<T>>
where
    T: DeserializeOwned,
{
    let resp = send_request::<()>(HttpMethod::Get, url, None, None).await?;
    if resp.status == 404 {
        return Ok(None);
    }
    let resp = ensure_success(resp)?;
    parse_envelope(&resp).map(Some)
}

/// Probe one daemon path for its status code, without reading a body.
pub async fn head_status(url: &str) -> ApiResult<u16> {
    dispatch(HttpRequest {
        method: HttpMethod::Head,
        path: url.to_owned(),
        headers: Vec::new(),
        body: HttpRequestBody::Empty,
    })
    .await
    .map(|response| response.status)
}

/// Read one browser `File` into a transport-neutral multipart part.
pub async fn multipart_file_part(name: &str, file: &File) -> ApiResult<HttpMultipartPart> {
    let buffer = JsFuture::from(file.array_buffer())
        .await
        .map_err(|error| ApiError::Network(format!("{error:?}")))?;
    let bytes = Uint8Array::new(&buffer);
    let mut body = vec![0; bytes.length() as usize];
    bytes.copy_to(&mut body);
    let content_type = file.type_().trim().to_owned();
    Ok(HttpMultipartPart {
        name: name.to_owned(),
        file_name: Some(file.name()),
        content_type: (!content_type.is_empty()).then_some(content_type),
        body,
    })
}

pub(crate) async fn send_multipart(
    url: &str,
    parts: Vec<HttpMultipartPart>,
) -> Result<HttpResponse, ApiError> {
    dispatch(HttpRequest {
        method: HttpMethod::Post,
        path: url.to_owned(),
        headers: Vec::new(),
        body: HttpRequestBody::Multipart(parts),
    })
    .await
}

pub async fn post_multipart<Res>(url: &str, parts: Vec<HttpMultipartPart>) -> Result<Res, ApiError>
where
    Res: DeserializeOwned,
{
    let response = ensure_success(send_multipart(url, parts).await?)?;
    parse_envelope(&response)
}

// ── Write helpers that return a parsed response ─────────────────────────────

/// POST JSON body, parse envelope, return inner data.
pub async fn post_json<Req, Res>(url: &str, body: &Req) -> ApiResult<Res>
where
    Req: Serialize + ?Sized,
    Res: DeserializeOwned,
{
    send_json(HttpMethod::Post, url, Some(body)).await
}

/// PATCH JSON body, parse envelope, return inner data.
pub async fn patch_json<Req, Res>(url: &str, body: &Req) -> ApiResult<Res>
where
    Req: Serialize + ?Sized,
    Res: DeserializeOwned,
{
    send_json(HttpMethod::Patch, url, Some(body)).await
}

/// PUT JSON body, parse envelope, return inner data.
pub async fn put_json<Req, Res>(url: &str, body: &Req) -> ApiResult<Res>
where
    Req: Serialize + ?Sized,
    Res: DeserializeOwned,
{
    send_json(HttpMethod::Put, url, Some(body)).await
}

// ── Write helpers that discard the response body ────────────────────────────

/// POST with no request body, discard the response. Used for trigger actions
/// like `apply_effect` or `discover_devices`.
pub async fn post_empty(url: &str) -> ApiResult<()> {
    send_json_discard::<()>(HttpMethod::Post, url, None).await
}

/// POST JSON body, discard the response. Used for actions that send a payload
/// but don't return anything meaningful (e.g., `identify_device`, `add_favorite`).
pub async fn post_json_discard<Req>(url: &str, body: &Req) -> ApiResult<()>
where
    Req: Serialize + ?Sized,
{
    send_json_discard(HttpMethod::Post, url, Some(body)).await
}

/// PUT JSON body, discard the response. Used for idempotent actions that
/// send a payload but don't return anything (e.g., `set_config_value`).
pub async fn put_json_discard<Req>(url: &str, body: &Req) -> ApiResult<()>
where
    Req: Serialize + ?Sized,
{
    send_json_discard(HttpMethod::Put, url, Some(body)).await
}

/// PATCH JSON body, discard the response. Used for partial updates that
/// don't echo the updated resource (e.g., `update_controls`).
pub async fn patch_json_discard<Req>(url: &str, body: &Req) -> ApiResult<()>
where
    Req: Serialize + ?Sized,
{
    send_json_discard(HttpMethod::Patch, url, Some(body)).await
}

/// DELETE `url`, parse envelope, return inner data. Used for deletes that
/// echo a confirmation payload (e.g., `unpair_device`).
pub async fn delete_json<Res>(url: &str) -> ApiResult<Res>
where
    Res: DeserializeOwned,
{
    send_json::<(), Res>(HttpMethod::Delete, url, None).await
}

/// DELETE `url`, discard the response body.
pub async fn delete_empty(url: &str) -> ApiResult<()> {
    send_json_discard::<()>(HttpMethod::Delete, url, None).await
}

#[cfg(test)]
mod tests {
    use super::{
        ApiError, DaemonTransport, MutationOutcome, authorization_token,
        begin_native_daemon_verification, clear_verified_daemon_connection, daemon_url,
        extract_error_message, install_verified_daemon_connection,
        parse_dev_control_credential_fragment, reset_daemon_transport_for_test,
        stale_current_version, validate_relative_path,
    };
    use hypercolor_types::service::ProtectedControlCredential;

    #[test]
    fn native_transport_routes_relative_urls_and_preserves_absolute_urls() {
        let transport = DaemonTransport {
            native_app: true,
            base_url: Some("http://127.0.0.1:9420".to_owned()),
            protected_control_credential: None,
        };
        assert_eq!(
            transport.resolve_url("/api/v1/devices"),
            Some("http://127.0.0.1:9420/api/v1/devices".to_owned())
        );
        assert_eq!(
            transport.resolve_url("https://example.test/image.png"),
            Some("https://example.test/image.png".to_owned())
        );
    }

    #[test]
    fn dev_control_fragment_accepts_only_a_canonical_credential() {
        let expected = ProtectedControlCredential::from_bytes([0x42; 32]);

        assert_eq!(
            parse_dev_control_credential_fragment(&format!("#{}", expected.expose_secret())),
            Some(expected.expose_secret().to_owned())
        );
        assert_eq!(
            parse_dev_control_credential_fragment("#hc_pc_not-a-token"),
            None
        );
        assert_eq!(parse_dev_control_credential_fragment("hc_pc_0123"), None);
    }

    #[test]
    fn native_test_transport_has_no_ambient_dev_credential() {
        let transport = DaemonTransport::default();

        assert!(!transport.native_app);
        assert_eq!(transport.base_url, None);
        assert_eq!(transport.protected_control_credential, None);
    }

    #[test]
    fn verified_credential_precedes_public_key_and_clears_without_persistence() {
        let transport = DaemonTransport {
            native_app: true,
            base_url: None,
            protected_control_credential: Some("protected".to_owned()),
        };
        assert_eq!(
            transport.authorization_token(Some("public".to_owned())),
            Some("protected".to_owned())
        );

        begin_native_daemon_verification();
        assert_eq!(daemon_url("/api/v1/system"), None);
        install_verified_daemon_connection("http://127.0.0.1:9420", Some("protected"));
        assert!(validate_relative_path("https://attacker.example/steal").is_err());
        assert!(validate_relative_path("//attacker.example/steal").is_err());
        assert_eq!(authorization_token().as_deref(), Some("protected"));
        assert_eq!(
            daemon_url("/api/v1/system"),
            Some("http://127.0.0.1:9420/api/v1/system".to_owned())
        );
        clear_verified_daemon_connection();
        assert_eq!(authorization_token(), None);
        assert_eq!(daemon_url("/api/v1/system"), None);
        reset_daemon_transport_for_test();
    }

    #[test]
    fn stale_current_version_parses_daemon_412_body() {
        let body = serde_json::json!({
            "error": {
                "code": "precondition_failed",
                "message": "version mismatch: expected 16, current 17",
                "details": { "expected": 16, "current": 17 }
            },
            "meta": { "api_version": "1.0", "request_id": "req_x", "timestamp": "t" }
        });
        assert_eq!(stale_current_version(&body), Some(17));
    }

    #[test]
    fn stale_current_version_rejects_missing_or_nonnumeric_token() {
        assert_eq!(stale_current_version(&serde_json::json!({})), None);
        assert_eq!(
            stale_current_version(&serde_json::json!({ "error": { "details": {} } })),
            None
        );
        assert_eq!(
            stale_current_version(
                &serde_json::json!({ "error": { "details": { "current": "7" } } })
            ),
            None
        );
        // The pre-canonical shape carried `current` at the top level; it
        // is no longer a version token the daemon can send.
        assert_eq!(
            stale_current_version(&serde_json::json!({ "current": 17 })),
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
        let error = ApiError::http(409, Some("Scene is snapshot locked".to_owned()));

        assert_eq!(error.to_string(), "Scene is snapshot locked (HTTP 409)");
    }
}
