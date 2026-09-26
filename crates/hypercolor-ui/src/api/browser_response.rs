//! Single-reader ownership of an incremental browser response.

use std::{
    future::Future,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll},
};

use js_sys::{Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
use wasm_bindgen_futures::JsFuture;
use web_sys::{AbortController, ReadableStreamDefaultReader, Response};

use super::http_transport::{HttpBodySource, HttpStreamError};

#[wasm_bindgen(inline_js = r#"
export function cancelResponseReader(reader) {
    // Cancellation is cleanup; an already failed stream can reject its promise.
    reader.cancel().catch(() => {});
    reader.releaseLock();
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = cancelResponseReader)]
    fn cancel_response_reader(reader: &ReadableStreamDefaultReader);
}

/// Keeps at most one browser-provided chunk and one caller-sized Rust copy.
/// Browser network buffering and the size of its chunk remain browser-owned.
/// Content-Length is not an exact source length because fetch may decode content.
pub struct BrowserResponseSource {
    reader: Option<ReadableStreamDefaultReader>,
    controller: Option<AbortController>,
    pending: Option<JsFuture>,
    buffered: Option<(Uint8Array, u32)>,
    complete: bool,
}

impl BrowserResponseSource {
    /// Acquire the response's single reader without reading ahead.
    ///
    /// # Errors
    /// Rejects a response whose body is already locked, and aborts the exchange.
    pub fn new(response: &Response, controller: AbortController) -> Result<Self, HttpStreamError> {
        let reader = response
            .body()
            .map(|body| {
                ReadableStreamDefaultReader::new(&body).map_err(|error| {
                    controller.abort();
                    browser_error(error)
                })
            })
            .transpose()?;
        Ok(Self {
            complete: reader.is_none(),
            controller: reader.as_ref().map(|_| controller),
            reader,
            pending: None,
            buffered: None,
        })
    }

    fn poll_read(
        &mut self,
        context: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Vec<u8>>, HttpStreamError>> {
        loop {
            if let Some((bytes, offset)) = &mut self.buffered {
                let remaining = (bytes.length() - *offset) as usize;
                let length = remaining.min(maximum.get());
                let end = *offset + length as u32;
                let mut output = vec![0; length];
                bytes.subarray(*offset, end).copy_to(&mut output);
                *offset = end;
                if end == bytes.length() {
                    self.buffered = None;
                }
                return Poll::Ready(Ok(Some(output)));
            }
            if self.complete {
                return Poll::Ready(Ok(None));
            }
            if self.pending.is_none() {
                self.pending = Some(JsFuture::from(
                    self.reader
                        .as_ref()
                        .expect("active response owns its reader")
                        .read(),
                ));
            }
            let result = match Pin::new(self.pending.as_mut().expect("read exists")).poll(context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(result) => result,
            };
            self.pending = None;
            let result = result.map_err(browser_error)?;
            let done = Reflect::get(&result, &JsValue::from_str("done")).map_err(browser_error)?;
            if done.as_bool() == Some(true) {
                self.complete = true;
                if let Some(reader) = self.reader.take() {
                    reader.release_lock();
                }
                self.controller = None;
                return Poll::Ready(Ok(None));
            }
            let value =
                Reflect::get(&result, &JsValue::from_str("value")).map_err(browser_error)?;
            let bytes = value
                .dyn_into::<Uint8Array>()
                .map_err(|_| HttpStreamError::InvalidChunk)?;
            if bytes.length() != 0 {
                self.buffered = Some((bytes, 0));
            }
        }
    }
}

impl HttpBodySource for BrowserResponseSource {
    fn exact_length(&self) -> Option<u64> {
        None
    }

    fn poll_chunk(
        &mut self,
        context: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Vec<u8>>, HttpStreamError>> {
        let result = self.poll_read(context, maximum);
        if matches!(result, Poll::Ready(Err(_))) {
            self.cancel();
        }
        result
    }

    fn cancel(&mut self) {
        self.complete = true;
        if let Some(controller) = self.controller.take() {
            controller.abort();
        }
        if let Some(reader) = self.reader.take() {
            cancel_response_reader(&reader);
        }
        self.pending = None;
        self.buffered = None;
    }
}

impl Drop for BrowserResponseSource {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn browser_error(error: JsValue) -> HttpStreamError {
    HttpStreamError::Transport(format!("browser response read failed: {error:?}"))
}
