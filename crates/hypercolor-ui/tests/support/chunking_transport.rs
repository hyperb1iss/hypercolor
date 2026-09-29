//! A daemon stand-in that answers media routes in fixed-size chunks, the way
//! a bridged transport delivers a body larger than one frame.

#![allow(dead_code, reason = "each test binary uses a subset of the stub")]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::poll_fn;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use hypercolor_ui::api::http_transport::{
    HttpBody, HttpBodySource, HttpCancellation, HttpHeader, HttpRequest, HttpResponse,
    HttpStreamError, HttpStreamFuture, HttpStreamRequest, HttpStreamResponse, HttpTransport,
    HttpTransportError, HttpTransportFuture,
};

/// Holds every streamed answer until opened, so a test can observe requests
/// while they are still in flight.
#[derive(Default)]
pub struct Gate {
    open: Cell<bool>,
    waiters: RefCell<Vec<Waker>>,
}

impl Gate {
    pub fn open(&self) {
        self.open.set(true);
        for waker in self.waiters.take() {
            waker.wake();
        }
    }

    async fn passed(&self) {
        poll_fn(|context| {
            if self.open.get() {
                Poll::Ready(())
            } else {
                self.waiters.borrow_mut().push(context.waker().clone());
                Poll::Pending
            }
        })
        .await;
    }
}

/// One canned answer: `prefix` followed by `filler` zero bytes, generated as
/// it is read so a huge body costs no memory.
#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub content_type: Option<&'static str>,
    pub prefix: Rc<[u8]>,
    pub filler: usize,
}

impl Reply {
    pub fn ok(content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status: 200,
            content_type: Some(content_type),
            prefix: body.into(),
            filler: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.prefix.len() + self.filler
    }

    fn headers(&self) -> Vec<HttpHeader> {
        self.content_type
            .map(|value| HttpHeader {
                name: "Content-Type".to_owned(),
                value: value.to_owned(),
            })
            .into_iter()
            .collect()
    }

    fn bytes(&self) -> Vec<u8> {
        let mut bytes = self.prefix.to_vec();
        bytes.resize(self.len(), 0);
        bytes
    }
}

/// A WebP cover of `len` bytes: a real RIFF/WEBP header over filler.
pub fn webp(len: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(len);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&u32::try_from(len - 8).expect("small cover").to_le_bytes());
    bytes.extend_from_slice(b"WEBPVP8L");
    bytes.extend((bytes.len()..len).map(|index| (index % 251) as u8));
    bytes
}

pub const SVG: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;

#[derive(Default)]
pub struct ChunkingTransport {
    pub replies: HashMap<String, Reply>,
    pub chunk: usize,
    /// Answer `send_stream` with `Unsupported`, as a buffered transport does.
    pub buffered_only: bool,
    /// Hold streamed answers until the gate opens.
    pub gate: Option<Rc<Gate>>,
    pub requests: RefCell<Vec<String>>,
    /// Each streamed exchange's cancellation, in request order.
    pub exchanges: RefCell<Vec<HttpCancellation>>,
    pub chunk_sizes: Rc<RefCell<Vec<usize>>>,
}

impl ChunkingTransport {
    pub fn new(chunk: usize) -> Self {
        Self {
            chunk,
            ..Self::default()
        }
    }

    pub fn reply(mut self, route: &str, reply: Reply) -> Self {
        self.replies.insert(route.to_owned(), reply);
        self
    }

    pub fn gated(mut self, gate: Rc<Gate>) -> Self {
        self.gate = Some(gate);
        self
    }

    fn answer(&self, path: &str) -> Reply {
        self.requests.borrow_mut().push(path.to_owned());
        self.replies.get(path).cloned().unwrap_or(Reply {
            status: 404,
            content_type: Some("application/json"),
            prefix: Rc::from(&b"{}"[..]),
            filler: 0,
        })
    }
}

struct ChunkedBody {
    reply: Reply,
    offset: usize,
    chunk: usize,
    sizes: Rc<RefCell<Vec<usize>>>,
}

impl HttpBodySource for ChunkedBody {
    fn exact_length(&self) -> Option<u64> {
        None
    }

    fn poll_chunk(
        &mut self,
        _: &mut Context<'_>,
        maximum: NonZeroUsize,
    ) -> Poll<Result<Option<Vec<u8>>, HttpStreamError>> {
        let remaining = self.reply.len() - self.offset;
        if remaining == 0 {
            return Poll::Ready(Ok(None));
        }
        let take = remaining.min(self.chunk).min(maximum.get());
        let bytes = (self.offset..self.offset + take)
            .map(|index| self.reply.prefix.get(index).copied().unwrap_or(0))
            .collect::<Vec<_>>();
        self.offset += take;
        self.sizes.borrow_mut().push(take);
        Poll::Ready(Ok(Some(bytes)))
    }

    fn cancel(&mut self) {}
}

impl HttpTransport for ChunkingTransport {
    fn send(&self, request: HttpRequest) -> HttpTransportFuture<'_> {
        let reply = self.answer(&request.path);
        Box::pin(async move {
            Ok::<_, HttpTransportError>(HttpResponse {
                status: reply.status,
                headers: reply.headers(),
                body: reply.bytes(),
            })
        })
    }

    fn send_stream(&self, request: HttpStreamRequest) -> HttpStreamFuture<'_> {
        let cancellation = request.body.cancellation();
        self.exchanges.borrow_mut().push(cancellation.clone());
        let gate = self.gate.clone();
        let buffered_only = self.buffered_only;
        let reply = (!buffered_only).then(|| self.answer(&request.path));
        let chunk = self.chunk;
        let sizes = Rc::clone(&self.chunk_sizes);
        let body_cancellation = cancellation.clone();
        HttpStreamFuture::new(cancellation, async move {
            let Some(reply) = reply else {
                return Err(HttpStreamError::Unsupported);
            };
            if let Some(gate) = gate {
                gate.passed().await;
            }
            // Upload the request body, as a transport does before it answers;
            // an owner dropped mid-upload would cancel the exchange.
            let mut upload = request.body;
            let upload_chunk = NonZeroUsize::new(16 * 1024).expect("positive chunk");
            while upload.read_chunk(upload_chunk).await?.is_some() {}
            Ok(HttpStreamResponse {
                status: reply.status,
                headers: reply.headers(),
                body: HttpBody::new(
                    Box::new(ChunkedBody {
                        reply,
                        offset: 0,
                        chunk,
                        sizes,
                    }),
                    body_cancellation,
                ),
            })
        })
    }
}
