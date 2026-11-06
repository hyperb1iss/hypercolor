//! Bounded lines and framed writes: endless lines and slow peers fail at
//! the bound, and a frame that does not go out whole costs the stream.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use hypercolor_driver_support::framing::{BoundedLines, FrameWriteError, FramedWriter, LineError};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::time::Instant;

const LIMIT: usize = 64;

fn soon() -> Instant {
    Instant::now() + Duration::from_secs(2)
}

fn after(millis: u64) -> Instant {
    Instant::now() + Duration::from_millis(millis)
}

fn lines(reader: DuplexStream) -> BoundedLines<DuplexStream> {
    BoundedLines::new(reader, LIMIT)
}

#[tokio::test]
async fn lines_split_on_crlf_and_lf() {
    let (mut writer, reader) = tokio::io::duplex(256);
    writer
        .write_all(b"{\"id\":1}\r\n{\"method\":\"props\"}\nlast")
        .await
        .expect("write");
    drop(writer);
    let mut lines = lines(reader);
    let first = lines.next(soon()).await.expect("first line");
    assert_eq!(first.as_deref(), Some(b"{\"id\":1}".as_slice()));
    let second = lines.next(soon()).await.expect("second line");
    assert_eq!(
        second.as_deref(),
        Some(b"{\"method\":\"props\"}".as_slice())
    );
    // An unterminated tail at end of stream is not a line.
    assert_eq!(lines.next(soon()).await.expect("end of stream"), None);
}

#[tokio::test]
async fn a_line_at_the_cap_is_accepted_and_one_over_is_refused() {
    for terminator in [b"\r\n".as_slice(), b"\n"] {
        let (mut writer, reader) = tokio::io::duplex(1024);
        let at_cap = [vec![b'x'; LIMIT].as_slice(), terminator].concat();
        let over_cap = [vec![b'x'; LIMIT + 1].as_slice(), terminator].concat();
        writer.write_all(&at_cap).await.expect("write");
        writer.write_all(&over_cap).await.expect("write");
        let mut lines = lines(reader);
        let line = lines.next(soon()).await.expect("line at the cap");
        assert_eq!(line.map(|line| line.len()), Some(LIMIT));
        let error = lines.next(soon()).await.expect_err("line over the cap");
        assert!(matches!(error, LineError::TooLong { limit: LIMIT }));
    }
}

#[tokio::test]
async fn an_endless_line_stops_at_the_cap() {
    let (mut writer, reader) = tokio::io::duplex(64 * 1024);
    let flood = tokio::spawn(async move {
        let chunk = vec![b'a'; 1024];
        while writer.write_all(&chunk).await.is_ok() {}
    });
    let mut lines = lines(reader);
    let error = lines.next(soon()).await.expect_err("endless line");
    assert!(matches!(error, LineError::TooLong { .. }));
    flood.abort();
}

#[tokio::test]
async fn the_tail_of_an_oversized_line_never_reads_as_a_line() {
    let (mut writer, reader) = tokio::io::duplex(1024);
    let oversized = [vec![b'x'; LIMIT * 2].as_slice(), b"tail\n"].concat();
    writer.write_all(&oversized).await.expect("write");
    writer.write_all(b"next\n").await.expect("write");
    let mut lines = lines(reader);
    for _ in 0..3 {
        let error = lines.next(soon()).await.expect_err("still oversized");
        assert!(matches!(error, LineError::TooLong { .. }));
    }
    let error = lines
        .next_after_idle(Duration::from_secs(1))
        .await
        .expect_err("idle reads are refused too");
    assert!(matches!(error, LineError::TooLong { .. }));
}

