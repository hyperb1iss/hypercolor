//! Generic browser bridge for host-served Remote UI builds.
//!
//! The module knows only the JavaScript transport contract. Cloud identity,
//! tickets, encryption, and account policy remain in the embedding loader.

use crate::route_ui::UiMount;

pub const CONTRACT_MIN: u32 = 1;
pub const CONTRACT_MAX: u32 = 1;

/// Whether this browser page exposes the Remote host bridge.
///
/// Callers may use this before [`crate::run_with_extensions`] initializes the
/// transport to avoid issuing ordinary same-origin requests from a Remote
/// page. The full contract is still validated by [`initialize`].
/// Native builds never expose the browser Remote bridge.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub const fn is_available() -> bool {
    false
}

/// Resolve a daemon API path inside the host-provided Remote mount.
///
/// Only relative `/api/v1` routes are accepted. Browser normalization never
/// gets an opportunity to turn encoded traversal or separators into a route
/// outside the daemon bridge.
pub fn resolve_remote_api_url(mount: &str, daemon_id: &str, value: &str) -> Option<String> {
    let (path, query) = value
        .split_once('?')
        .map_or((value, None), |(path, query)| (path, Some(query)));
    if !(path == "/api/v1" || path.starts_with("/api/v1/"))
        || path.starts_with("//")
        || path.contains('\\')
        || path.contains('#')
        || path.chars().any(char::is_control)
        || !safe_path_segments(path)
        || query.is_some_and(|query| query.contains('#') || query.chars().any(char::is_control))
    {
        return None;
    }
    uuid::Uuid::parse_str(daemon_id).ok()?;
    let expected_mount = format!("/remote/{daemon_id}");
    if mount.trim_end_matches('/') != expected_mount {
        return None;
    }
    let mount = UiMount::new(mount, mount).ok()?;
    let suffix = query.map_or_else(String::new, |query| format!("?{query}"));
    Some(format!("{}/_d{path}{suffix}", mount.route_base()))
}

pub(crate) fn resolve_remote_api_url_from_base(base: &str, value: &str) -> Option<String> {
    let (path, query) = value
        .split_once('?')
        .map_or((value, None), |(path, query)| (path, Some(query)));
    if !(path == "/api/v1" || path.starts_with("/api/v1/"))
        || path.starts_with("//")
        || path.contains('\\')
        || path.contains('#')
        || path.chars().any(char::is_control)
        || !safe_path_segments(path)
        || query.is_some_and(|query| query.contains('#') || query.chars().any(char::is_control))
    {
        return None;
    }
    Some(format!("{}{value}", base.trim_end_matches('/')))
}

