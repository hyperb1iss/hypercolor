//! Audit trail contracts for state-changing daemon requests.

use serde::{Deserialize, Serialize};

/// Transport a state-changing request arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum AuditTransport {
    /// A REST request.
    Http,
    /// A REST-equivalent `command` message on the WebSocket.
    Websocket,
    /// An MCP tool call that is not read-only.
    Mcp,
}

/// One state-changing request, as the daemon recorded it.
///
/// Entries never carry request bodies, query strings, headers other than
/// the user agent, or tool arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct AuditEntry {
    /// When the request finished, RFC 3339 UTC with milliseconds.
    pub timestamp: String,
    pub transport: AuditTransport,
    /// HTTP method, or `tools/call` for MCP.
    pub method: String,
    /// Request path without its query string, or the MCP mount path.
    pub path: String,
    /// MCP tool name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Response status. MCP tool calls report 200 for success and the
    /// closest HTTP status for a tool error (400, 404, 409, or 500).
    pub status: u16,
    /// The socket peer's address, `in-process` for trusted in-process
    /// calls, or `unknown` when the transport has none. Never read from a
    /// header.
    pub remote: String,
    /// What the client claimed through `X-Forwarded-For` or `X-Real-IP`,
    /// verbatim and capped at 256 characters. Only as trustworthy as the
    /// process that sent it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarded_for: Option<String>,
    /// Client `User-Agent`, capped at 256 characters, empty when absent.
    pub user_agent: String,
    /// Durable stores whose bytes this request changed, by inventory name.
    #[serde(default)]
    pub stores: Vec<String>,
    /// Handling time in milliseconds.
    pub latency_ms: f64,
}

/// Query parameters for `GET /api/v1/system/audit`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema, utoipa::IntoParams))]
pub struct AuditLogQuery {
    /// Most entries to return, newest first. Defaults to 100, at most 1000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}
