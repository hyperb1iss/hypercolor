//! Browser-owned Blob handles with bounded, abortable reads for injected transports.

use std::any::Any;
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use js_sys::{ArrayBuffer, Uint8Array};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{Blob, File, FileReader};

use super::http_transport::{HttpBodySource, HttpMultipartField, HttpStreamError};

/// The original immutable Blob is the native representation of this exact source.
/// At most one requested slice and its Rust copy are retained; no read-ahead occurs.
/// This bounds application-owned reads, not the browser process's internal memory.
pub struct BrowserBlobSource {
    blob: Option<Blob>,
    length: u64,
    offset: u64,
    attempt: Option<ReadAttempt>,
    buffered: Option<(Uint8Array, u32)>,
    started: bool,
    cancelled: bool,
}

impl BrowserBlobSource {
    /// # Errors
    /// Rejects sizes that cannot be represented exactly by JavaScript range offsets.
    pub fn new(blob: Blob) -> Result<Self, HttpStreamError> {
        let size = blob.size();
        if !size.is_finite() || size < 0.0 || size.fract() != 0.0 || size > 9_007_199_254_740_991.0
        {
            return Err(HttpStreamError::LengthMismatch);
        }
        Ok(Self {
            blob: Some(blob),
            length: size as u64,
            offset: 0,
            attempt: None,
            buffered: None,
            started: false,
            cancelled: false,
        })
    }

    /// Preserve File name and MIME semantics, without reading file contents.
    ///
    /// # Errors
    /// Returns the same size-validation errors as `new`.
    pub fn file_field(name: &str, file: File) -> Result<HttpMultipartField, HttpStreamError> {
        let filename = file.name();
        let content_type = file.type_();
        Ok(HttpMultipartField::file(
            name,
            filename,
            &content_type,
            Box::new(Self::new(file.into())?),
        ))
    }

    /// Preserve FormData's unnamed-Blob filename default explicitly.
    ///
    /// # Errors
    /// Returns the same size-validation errors as `new`.
    pub fn blob_field(name: &str, blob: Blob) -> Result<HttpMultipartField, HttpStreamError> {
        let content_type = blob.type_();
        Ok(HttpMultipartField::file(
            name,
            "blob".into(),
            &content_type,
            Box::new(Self::new(blob)?),
        ))
    }

    /// Borrow the original handle only before any read. The native transport must
    /// acquire exchange cancellation ownership before discarding unused pull sources.
    #[must_use]
    pub fn blob(&self) -> Option<&Blob> {
        if self.started || self.cancelled {
            None
        } else {
            self.blob.as_ref()
        }
    }

    fn poll_read(
        &mut self,
        context: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Vec<u8>>, HttpStreamError>> {
        if self.cancelled {
            return Poll::Ready(Err(HttpStreamError::Cancelled));
        }
        if let Some((bytes, consumed)) = &mut self.buffered {
            let remaining = (bytes.length() - *consumed) as usize;
            let length = maximum.get().min(remaining);
            let end = *consumed + length as u32;
            let mut output = vec![0; length];
            bytes.subarray(*consumed, end).copy_to(&mut output);
            *consumed = end;
            self.offset += length as u64;
            if end == bytes.length() {
                self.buffered = None;
            }
            return Poll::Ready(Ok(Some(output)));
        }
        if self.offset == self.length {
            return Poll::Ready(Ok(None));
        }
        if self.attempt.is_none() {
            let length = (self.length - self.offset)
                .min(maximum.get() as u64)
                .min(u64::from(u32::MAX));
            // Constructor validation makes every offset exact in JavaScript's f64 domain.
            let slice = self
                .blob
                .as_ref()
                .expect("active source owns its Blob")
                .slice_with_f64_and_f64(self.offset as f64, (self.offset + length) as f64)
                .map_err(js_error)?;
            self.attempt = Some(ReadAttempt::new(&slice, length as u32)?);
        }
        let attempt = self.attempt.as_ref().expect("read attempt exists");
        let result = {
            let mut state = attempt.state.borrow_mut();
            state.waker = Some(context.waker().clone());
            state.result.take()
        };
        let Some(result) = result else {
            return Poll::Pending;
        };
        let expected = attempt.expected;
        self.attempt = None;
        let buffer = result?;
        if buffer.byte_length() != expected {
            return Poll::Ready(Err(HttpStreamError::LengthMismatch));
        }
        self.buffered = Some((Uint8Array::new(&buffer), 0));
        self.poll_read(context, maximum)
    }
}

impl HttpBodySource for BrowserBlobSource {
    fn source_hint(&self) -> Option<&dyn Any> {
        self.blob().map(|_| self as &dyn Any)
    }
    fn exact_length(&self) -> Option<u64> {
        Some(self.length)
    }
    fn poll_chunk(
        &mut self,
        context: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Vec<u8>>, HttpStreamError>> {
        self.started = true;
        let result = self.poll_read(context, maximum);
        if matches!(result, Poll::Ready(Err(_))) {
            self.cancel();
        }
        result
    }
    fn cancel(&mut self) {
        self.cancelled = true;
        self.attempt = None;
        self.buffered = None;
        self.blob = None;
    }
}

impl Drop for BrowserBlobSource {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct ReadState {
    active: bool,
    completed: bool,
    result: Option<Result<ArrayBuffer, HttpStreamError>>,
    waker: Option<Waker>,
}

struct ReadAttempt {
    reader: FileReader,
    state: Rc<RefCell<ReadState>>,
    expected: u32,
    _completion: Closure<dyn FnMut()>,
}

impl ReadAttempt {
    fn new(blob: &Blob, expected: u32) -> Result<Self, HttpStreamError> {
        let reader = FileReader::new().map_err(js_error)?;
        let state = Rc::new(RefCell::new(ReadState {
            active: true,
            completed: false,
            result: None,
            waker: None,
        }));
        let weak = Rc::downgrade(&state);
        let callback_reader = reader.clone();
        let completion = Closure::wrap(Box::new(move || {
            let Some(state) = weak.upgrade() else {
                return;
            };
            let waker =
                {
                    let mut state = state.borrow_mut();
                    // Each attempt has separate state. Aborted, duplicate and late events
                    // cannot publish into a later read or resurrect a cancelled producer.
                    if !state.active || state.completed {
                        return;
                    }
                    state.completed = true;
                    state.result = Some(callback_reader.result().map_err(js_error).and_then(
                        |value| {
                            value.dyn_into::<ArrayBuffer>().map_err(|_| {
                                HttpStreamError::Transport("Blob read failed or was aborted".into())
                            })
                        },
                    ));
                    state.waker.take()
                };
            if let Some(waker) = waker {
                waker.wake();
            }
        }) as Box<dyn FnMut()>);
        reader.set_onloadend(Some(completion.as_ref().unchecked_ref()));
        let attempt = Self {
            reader,
            state,
            expected,
            _completion: completion,
        };
        attempt
            .reader
            .read_as_array_buffer(blob)
            .map_err(js_error)?;
        Ok(attempt)
    }
}

impl Drop for ReadAttempt {
    fn drop(&mut self) {
        {
            let mut state = self.state.borrow_mut();
            state.active = false;
            state.result = None;
            state.waker = None;
        }
        self.reader.set_onloadend(None);
        if self.reader.ready_state() == FileReader::LOADING {
            self.reader.abort();
        }
    }
}

fn js_error(error: JsValue) -> HttpStreamError {
    HttpStreamError::Transport(format!("browser Blob read failed: {error:?}"))
}