fn safe_path_segments(path: &str) -> bool {
    path.split('/').all(|segment| {
        let Some(decoded) = percent_decode(segment) else {
            return false;
        };
        decoded != "." && decoded != ".." && !decoded.contains('/') && !decoded.contains('\\')
    })
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            decoded.push(hex(high)? << 4 | hex(low)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

const fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use std::num::NonZeroUsize;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::task::{Context, Poll};

    use futures_util::{
        Stream,
        future::{AbortHandle, Abortable},
    };
    use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use wasm_bindgen_futures::JsFuture;

    use crate::api::client::{install_http_transport, install_remote_daemon_connection};
    use crate::api::http_transport::{
        HttpBody, HttpBodySource, HttpCancellation, HttpHeader, HttpMethod, HttpMultipartField,
        HttpMultipartSource, HttpRequest, HttpRequestBody, HttpResponse, HttpStreamError,
        HttpStreamFuture, HttpStreamRequest, HttpStreamResponse, HttpTransport, HttpTransportError,
    };
    use crate::ws::transport::{
        WebSocketBinaryFrame, WebSocketConnectRequest, WebSocketConnection, WebSocketEvent,
        WebSocketEventHandler, WebSocketMessage, WebSocketTransport, WebSocketTransportError,
        install_websocket_transport,
    };

    use super::{
        CONTRACT_MAX, CONTRACT_MIN, UiMount, resolve_remote_api_url,
        resolve_remote_api_url_from_base,
    };

    pub struct RemoteBridge {
        value: JsValue,
        pub mount: UiMount,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct RemoteBridgeError {
        pub code: &'static str,
    }

    impl RemoteBridge {
        pub fn ready(&self) {
            let _ = call0(&self.value, "ready");
        }
    }

    #[must_use]
    pub fn is_available() -> bool {
        bridge_value().is_some()
    }

    pub fn initialize() -> Result<Option<RemoteBridge>, RemoteBridgeError> {
        let Some(value) = bridge_value() else {
            return Ok(None);
        };
        let result = initialize_value(value.clone());
        if let Err(code) = &result {
            fatal(&value, code);
        }
        result.map(Some).map_err(|code| RemoteBridgeError { code })
    }

    fn bridge_value() -> Option<JsValue> {
        let window = web_sys::window()?;
        Reflect::get(window.as_ref(), &JsValue::from_str("__HYPERCOLOR_REMOTE__"))
            .ok()
            .filter(|value| !value.is_null() && !value.is_undefined())
    }

    fn initialize_value(value: JsValue) -> Result<RemoteBridge, &'static str> {
        let contract = get(&value, "contract")?;
        let minimum = integer(&contract, "min")?;
        let maximum = integer(&contract, "max")?;
        if minimum == 0 || maximum == 0 || minimum > maximum {
            return Err("remote_contract_mismatch");
        }
        let selected = CONTRACT_MAX.min(maximum);
        if selected < CONTRACT_MIN || selected < minimum {
            return Err("remote_contract_mismatch");
        }
        let daemon_id = string(&value, "daemonId")?;
        let mount_value = string(&value, "mount")?;
        let mount = UiMount::new(&mount_value, &mount_value).map_err(|_| "remote_mount_invalid")?;
        let base = resolve_remote_api_url(&mount_value, &daemon_id, "/api/v1")
            .ok_or("remote_mount_invalid")?
            .trim_end_matches("/api/v1")
            .to_owned();
        for name in ["request", "openSocket", "ready", "fatal"] {
            method(&value, name)?;
        }
        install_remote_daemon_connection(&base);
        let bridge = Rc::new(value.clone());
        install_http_transport(Rc::new(BridgeHttp(Rc::clone(&bridge))))
            .map_err(|_| "remote_http_transport_unavailable")?;
        install_websocket_transport(Rc::new(BridgeWebSocket(bridge)))
            .map_err(|_| "remote_socket_transport_unavailable")?;
        Ok(RemoteBridge { value, mount })
    }

    fn fatal(value: &JsValue, code: &str) {
        let _ = method(value, "fatal").and_then(|function| {
            function
                .call1(value, &JsValue::from_str(code))
                .map_err(|_| "remote_fatal_failed")
        });
    }

    struct BridgeHttp(Rc<JsValue>);

    impl HttpTransport for BridgeHttp {
        fn send(
            &self,
            request: HttpRequest,
        ) -> crate::api::http_transport::HttpTransportFuture<'_> {
            let bridge = Rc::clone(&self.0);
            Box::pin(async move {
                let cancellation = HttpCancellation::new();
                let body = match request.body {
                    HttpRequestBody::Empty => {
                        HttpBody::new(Box::new(EmptyBody), cancellation.clone())
                    }
                    HttpRequestBody::Bytes(bytes) => {
                        HttpBody::new(Box::new(BytesBody(Some(bytes))), cancellation.clone())
                    }
                    HttpRequestBody::Multipart(parts) => {
                        let boundary = multipart_boundary(&parts);
                        let fields = parts
                            .into_iter()
                            .map(|part| {
                                let filename = part.file_name.unwrap_or_else(|| "blob".to_owned());
                                let content_type = part.content_type.as_deref().unwrap_or_default();
                                HttpMultipartField::file(
                                    &part.name,
                                    filename,
                                    content_type,
                                    Box::new(BytesBody(Some(part.body))),
                                )
                            })
                            .collect();
                        let source =
                            HttpMultipartSource::new(boundary, fields).map_err(|error| {
                                HttpTransportError {
                                    message: error.to_string(),
                                }
                            })?;
                        let (body, content_type) = source.into_body(cancellation.clone());
                        let mut headers = request.headers;
                        headers.push(content_type);
                        let response = request_stream(
                            &bridge,
                            HttpStreamRequest {
                                method: request.method,
                                path: request.path,
                                headers,
                                body,
                            },
                        )
                        .await
                        .map_err(|error| HttpTransportError {
                            message: error.to_string(),
                        })?;
                        let bytes = collect_body(response.body).await.map_err(|error| {
                            HttpTransportError {
                                message: error.to_string(),
                            }
                        })?;
                        return Ok(HttpResponse {
                            status: response.status,
                            headers: response.headers,
                            body: bytes,
                        });
                    }
                };
                let response = request_stream(
                    &bridge,
                    HttpStreamRequest {
                        method: request.method,
                        path: request.path,
                        headers: request.headers,
                        body,
                    },
                )
                .await
                .map_err(|error| HttpTransportError {
                    message: error.to_string(),
                })?;
                let bytes =
                    collect_body(response.body)
                        .await
                        .map_err(|error| HttpTransportError {
                            message: error.to_string(),
                        })?;
                Ok(HttpResponse {
                    status: response.status,
                    headers: response.headers,
                    body: bytes,
                })
            })
        }

        fn send_stream(&self, request: HttpStreamRequest) -> HttpStreamFuture<'_> {
            let bridge = Rc::clone(&self.0);
            let cancellation = request.body.cancellation();
            HttpStreamFuture::new(cancellation, async move {
                request_stream(&bridge, request).await
            })
        }
    }

    fn multipart_boundary(parts: &[crate::api::http_transport::HttpMultipartPart]) -> String {
        for nonce in 0_u64.. {
            let candidate = format!("hypercolor-remote-boundary-{nonce:016x}");
            if parts.iter().all(|part| {
                !part
                    .body
                    .windows(candidate.len())
                    .any(|window| window == candidate.as_bytes())
            }) {
                return candidate;
            }
        }
        unreachable!("u64 boundary space cannot be exhausted")
    }

    async fn request_stream(
        bridge: &JsValue,
        request: HttpStreamRequest,
    ) -> Result<HttpStreamResponse, HttpStreamError> {
        if resolve_remote_api_url_from_base("", &request.path).is_none() {
            return Err(HttpStreamError::Transport(
                "Remote request path is outside /api/v1".to_owned(),
            ));
        }
        let cancellation = request.body.cancellation();
        let response_cancellation = cancellation.clone();
        let controller = web_sys::AbortController::new().map_err(js_transport)?;
        let stream = body_stream(request.body);
        let init = Object::new();
        set(
            &init,
            "method",
            JsValue::from_str(method_name(request.method)),
        )?;
        set(&init, "path", JsValue::from_str(&request.path))?;
        set(&init, "headers", header_array(&request.headers).into())?;
        set(&init, "body", stream.into())?;
        set(&init, "signal", controller.signal().into())?;
        let promise = method(bridge, "request")
            .map_err(transport_message)?
            .call1(bridge, &init)
            .map_err(js_transport)?
            .dyn_into::<Promise>()
            .map_err(|_| {
                HttpStreamError::Transport("Remote request did not return a Promise".to_owned())
            })?;
        let abort = controller.clone();
        let (watcher, registration) = AbortHandle::new_pair();
        let watcher = CancellationWatcher {
            task: watcher,
            controller,
            finished: false,
        };
        wasm_bindgen_futures::spawn_local(async move {
            let _ = Abortable::new(
                async move {
                    cancellation.cancelled().await;
                    abort.abort();
                },
                registration,
            )
            .await;
        });
        let value = JsFuture::from(promise).await.map_err(js_transport)?;
        let status = integer(&value, "status").map_err(transport_message)?;
        let headers = parse_headers(get(&value, "headers").map_err(transport_message)?)?;
        let raw = get(&value, "body")
            .map_err(transport_message)?
            .dyn_into::<web_sys::ReadableStream>()
            .map_err(|_| {
                HttpStreamError::Transport(
                    "Remote response body is not a ReadableStream".to_owned(),
                )
            })?;
        let body = HttpBody::new(
            Box::new(JsBody {
                stream: Box::pin(wasm_streams::ReadableStream::from_raw(raw).into_stream()),
                buffered: None,
                watcher,
            }),
            response_cancellation,
        );
        Ok(HttpStreamResponse {
            status: u16::try_from(status).map_err(|_| {
                HttpStreamError::Transport("Remote response status is invalid".to_owned())
            })?,
            headers,
            body,
        })
    }

    fn body_stream(body: HttpBody) -> web_sys::ReadableStream {
        let stream = futures_util::stream::unfold(body, |mut body| async move {
            match body.read_chunk(NonZeroUsize::new(64 * 1024).unwrap()).await {
                Ok(Some(bytes)) => Some((Ok(Uint8Array::from(bytes.as_slice()).into()), body)),
                Ok(None) => None,
                Err(error) => Some((Err(JsValue::from_str(&error.to_string())), body)),
            }
        });
        wasm_streams::ReadableStream::from_stream(stream).into_raw()
    }

    async fn collect_body(mut body: HttpBody) -> Result<Vec<u8>, HttpStreamError> {
        let mut bytes = Vec::new();
        while let Some(chunk) = body
            .read_chunk(NonZeroUsize::new(64 * 1024).unwrap())
            .await?
        {
            bytes.extend(chunk);
        }
        Ok(bytes)
    }

    struct EmptyBody;
    impl HttpBodySource for EmptyBody {
        fn exact_length(&self) -> Option<u64> {
            Some(0)
        }
        fn poll_chunk(
            &mut self,
            _: &mut Context<'_>,
            _: NonZeroUsize,
        ) -> Poll<Result<Option<Vec<u8>>, HttpStreamError>> {
            Poll::Ready(Ok(None))
        }
        fn cancel(&mut self) {}
    }
    struct BytesBody(Option<Vec<u8>>);
    impl HttpBodySource for BytesBody {
        fn exact_length(&self) -> Option<u64> {
            self.0.as_ref().map(|v| v.len() as u64)
        }
        fn poll_chunk(
            &mut self,
            _: &mut Context<'_>,
            maximum: NonZeroUsize,
        ) -> Poll<Result<Option<Vec<u8>>, HttpStreamError>> {
            let Some(mut bytes) = self.0.take() else {
                return Poll::Ready(Ok(None));
            };
            if bytes.len() <= maximum.get() {
                return Poll::Ready(Ok(Some(bytes)));
            }
            let rest = bytes.split_off(maximum.get());
            self.0 = Some(rest);
            Poll::Ready(Ok(Some(bytes)))
        }
        fn cancel(&mut self) {
            self.0 = None;
        }
    }

    struct JsBody {
        stream: Pin<Box<dyn Stream<Item = Result<JsValue, JsValue>>>>,
        buffered: Option<Vec<u8>>,
        watcher: CancellationWatcher,
    }
    impl HttpBodySource for JsBody {
        fn exact_length(&self) -> Option<u64> {
            None
        }
        fn poll_chunk(
            &mut self,
            cx: &mut Context<'_>,
            maximum: NonZeroUsize,
        ) -> Poll<Result<Option<Vec<u8>>, HttpStreamError>> {
            if let Some(mut bytes) = self.buffered.take() {
                if bytes.len() > maximum.get() {
                    let rest = bytes.split_off(maximum.get());
                    self.buffered = Some(rest);
                }
                return Poll::Ready(Ok(Some(bytes)));
            }
            match self.stream.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(value))) => {
                    let mut bytes = Uint8Array::new(&value).to_vec();
                    if bytes.is_empty() {
                        return Poll::Ready(Err(HttpStreamError::InvalidChunk));
                    }
                    if bytes.len() > maximum.get() {
                        let rest = bytes.split_off(maximum.get());
                        self.buffered = Some(rest);
                    }
                    Poll::Ready(Ok(Some(bytes)))
                }
                Poll::Ready(Some(Err(error))) => Poll::Ready(Err(js_transport(error))),
                Poll::Ready(None) => {
                    self.watcher.finish();
                    Poll::Ready(Ok(None))
                }
                Poll::Pending => Poll::Pending,
            }
        }
        fn cancel(&mut self) {
            self.watcher.cancel();
            self.buffered = None;
        }
    }

    impl Drop for JsBody {
        fn drop(&mut self) {
            self.watcher.cancel();
        }
    }

    struct CancellationWatcher {
        task: AbortHandle,
        controller: web_sys::AbortController,
        finished: bool,
    }

    impl CancellationWatcher {
        fn cancel(&mut self) {
            if !self.finished {
                self.finished = true;
                self.controller.abort();
            }
            self.task.abort();
        }

        fn finish(&mut self) {
            self.finished = true;
            self.task.abort();
        }
    }

    impl Drop for CancellationWatcher {
        fn drop(&mut self) {
            self.cancel();
        }
    }

    struct BridgeWebSocket(Rc<JsValue>);
    struct BridgeSocket {
        value: JsValue,
        _callbacks: Vec<Closure<dyn FnMut(JsValue)>>,
    }
    impl WebSocketConnection for BridgeSocket {
        fn send(&self, message: WebSocketMessage) -> Result<(), WebSocketTransportError> {
            let value = match message {
                WebSocketMessage::Text(v) => JsValue::from_str(&v),
                WebSocketMessage::Binary(v) => Uint8Array::from(v.to_vec().as_slice()).into(),
            };
            method(&self.value, "send")
                .and_then(|f| {
                    f.call1(&self.value, &value)
                        .map(|_| ())
                        .map_err(|_| "Remote socket send failed")
                })
                .map_err(ws_error)
        }
        fn close(&self) -> Result<(), WebSocketTransportError> {
            call0(&self.value, "close").map(|_| ()).map_err(ws_error)
        }
    }
    impl Drop for BridgeSocket {
        fn drop(&mut self) {
            for name in ["onopen", "onmessage", "onclose", "onerror"] {
                let _ = Reflect::set(&self.value, &JsValue::from_str(name), &JsValue::NULL);
            }
            let _ = call0(&self.value, "close");
        }
    }
    impl WebSocketTransport for BridgeWebSocket {
        fn connect(
            &self,
            request: WebSocketConnectRequest,
            events: WebSocketEventHandler,
        ) -> Result<Rc<dyn WebSocketConnection>, WebSocketTransportError> {
            if resolve_remote_api_url_from_base("", &request.path).is_none() {
                return Err(ws_error("Remote socket path is outside /api/v1"));
            }
            let socket = method(&self.0, "openSocket")
                .and_then(|f| {
                    f.call1(&self.0, &JsValue::from_str(&request.path))
                        .map_err(|_| "Remote openSocket failed")
                })
                .map_err(ws_error)?;
            let mut callbacks = Vec::new();
            for (name, event) in [
                ("onopen", WebSocketEvent::Opened),
                (
                    "onerror",
                    WebSocketEvent::Error {
                        message: "Remote socket error".to_owned(),
                    },
                ),
            ] {
                let events = Rc::clone(&events);
                let event = event.clone();
                let callback =
                    Closure::wrap(Box::new(move |_: JsValue| events(event.clone()))
                        as Box<dyn FnMut(JsValue)>);
                Reflect::set(&socket, &JsValue::from_str(name), callback.as_ref())
                    .map_err(|_| ws_error("Remote socket callback install failed"))?;
                callbacks.push(callback);
            }
            let messages = Rc::clone(&events);
            let callback = Closure::wrap(Box::new(move |value: JsValue| {
                if let Some(text) = value.as_string() {
                    messages(WebSocketEvent::Message(WebSocketMessage::Text(text)));
                } else {
                    messages(WebSocketEvent::Message(WebSocketMessage::Binary(
                        WebSocketBinaryFrame::from_bytes(Uint8Array::new(&value).to_vec()),
                    )));
                }
            }) as Box<dyn FnMut(JsValue)>);
            Reflect::set(&socket, &JsValue::from_str("onmessage"), callback.as_ref())
                .map_err(|_| ws_error("Remote socket callback install failed"))?;
            callbacks.push(callback);
            let closes = Rc::clone(&events);
            let callback = Closure::wrap(Box::new(move |value: JsValue| {
                closes(WebSocketEvent::Closed {
                    code: Reflect::get(&value, &JsValue::from_str("code"))
                        .ok()
                        .and_then(|v| v.as_f64())
                        .unwrap_or(1006.0) as u16,
                    reason: Reflect::get(&value, &JsValue::from_str("reason"))
                        .ok()
                        .and_then(|v| v.as_string())
                        .unwrap_or_default(),
                })
            }) as Box<dyn FnMut(JsValue)>);
            Reflect::set(&socket, &JsValue::from_str("onclose"), callback.as_ref())
                .map_err(|_| ws_error("Remote socket callback install failed"))?;
            callbacks.push(callback);
            Ok(Rc::new(BridgeSocket {
                value: socket,
                _callbacks: callbacks,
            }))
        }
    }

    fn get(value: &JsValue, name: &str) -> Result<JsValue, &'static str> {
        Reflect::get(value, &JsValue::from_str(name)).map_err(|_| "remote_contract_invalid")
    }
    fn method(value: &JsValue, name: &str) -> Result<Function, &'static str> {
        get(value, name)?
            .dyn_into()
            .map_err(|_| "remote_contract_invalid")
    }
    fn call0(value: &JsValue, name: &str) -> Result<JsValue, &'static str> {
        method(value, name)?
            .call0(value)
            .map_err(|_| "remote_contract_invalid")
    }
    fn string(value: &JsValue, name: &str) -> Result<String, &'static str> {
        get(value, name)?
            .as_string()
            .ok_or("remote_contract_invalid")
    }
    fn integer(value: &JsValue, name: &str) -> Result<u32, &'static str> {
        let number = get(value, name)?
            .as_f64()
            .ok_or("remote_contract_invalid")?;
        if number.fract() == 0.0 && number >= 0.0 && number <= u32::MAX as f64 {
            Ok(number as u32)
        } else {
            Err("remote_contract_invalid")
        }
    }
    fn set(object: &Object, name: &str, value: JsValue) -> Result<(), HttpStreamError> {
        Reflect::set(object, &JsValue::from_str(name), &value)
            .map(|_| ())
            .map_err(js_transport)
    }
    fn method_name(method: HttpMethod) -> &'static str {
        match method {
            HttpMethod::Get => "GET",
            HttpMethod::Head => "HEAD",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DELETE",
        }
    }
    fn header_array(headers: &[HttpHeader]) -> Array {
        let rows = Array::new();
        for header in headers {
            let pair = Array::new();
            pair.push(&JsValue::from_str(&header.name));
            pair.push(&JsValue::from_str(&header.value));
            rows.push(&pair);
        }
        rows
    }
    fn parse_headers(value: JsValue) -> Result<Vec<HttpHeader>, HttpStreamError> {
        let rows = value
            .dyn_into::<Array>()
            .map_err(|_| transport_message("Remote response headers must be an array"))?;
        rows.iter()
            .map(|row| {
                let pair = row
                    .dyn_into::<Array>()
                    .map_err(|_| transport_message("Remote response header must be a pair"))?;
                if pair.length() != 2 {
                    return Err(transport_message("Remote response header must be a pair"));
                }
                Ok(HttpHeader {
                    name: pair.get(0).as_string().ok_or_else(|| {
                        HttpStreamError::Transport(
                            "Remote response header name is invalid".to_owned(),
                        )
                    })?,
                    value: pair.get(1).as_string().ok_or_else(|| {
                        HttpStreamError::Transport(
                            "Remote response header value is invalid".to_owned(),
                        )
                    })?,
                })
            })
            .collect()
    }
    fn js_transport(error: JsValue) -> HttpStreamError {
        HttpStreamError::Transport(
            error
                .as_string()
                .unwrap_or_else(|| "Remote bridge JavaScript error".to_owned()),
        )
    }
    fn transport_message(message: &str) -> HttpStreamError {
        HttpStreamError::Transport(message.to_owned())
    }
    fn ws_error(message: &str) -> WebSocketTransportError {
        WebSocketTransportError {
            message: message.to_owned(),
        }
    }

    #[cfg(test)]
    mod tests {
        use std::{
            future::Future,
            num::NonZeroUsize,
            rc::Rc,
            task::{Context, Waker},
        };

        use js_sys::Promise;
        use wasm_bindgen::prelude::*;
        use wasm_bindgen_futures::JsFuture;
        use wasm_bindgen_test::*;

        use super::*;

        wasm_bindgen_test_configure!(run_in_browser);

        #[wasm_bindgen_test]
        fn response_headers_reject_malformed_javascript_values() {
            for value in [
                JsValue::NULL,
                JsValue::UNDEFINED,
                JsValue::from_str("headers"),
            ] {
                assert!(parse_headers(value).is_err());
            }
            for json in [
                "[null]",
                "[\"ab\"]",
                "[[\"name\"]]",
                "[[\"name\",1]]",
                "[[\"a\",\"b\",\"c\"]]",
            ] {
                assert!(parse_headers(js_sys::JSON::parse(json).unwrap()).is_err());
            }
            let headers =
                parse_headers(js_sys::JSON::parse("[[\"content-type\",\"text/plain\"]]").unwrap())
                    .unwrap();
            assert_eq!(headers.len(), 1);
            assert_eq!(headers[0].name, "content-type");
            assert_eq!(headers[0].value, "text/plain");
        }

        #[wasm_bindgen(inline_js = r#"
export function requestFixture(mode) {
  window.__remoteBridgeProbe = {aborted: 0, uploaded: []};
  return {
    request: async init => {
      init.signal.addEventListener('abort', () => window.__remoteBridgeProbe.aborted++);
      if (mode === 'pending') return new Promise(() => {});
      const bytes = new Uint8Array(await new Response(init.body).arrayBuffer());
      window.__remoteBridgeProbe.uploaded = Array.from(bytes);
      return {
        status: 200,
        headers: [['content-type', 'application/octet-stream']],
        body: new ReadableStream({start(controller) {
          controller.enqueue(new Uint8Array([1, 2, 3, 4, 5]));
          controller.close();
        }}),
      };
    },
  };
}
export function uploaded() { return new Uint8Array(window.__remoteBridgeProbe.uploaded); }
export function aborts() { return window.__remoteBridgeProbe.aborted; }
export function nextTask() { return new Promise(resolve => setTimeout(resolve, 0)); }
export function socketFixture() {
  window.__remoteSocketProbe = null;
  return {openSocket() {
    const socket = {closes: 0, send() {}, close() { this.closes++; }};
    window.__remoteSocketProbe = socket;
    return socket;
  }};
}
export function socketClosed() { return window.__remoteSocketProbe.closes; }
export function socketHandlersCleared() {
  const s = window.__remoteSocketProbe;
  return s.onopen === null && s.onmessage === null && s.onclose === null && s.onerror === null;
}
"#)]
        extern "C" {
            #[wasm_bindgen(js_name = requestFixture)]
            fn request_fixture(mode: &str) -> JsValue;
            fn uploaded() -> js_sys::Uint8Array;
            fn aborts() -> u32;
            #[wasm_bindgen(js_name = nextTask)]
            fn next_task() -> Promise;
            #[wasm_bindgen(js_name = socketFixture)]
            fn socket_fixture() -> JsValue;
            #[wasm_bindgen(js_name = socketClosed)]
            fn socket_closed() -> u32;
            #[wasm_bindgen(js_name = socketHandlersCleared)]
            fn socket_handlers_cleared() -> bool;
        }

        fn stream_request(body: Vec<u8>) -> HttpStreamRequest {
            let cancellation = HttpCancellation::new();
            HttpStreamRequest {
                method: HttpMethod::Post,
                path: "/api/v1/upload".to_owned(),
                headers: Vec::new(),
                body: HttpBody::new(Box::new(BytesBody(Some(body))), cancellation),
            }
        }

        #[wasm_bindgen_test]
        async fn request_stream_carries_uploads_and_splits_browser_chunks() {
            let transport = BridgeHttp(Rc::new(request_fixture("complete")));
            let mut response = transport
                .send_stream(stream_request(vec![9, 8, 7]))
                .await
                .expect("response headers");
            assert_eq!(uploaded().to_vec(), vec![9, 8, 7]);
            let maximum = NonZeroUsize::new(2).expect("nonzero");
            assert_eq!(
                response.body.read_chunk(maximum).await.expect("chunk"),
                Some(vec![1, 2])
            );
            assert_eq!(
                response.body.read_chunk(maximum).await.expect("chunk"),
                Some(vec![3, 4])
            );
            assert_eq!(
                response.body.read_chunk(maximum).await.expect("chunk"),
                Some(vec![5])
            );
            assert!(
                response
                    .body
                    .read_chunk(maximum)
                    .await
                    .expect("EOF")
                    .is_none()
            );
        }

        #[wasm_bindgen_test]
        async fn dropping_pending_headers_aborts_the_bridge_signal() {
            let transport = BridgeHttp(Rc::new(request_fixture("pending")));
            let mut future = Box::pin(transport.send_stream(stream_request(vec![1])));
            assert!(
                future
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
            drop(future);
            JsFuture::from(next_task()).await.expect("next task");
            assert_eq!(aborts(), 1);
        }

        #[wasm_bindgen_test]
        fn dropping_socket_closes_it_before_releasing_callbacks() {
            let transport = BridgeWebSocket(Rc::new(socket_fixture()));
            let connection = transport
                .connect(
                    WebSocketConnectRequest {
                        path: "/api/v1/ws".to_owned(),
                        protocol: "hypercolor".to_owned(),
                    },
                    Rc::new(|_| {}),
                )
                .expect("socket");
            drop(connection);
            assert_eq!(socket_closed(), 1);
            assert!(socket_handlers_cleared());
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use browser::{RemoteBridge, RemoteBridgeError, initialize, is_available};

#[cfg(test)]
mod tests {
    use super::{is_available, resolve_remote_api_url};

    const DAEMON: &str = "018f4c36-4a44-7cc9-9f57-0d2e9224d2f1";

    #[test]
    fn native_runtime_never_reports_a_browser_bridge() {
        assert!(!is_available());
    }

    #[test]
    fn rebases_api_routes_under_the_remote_daemon_mount() {
        assert_eq!(
            resolve_remote_api_url(
                &format!("/remote/{DAEMON}"),
                DAEMON,
                "/api/v1/devices?limit=2"
            ),
            Some(format!("/remote/{DAEMON}/_d/api/v1/devices?limit=2"))
        );
    }

    #[test]
    fn refuses_absolute_protocol_relative_and_traversal_routes() {
        for path in [
            "https://evil.test/api/v1",
            "//evil.test/api/v1",
            "/api/v1/../admin",
            "/api/v1/%2e%2e/admin",
            "/api/v1/%2E%2E/admin",
            "/api/v1/%2fadmin",
            "/api/v1/a\\b",
            "/api/v2/devices",
        ] {
            assert_eq!(
                resolve_remote_api_url(&format!("/remote/{DAEMON}"), DAEMON, path),
                None,
                "{path}"
            );
        }
    }

    #[test]
    fn binds_the_remote_mount_to_the_selected_daemon() {
        assert_eq!(
            resolve_remote_api_url("/remote/not-the-daemon", DAEMON, "/api/v1/devices"),
            None
        );
        assert_eq!(
            resolve_remote_api_url(
                "/remote/018f4c36-4a44-7cc9-9f57-0d2e9224d2f2",
                DAEMON,
                "/api/v1/devices"
            ),
            None
        );
    }
}
