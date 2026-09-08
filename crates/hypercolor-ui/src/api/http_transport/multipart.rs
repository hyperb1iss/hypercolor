//! Ordered multipart fields with a canonical, demand-driven wire encoding.

use std::any::Any;
use std::num::NonZeroUsize;
use std::task::{Context, Poll};

use super::{HttpBody, HttpBodySource, HttpCancellation, HttpHeader, HttpStreamError};

/// Multipart fields distinguish strings from files, including empty filenames.
/// Native adapters must preserve this distinction and the original field order.
pub struct HttpMultipartField {
    name: String,
    value: FieldValue,
    header: Vec<u8>,
}

enum FieldValue {
    Text(String),
    File {
        filename: String,
        content_type: String,
        source: Box<dyn HttpBodySource>,
        length: Option<u64>,
        consumed: u64,
        finished: bool,
    },
}

/// Borrowed immutable semantics for selecting an equivalent native representation.
pub enum HttpMultipartValue<'a> {
    Text(&'a str),
    File {
        filename: &'a str,
        content_type: &'a str,
        source: &'a dyn HttpBodySource,
    },
}

impl HttpMultipartField {
    /// Text uses HTML multipart newline normalization and has no Content-Type header.
    #[must_use]
    pub fn text(name: &str, value: &str) -> Self {
        Self {
            name: normalize_newlines(name),
            value: FieldValue::Text(normalize_newlines(value)),
            header: Vec::new(),
        }
    }

    /// File metadata is explicit: pass `blob` for an unnamed Blob and the File's
    /// actual name for a File. Empty MIME becomes application/octet-stream, matching
    /// browser FormData. MIME uses the File API printable-ASCII/lowercase rules.
    #[must_use]
    pub fn file(
        name: &str,
        filename: String,
        content_type: &str,
        source: Box<dyn HttpBodySource>,
    ) -> Self {
        let length = source.exact_length();
        let content_type = if content_type.is_empty()
            || !content_type
                .bytes()
                .all(|byte| (0x20..=0x7e).contains(&byte))
        {
            "application/octet-stream".to_owned()
        } else {
            content_type.to_ascii_lowercase()
        };
        Self {
            name: normalize_newlines(name),
            value: FieldValue::File {
                filename,
                content_type,
                source,
                length,
                consumed: 0,
                finished: false,
            },
            header: Vec::new(),
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn value(&self) -> HttpMultipartValue<'_> {
        match &self.value {
            FieldValue::Text(value) => HttpMultipartValue::Text(value),
            FieldValue::File {
                filename,
                content_type,
                source,
                ..
            } => HttpMultipartValue::File {
                filename,
                content_type,
                source: source.as_ref(),
            },
        }
    }

    fn length(&self) -> Option<u64> {
        match &self.value {
            FieldValue::Text(value) => Some(value.len() as u64),
            FieldValue::File { length, .. } => *length,
        }
    }

    fn cancel(&mut self) {
        if let FieldValue::File {
            source, finished, ..
        } = &mut self.value
            && !*finished
        {
            *finished = true;
            source.cancel();
        }
    }
}

impl Drop for HttpMultipartField {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// One encoding and one ordered source list. Native adapters may inspect this source
/// through a pristine HttpBody and use an equivalent native multipart representation.
pub struct HttpMultipartSource {
    fields: Vec<HttpMultipartField>,
    boundary: String,
    ending: Vec<u8>,
    length: Option<u64>,
    index: usize,
    offset: usize,
    phase: Phase,
    started: bool,
}

#[derive(Clone, Copy)]
enum Phase {
    Header,
    Value,
    Separator,
    Ending,
    Done,
}

impl HttpMultipartSource {
    /// The caller supplies an unpredictable boundary (27..=70 ASCII token characters).
    /// Random boundary generation belongs to the platform, without reading file data.
    ///
    /// # Errors
    /// Rejects invalid boundaries or an encoded length exceeding u64.
    pub fn new(
        boundary: String,
        mut fields: Vec<HttpMultipartField>,
    ) -> Result<Self, HttpStreamError> {
        if !(27..=70).contains(&boundary.len())
            || !boundary
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"'-_".contains(&byte))
        {
            return Err(HttpStreamError::Transport(
                "invalid multipart boundary".into(),
            ));
        }
        let ending = format!("--{boundary}--\r\n").into_bytes();
        let mut known_bytes = ending.len() as u64;
        let mut exact = true;
        for field in &mut fields {
            let mut header = format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"",
                escape(&field.name)
            );
            if let FieldValue::File {
                filename,
                content_type,
                ..
            } = &field.value
            {
                header.push_str(&format!(
                    "; filename=\"{}\"\r\nContent-Type: {content_type}",
                    escape(filename)
                ));
            }
            header.push_str("\r\n\r\n");
            field.header = header.into_bytes();
            let body_length = field.length();
            exact &= body_length.is_some();
            // Even with unknown parts, the known lower bound must fit the wire length.
            known_bytes = known_bytes
                .checked_add(field.header.len() as u64)
                .and_then(|value| value.checked_add(body_length.unwrap_or(0)))
                .and_then(|value| value.checked_add(2))
                .ok_or(HttpStreamError::LengthMismatch)?;
        }
        Ok(Self {
            fields,
            boundary,
            ending,
            length: exact.then_some(known_bytes),
            index: 0,
            offset: 0,
            phase: Phase::Header,
            started: false,
        })
    }

