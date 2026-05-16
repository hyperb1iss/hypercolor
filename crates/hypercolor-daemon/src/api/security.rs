//! API authentication and rate limiting middleware.
//!
//! Security is enabled when API key environment variables are present:
//! - `HYPERCOLOR_API_KEY` (control tier)
//! - `HYPERCOLOR_READ_API_KEY` (read-only tier, optional)
//!
//! Read-only keys can call GET/HEAD/OPTIONS endpoints. Mutating endpoints
//! require a control-tier key.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderName, HeaderValue, Method, Request, header};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::json;
use tokio::sync::Mutex;

use crate::api::envelope::ApiError;
use hypercolor_types::config::{HypercolorConfig, NetworkConfig};

const RATE_WINDOW: Duration = Duration::from_mins(1);
const READ_LIMIT_PER_MIN: u32 = 120;
const WRITE_LIMIT_PER_MIN: u32 = 60;
const DISCOVERY_LIMIT_PER_MIN: u32 = 2;
const PAIRING_LIMIT_PER_MIN: u32 = 6;

const HEADER_RATE_LIMIT_LIMIT: HeaderName = HeaderName::from_static("x-ratelimit-limit");
const HEADER_RATE_LIMIT_REMAINING: HeaderName = HeaderName::from_static("x-ratelimit-remaining");
const HEADER_RATE_LIMIT_RESET: HeaderName = HeaderName::from_static("x-ratelimit-reset");
const HEADER_RETRY_AFTER: HeaderName = HeaderName::from_static("retry-after");

#[derive(Clone)]
pub struct SecurityState {
    auth: AuthConfig,
    network: NetworkAccessPolicy,
    rate_limiter: Arc<Mutex<RateLimiter>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RequestAuthContext {
    security_enabled: bool,
    granted_tier: Option<AccessTier>,
}

impl RequestAuthContext {
    #[must_use]
    pub(crate) const fn unsecured() -> Self {
        Self {
            security_enabled: false,
            granted_tier: None,
        }
    }

    #[must_use]
    const fn preflight() -> Self {
        Self {
            security_enabled: true,
            granted_tier: None,
        }
    }

    #[must_use]
    const fn authenticated(granted_tier: AccessTier) -> Self {
        Self {
            security_enabled: true,
            granted_tier: Some(granted_tier),
        }
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) const fn read_only() -> Self {
        Self::authenticated(AccessTier::Read)
    }

    #[must_use]
    pub(crate) const fn security_enabled(self) -> bool {
        self.security_enabled
    }

    #[must_use]
    const fn granted_tier(self) -> Option<AccessTier> {
        self.granted_tier
    }
}

impl SecurityState {
    #[must_use]
    pub fn from_env() -> Self {
        if cfg!(test) {
            return Self {
                auth: AuthConfig::default(),
                network: NetworkAccessPolicy::default(),
                rate_limiter: Arc::new(Mutex::new(RateLimiter::new())),
            };
        }

        let control_key = api_key_from_env("HYPERCOLOR_API_KEY");
        let read_key = api_key_from_env("HYPERCOLOR_READ_API_KEY");
        Self {
            auth: AuthConfig {
                control_key,
                read_key,
            },
            network: NetworkAccessPolicy::default(),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new())),
        }
    }

    #[must_use]
    pub fn from_config(config: &HypercolorConfig) -> Self {
        let mut state = Self::from_env();
        state.network = NetworkAccessPolicy::from_config(&config.network);
        state
    }

    pub(crate) fn security_enabled(&self) -> bool {
        self.auth.control_key.is_some() || self.auth.read_key.is_some()
    }
}

#[must_use]
pub fn api_auth_required_from_env() -> bool {
    let control_key = api_key_from_env("HYPERCOLOR_API_KEY");
    let read_key = api_key_from_env("HYPERCOLOR_READ_API_KEY");
    control_key.is_some() || read_key.is_some()
}

#[must_use]
pub fn control_api_key_configured_from_env() -> bool {
    api_key_from_env("HYPERCOLOR_API_KEY").is_some()
}

