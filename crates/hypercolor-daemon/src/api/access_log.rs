//! HTTP access-log middleware.
//!
//! Emits one `tracing` event per completed request with method, path, status,
//! latency, and client address. The log level tracks response status so
//! operators can spot failures at a glance:
//!
//! - `5xx` → `ERROR`
//! - `4xx` → `WARN`
//! - `2xx` / `3xx` → `INFO` (demoted to `DEBUG` for `/health` so systemd and
//!   orchestrator probes don't drown out real traffic)
//!
//! Query strings are logged, but `token` values are redacted: WebSocket
//! upgrades authenticate via `?token=...` and plaintext keys must never hit
//! stdout or log files.
//!
//! Mounted as the outermost layer so it sees the final response from CORS,
//! auth, and every handler. WebSocket upgrades produce a single `101` entry;
//! post-upgrade frame traffic is not HTTP and is not logged here.

use std::net::SocketAddr;
use std::time::Instant;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, header};
use axum::middleware::Next;
use axum::response::Response;
use tracing::Level;

pub async fn log_access(request: Request<Body>, next: Next) -> Response {
    let start = Instant::now();
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let query = request.uri().query().map(redact_sensitive_query);
    let remote = client_addr(&request).unwrap_or_else(|| "unknown".to_owned());
    let user_agent = request
        .headers()
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();

    let response = next.run(request).await;
    let status = response.status().as_u16();
    let latency_ms = start.elapsed().as_secs_f64() * 1000.0;

    emit(
        select_level(status, &path),
        &method,
        &path,
        query.as_deref().unwrap_or(""),
        status,
        latency_ms,
        &remote,
        &user_agent,
    );

    response
}

#[allow(clippy::too_many_arguments)]
fn emit(
    level: Level,
    method: &Method,
    path: &str,
    query: &str,
    status: u16,
    latency_ms: f64,
    remote: &str,
    user_agent: &str,
) {
    // `tracing::event!` bakes the level into a static callsite, so the level
    // must be a compile-time constant. Dispatching manually gives every arm
    // its own callsite while keeping field layout identical.
    macro_rules! log_access_event {
        ($mac:ident) => {
            tracing::$mac!(
                method = %method,
                path,
                query,
                status,
                latency_ms,
                remote,
                user_agent,
                "http"
            )
        };
    }
    match level {
        Level::ERROR => log_access_event!(error),
        Level::WARN => log_access_event!(warn),
        Level::INFO => log_access_event!(info),
        Level::DEBUG => log_access_event!(debug),
        Level::TRACE => log_access_event!(trace),
    }
}

fn select_level(status: u16, path: &str) -> Level {
    if status >= 500 {
        Level::ERROR
    } else if status >= 400 {
        Level::WARN
    } else if matches!(path, "/health") {
        Level::DEBUG
    } else {
        Level::INFO
    }
}

fn client_addr(request: &Request<Body>) -> Option<String> {
    let ConnectInfo(socket_addr) = request.extensions().get::<ConnectInfo<SocketAddr>>()?;

    if socket_addr.ip().is_loopback()
        && let Some(forwarded) = forwarded_ip(request)
    {
        return Some(forwarded);
    }

    Some(socket_addr.ip().to_string())
}

fn forwarded_ip(request: &Request<Body>) -> Option<String> {
    let headers = request.headers();

    if let Some(raw) = headers.get("x-forwarded-for")
        && let Ok(value) = raw.to_str()
        && let Some(first) = value.split(',').next()
    {
        let trimmed = first.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }

    if let Some(raw) = headers.get("x-real-ip")
        && let Ok(value) = raw.to_str()
    {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }

    None
}

fn redact_sensitive_query(query: &str) -> String {
    query
        .split('&')
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            if key.eq_ignore_ascii_case("token") && !value.is_empty() {
                format!("{key}=***")
            } else {
                pair.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use tracing::Level;

    use super::{client_addr, redact_sensitive_query, select_level};

    #[test]
    fn redacts_token_query_parameter() {
        assert_eq!(
            redact_sensitive_query("token=hc_ak_control_secret"),
            "token=***"
        );
    }

    #[test]
    fn redacts_token_mixed_with_other_params() {
        assert_eq!(
            redact_sensitive_query("foo=bar&token=secret&baz=qux"),
            "foo=bar&token=***&baz=qux"
        );
    }

    #[test]
    fn redacts_token_case_insensitive() {
        assert_eq!(redact_sensitive_query("Token=secret"), "Token=***");
    }

    #[test]
    fn preserves_non_sensitive_params() {
        assert_eq!(
            redact_sensitive_query("limit=10&cursor=abc"),
            "limit=10&cursor=abc"
        );
    }

    #[test]
    fn empty_token_value_is_left_alone() {
        assert_eq!(redact_sensitive_query("token="), "token=");
    }

    #[test]
    fn bare_flags_without_values_are_preserved() {
        assert_eq!(redact_sensitive_query("debug&verbose"), "debug&verbose");
    }

    #[test]
    fn level_scales_with_status() {
        assert_eq!(select_level(200, "/api/v1/effects"), Level::INFO);
        assert_eq!(select_level(302, "/api/v1/effects"), Level::INFO);
        assert_eq!(select_level(404, "/api/v1/effects"), Level::WARN);
        assert_eq!(select_level(500, "/api/v1/effects"), Level::ERROR);
    }

    #[test]
    fn health_probes_log_at_debug() {
        assert_eq!(select_level(200, "/health"), Level::DEBUG);
    }

    #[test]
    fn health_errors_still_escalate() {
        assert_eq!(select_level(503, "/health"), Level::ERROR);
        assert_eq!(select_level(401, "/health"), Level::WARN);
    }

    fn request_with_connect_info(ip: IpAddr) -> Request<Body> {
        let mut request = Request::builder()
            .uri("/api/v1/status")
            .body(Body::empty())
            .expect("request should build");
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(ip, 9420)));
        request
    }

    #[test]
    fn client_addr_uses_connect_info_for_non_loopback_peers() {
        let request = request_with_connect_info(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)));
        assert_eq!(client_addr(&request).as_deref(), Some("10.1.2.3"));
    }

    #[test]
    fn client_addr_trusts_forwarded_headers_only_for_loopback_peers() {
        let mut request = request_with_connect_info(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)));
        request.headers_mut().insert(
            "x-forwarded-for",
            "203.0.113.50".parse().expect("header value parses"),
        );
        assert_eq!(client_addr(&request).as_deref(), Some("10.1.2.3"));
    }

    #[test]
    fn client_addr_honors_forwarded_for_when_peer_is_loopback() {
        let mut request = request_with_connect_info(IpAddr::V4(Ipv4Addr::LOCALHOST));
        request.headers_mut().insert(
            "x-forwarded-for",
            "203.0.113.50, 10.0.0.1"
                .parse()
                .expect("header value parses"),
        );
        assert_eq!(client_addr(&request).as_deref(), Some("203.0.113.50"));
    }

    #[test]
    fn client_addr_falls_back_to_x_real_ip() {
        let mut request = request_with_connect_info(IpAddr::V4(Ipv4Addr::LOCALHOST));
        request.headers_mut().insert(
            "x-real-ip",
            "198.51.100.7".parse().expect("header value parses"),
        );
        assert_eq!(client_addr(&request).as_deref(), Some("198.51.100.7"));
    }

    #[test]
    fn client_addr_is_none_without_connect_info() {
        let request = Request::builder()
            .uri("/api/v1/status")
            .body(Body::empty())
            .expect("request should build");
        assert!(client_addr(&request).is_none());
    }
}