    /// Canonical encoded body and its matching Content-Type header.
    #[must_use]
    pub fn into_body(self, cancellation: HttpCancellation) -> (HttpBody, HttpHeader) {
        let header = HttpHeader {
            name: "Content-Type".into(),
            value: format!("multipart/form-data; boundary={}", self.boundary),
        };
        (HttpBody::new(Box::new(self), cancellation), header)
    }

    /// Pristine ordered fields. Native adapters must also verify every file source
    /// has a native representation with equivalent filename and MIME semantics.
    #[must_use]
    pub fn fields(&self) -> Option<&[HttpMultipartField]> {
        (!self.started).then_some(self.fields.as_slice())
    }

    fn advance(&mut self, phase: Phase) {
        self.phase = phase;
        self.offset = 0;
    }

    fn poll_next(
        &mut self,
        context: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Vec<u8>>, HttpStreamError>> {
        loop {
            match self.phase {
                Phase::Header => {
                    if self.index == self.fields.len() {
                        self.advance(Phase::Ending);
                        continue;
                    }
                    if let Some(chunk) =
                        take_bytes(&self.fields[self.index].header, &mut self.offset, maximum)
                    {
                        return Poll::Ready(Ok(Some(chunk)));
                    }
                    self.advance(Phase::Value);
                }
                Phase::Value => {
                    match &mut self.fields[self.index].value {
                        FieldValue::Text(value) => {
                            if let Some(chunk) =
                                take_bytes(value.as_bytes(), &mut self.offset, maximum)
                            {
                                return Poll::Ready(Ok(Some(chunk)));
                            }
                        }
                        FieldValue::File {
                            source,
                            length,
                            consumed,
                            finished,
                            ..
                        } => match source.poll_chunk(context, maximum) {
                            Poll::Pending => return Poll::Pending,
                            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                            Poll::Ready(Ok(Some(chunk))) => {
                                if chunk.is_empty() || chunk.len() > maximum.get() {
                                    return Poll::Ready(Err(HttpStreamError::InvalidChunk));
                                }
                                let Some(total) = consumed.checked_add(chunk.len() as u64) else {
                                    return Poll::Ready(Err(HttpStreamError::LengthMismatch));
                                };
                                if length.is_some_and(|length| total > length) {
                                    return Poll::Ready(Err(HttpStreamError::LengthMismatch));
                                }
                                *consumed = total;
                                return Poll::Ready(Ok(Some(chunk)));
                            }
                            Poll::Ready(Ok(None)) => {
                                if length.is_some_and(|length| *consumed != length) {
                                    return Poll::Ready(Err(HttpStreamError::LengthMismatch));
                                }
                                *finished = true;
                            }
                        },
                    }
                    self.advance(Phase::Separator);
                }
                Phase::Separator => {
                    if let Some(chunk) = take_bytes(b"\r\n", &mut self.offset, maximum) {
                        return Poll::Ready(Ok(Some(chunk)));
                    }
                    self.index += 1;
                    self.advance(Phase::Header);
                }
                Phase::Ending => {
                    if let Some(chunk) = take_bytes(&self.ending, &mut self.offset, maximum) {
                        return Poll::Ready(Ok(Some(chunk)));
                    }
                    self.advance(Phase::Done);
                }
                Phase::Done => return Poll::Ready(Ok(None)),
            }
        }
    }
}

impl HttpBodySource for HttpMultipartSource {
    fn source_hint(&self) -> Option<&dyn Any> {
        (!self.started).then_some(self as &dyn Any)
    }
    fn exact_length(&self) -> Option<u64> {
        self.length
    }
    fn poll_chunk(
        &mut self,
        context: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Vec<u8>>, HttpStreamError>> {
        self.started = true;
        let result = self.poll_next(context, maximum);
        if matches!(result, Poll::Ready(Err(_))) {
            self.cancel();
        }
        result
    }
    fn cancel(&mut self) {
        self.started = true;
        self.phase = Phase::Done;
        for field in &mut self.fields {
            field.cancel();
        }
        self.fields.clear();
        self.ending.clear();
    }
}

fn take_bytes(bytes: &[u8], offset: &mut usize, maximum: NonZeroUsize) -> Option<Vec<u8>> {
    if *offset == bytes.len() {
        return None;
    }
    let end = offset.saturating_add(maximum.get()).min(bytes.len());
    let result = bytes[*offset..end].to_vec();
    *offset = end;
    Some(result)
}

fn normalize_newlines(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n")
}

fn escape(value: &str) -> String {
    value
        .replace('\r', "%0D")
        .replace('\n', "%0A")
        .replace('"', "%22")
}
