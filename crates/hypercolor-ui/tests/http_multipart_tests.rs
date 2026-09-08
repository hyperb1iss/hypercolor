use std::any::Any;
use std::cell::Cell;
use std::future::Future;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use hypercolor_ui::api::http_transport::{
    HttpBody, HttpBodySource, HttpCancellation, HttpMultipartField, HttpMultipartSource,
    HttpMultipartValue, HttpStreamError, HttpStreamFuture,
};

const BOUNDARY: &str = "hypercolor-test-boundary-0123456789";

struct Source {
    remaining: usize,
    declared: Option<u64>,
    pending: bool,
    polls: Rc<Cell<usize>>,
    cancels: Rc<Cell<usize>>,
}
impl HttpBodySource for Source {
    fn source_hint(&self) -> Option<&dyn Any> {
        Some(self)
    }
    fn exact_length(&self) -> Option<u64> {
        self.declared
    }
    fn poll_chunk(
        &mut self,
        _: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Vec<u8>>, HttpStreamError>> {
        self.polls.set(self.polls.get() + 1);
        if self.pending {
            return Poll::Pending;
        }
        if self.remaining == 0 {
            return Poll::Ready(Ok(None));
        }
        let count = self.remaining.min(maximum.get());
        self.remaining -= count;
        Poll::Ready(Ok(Some(vec![b'x'; count])))
    }
    fn cancel(&mut self) {
        self.cancels.set(self.cancels.get() + 1);
    }
}
fn source(length: usize, declared: Option<u64>, pending: bool) -> Source {
    Source {
        remaining: length,
        declared,
        pending,
        polls: Rc::default(),
        cancels: Rc::default(),
    }
}
fn maximum(size: usize) -> NonZeroUsize {
    NonZeroUsize::new(size).expect("positive")
}
fn ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("expected ready"),
    }
}
fn collect(body: &mut HttpBody, credit: usize) -> Result<Vec<u8>, HttpStreamError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = ready(body.read_chunk(maximum(credit)))? {
        assert!(!chunk.is_empty() && chunk.len() <= credit);
        bytes.extend(chunk);
    }
    Ok(bytes)
}
fn multipart(fields: Vec<HttpMultipartField>) -> HttpBody {
    HttpMultipartSource::new(BOUNDARY.into(), fields)
        .expect("multipart")
        .into_body(HttpCancellation::new())
        .0
}

#[test]
fn inspection_is_borrowed_and_first_pending_poll_invalidates_it() {
    let source = source(1, Some(1), true);
    let polls = source.polls.clone();
    let cancels = source.cancels.clone();
    let mut body = HttpBody::new(Box::new(source), HttpCancellation::new());
    assert!(body.source_hint().expect("hint").is::<Source>());
    assert_eq!(polls.get(), 0);
    {
        let future = body.read_chunk(maximum(1));
        let mut future = std::pin::pin!(future);
        assert!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
    }
    assert!(body.source_hint().is_none());
    assert_eq!(polls.get(), 1);
    body.cancel();
    assert!(body.source_hint().is_none());
    drop(body);
    assert_eq!(cancels.get(), 1);
}

#[test]
fn externally_cancelled_body_never_exposes_a_native_hint() {
    let body = HttpBody::new(Box::new(source(1, Some(1), false)), HttpCancellation::new());
    body.cancellation().cancel();
    assert!(body.source_hint().is_none());
}