fn api_key_from_env(name: &str) -> Option<String> {
    normalize_api_key(std::env::var(name).ok())
}

fn normalize_api_key(value: Option<String>) -> Option<String> {
    value.filter(|key| !key.trim().is_empty())
}

#[cfg(test)]
impl SecurityState {
    pub(crate) fn with_keys(control_key: Option<&str>, read_key: Option<&str>) -> Self {
        Self {
            auth: AuthConfig {
                control_key: control_key.map(ToOwned::to_owned),
                read_key: read_key.map(ToOwned::to_owned),
            },
            network: NetworkAccessPolicy::default(),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new())),
        }
    }

    pub(crate) fn with_network_config(network: NetworkConfig) -> Self {
        Self {
            auth: AuthConfig::default(),
            network: NetworkAccessPolicy::from_config(&network),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new())),
        }
    }
}

#[derive(Clone, Default)]
struct AuthConfig {
    control_key: Option<String>,
    read_key: Option<String>,
}

#[derive(Clone, Default)]
struct NetworkAccessPolicy {
    allowed_clients: Vec<ClientAddressRule>,
    invalid_rules: Vec<String>,
}

impl NetworkAccessPolicy {
    fn from_config(config: &NetworkConfig) -> Self {
        let mut allowed_clients = Vec::new();
        let mut invalid_rules = Vec::new();

        for raw_rule in &config.allowed_clients {
            let trimmed = raw_rule.trim();
            if trimmed.is_empty() {
                continue;
            }

            match ClientAddressRule::parse(trimmed) {
                Some(rule) => allowed_clients.push(rule),
                None => invalid_rules.push(trimmed.to_owned()),
            }
        }

        Self {
            allowed_clients,
            invalid_rules,
        }
    }

    fn reject_request(&self, request: &Request<Body>) -> Option<Response> {
        if self.allowed_clients.is_empty() && self.invalid_rules.is_empty() {
            return None;
        }

        let Some(client_ip) = client_ip(request) else {
            return Some(ApiError::forbidden(
                "Client IP is required by network.allowed_clients",
            ));
        };

        if client_ip.is_loopback() {
            return None;
        }

        if !self.invalid_rules.is_empty() {
            return Some(ApiError::forbidden_with_details(
                "Invalid network.allowed_clients entries; remote clients are blocked",
                json!({ "invalid_rules": &self.invalid_rules }),
            ));
        }

        if self
            .allowed_clients
            .iter()
            .any(|rule| rule.matches(client_ip))
        {
            return None;
        }

        Some(ApiError::forbidden_with_details(
            "Client IP is not allowed by network.allowed_clients",
            json!({ "client_ip": client_ip.to_string() }),
        ))
    }
}

#[derive(Clone)]
enum ClientAddressRule {
    Exact(IpAddr),
    Cidr { network: IpAddr, prefix: u8 },
}

impl ClientAddressRule {
    fn parse(raw: &str) -> Option<Self> {
        if let Some((network, prefix)) = raw.split_once('/') {
            let network = network.parse::<IpAddr>().ok()?;
            let prefix = prefix.parse::<u8>().ok()?;
            if cidr_prefix_valid(network, prefix) {
                return Some(Self::Cidr { network, prefix });
            }
            return None;
        }

        raw.parse::<IpAddr>().ok().map(Self::Exact)
    }

