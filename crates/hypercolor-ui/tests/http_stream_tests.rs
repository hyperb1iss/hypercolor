use std::cell::{Cell, RefCell};
use std::future::Future;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use hypercolor_ui::api::http_transport::{
    HttpBody, HttpBodySink, HttpBodySource, HttpCancellation, HttpMethod, HttpRequest,
    HttpStreamError, HttpStreamFuture, HttpStreamRequest, HttpStreamResponse, HttpTransport,
    HttpTransportFuture,
};

#[derive(Default)]
struct Probe {
    reads: RefCell<Vec<usize>>,
    cancels: Cell<usize>,
    pending: Cell<bool>,
}

struct Source {
    remaining: usize,
    declared: Option<u64>,
    probe: Rc<Probe>,
    failure: Option<HttpStreamError>,
    invalid: bool,
}
impl HttpBodySource for Source {
    fn exact_length(&self) -> Option<u64> {
        self.declared
    }
    fn poll_chunk(
        &mut self,
        _: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Vec<u8>>, HttpStreamError>> {
        self.probe.reads.borrow_mut().push(maximum.get());
        if self.probe.pending.get() {
            return Poll::Pending;
        }
        if let Some(error) = self.failure.take() {
            return Poll::Ready(Err(error));
        }
        if self.invalid {
            return Poll::Ready(Ok(Some(vec![0; maximum.get() + 1])));
        }
        if self.remaining == 0 {
            return Poll::Ready(Ok(None));
        }
        let length = self.remaining.min(maximum.get());
        self.remaining -= length;
        Poll::Ready(Ok(Some(vec![b'1'; length])))
    }
    fn cancel(&mut self) {
        self.probe.cancels.set(self.probe.cancels.get() + 1);
    }
}

fn body(length: usize, declared: Option<u64>) -> (HttpBody, Rc<Probe>) {
    let probe = Rc::new(Probe::default());
    (
        HttpBody::new(
            Box::new(Source {
                remaining: length,
                declared,
                probe: Rc::clone(&probe),
                failure: None,
                invalid: false,
            }),
            HttpCancellation::new(),
        ),
        probe,
    )
}
fn size(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("positive test size")
}

#[derive(Default)]
struct WakeCount(AtomicUsize);
impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
fn ready<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(WakeCount::default()));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("test expected an immediately ready future"),
    }
}

#[test]
fn source_only_reads_requested_capacity_and_checks_exact_length() {
    let (mut body, probe) = body(7, Some(7));
    assert_eq!(body.exact_length(), Some(7));
    assert!(probe.reads.borrow().is_empty());
    assert_eq!(
        ready(body.read_chunk(size(3))).expect("first chunk"),
        Some(vec![b'1'; 3])
    );
    assert_eq!(
        ready(body.read_chunk(size(2))).expect("second chunk"),
        Some(vec![b'1'; 2])
    );
    assert_eq!(
        ready(body.read_chunk(size(3))).expect("last chunk"),
        Some(vec![b'1'; 2])
    );
    assert_eq!(ready(body.read_chunk(size(3))).expect("EOF"), None);
    drop(body);
    assert_eq!(*probe.reads.borrow(), [3, 2, 3, 3]);
    assert_eq!(probe.cancels.get(), 0);
}

#[test]
fn unknown_length_and_empty_sources_terminate_normally() {
    let (mut unknown, _) = body(1, None);
    assert_eq!(unknown.exact_length(), None);
    assert_eq!(
        ready(unknown.read_chunk(size(8))).expect("one byte"),
        Some(vec![b'1'])
    );
    assert_eq!(ready(unknown.read_chunk(size(8))).expect("EOF"), None);
    let (mut empty, _) = body(0, Some(0));
    assert_eq!(ready(empty.read_chunk(size(1))).expect("empty EOF"), None);
}

#[test]
fn early_eof_and_excess_declared_bytes_are_terminal_errors() {
    for (actual, declared) in [(2, 3), (3, 2)] {
        let (mut stream, probe) = body(actual, Some(declared));
        let first = ready(stream.read_chunk(size(4)));
        let result = if first.is_err() {
            first
        } else {
            ready(stream.read_chunk(size(4)))
        };
        assert_eq!(result, Err(HttpStreamError::LengthMismatch));
        assert_eq!(
            ready(stream.read_chunk(size(4))),
            Err(HttpStreamError::Cancelled)
        );
        drop(stream);
        assert_eq!(probe.cancels.get(), 1);
    }
}

#[test]
fn failed_and_oversized_reads_cancel_the_source_once() {
    for invalid in [false, true] {
        let probe = Rc::new(Probe::default());
        let expected = if invalid {
            HttpStreamError::InvalidChunk
        } else {
            HttpStreamError::Transport("source lost".to_owned())
        };
        let mut body = HttpBody::new(
            Box::new(Source {
                remaining: 4,
                declared: None,
                probe: Rc::clone(&probe),
                failure: if invalid {
                    None
                } else {
                    Some(expected.clone())
                },
                invalid,
            }),
            HttpCancellation::new(),
        );
        assert_eq!(ready(body.read_chunk(size(2))), Err(expected));
        drop(body);
        assert_eq!(probe.cancels.get(), 1);
    }
}

