//! API authentication and rate limiting middleware.
//!
//! Security is enabled when API key environment variables are present:
//! - `HYPERCOLOR_API_KEY` (control tier)
//! - `HYPERCOLOR_READ_API_KEY` (read-only tier, optional)
//!
//! Read-only keys can call GET/HEAD/OPTIONS endpoints. Mutating endpoints
//! require a control-tier key.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderName, HeaderValue, Method, Request};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::json;
use tokio::sync::Mutex;

use crate::api::envelope::ApiError;

const RATE_WINDOW: Duration = Duration::from_secs(60);
const READ_LIMIT_PER_MIN: u32 = 120;
const WRITE_LIMIT_PER_MIN: u32 = 60;
const DISCOVERY_LIMIT_PER_MIN: u32 = 2;

const HEADER_RATE_LIMIT_LIMIT: HeaderName = HeaderName::from_static("x-ratelimit-limit");
const HEADER_RATE_LIMIT_REMAINING: HeaderName = HeaderName::from_static("x-ratelimit-remaining");
const HEADER_RATE_LIMIT_RESET: HeaderName = HeaderName::from_static("x-ratelimit-reset");
const HEADER_RETRY_AFTER: HeaderName = HeaderName::from_static("retry-after");

#[derive(Clone)]
pub struct SecurityState {
    auth: AuthConfig,
    rate_limiter: Arc<Mutex<RateLimiter>>,
}

impl SecurityState {
    #[must_use]
    pub fn from_env() -> Self {
        if cfg!(test) {
            return Self {
                auth: AuthConfig::default(),
                rate_limiter: Arc::new(Mutex::new(RateLimiter::new())),
            };
        }

        let control_key = std::env::var("HYPERCOLOR_API_KEY").ok();
        let read_key = std::env::var("HYPERCOLOR_READ_API_KEY").ok();
        Self {
            auth: AuthConfig {
                control_key,
                read_key,
            },
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new())),
        }
    }

    fn security_enabled(&self) -> bool {
        self.auth.control_key.is_some() || self.auth.read_key.is_some()
    }
}

#[must_use]
pub fn api_auth_required_from_env() -> bool {
    let control_key = std::env::var("HYPERCOLOR_API_KEY").ok();
    let read_key = std::env::var("HYPERCOLOR_READ_API_KEY").ok();
    control_key.is_some() || read_key.is_some()
}

#[must_use]
pub fn control_api_key_configured_from_env() -> bool {
    std::env::var("HYPERCOLOR_API_KEY").ok().is_some()
}

#[cfg(test)]
impl SecurityState {
    fn with_keys(control_key: Option<&str>, read_key: Option<&str>) -> Self {
        Self {
            auth: AuthConfig {
                control_key: control_key.map(ToOwned::to_owned),
                read_key: read_key.map(ToOwned::to_owned),
            },
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new())),
        }
    }
}

#[derive(Clone, Default)]
struct AuthConfig {
    control_key: Option<String>,
    read_key: Option<String>,
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
            });

        if window.window_start.elapsed() >= RATE_WINDOW {
            window.window_start = now;
            window.read_count = 0;
            window.write_count = 0;
        }

        let (count_ref, limit) = match class {
            OperationClass::Read => (&mut window.read_count, READ_LIMIT_PER_MIN),
            OperationClass::Write | OperationClass::Discovery => {
                (&mut window.write_count, WRITE_LIMIT_PER_MIN)
            }
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
    if is_exempt_path(request.uri().path()) {
        return next.run(request).await;
    }

    if !state.security_enabled() {
        return next.run(request).await;
    }

    let method = request.method().clone();
    let path = request.uri().path().to_owned();

    if method != Method::OPTIONS {
        let required_tier = required_tier_for_method(&method);
        let Some(token) = extract_token(&request) else {
            return ApiError::unauthorized("Missing API key. Use Authorization: Bearer <token>.");
        };

        let Some(granted_tier) = resolve_token_tier(&token, &state.auth) else {
            return ApiError::unauthorized("Invalid API key");
        };

        if !tier_satisfies(granted_tier, required_tier) {
            return ApiError::forbidden_with_details(
                "Read-only API key cannot perform write operations",
                json!({
                    "required_tier": "control",
                    "current_tier": "read"
                }),
            );
        }
    }

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
    } else if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        OperationClass::Read
    } else {
        OperationClass::Write
    }
}

fn rate_limit_message(class: OperationClass, retry_after: u64) -> String {
    let scope = match class {
        OperationClass::Read => "Read operation",
        OperationClass::Write => "Write operation",
        OperationClass::Discovery => "Discovery operation",
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

    token_from_query(request.uri().query())
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

fn client_identity(request: &Request<Body>) -> String {
    if let Some(forwarded) = request.headers().get("x-forwarded-for")
        && let Ok(value) = forwarded.to_str()
        && let Some(first) = value.split(',').next()
    {
        let trimmed = first.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }

    if let Some(real_ip) = request.headers().get("x-real-ip")
        && let Ok(value) = real_ip.to_str()
    {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }

    if let Some(ConnectInfo(socket_addr)) = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
    {
        return socket_addr.ip().to_string();
    }

    "unknown".to_owned()
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

#[cfg(test)]
mod tests {
    use axum::http::header::AUTHORIZATION;
    use axum::routing::{get, post};
    use axum::{Router, body::Body};
    use http::{Method, Request, StatusCode};
    use serde_json::Value;
    use tower::ServiceExt;

    use super::{SecurityState, enforce_security};

    const CONTROL_KEY: &str = "hc_ak_control_test";
    const READ_KEY: &str = "hc_ak_r_read_test";

    fn secured_test_router() -> Router {
        let state = SecurityState::with_keys(Some(CONTROL_KEY), Some(READ_KEY));

        Router::new()
            .route("/health", get(|| async { StatusCode::OK }))
            .route("/api/v1/status", get(|| async { StatusCode::OK }))
            .route("/api/v1/scenes", post(|| async { StatusCode::CREATED }))
            .route(
                "/api/v1/devices/discover",
                post(|| async { StatusCode::ACCEPTED }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state,
                enforce_security,
            ))
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
    async fn supports_query_token_authentication() {
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

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn discovery_rate_limit_is_global() {
        let app = secured_test_router();

        let first = app
            .clone()
            .oneshot(
                with_bearer(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/devices/discover")
                        .header("x-forwarded-for", "10.0.0.1"),
                    CONTROL_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
            )
            .await
            .expect("first request failed");

        let second = app
            .clone()
            .oneshot(
                with_bearer(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/devices/discover")
                        .header("x-forwarded-for", "10.0.0.2"),
                    CONTROL_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
            )
            .await
            .expect("second request failed");

        let third = app
            .oneshot(
                with_bearer(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/devices/discover")
                        .header("x-forwarded-for", "10.0.0.3"),
                    CONTROL_KEY,
                )
                .body(Body::empty())
                .expect("failed to build request"),
            )
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

    #[test]
    fn rate_limiter_evicts_stale_clients() {
        let mut limiter = super::RateLimiter::new();
        limiter.clients.insert(
            "stale".to_owned(),
            super::ClientWindow {
                window_start: std::time::Instant::now()
                    - super::RATE_WINDOW
                    - std::time::Duration::from_secs(1),
                read_count: 1,
                write_count: 0,
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
}