    fn matches(&self, client: IpAddr) -> bool {
        match *self {
            Self::Exact(ip) => ip == client,
            Self::Cidr { network, prefix } => cidr_contains(network, prefix, client),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessTier {
    Read,
    Control,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationClass {
    Read,
    Write,
    Discovery,
    Pairing,
}

struct RateLimiter {
    clients: HashMap<String, ClientWindow>,
    discovery_window_start: Instant,
    discovery_count: u32,
}

struct ClientWindow {
    window_start: Instant,
    read_count: u32,
    write_count: u32,
    pairing_count: u32,
}

struct RateDecision {
    allowed: bool,
    limit: u32,
    remaining: u32,
    reset_epoch_secs: u64,
    retry_after_secs: u64,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            clients: HashMap::new(),
            discovery_window_start: Instant::now(),
            discovery_count: 0,
        }
    }

    fn check_and_record(&mut self, client_id: &str, class: OperationClass) -> RateDecision {
        let now = Instant::now();
        let now_epoch = unix_now_secs();
        let now_epoch_plus_window = now_epoch.saturating_add(RATE_WINDOW.as_secs());

        self.clients
            .retain(|_, window| now.saturating_duration_since(window.window_start) < RATE_WINDOW);

        if self.discovery_window_start.elapsed() >= RATE_WINDOW {
            self.discovery_window_start = now;
            self.discovery_count = 0;
        }

        if class == OperationClass::Discovery {
            if self.discovery_count >= DISCOVERY_LIMIT_PER_MIN {
                let retry_after = remaining_window_secs(self.discovery_window_start, now);
                return RateDecision {
                    allowed: false,
                    limit: DISCOVERY_LIMIT_PER_MIN,
                    remaining: 0,
                    reset_epoch_secs: now_epoch.saturating_add(retry_after),
                    retry_after_secs: retry_after,
                };
            }
            self.discovery_count = self.discovery_count.saturating_add(1);
        }

        let window = self
            .clients
            .entry(client_id.to_owned())
            .or_insert_with(|| ClientWindow {
                window_start: now,
                read_count: 0,
                write_count: 0,
                pairing_count: 0,
            });

        if window.window_start.elapsed() >= RATE_WINDOW {
            window.window_start = now;
            window.read_count = 0;
            window.write_count = 0;
            window.pairing_count = 0;
        }

        let (count_ref, limit) = match class {
            OperationClass::Read => (&mut window.read_count, READ_LIMIT_PER_MIN),
            OperationClass::Write | OperationClass::Discovery => {
                (&mut window.write_count, WRITE_LIMIT_PER_MIN)
            }
            OperationClass::Pairing => (&mut window.pairing_count, PAIRING_LIMIT_PER_MIN),
        };

        let retry_after = remaining_window_secs(window.window_start, now);
        if *count_ref >= limit {
            return RateDecision {
                allowed: false,
                limit,
                remaining: 0,
                reset_epoch_secs: now_epoch.saturating_add(retry_after),
                retry_after_secs: retry_after,
            };
        }

        *count_ref = count_ref.saturating_add(1);
        let remaining = limit.saturating_sub(*count_ref);
        let reset_epoch_secs = if retry_after == 0 {
            now_epoch_plus_window
        } else {
            now_epoch.saturating_add(retry_after)
        };

        RateDecision {
            allowed: true,
            limit,
            remaining,
            reset_epoch_secs,
            retry_after_secs: retry_after,
        }
    }
}

pub async fn enforce_security(
    State(state): State<SecurityState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let mut request = request;

    if let Some(response) = state.network.reject_request(&request) {
        return response;
    }

    if is_exempt_path(request.uri().path()) {
        request
            .extensions_mut()
            .insert(RequestAuthContext::unsecured());
        return next.run(request).await;
    }

    if !state.security_enabled() {
        request
            .extensions_mut()
            .insert(RequestAuthContext::unsecured());
        return next.run(request).await;
    }

    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let mut granted_tier = request
        .extensions()
        .get::<RequestAuthContext>()
        .copied()
        .filter(|context| context.security_enabled())
        .and_then(RequestAuthContext::granted_tier);

    if method != Method::OPTIONS {
        let required_tier = required_tier_for_method(&method);
        let granted = if let Some(granted_tier) = granted_tier {
            granted_tier
        } else {
            let Some(token) = extract_token(&request) else {
                return ApiError::unauthorized(
                    "Missing API key. Use Authorization: Bearer <token>.",
                );
            };

            let Some(granted_tier) = resolve_token_tier(&token, &state.auth) else {
                return ApiError::unauthorized("Invalid API key");
            };
            granted_tier
        };
        granted_tier = Some(granted);

        if !tier_satisfies(granted, required_tier) {
            return ApiError::forbidden_with_details(
                "Read-only API key cannot perform write operations",
                json!({
                    "required_tier": "control",
                    "current_tier": "read"
                }),
            );
        }
    }

    request.extensions_mut().insert(granted_tier.map_or_else(
        RequestAuthContext::preflight,
        RequestAuthContext::authenticated,
    ));

    let operation = classify_operation(&method, &path);
    let client_id = client_identity(&request);

    let decision = {
        let mut limiter = state.rate_limiter.lock().await;
        limiter.check_and_record(&client_id, operation)
    };

    if !decision.allowed {
        let message = rate_limit_message(operation, decision.retry_after_secs);
        let mut response = ApiError::rate_limited_with_details(
            message,
            json!({
                "limit": decision.limit,
                "window_seconds": RATE_WINDOW.as_secs(),
                "retry_after": decision.retry_after_secs
            }),
        );
        apply_rate_headers(&mut response, &decision);
        return response;
    }

    let mut response = next.run(request).await;
    apply_rate_headers(&mut response, &decision);
    response
}

fn is_exempt_path(path: &str) -> bool {
    matches!(path, "/health" | "/api/v1/server")
}

fn required_tier_for_method(method: &Method) -> AccessTier {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        AccessTier::Read
    } else {
        AccessTier::Control
    }
}

fn resolve_token_tier(token: &str, auth: &AuthConfig) -> Option<AccessTier> {
    if auth.control_key.as_deref() == Some(token) {
        if token.starts_with("hc_ak_r_") {
            Some(AccessTier::Read)
        } else {
            Some(AccessTier::Control)
        }
    } else if auth.read_key.as_deref() == Some(token) {
        Some(AccessTier::Read)
    } else {
        None
    }
}

fn tier_satisfies(granted: AccessTier, required: AccessTier) -> bool {
    matches!(
        (granted, required),
        (AccessTier::Control, _) | (AccessTier::Read, AccessTier::Read)
    )
}

fn classify_operation(method: &Method, path: &str) -> OperationClass {
    if *method == Method::POST && path == "/api/v1/devices/discover" {
        OperationClass::Discovery
    } else if is_pairing_path(path) && matches!(*method, Method::POST | Method::DELETE) {
        OperationClass::Pairing
    } else if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        OperationClass::Read
    } else {
        OperationClass::Write
    }
}

