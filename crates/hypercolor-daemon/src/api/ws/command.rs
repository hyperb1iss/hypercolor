//! WebSocket command dispatch — REST-equivalent commands over the socket.
//!
//! Client messages of type `command` get routed through the normal Axum router
//! by injecting a synthetic request and relaying the response through the
//! `Response` server message.

use std::sync::Arc;

use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use serde_json::json;
use tower::ServiceExt;

use super::cache::cached_command_router;
use super::protocol::{ServerMessage, WsProtocolError};
use crate::api::security::{CredentialGrant, RequestAuthContext};
use crate::app_state::AppState;

/// Maximum WebSocket command response body we buffer before relaying to the client.
/// Guards against memory exhaustion from crafted or runaway handler responses.
const WS_COMMAND_BODY_MAX: usize = 1024 * 1024;

pub(super) async fn dispatch_command(
    state: &Arc<AppState>,
    auth_context: RequestAuthContext,
    grant: Option<CredentialGrant>,
    id: String,
    method_raw: String,
    path_raw: String,
    body: Option<serde_json::Value>,
) -> ServerMessage {
    let method = match parse_command_method(&method_raw) {
        Ok(method) => method,
        Err(error) => {
            return ServerMessage::Response {
                id,
                status: 400,
                data: None,
                error: Some(protocol_error_json(error)),
            };
        }
    };
    let path = match normalize_command_path(&path_raw) {
        Ok(path) => path,
        Err(error) => {
            return ServerMessage::Response {
                id,
                status: 400,
                data: None,
                error: Some(protocol_error_json(error)),
            };
        }
    };

    let body_bytes = match body {
        Some(payload) => serde_json::to_vec(&payload).unwrap_or_default(),
        None => Vec::new(),
    };

    let mut request_builder = Request::builder().method(method).uri(path);
    if !body_bytes.is_empty() {
        request_builder = request_builder.header(header::CONTENT_TYPE, "application/json");
    }

    let mut request = match request_builder.body(axum::body::Body::from(body_bytes)) {
        Ok(request) => request,
        Err(error) => {
            return ServerMessage::Response {
                id,
                status: 400,
                data: None,
                error: Some(protocol_error_json(WsProtocolError::invalid_request(
                    format!("Invalid command request: {error}"),
                ))),
            };
        }
    };
    request.extensions_mut().insert(auth_context);
    request
        .extensions_mut()
        .insert(crate::api::security::AdmittedSessionReplay);
    // The credential that opened the session rides along, so a handler
    // sees who called and the middleware refuses it once revoked.
    if let Some(grant) = grant {
        request.extensions_mut().insert(grant);
    }
    // The replayed request has no socket of its own; the audit trail names
    // the session's client instead.
    if let Some(peer) = crate::audit_log::current_ws_peer() {
        request
            .extensions_mut()
            .insert(crate::api::access_log::AuditCaller {
                transport: hypercolor_types::api::system::AuditTransport::Websocket,
                peer,
            });
    }

    let response = cached_command_router(state)
        .oneshot(request)
        .await
        .unwrap_or_else(|never| match never {});

    command_response_from_http(id, response).await
}

pub(super) fn parse_command_method(method_raw: &str) -> Result<Method, WsProtocolError> {
    let method = Method::from_bytes(method_raw.trim().as_bytes()).map_err(|_| {
        WsProtocolError::invalid_request("command.method must be a valid HTTP verb")
    })?;

    if matches!(
        method,
        Method::GET | Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        Ok(method)
    } else {
        Err(WsProtocolError::invalid_request(
            "command.method must be one of GET, POST, PUT, PATCH, DELETE",
        ))
    }
}

pub(super) fn normalize_command_path(path_raw: &str) -> Result<String, WsProtocolError> {
    let path = path_raw.trim();
    if path.is_empty() {
        return Err(WsProtocolError::invalid_request(
            "command.path must not be empty",
        ));
    }
    if !path.starts_with('/') {
        return Err(WsProtocolError::invalid_request(
            "command.path must start with '/'",
        ));
    }
    if path.starts_with("/api/v1") {
        return Ok(path.to_owned());
    }
    Ok(format!("/api/v1{path}"))
}

pub(super) async fn command_response_from_http(id: String, response: Response) -> ServerMessage {
    let status = response.status().as_u16();
    let body = response.into_body();
    let bytes = match axum::body::to_bytes(body, WS_COMMAND_BODY_MAX).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return ServerMessage::Response {
                id,
                status: 502,
                data: None,
                error: Some(protocol_error_json(WsProtocolError::invalid_request(
                    format!("Command response body exceeded {WS_COMMAND_BODY_MAX} bytes: {error}"),
                ))),
            };
        }
    };
    let parsed = serde_json::from_slice::<serde_json::Value>(&bytes).ok();

    if (200..300).contains(&status) {
        let data = parsed
            .map(|value| value.get("data").cloned().unwrap_or(value))
            .or_else(|| Some(json!({})));
        return ServerMessage::Response {
            id,
            status,
            data,
            error: None,
        };
    }

    // The route's own error object rides through verbatim, so a socket
    // client reads the same code, message, and details a REST client does.
    let error = parsed
        .and_then(|value| value.get("error").cloned())
        .unwrap_or_else(|| unenveloped_error(status, &bytes));
    ServerMessage::Response {
        id,
        status,
        data: None,
        error: Some(error),
    }
}

/// Project a failure that reached the socket without the canonical
/// envelope.
///
/// Built-in routes always answer with the envelope, so this covers a
/// route that does not, such as an extension that kept Axum's stock
/// extractors. The real status already rides on the response, the code
/// is the one the error table pairs with that status (`internal_error`
/// when the status names no single code), and the body text survives as
/// the message.
fn unenveloped_error(status: u16, body: &[u8]) -> serde_json::Value {
    let text = String::from_utf8_lossy(body).trim().to_owned();
    let code = match StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR) {
        StatusCode::BAD_REQUEST => "malformed_request",
        StatusCode::UNAUTHORIZED => "unauthorized",
        StatusCode::FORBIDDEN => "forbidden",
        StatusCode::CONFLICT => "conflict",
        StatusCode::PRECONDITION_FAILED => "precondition_failed",
        StatusCode::PAYLOAD_TOO_LARGE => "payload_too_large",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported_media_type",
        StatusCode::UNPROCESSABLE_ENTITY => "validation_error",
        StatusCode::TOO_MANY_REQUESTS => "rate_limited",
        StatusCode::SERVICE_UNAVAILABLE => "service_unavailable",
        _ => "internal_error",
    };
    let message = if text.is_empty() {
        format!("Command failed with status {status}")
    } else {
        text
    };
    json!({ "code": code, "message": message })
}

fn protocol_error_json(error: WsProtocolError) -> serde_json::Value {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "code".to_owned(),
        serde_json::Value::String(error.code.to_owned()),
    );
    payload.insert(
        "message".to_owned(),
        serde_json::Value::String(error.message),
    );
    if let Some(details) = error.details {
        payload.insert("details".to_owned(), details);
    }
    serde_json::Value::Object(payload)
}