#[test]
fn ordered_text_and_file_fields_match_html_multipart_rules_at_every_credit() {
    for credit in 1..=150 {
        let file = source(3, Some(3), false);
        let polls = file.polls.clone();
        let cancels = file.cancels.clone();
        let (mut body, header) = HttpMultipartSource::new(
            BOUNDARY.into(),
            vec![
                HttpMultipartField::text("same\n\"", "one\rtwo\nthree\r\nfour"),
                HttpMultipartField::file(
                    "same\n\"",
                    "π\n\"\\.bin".into(),
                    "Application/OCTET-STREAM",
                    Box::new(file),
                ),
                HttpMultipartField::text("same\n\"", ""),
            ],
        )
        .expect("multipart")
        .into_body(HttpCancellation::new());
        assert_eq!(polls.get(), 0);
        assert_eq!(
            header.value,
            format!("multipart/form-data; boundary={BOUNDARY}")
        );
        let fields = body
            .source_hint()
            .expect("hint")
            .downcast_ref::<HttpMultipartSource>()
            .expect("source")
            .fields()
            .expect("pristine");
        assert_eq!(fields.len(), 3);
        assert!(matches!(
            fields[0].value(),
            HttpMultipartValue::Text("one\r\ntwo\r\nthree\r\nfour")
        ));
        assert_eq!(fields[1].name(), "same\r\n\"");
        assert!(matches!(
            fields[1].value(),
            HttpMultipartValue::File {
                filename: "π\n\"\\.bin",
                content_type: "application/octet-stream",
                ..
            }
        ));
        let expected = format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"same%0D%0A%22\"\r\n\r\none\r\ntwo\r\nthree\r\nfour\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"same%0D%0A%22\"; filename=\"π%0A%22\\.bin\"\r\nContent-Type: application/octet-stream\r\n\r\nxxx\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"same%0D%0A%22\"\r\n\r\n\r\n--{BOUNDARY}--\r\n"
        );
        assert_eq!(body.exact_length(), Some(expected.len() as u64));
        assert_eq!(
            collect(&mut body, credit).expect("encoded"),
            expected.as_bytes()
        );
        assert!(body.source_hint().is_none());
        drop(body);
        assert_eq!(cancels.get(), 0);
    }
}

#[test]
fn empty_unknown_and_invalid_child_lengths_are_handled_without_read_ahead() {
    assert_eq!(
        collect(&mut multipart(vec![]), 1).expect("empty"),
        format!("--{BOUNDARY}--\r\n").as_bytes()
    );
    let mut unknown = multipart(vec![HttpMultipartField::file(
        "f",
        "".into(),
        "",
        Box::new(source(0, None, false)),
    )]);
    assert_eq!(unknown.exact_length(), None);
    let encoded = String::from_utf8(collect(&mut unknown, 8).expect("empty file")).expect("utf8");
    assert!(
        encoded.contains("filename=\"\"\r\nContent-Type: application/octet-stream\r\n\r\n\r\n")
    );
    for declared in [Some(1), Some(3)] {
        let file = source(2, declared, false);
        let cancels = file.cancels.clone();
        let mut body = multipart(vec![HttpMultipartField::file(
            "f",
            "f".into(),
            "",
            Box::new(file),
        )]);
        assert_eq!(collect(&mut body, 4), Err(HttpStreamError::LengthMismatch));
        assert!(body.cancellation().is_cancelled());
        assert_eq!(cancels.get(), 1);
    }
}

#[test]
fn early_discard_keeps_exchange_owned_and_cancels_unread_files_once() {
    let file = source(10, Some(10), false);
    let cancels = file.cancels.clone();
    let polls = file.polls.clone();
    let body = multipart(vec![HttpMultipartField::file(
        "f",
        "f".into(),
        "",
        Box::new(file),
    )]);
    let cancellation = body.cancellation();
    let exchange = HttpStreamFuture::new(cancellation.clone(), std::future::pending());
    body.discard();
    assert_eq!(polls.get(), 0);
    assert_eq!(cancels.get(), 1);
    assert!(!cancellation.is_cancelled());
    drop(exchange);
    assert!(cancellation.is_cancelled());
}

#[test]
fn metadata_rejects_boundary_injection_and_canonicalizes_file_api_mime() {
    for boundary in [
        "short",
        "hypercolor-test-boundary-0123456789\r\nInjected: 1",
        &"x".repeat(71),
    ] {
        assert!(HttpMultipartSource::new(boundary.into(), vec![]).is_err());
    }
    for mime in ["", "image/png\r\nX-Evil: true", "café"] {
        let field =
            HttpMultipartField::file("f", "f".into(), mime, Box::new(source(0, Some(0), false)));
        assert!(matches!(
            field.value(),
            HttpMultipartValue::File {
                content_type: "application/octet-stream",
                ..
            }
        ));
    }
    assert!(
        HttpMultipartSource::new(
            BOUNDARY.into(),
            vec![HttpMultipartField::file(
                "f",
                "f".into(),
                "",
                Box::new(source(0, Some(u64::MAX), false))
            )]
        )
        .is_err()
    );
}

#[test]
fn unknown_part_does_not_hide_overflow_in_the_known_lower_bound() {
    let fields = vec![
        HttpMultipartField::file("unknown", "f".into(), "", Box::new(source(0, None, false))),
        HttpMultipartField::file(
            "huge",
            "f".into(),
            "",
            Box::new(source(0, Some(u64::MAX), false)),
        ),
    ];
    assert!(HttpMultipartSource::new(BOUNDARY.into(), fields).is_err());
}
