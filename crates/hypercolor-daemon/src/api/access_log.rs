//! HTTP access-log and audit middleware.
//!
//! Emits one `tracing` event per completed request with method, path, status,
//! latency, and client address. The log level tracks response status so
//! operators can spot failures at a glance:
//!
//! - `5xx` → `ERROR`
//! - `4xx` → `WARN`
//! - `2xx` / `3xx` → `INFO` (demoted to `DEBUG` for `/health` and high-volume
//!   UI polling reads so systemd and app refresh loops don't drown out real
//!   traffic)
//!
//! Query strings are logged, but `token` values are redacted: WebSocket
//! upgrades authenticate via `?token=...` and plaintext keys must never hit
//! stdout or log files.
//!
//! State-changing requests (POST, PUT, PATCH, DELETE) also produce an audit
//! entry (see [`crate::audit_log`]) naming the durable stores they changed.
//! The MCP mount is skipped here because MCP carries reads and writes over
//! the same POST; its tool calls are audited one by one instead.
//!
//! Mounted as the outermost layer so it sees the final response from CORS,
//! auth, and every handler. WebSocket upgrades produce a single `101` entry;
//! post-upgrade frame traffic is not HTTP and is not logged here, except for
//! `command` messages, which replay through this router carrying the
//! session's [`AuditCaller`].
//!
//! The access log trusts `X-Forwarded-For` / `X-Real-IP` from loopback peers,
//! for local reverse proxies. The audit trail does not: it records the
//! socket peer as `remote` and any forwarded value, verbatim and capped, as
//! `forwarded_for`, so a local process cannot put another address in the
//! `remote` field.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, Method, Request, header};
use axum::middleware::Next;
use axum::response::Response;
use hypercolor_types::api::system::AuditTransport;
use tracing::Level;

use crate::audit_log::{self, AuditLog, AuditPeer, RequestLine};

/// Middleware state: the audit sink and the path it leaves to MCP.
#[derive(Debug, Clone, Default)]
pub struct AccessLogState {
    /// The daemon's audit trail; tracing only when absent.
    pub audit: Option<Arc<AuditLog>>,
    /// MCP mount path, audited per tool call rather than per POST.
    pub mcp_path: Option<String>,
}

/// Who sent a request that has no socket of its own: a WebSocket `command`
/// replayed through the router, or an in-process trusted call.
#[derive(Debug, Clone)]
pub struct AuditCaller {
    pub transport: AuditTransport,
    pub peer: Arc<AuditPeer>,
}

pub async fn log_access(
    State(state): State<AccessLogState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let start = Instant::now();
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let query = request.uri().query().map(redact_sensitive_query);
    let caller = request.extensions().get::<AuditCaller>().cloned();
    let (remote, user_agent) = match &caller {
        Some(caller) => (caller.peer.remote.clone(), caller.peer.user_agent.clone()),
        None => (
            client_addr(&request).unwrap_or_else(|| "unknown".to_owned()),
            user_agent(request.headers()),
        ),
    };
    let mutating = audit_log::is_mutating(&method);
    let mcp = is_mcp_path(state.mcp_path.as_deref(), &path);
    let peer = caller.as_ref().map_or_else(
        || {
            peer_identity(
                request
                    .extensions()
                    .get::<ConnectInfo<SocketAddr>>()
                    .map(|ConnectInfo(address)| *address),
                request.headers(),
            )
        },
        |caller| caller.peer.as_ref().clone(),
    );

    let (response, changed) = if mutating && !mcp {
        audit_log::collect_changes(next.run(request)).await
    } else {
        (next.run(request).await, Vec::new())
    };
    let status = response.status().as_u16();
    let latency_ms = start.elapsed().as_secs_f64() * 1000.0;
    let audited = should_audit(mutating, mcp, status);

    emit(
        select_level(status, &method, &path),
        &method,
        &path,
        query.as_deref().unwrap_or(""),
        status,
        latency_ms,
        &remote,
        &user_agent,
    );

    if audited {
        let transport = caller
            .as_ref()
            .map_or(AuditTransport::Http, |caller| caller.transport);
        let entry = audit_log::entry(
            state.audit.as_deref(),
            transport,
            RequestLine {
                method: method.as_str(),
                path: &path,
                tool: None,
                peer: &peer,
            },
            status,
            &changed,
            latency_ms,
        );
        audit_log::record(state.audit.as_deref(), &entry);
    }

    response
}

/// MCP tool calls are audited where they run. A mutating request to the MCP
/// mount that the security layer turns away never reaches a tool, so it is
/// recorded here instead.
const fn should_audit(mutating: bool, mcp: bool, status: u16) -> bool {
    mutating && (!mcp || matches!(status, 401 | 403))
}