fn is_pairing_path(path: &str) -> bool {
    let mut segments = path.trim_matches('/').split('/');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
        ),
        (
            Some("api"),
            Some("v1"),
            Some("devices"),
            Some(_),
            Some("pair"),
            None,
        )
    )
}

fn rate_limit_message(class: OperationClass, retry_after: u64) -> String {
    let scope = match class {
        OperationClass::Read => "Read operation",
        OperationClass::Write => "Write operation",
        OperationClass::Discovery => "Discovery operation",
        OperationClass::Pairing => "Pairing operation",
    };
    format!("{scope} rate limit exceeded. Retry in {retry_after} seconds.")
}

fn extract_token(request: &Request<Body>) -> Option<String> {
    if let Some(raw_header) = request.headers().get(axum::http::header::AUTHORIZATION) {
        let header_value = raw_header.to_str().ok()?;
        if let Some(token) = parse_bearer_header(header_value) {
            return Some(token.to_owned());
        }
    }

    if allows_query_token(request) {
        return token_from_query(request.uri().query());
    }

    None
}

fn parse_bearer_header(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() {
        Some(token)
    } else {
        None
    }
}

fn token_from_query(query: Option<&str>) -> Option<String> {
    let query = query?;
    for pair in query.split('&') {
        let (raw_key, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        if raw_key == "token" && !raw_value.is_empty() {
            return Some(raw_value.to_owned());
        }
    }
    None
}

fn allows_query_token(request: &Request<Body>) -> bool {
    matches!(request.uri().path(), "/api/v1/ws")
        && request.method() == Method::GET
        && request
            .headers()
            .get(header::UPGRADE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

fn client_identity(request: &Request<Body>) -> String {
    client_ip(request).map_or_else(|| "unknown".to_owned(), |ip| ip.to_string())
}

fn client_ip(request: &Request<Body>) -> Option<IpAddr> {
    if let Some(ConnectInfo(socket_addr)) = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
    {
        if socket_addr.ip().is_loopback()
            && let Some(forwarded_client) = forwarded_client_ip(request)
            && let Ok(forwarded_ip) = forwarded_client.parse::<IpAddr>()
        {
            return Some(forwarded_ip);
        }
        return Some(socket_addr.ip());
    }

    None
}

fn forwarded_client_ip(request: &Request<Body>) -> Option<String> {
    if let Some(forwarded) = request.headers().get("x-forwarded-for")
        && let Ok(value) = forwarded.to_str()
        && let Some(first) = value.split(',').next()
    {
        let trimmed = first.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }

    if let Some(real_ip) = request.headers().get("x-real-ip")
        && let Ok(value) = real_ip.to_str()
    {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }

    None
}

fn apply_rate_headers(response: &mut Response, decision: &RateDecision) {
    let headers = response.headers_mut();
    insert_header(headers, HEADER_RATE_LIMIT_LIMIT, u64::from(decision.limit));
    insert_header(
        headers,
        HEADER_RATE_LIMIT_REMAINING,
        u64::from(decision.remaining),
    );
    insert_header(headers, HEADER_RATE_LIMIT_RESET, decision.reset_epoch_secs);
    if !decision.allowed {
        insert_header(headers, HEADER_RETRY_AFTER, decision.retry_after_secs);
    }
}

fn insert_header(headers: &mut axum::http::HeaderMap, name: HeaderName, value: u64) {
    if let Ok(header_value) = HeaderValue::from_str(&value.to_string()) {
        headers.insert(name, header_value);
    }
}

fn remaining_window_secs(window_start: Instant, now: Instant) -> u64 {
    let elapsed = now.saturating_duration_since(window_start);
    RATE_WINDOW.saturating_sub(elapsed).as_secs()
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn cidr_prefix_valid(network: IpAddr, prefix: u8) -> bool {
    match network {
        IpAddr::V4(_) => prefix <= 32,
        IpAddr::V6(_) => prefix <= 128,
    }
}

fn cidr_contains(network: IpAddr, prefix: u8, client: IpAddr) -> bool {
    match (network, client) {
        (IpAddr::V4(network), IpAddr::V4(client)) => {
            masked_v4(network, prefix) == masked_v4(client, prefix)
        }
        (IpAddr::V6(network), IpAddr::V6(client)) => {
            masked_v6(network, prefix) == masked_v6(client, prefix)
        }
        _ => false,
    }
}

fn masked_v4(address: Ipv4Addr, prefix: u8) -> u32 {
    let bits = u32::from(address);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    bits & mask
}

fn masked_v6(address: Ipv6Addr, prefix: u8) -> u128 {
    let bits = u128::from_be_bytes(address.octets());
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    };
    bits & mask
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use axum::extract::ConnectInfo;
    use axum::http::header::AUTHORIZATION;
    use axum::routing::{get, post};
    use axum::{Router, body::Body};
    use http::{Method, Request, StatusCode};
    use serde_json::Value;
    use tower::ServiceExt;

    use hypercolor_types::config::NetworkConfig;

    use super::{SecurityState, enforce_security, normalize_api_key};

    const CONTROL_KEY: &str = "hc_ak_control_test";
    const READ_KEY: &str = "hc_ak_r_read_test";

    fn secured_test_router() -> Router {
        let state = SecurityState::with_keys(Some(CONTROL_KEY), Some(READ_KEY));
        router_with_security_state(state)
    }

    fn router_with_security_state(state: SecurityState) -> Router {
        Router::new()
            .route("/health", get(|| async { StatusCode::OK }))
            .route("/api/v1/status", get(|| async { StatusCode::OK }))
            .route("/api/v1/ws", get(|| async { StatusCode::OK }))
            .route("/api/v1/scenes", post(|| async { StatusCode::CREATED }))
            .route(
                "/api/v1/devices/discover",
                post(|| async { StatusCode::ACCEPTED }),
            )
            .route(
                "/api/v1/devices/device-1/pair",
                post(|| async { StatusCode::OK }).delete(|| async { StatusCode::NO_CONTENT }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state,
                enforce_security,
            ))
    }

    fn allowlist_test_router(allowed_clients: Vec<String>) -> Router {
        router_with_security_state(SecurityState::with_network_config(NetworkConfig {
            allowed_clients,
            ..NetworkConfig::default()
        }))
    }

    async fn response_json(response: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("failed to read body");
        serde_json::from_slice(&bytes).expect("failed to parse JSON body")
    }

    fn with_bearer(request: http::request::Builder, token: &str) -> http::request::Builder {
        request.header(AUTHORIZATION, format!("Bearer {token}"))
    }

    fn with_connect_info(mut request: Request<Body>, ip: IpAddr, port: u16) -> Request<Body> {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(ip, port)));
        request
    }

    #[test]
    fn normalize_api_key_ignores_missing_or_blank_values() {
        assert_eq!(normalize_api_key(None), None);
        assert_eq!(normalize_api_key(Some(String::new())), None);
        assert_eq!(normalize_api_key(Some("   ".to_owned())), None);
    }

    #[test]
    fn normalize_api_key_keeps_configured_value() {
        assert_eq!(
            normalize_api_key(Some(CONTROL_KEY.to_owned())),
            Some(CONTROL_KEY.to_owned())
        );
    }

    #[tokio::test]
    async fn health_endpoint_remains_open_when_security_is_enabled() {
        let app = secured_test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("x-ratelimit-limit").is_none());
    }

    #[tokio::test]
    async fn rejects_missing_token_when_security_enabled() {
        let app = secured_test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/status")
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let json = response_json(response).await;
        assert_eq!(json["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn read_key_can_access_read_endpoint() {
        let app = secured_test_router();
        let response = app
            .oneshot(
                with_bearer(Request::builder().uri("/api/v1/status"), READ_KEY)
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-ratelimit-limit"], "120");
        assert_eq!(response.headers()["x-ratelimit-remaining"], "119");
        assert!(response.headers().contains_key("x-ratelimit-reset"));
    }

    #[tokio::test]
    async fn read_key_cannot_access_write_endpoint() {
        let app = secured_test_router();
        let response = app
            .oneshot(
                with_bearer(
                    Request::builder().method("POST").uri("/api/v1/scenes"),
                    READ_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let json = response_json(response).await;
        assert_eq!(json["error"]["code"], "forbidden");
    }

    #[tokio::test]
    async fn control_key_can_access_write_endpoint() {
        let app = secured_test_router();
        let response = app
            .oneshot(
                with_bearer(
                    Request::builder().method("POST").uri("/api/v1/scenes"),
                    CONTROL_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()["x-ratelimit-limit"], "60");
        assert_eq!(response.headers()["x-ratelimit-remaining"], "59");
    }

    #[tokio::test]
    async fn rejects_query_token_authentication_for_http_endpoints() {
        let app = secured_test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/status?token={READ_KEY}"))
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn websocket_upgrade_allows_query_token_authentication() {
        let app = secured_test_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/ws?token={READ_KEY}"))
                    .header("upgrade", "websocket")
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn network_allowlist_allows_configured_cidr() {
        let app = allowlist_test_router(vec!["192.168.1.0/24".to_owned()]);
        let response = app
            .oneshot(with_connect_info(
                Request::builder()
                    .uri("/api/v1/status")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)),
                1042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn network_allowlist_rejects_clients_outside_configured_cidr() {
        let app = allowlist_test_router(vec!["192.168.1.0/24".to_owned()]);
        let response = app
            .oneshot(with_connect_info(
                Request::builder()
                    .uri("/api/v1/status")
                    .body(Body::empty())
                    .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(192, 168, 2, 42)),
                2042,
            ))
            .await
            .expect("request failed");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let json = response_json(response).await;
        assert_eq!(json["error"]["code"], "forbidden");
    }

    #[tokio::test]
    async fn discovery_rate_limit_is_global() {
        let app = secured_test_router();

        let first = app
            .clone()
            .oneshot(with_connect_info(
                with_bearer(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/devices/discover"),
                    CONTROL_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                1001,
            ))
            .await
            .expect("first request failed");

        let second = app
            .clone()
            .oneshot(with_connect_info(
                with_bearer(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/devices/discover"),
                    CONTROL_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                1002,
            ))
            .await
            .expect("second request failed");

        let third = app
            .oneshot(with_connect_info(
                with_bearer(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/devices/discover"),
                    CONTROL_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3)),
                1003,
            ))
            .await
            .expect("third request failed");

        assert_eq!(first.status(), StatusCode::ACCEPTED);
        assert_eq!(second.status(), StatusCode::ACCEPTED);
        assert_eq!(third.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(third.headers()["x-ratelimit-limit"], "2");
        assert_eq!(third.headers()["x-ratelimit-remaining"], "0");
        assert!(third.headers().contains_key("retry-after"));

        let json = response_json(third).await;
        assert_eq!(json["error"]["code"], "rate_limited");
    }

    #[tokio::test]
    async fn pairing_rate_limit_is_scoped_per_client() {
        let app = secured_test_router();

        for _ in 0..super::PAIRING_LIMIT_PER_MIN {
            let response = app
                .clone()
                .oneshot(with_connect_info(
                    with_bearer(
                        Request::builder()
                            .method("POST")
                            .uri("/api/v1/devices/device-1/pair"),
                        CONTROL_KEY,
                    )
                    .body(Body::empty())
                    .expect("failed to build request"),
                    IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10)),
                    1010,
                ))
                .await
                .expect("pairing request failed");
            assert_eq!(response.status(), StatusCode::OK);
        }

        let limited = app
            .oneshot(with_connect_info(
                with_bearer(
                    Request::builder()
                        .method("DELETE")
                        .uri("/api/v1/devices/device-1/pair"),
                    CONTROL_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10)),
                1010,
            ))
            .await
            .expect("limited pairing request failed");

        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(limited.headers()["x-ratelimit-limit"], "6");
        assert_eq!(limited.headers()["x-ratelimit-remaining"], "0");
    }

    #[test]
    fn rate_limiter_evicts_stale_clients() {
        let mut limiter = super::RateLimiter::new();
        limiter.clients.insert(
            "stale".to_owned(),
            super::ClientWindow {
                window_start: std::time::Instant::now()
                    .checked_sub(super::RATE_WINDOW + std::time::Duration::from_secs(1))
                    .expect("duration should be representable"),
                read_count: 1,
                write_count: 0,
                pairing_count: 0,
            },
        );

        let decision = limiter.check_and_record("fresh", super::OperationClass::Read);

        assert!(decision.allowed);
        assert!(!limiter.clients.contains_key("stale"));
        assert!(limiter.clients.contains_key("fresh"));
    }

    #[test]
    fn nonexistent_bulk_route_is_treated_as_write() {
        assert_eq!(
            super::classify_operation(&Method::POST, "/api/v1/bulk"),
            super::OperationClass::Write
        );
    }

    #[test]
    fn generic_pair_route_is_classified_as_pairing() {
        assert_eq!(
            super::classify_operation(&Method::POST, "/api/v1/devices/abc123/pair"),
            super::OperationClass::Pairing
        );
        assert_eq!(
            super::classify_operation(&Method::DELETE, "/api/v1/devices/abc123/pair"),
            super::OperationClass::Pairing
        );
    }

    #[test]
    fn forwarded_headers_are_ignored_for_non_loopback_peers() {
        let request = Request::builder()
            .uri("/api/v1/status")
            .header("x-forwarded-for", "203.0.113.50")
            .body(Body::empty())
            .expect("failed to build request");
        let request = with_connect_info(request, IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)), 9420);

        assert_eq!(super::client_identity(&request), "10.1.2.3");
    }

    #[test]
    fn forwarded_headers_are_honored_for_loopback_proxy_peers() {
        let request = Request::builder()
            .uri("/api/v1/status")
            .header("x-forwarded-for", "203.0.113.50")
            .body(Body::empty())
            .expect("failed to build request");
        let request = with_connect_info(request, IpAddr::V4(Ipv4Addr::LOCALHOST), 9420);

        assert_eq!(super::client_identity(&request), "203.0.113.50");
    }
}
