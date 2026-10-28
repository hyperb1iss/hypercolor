//! Native browser exchange for immutable Blob uploads and streamed responses.

use super::http_transport::{
    HttpBody, HttpHeader, HttpMethod, HttpStreamError, HttpStreamFuture, HttpStreamRequest,
    HttpStreamResponse,
};
use super::{browser_body::BrowserBlobSource, browser_response::BrowserResponseSource, client};
use gloo_net::http::{Method, RequestBuilder};

struct AbortOnDrop(Option<web_sys::AbortController>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(controller) = &self.0 {
            controller.abort();
        }
    }
}

pub(super) fn send(request: HttpStreamRequest) -> HttpStreamFuture<'static> {
    let cancellation = request.body.cancellation();
    HttpStreamFuture::new(cancellation.clone(), async move {
        let url = client::daemon_url(&request.path)
            .ok_or_else(|| error("verified Hypercolor connection is unavailable"))?;
        let controller = web_sys::AbortController::new().map_err(|e| error(format!("{e:?}")))?;
        let mut abort = AbortOnDrop(Some(controller.clone()));
        let signal = controller.signal();
        let method = match request.method {
            HttpMethod::Get => Method::GET,
            HttpMethod::Head => Method::HEAD,
            HttpMethod::Post => Method::POST,
            HttpMethod::Put => Method::PUT,
            HttpMethod::Patch => Method::PATCH,
            HttpMethod::Delete => Method::DELETE,
        };
        let mut builder = RequestBuilder::new(&url)
            .method(method)
            .abort_signal(Some(&signal));
        if let Some(token) = client::authorization_token() {
            builder = builder.header("Authorization", &format!("Bearer {token}"));
        }
        for header in request.headers {
            builder = builder.header(&header.name, &header.value);
        }
        let blob = request
            .body
            .source_hint()
            .and_then(|source| source.downcast_ref::<BrowserBlobSource>())
            .and_then(BrowserBlobSource::blob)
            .cloned();
        let empty = request.body.exact_length() == Some(0);
        let response = if let Some(blob) = blob {
            request.body.discard();
            builder
                .body(blob)
                .map_err(|e| error(e.to_string()))?
                .send()
                .await
        } else if empty {
            request.body.discard();
            builder.send().await
        } else {
            return Err(HttpStreamError::Unsupported);
        }
        .map_err(|e| error(e.to_string()))?;
        let status = response.status();
        let headers = response
            .headers()
            .entries()
            .map(|(name, value)| HttpHeader { name, value })
            .collect();
        let raw: web_sys::Response = response.into();
        let source = BrowserResponseSource::new(&raw, controller)?;
        abort.0.take();
        Ok(HttpStreamResponse {
            status,
            headers,
            body: HttpBody::new(Box::new(source), cancellation),
        })
    })
}

fn error(message: impl Into<String>) -> HttpStreamError {
    HttpStreamError::Transport(message.into())
}
