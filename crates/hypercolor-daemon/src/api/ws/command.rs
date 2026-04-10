//! WebSocket command dispatch — REST-equivalent commands over the socket.
//!
//! Client messages of type `command` get routed through the normal Axum router
//! by injecting a synthetic request and relaying the response through the
//! `Response` server message.

use std::sync::Arc;

use axum::http::{Method, Request, header};
use axum::response::Response;
use serde_json::json;
use tower::ServiceExt;

use super::cache::cached_command_router;
use super::protocol::{ServerMessage, WsProtocolError};
use crate::api::AppState;
use crate::api::security::RequestAuthContext;

/// Maximum WebSocket command response body we buffer before relaying to the client.
/// Guards against memory exhaustion from crafted or runaway handler responses.
const WS_COMMAND_BODY_MAX: usize = 1024 * 1024;

pub(super) async fn dispatch_command(
    state: &Arc<AppState>,
    auth_context: RequestAuthContext,
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

    let error = parsed
        .and_then(|value| value.get("error").cloned())
        .or_else(|| {
            Some(json!({
                "code": "internal_error",
                "message": format!("Command failed with status {status}"),
            }))
        });
    ServerMessage::Response {
        id,
        status,
        data: None,
        error,
    }
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