#[test]
fn cancellation_wakes_a_pending_read_and_releases_its_source() {
    let (mut body, probe) = body(100, None);
    probe.pending.set(true);
    let cancellation = body.cancellation();
    let count = Arc::new(WakeCount::default());
    let waker = Waker::from(Arc::clone(&count));
    let mut context = Context::from_waker(&waker);
    {
        let mut read = std::pin::pin!(body.read_chunk(size(4)));
        assert!(read.as_mut().poll(&mut context).is_pending());
        cancellation.cancel();
        cancellation.cancel();
        assert_eq!(count.0.load(Ordering::SeqCst), 1);
        assert_eq!(
            read.as_mut().poll(&mut context),
            Poll::Ready(Err(HttpStreamError::Cancelled))
        );
    }
    drop(body);
    assert_eq!(probe.cancels.get(), 1);
    assert_eq!(*probe.reads.borrow(), [4]);
}

#[test]
fn early_body_drop_cancels_the_transport_signal() {
    let (body, probe) = body(100, None);
    let cancellation = body.cancellation();
    drop(body);
    assert!(cancellation.is_cancelled());
    assert_eq!(probe.cancels.get(), 1);
}

#[test]
fn json_collection_checks_known_and_unknown_limits_before_retaining_overflow() {
    let (known, probe) = body(5, Some(5));
    assert_eq!(
        ready(known.collect_json::<serde_json::Value>(4, size(3))),
        Err(HttpStreamError::BodyTooLarge)
    );
    assert!(probe.reads.borrow().is_empty());
    let (unknown, probe) = body(5, None);
    assert_eq!(
        ready(unknown.collect_json::<serde_json::Value>(4, size(3))),
        Err(HttpStreamError::BodyTooLarge)
    );
    assert_eq!(*probe.reads.borrow(), [3, 2]);
    let (exact, _) = body(4, None);
    assert_eq!(
        ready(exact.collect_json::<u64>(4, size(3))).expect("exact limit JSON"),
        1111
    );
    let (empty, _) = body(0, None);
    assert!(matches!(
        ready(empty.collect_json::<u64>(0, size(3))),
        Err(HttpStreamError::InvalidJson(_))
    ));
}

#[derive(Default)]
struct SinkProbe {
    accept: Cell<bool>,
    bytes: Cell<usize>,
    finished: Cell<bool>,
    finish_pending: Cell<bool>,
    finish_failure: Cell<bool>,
    cancelled: Cell<bool>,
}
struct Sink(Rc<SinkProbe>);
impl HttpBodySink for Sink {
    fn poll_write(
        &mut self,
        _: &mut Context<'_>,
        chunk: &[u8],
    ) -> Poll<Result<(), HttpStreamError>> {
        if !self.0.accept.get() {
            return Poll::Pending;
        }
        self.0.bytes.set(self.0.bytes.get() + chunk.len());
        Poll::Ready(Ok(()))
    }
    fn poll_finish(&mut self, _: &mut Context<'_>) -> Poll<Result<(), HttpStreamError>> {
        if self.0.finish_pending.get() {
            return Poll::Pending;
        }
        if self.0.finish_failure.get() {
            return Poll::Ready(Err(HttpStreamError::Transport(
                "sink finish failed".to_owned(),
            )));
        }
        self.0.finished.set(true);
        Poll::Ready(Ok(()))
    }
    fn cancel(&mut self) {
        self.0.cancelled.set(true);
    }
}

#[test]
fn a_pending_sink_prevents_source_read_ahead() {
    let (body, source) = body(10, None);
    let probe = Rc::new(SinkProbe::default());
    let mut sink = Sink(Rc::clone(&probe));
    let waker = Waker::from(Arc::new(WakeCount::default()));
    let mut context = Context::from_waker(&waker);
    {
        let mut copy = std::pin::pin!(body.copy_to(&mut sink, size(3)));
        assert!(copy.as_mut().poll(&mut context).is_pending());
        assert_eq!(*source.reads.borrow(), [3]);
        assert_eq!(probe.bytes.get(), 0);
        probe.accept.set(true);
        let mut complete = false;
        for _ in 0..8 {
            if let Poll::Ready(result) = copy.as_mut().poll(&mut context) {
                assert_eq!(result, Ok(10));
                complete = true;
                break;
            }
        }
        assert!(
            complete,
            "bounded copy completes after cooperative chunk yields"
        );
    }
    assert!(probe.finished.get());
    assert!(!probe.cancelled.get());
    assert_eq!(probe.bytes.get(), 10);
}

#[test]
fn dropping_a_pending_copy_releases_source_and_sink() {
    let (body, source) = body(10, None);
    let probe = Rc::new(SinkProbe::default());
    let mut sink = Sink(Rc::clone(&probe));
    let waker = Waker::from(Arc::new(WakeCount::default()));
    let mut context = Context::from_waker(&waker);
    {
        let mut copy = std::pin::pin!(body.copy_to(&mut sink, size(3)));
        assert!(copy.as_mut().poll(&mut context).is_pending());
    }
    assert!(probe.cancelled.get());
    assert_eq!(source.cancels.get(), 1);
}