#[tokio::test]
async fn a_slow_drip_cannot_extend_a_line_past_its_deadline() {
    let (mut writer, reader) = tokio::io::duplex(1024);
    tokio::spawn(async move {
        while writer.write_all(b"x").await.is_ok() {
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    });
    // Unlimited in length, so only the deadline can end this.
    let mut lines = BoundedLines::new(reader, usize::MAX - 2);
    let started = Instant::now();
    let error = lines.next(after(200)).await.expect_err("dripped line");
    assert!(matches!(error, LineError::TimedOut), "{error}");
    let elapsed = started.elapsed();
    assert!(
        (Duration::from_millis(150)..Duration::from_secs(2)).contains(&elapsed),
        "the deadline is absolute: {elapsed:?}"
    );
}

/// Always has another short line ready, so a reader never has to wait.
struct ReadyLines;

impl AsyncRead for ReadyLines {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let line = b"not the reply\n";
        while buffer.remaining() >= line.len() {
            buffer.put_slice(line);
        }
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn a_flood_of_short_lines_cannot_outrun_a_shared_deadline() {
    let mut lines = BoundedLines::new(ReadyLines, LIMIT);
    let deadline = after(100);
    let started = Instant::now();
    let error = loop {
        match lines.next(deadline).await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("the flood never ends"),
            Err(error) => break error,
        }
    };
    assert!(matches!(error, LineError::TimedOut), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the loop ends at the shared deadline: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_missed_deadline_keeps_the_partial_line() {
    let (mut writer, reader) = tokio::io::duplex(1024);
    let mut lines = lines(reader);
    writer.write_all(b"half").await.expect("write");
    let error = lines.next(after(50)).await.expect_err("unfinished line");
    assert!(matches!(error, LineError::TimedOut));
    writer.write_all(b" and half\n").await.expect("write");
    let line = lines.next(soon()).await.expect("resumed line");
    assert_eq!(line.as_deref(), Some(b"half and half".as_slice()));
}

#[tokio::test]
async fn dropping_a_read_in_progress_loses_nothing() {
    let (mut writer, reader) = tokio::io::duplex(1024);
    let mut lines = lines(reader);
    writer.write_all(b"half").await.expect("write");
    let cancelled = tokio::time::timeout(Duration::from_millis(50), lines.next(soon())).await;
    assert!(cancelled.is_err(), "the caller gave up first");
    writer.write_all(b" and half\n").await.expect("write");
    let line = lines.next(soon()).await.expect("resumed line");
    assert_eq!(line.as_deref(), Some(b"half and half".as_slice()));
}

#[tokio::test]
async fn an_idle_connection_may_stay_silent_past_the_completion_bound() {
    let (mut writer, reader) = tokio::io::duplex(1024);
    let mut lines = lines(reader);
    let completion = Duration::from_millis(50);
    let idle = tokio::spawn(async move { lines.next_after_idle(completion).await });
    tokio::time::sleep(completion * 3).await;
    assert!(!idle.is_finished(), "silence is not a failure");
    writer.write_all(b"pushed\n").await.expect("write");
    let line = idle.await.expect("join").expect("pushed line");
    assert_eq!(line.as_deref(), Some(b"pushed".as_slice()));
}

#[tokio::test]
async fn a_started_line_must_finish_within_the_completion_bound() {
    let (mut writer, reader) = tokio::io::duplex(1024);
    let mut lines = lines(reader);
    writer.write_all(b"started").await.expect("write");
    let started = Instant::now();
    let error = lines
        .next_after_idle(Duration::from_millis(100))
        .await
        .expect_err("stalled line");
    assert!(matches!(error, LineError::TimedOut));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn cancelling_an_idle_read_does_not_renew_the_completion_bound() {
    let (mut writer, reader) = tokio::io::duplex(1024);
    let mut lines = lines(reader);
    writer.write_all(b"started").await.expect("write");
    let completion = Duration::from_millis(200);
    let started = Instant::now();
    let error = loop {
        let attempt = tokio::time::timeout(completion / 4, lines.next_after_idle(completion));
        match attempt.await {
            Ok(result) => break result.expect_err("stalled line"),
            Err(_) => assert!(
                started.elapsed() < completion * 4,
                "a cancelled call renewed the bound: {:?}",
                started.elapsed()
            ),
        }
    };
    assert!(matches!(error, LineError::TimedOut), "{error}");
}

#[tokio::test]
async fn a_line_started_by_next_keeps_its_first_byte_clock() {
    let (mut writer, reader) = tokio::io::duplex(1024);
    let mut lines = lines(reader);
    writer.write_all(b"started").await.expect("write");
    let error = lines.next(after(60)).await.expect_err("stalled line");
    assert!(matches!(error, LineError::TimedOut));
    tokio::time::sleep(Duration::from_millis(60)).await;

    let started = Instant::now();
    let error = lines
        .next_after_idle(Duration::from_millis(100))
        .await
        .expect_err("past its bound already");
    assert!(matches!(error, LineError::TimedOut));
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "the bound runs from the first byte, not from this call"
    );
}

#[tokio::test]
async fn each_line_gets_its_own_completion_bound() {
    let (mut writer, reader) = tokio::io::duplex(1024);
    let mut lines = lines(reader);
    let completion = Duration::from_millis(100);
    writer.write_all(b"first\n").await.expect("write");
    let line = lines.next_after_idle(completion).await.expect("first line");
    assert_eq!(line.as_deref(), Some(b"first".as_slice()));

    tokio::time::sleep(completion * 2).await;
    writer.write_all(b"second\n").await.expect("write");
    let line = lines
        .next_after_idle(completion)
        .await
        .expect("a quiet gap is not a stalled line");
    assert_eq!(line.as_deref(), Some(b"second".as_slice()));
}

/// Pends until `ready_at`, then has a whole line ready. It registers no
/// waker: the caller's deadline timer is what polls it again.
struct LateLine {
    ready_at: Instant,
}

impl AsyncRead for LateLine {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if Instant::now() < self.ready_at {
            return Poll::Pending;
        }
        buffer.put_slice(b"late\n");
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn a_read_that_finishes_after_the_deadline_missed_it_and_keeps_its_data() {
    let deadline = after(50);
    let mut lines = BoundedLines::new(LateLine { ready_at: deadline }, LIMIT);
    let error = lines.next(deadline).await.expect_err("late line");
    assert!(matches!(error, LineError::TimedOut), "{error}");
    let line = lines.next(soon()).await.expect("buffered line");
    assert_eq!(line.as_deref(), Some(b"late".as_slice()));
}

#[tokio::test]
async fn an_idle_read_sees_end_of_stream() {
    let (writer, reader) = tokio::io::duplex(1024);
    drop(writer);
    let mut lines = lines(reader);
    let line = lines
        .next_after_idle(Duration::from_secs(1))
        .await
        .expect("end of stream");
    assert_eq!(line, None);
}

/// Accepts `accept` bytes, then stalls or fails, counting every poll that
/// reaches it.
struct PartialStream {
    accept: usize,
    failure: Option<io::ErrorKind>,
    received: Arc<AtomicUsize>,
    polls: Arc<AtomicUsize>,
}

impl PartialStream {
    fn new(accept: usize, failure: Option<io::ErrorKind>) -> Self {
        Self {
            accept,
            failure,
            received: Arc::default(),
            polls: Arc::default(),
        }
    }
}

impl AsyncWrite for PartialStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        if self.accept == 0 {
            return self
                .failure
                .map_or(Poll::Pending, |kind| Poll::Ready(Err(kind.into())));
        }
        let taken = self.accept.min(bytes.len());
        self.accept -= taken;
        self.received.fetch_add(taken, Ordering::SeqCst);
        Poll::Ready(Ok(taken))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn whole_frames_reach_the_peer_in_order() {
    let (writer, mut reader) = tokio::io::duplex(1024);
    let mut writer = FramedWriter::new(writer);
    writer.write(b"one\n", soon()).await.expect("first frame");
    writer.write(b"two\n", soon()).await.expect("second frame");
    let mut received = [0_u8; 8];
    reader.read_exact(&mut received).await.expect("read");
    assert_eq!(&received, b"one\ntwo\n");
}

#[tokio::test]
async fn a_partial_write_that_stalls_invalidates_the_stream() {
    let stream = PartialStream::new(4, None);
    let (received, polls) = (Arc::clone(&stream.received), Arc::clone(&stream.polls));
    let mut writer = FramedWriter::new(stream);

    let error = writer
        .write(b"half a frame\n", after(50))
        .await
        .expect_err("stalled frame");
    assert!(matches!(error, FrameWriteError::TimedOut), "{error}");
    assert_eq!(received.load(Ordering::SeqCst), 4, "a partial frame landed");

    let polls_at_failure = polls.load(Ordering::SeqCst);
    let error = writer
        .write(b"next frame\n", soon())
        .await
        .expect_err("the stream is gone");
    assert!(matches!(error, FrameWriteError::Closed), "{error}");
    assert_eq!(
        polls.load(Ordering::SeqCst),
        polls_at_failure,
        "the refused frame never reached the stream"
    );
    assert_eq!(received.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn a_partial_write_that_fails_invalidates_the_stream() {
    let stream = PartialStream::new(4, Some(io::ErrorKind::BrokenPipe));
    let polls = Arc::clone(&stream.polls);
    let mut writer = FramedWriter::new(stream);

    let error = writer
        .write(b"half a frame\n", soon())
        .await
        .expect_err("failed frame");
    match error {
        FrameWriteError::Write(source) => assert_eq!(source.kind(), io::ErrorKind::BrokenPipe),
        other => panic!("expected a write error, got {other}"),
    }

    let polls_at_failure = polls.load(Ordering::SeqCst);
    let error = writer
        .write(b"next frame\n", soon())
        .await
        .expect_err("the stream is gone");
    assert!(matches!(error, FrameWriteError::Closed), "{error}");
    assert_eq!(polls.load(Ordering::SeqCst), polls_at_failure);
}

#[tokio::test]
async fn an_abandoned_write_invalidates_the_stream() {
    let stream = PartialStream::new(4, None);
    let polls = Arc::clone(&stream.polls);
    let mut writer = FramedWriter::new(stream);

    let abandoned = tokio::time::timeout(
        Duration::from_millis(50),
        writer.write(b"half a frame\n", soon()),
    )
    .await;
    assert!(abandoned.is_err(), "the caller gave up first");

    let polls_at_failure = polls.load(Ordering::SeqCst);
    let error = writer
        .write(b"next frame\n", soon())
        .await
        .expect_err("the stream is gone");
    assert!(matches!(error, FrameWriteError::Closed), "{error}");
    assert_eq!(polls.load(Ordering::SeqCst), polls_at_failure);
}

/// Always ready, but takes one byte per write and `delay` of wall clock
/// for it, so a frame can outlast its deadline without ever pending.
struct ReadyShortWrites {
    delay: Duration,
    received: Arc<AtomicUsize>,
    polls: Arc<AtomicUsize>,
}

impl AsyncWrite for ReadyShortWrites {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        std::thread::sleep(self.delay);
        self.polls.fetch_add(1, Ordering::SeqCst);
        self.received.fetch_add(1, Ordering::SeqCst);
        Poll::Ready(Ok(1))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn ready_short_writes_cannot_cross_the_deadline() {
    let stream = ReadyShortWrites {
        delay: Duration::from_millis(10),
        received: Arc::default(),
        polls: Arc::default(),
    };
    let (received, polls) = (Arc::clone(&stream.received), Arc::clone(&stream.polls));
    let mut writer = FramedWriter::new(stream);

    let error = writer
        .write(&[b'x'; 64], after(80))
        .await
        .expect_err("the frame outlasts its deadline");
    assert!(matches!(error, FrameWriteError::TimedOut), "{error}");
    let landed = received.load(Ordering::SeqCst);
    assert!(landed < 64, "stopped between writes, took {landed} bytes");

    let polls_at_failure = polls.load(Ordering::SeqCst);
    let error = writer
        .write(b"next frame\n", soon())
        .await
        .expect_err("the stream is gone");
    assert!(matches!(error, FrameWriteError::Closed), "{error}");
    assert_eq!(polls.load(Ordering::SeqCst), polls_at_failure);
}

/// Pends until `ready_at`, then takes everything at once. It registers no
/// waker: the caller's deadline timer is what polls it again.
struct LateStream {
    ready_at: Instant,
}

impl AsyncWrite for LateStream {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if Instant::now() < self.ready_at {
            return Poll::Pending;
        }
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn a_write_that_finishes_after_the_deadline_missed_it() {
    let deadline = after(50);
    let mut writer = FramedWriter::new(LateStream { ready_at: deadline });
    let error = writer
        .write(b"late frame\n", deadline)
        .await
        .expect_err("completion past the deadline does not count");
    assert!(matches!(error, FrameWriteError::TimedOut), "{error}");
    let error = writer
        .write(b"next frame\n", soon())
        .await
        .expect_err("the stream is gone");
    assert!(matches!(error, FrameWriteError::Closed), "{error}");
}

#[tokio::test]
async fn a_peer_that_stops_reading_sees_the_connection_close() {
    let (writer, mut reader) = tokio::io::duplex(16);
    let mut writer = FramedWriter::new(writer);
    let error = writer
        .write(&[b'x'; 256], after(50))
        .await
        .expect_err("stalled frame");
    assert!(matches!(error, FrameWriteError::TimedOut));

    let mut received = Vec::new();
    reader.read_to_end(&mut received).await.expect("read");
    assert_eq!(
        received.len(),
        16,
        "the peer sees the partial frame, then end of stream instead of a resumed write"
    );
}

#[tokio::test]
async fn close_ends_the_stream_for_the_peer_and_for_later_writes() {
    let (writer, mut reader) = tokio::io::duplex(16);
    let mut writer = FramedWriter::new(writer);
    writer.close();
    let error = writer.write(b"late", soon()).await.expect_err("closed");
    assert!(matches!(error, FrameWriteError::Closed));
    let mut received = Vec::new();
    reader.read_to_end(&mut received).await.expect("read");
    assert!(received.is_empty());
}