fn is_mcp_path(mcp_path: Option<&str>, path: &str) -> bool {
    mcp_path.is_some_and(|base| {
        path == base
            || path
                .strip_prefix(base)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

fn user_agent(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

/// The audit identity of a connection: its socket address, whatever it
/// claims through forwarding headers, and its user agent.
#[must_use]
pub fn peer_identity(socket_addr: Option<SocketAddr>, headers: &HeaderMap) -> AuditPeer {
    let forwarded = ["x-forwarded-for", "x-real-ip"]
        .into_iter()
        .filter_map(|name| headers.get(name)?.to_str().ok())
        .map(str::trim)
        .find(|value| !value.is_empty());
    AuditPeer::new(
        socket_addr.map_or_else(|| "unknown".to_owned(), |address| address.ip().to_string()),
        forwarded,
        &user_agent(headers),
    )
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

fn select_level(status: u16, method: &Method, path: &str) -> Level {
    if status >= 500 {
        Level::ERROR
    } else if status >= 400 {
        Level::WARN
    } else if quiet_success_request(method, path) {
        Level::DEBUG
    } else {
        Level::INFO
    }
}

fn quiet_success_request(method: &Method, path: &str) -> bool {
    if matches!(path, "/health") {
        return true;
    }

    if method != Method::GET {
        return false;
    }

    matches!(path, "/api/v1/scene")
}

fn client_addr(request: &Request<Body>) -> Option<String> {
    let ConnectInfo(socket_addr) = request.extensions().get::<ConnectInfo<SocketAddr>>()?;
    Some(remote_from(*socket_addr, request.headers()))
}

fn remote_from(socket_addr: SocketAddr, headers: &HeaderMap) -> String {
    if socket_addr.ip().is_loopback()
        && let Some(forwarded) = forwarded_ip(headers)
    {
        return forwarded;
    }

    socket_addr.ip().to_string()
}

fn forwarded_ip(headers: &HeaderMap) -> Option<String> {
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
    use axum::http::{Method, Request};
    use tracing::Level;

    use super::{
        client_addr, is_mcp_path, peer_identity, redact_sensitive_query, select_level, should_audit,
    };

    #[test]
    fn the_mcp_mount_and_its_children_are_left_to_tool_auditing() {
        assert!(is_mcp_path(Some("/mcp"), "/mcp"));
        assert!(is_mcp_path(Some("/mcp"), "/mcp/session"));
        assert!(!is_mcp_path(Some("/mcp"), "/mcpx"));
        assert!(!is_mcp_path(Some("/mcp"), "/api/v1/scenes"));
        assert!(!is_mcp_path(None, "/mcp"));
    }

    #[test]
    fn rejected_mcp_writes_are_audited_and_accepted_ones_left_to_tools() {
        assert!(should_audit(true, false, 200));
        assert!(!should_audit(false, false, 200));
        assert!(!should_audit(true, true, 200));
        assert!(should_audit(true, true, 401));
        assert!(should_audit(true, true, 403));
        assert!(!should_audit(false, true, 401));
    }

    #[test]
    fn the_audit_peer_is_the_socket_and_forwarding_is_only_a_claim() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "203.0.113.50".parse().expect("header value parses"),
        );
        headers.insert(
            axum::http::header::USER_AGENT,
            "x".repeat(1000).parse().expect("header value parses"),
        );
        let peer = peer_identity(
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9420)),
            &headers,
        );
        assert_eq!(peer.remote, "127.0.0.1");
        assert_eq!(peer.forwarded_for.as_deref(), Some("203.0.113.50"));
        assert_eq!(
            peer.user_agent.len(),
            crate::audit_log::MAX_CLIENT_FIELD_CHARS
        );
        assert_eq!(peer_identity(None, &headers).remote, "unknown");
    }

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
        assert_eq!(
            select_level(200, &Method::GET, "/api/v1/effects"),
            Level::INFO
        );
        assert_eq!(
            select_level(302, &Method::GET, "/api/v1/effects"),
            Level::INFO
        );
        assert_eq!(
            select_level(404, &Method::GET, "/api/v1/effects"),
            Level::WARN
        );
        assert_eq!(
            select_level(500, &Method::GET, "/api/v1/effects"),
            Level::ERROR
        );
    }

    #[test]
    fn health_probes_log_at_debug() {
        assert_eq!(select_level(200, &Method::GET, "/health"), Level::DEBUG);
    }

    #[test]
    fn health_errors_still_escalate() {
        assert_eq!(select_level(503, &Method::GET, "/health"), Level::ERROR);
        assert_eq!(select_level(401, &Method::GET, "/health"), Level::WARN);
    }

    #[test]
    fn live_scene_reads_log_at_debug() {
        assert_eq!(
            select_level(200, &Method::GET, "/api/v1/scene"),
            Level::DEBUG
        );
    }

    #[test]
    fn live_scene_layer_writes_still_log_at_info() {
        assert_eq!(
            select_level(
                200,
                &Method::DELETE,
                "/api/v1/scene/zones/zone-id/layers/layer-id"
            ),
            Level::INFO
        );
    }

    fn request_with_connect_info(ip: IpAddr) -> Request<Body> {
        let mut request = Request::builder()
            .uri("/api/v1/system")
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
            .uri("/api/v1/system")
            .body(Body::empty())
            .expect("request should build");
        assert!(client_addr(&request).is_none());
    }
}