#[test]
fn headers_arrive_without_reading_the_response_body() {
    let (body, probe) = body(1_000_000, None);
    let cancellation = body.cancellation();
    let response = ready(HttpStreamFuture::new(cancellation.clone(), async move {
        Ok(HttpStreamResponse {
            status: 200,
            headers: Vec::new(),
            body,
        })
    }))
    .expect("headers ready");
    assert_eq!(response.status, 200);
    assert!(probe.reads.borrow().is_empty());
    assert!(!cancellation.is_cancelled());
    drop(response);
    assert!(cancellation.is_cancelled());
}

#[test]
fn cancellation_and_drop_release_pending_headers_futures() {
    struct Released(Rc<Cell<bool>>);
    impl Drop for Released {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    for explicit in [false, true] {
        let cancellation = HttpCancellation::new();
        let released = Rc::new(Cell::new(false));
        let owner = Released(Rc::clone(&released));
        let mut future = HttpStreamFuture::new(cancellation.clone(), async move {
            let _owner = owner;
            std::future::pending::<Result<HttpStreamResponse, HttpStreamError>>().await
        });
        let waker = Waker::from(Arc::new(WakeCount::default()));
        let mut context = Context::from_waker(&waker);
        assert!(
            std::pin::Pin::new(&mut future)
                .poll(&mut context)
                .is_pending()
        );
        if explicit {
            cancellation.cancel();
            assert!(matches!(
                std::pin::Pin::new(&mut future).poll(&mut context),
                Poll::Ready(Err(HttpStreamError::Cancelled))
            ));
            assert!(released.get());
        }
        drop(future);
        assert!(released.get());
        assert!(cancellation.is_cancelled());
    }
}

#[test]
fn old_transports_explicitly_refuse_streaming_without_buffering() {
    struct Buffered;
    impl HttpTransport for Buffered {
        fn send(&self, _: HttpRequest) -> HttpTransportFuture<'_> {
            panic!("must not fallback to buffered send")
        }
    }
    let (body, probe) = body(1_000_000, None);
    let response = ready(Buffered.send_stream(HttpStreamRequest {
        method: HttpMethod::Post,
        path: "/api/v1/assets".to_owned(),
        headers: Vec::new(),
        body,
    }));
    assert!(matches!(response, Err(HttpStreamError::Unsupported)));
    assert!(probe.reads.borrow().is_empty());
    assert_eq!(probe.cancels.get(), 1);
}

#[test]
fn early_rejection_discards_upload_but_preserves_response() {
    let (upload, upload_probe) = body(1_000_000, None);
    let cancellation = upload.cancellation();
    let response_probe = Rc::new(Probe::default());
    let response_body = HttpBody::new(
        Box::new(Source {
            remaining: 3,
            declared: Some(3),
            probe: Rc::clone(&response_probe),
            failure: None,
            invalid: false,
        }),
        cancellation.clone(),
    );
    let response = ready(HttpStreamFuture::new(cancellation.clone(), async move {
        upload.discard();
        Ok(HttpStreamResponse {
            status: 413,
            headers: Vec::new(),
            body: response_body,
        })
    }))
    .expect("early rejection headers");
    assert_eq!(response.status, 413);
    assert_eq!(upload_probe.cancels.get(), 1);
    assert!(upload_probe.reads.borrow().is_empty());
    assert!(!cancellation.is_cancelled());
    assert_eq!(
        ready(response.body.collect_json::<u64>(3, size(2)))
            .expect("rejection body remains readable"),
        111
    );
    assert!(!cancellation.is_cancelled());
}

#[test]
fn pending_or_failed_finish_retains_exchange_cancellation_ownership() {
    for pending in [false, true] {
        let (body, source) = body(1, Some(1));
        let cancellation = body.cancellation();
        let probe = Rc::new(SinkProbe::default());
        probe.accept.set(true);
        probe.finish_pending.set(pending);
        probe.finish_failure.set(!pending);
        let mut sink = Sink(Rc::clone(&probe));
        let waker = Waker::from(Arc::new(WakeCount::default()));
        let mut context = Context::from_waker(&waker);
        {
            let mut copy = std::pin::pin!(body.copy_to(&mut sink, size(4)));
            assert!(
                copy.as_mut().poll(&mut context).is_pending(),
                "yield after first chunk"
            );
            let finishing = copy.as_mut().poll(&mut context);
            if pending {
                assert!(finishing.is_pending());
                assert!(!cancellation.is_cancelled());
            } else {
                assert_eq!(
                    finishing,
                    Poll::Ready(Err(HttpStreamError::Transport(
                        "sink finish failed".to_owned()
                    )))
                );
            }
            assert_eq!(
                source.reads.borrow().len(),
                2,
                "source reached EOF before sink finished"
            );
        }
        assert!(cancellation.is_cancelled());
        assert!(probe.cancelled.get());
        assert!(!probe.finished.get());
    }
}
