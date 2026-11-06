//! Serial stream stand-ins for USB serial transport tests.
//!
//! `TrapStream` wraps a real byte stream (a tokio duplex pipe or a pty) and
//! panics if the transport flushes or shuts it down, because on a
//! tokio-serial port either call runs `tcdrain` on the runtime worker. It can
//! also hold its drop until the test lets go, which models a tty close that
//! waits in the kernel for a wedged device to drain. `Ticker` is a task that
//! counts while the runtime is free, so a test can tell whether a stalled
//! port parked the thread every task shares. Nothing here opens a device.

#![allow(
    dead_code,
    reason = "each test binary uses a different subset of the stand-ins"
)]

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::task::JoinHandle;

/// Longest a held drop waits for the test before giving up on its own.
const HOLD_LIMIT: Duration = Duration::from_secs(10);

/// Interval between ticks of a [`Ticker`].
pub const TICK: Duration = Duration::from_millis(5);

/// Observes a [`TrapStream`] after the transport owns it.
#[derive(Debug, Default)]
pub struct TrapProbe {
    released: AtomicBool,
}

impl TrapProbe {
    /// Whether the wrapped stream has been dropped.
    pub fn released(&self) -> bool {
        self.released.load(Ordering::Acquire)
    }
}

/// A byte stream that must never be flushed or shut down.
pub struct TrapStream<S> {
    inner: Option<S>,
    probe: Arc<TrapProbe>,
    hold: Option<mpsc::Receiver<()>>,
}

impl<S> TrapStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Wrap `inner`; dropping the wrapper releases it right away.
    pub fn new(inner: S) -> (Self, Arc<TrapProbe>) {
        let probe = Arc::new(TrapProbe::default());
        let stream = Self {
            inner: Some(inner),
            probe: Arc::clone(&probe),
            hold: None,
        };
        (stream, probe)
    }

    /// Wrap `inner`; dropping the wrapper blocks the dropping thread until
    /// the returned sender sends or drops.
    pub fn holding_release(inner: S) -> (Self, Arc<TrapProbe>, mpsc::Sender<()>) {
        let (release_tx, release_rx) = mpsc::channel();
        let (mut stream, probe) = Self::new(inner);
        stream.hold = Some(release_rx);
        (stream, probe, release_tx)
    }

    fn inner(&mut self) -> Pin<&mut S> {
        Pin::new(
            self.inner
                .as_mut()
                .expect("trap stream keeps its inner stream until drop"),
        )
    }
}

impl<S> AsyncRead for TrapStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.get_mut().inner().poll_read(cx, buf)
    }
}

impl<S> AsyncWrite for TrapStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.get_mut().inner().poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        panic!(
            "serial transport flushed the port; on tokio-serial that is a tcdrain on a runtime worker"
        );
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        panic!(
            "serial transport shut the port down; on tokio-serial that is a tcdrain on a runtime worker"
        );
    }
}

impl<S> Drop for TrapStream<S> {
    fn drop(&mut self) {
        if let Some(hold) = self.hold.take() {
            let _ = hold.recv_timeout(HOLD_LIMIT);
        }
        drop(self.inner.take());
        self.probe.released.store(true, Ordering::Release);
    }
}

/// A task that counts every [`TICK`] while the runtime lets it run.
pub struct Ticker {
    ticks: Arc<AtomicU64>,
    task: JoinHandle<()>,
}

impl Ticker {
    /// Start ticking on the current runtime.
    pub fn start() -> Self {
        let ticks = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&ticks);
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(TICK).await;
                counter.fetch_add(1, Ordering::AcqRel);
            }
        });
        Self { ticks, task }
    }

    /// Ticks counted so far.
    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Acquire)
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Answer `commands` Focus commands the way a Defy does: each reply is the
/// command line echoed back, then the `.` terminator line. Returns the
/// device end so the caller decides when it hangs up.
pub async fn focus_echo<S>(mut device: S, commands: usize) -> S
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut line = Vec::new();
    let mut answered = 0;
    while answered < commands {
        let mut byte = [0_u8; 1];
        let read = device
            .read(&mut byte)
            .await
            .expect("device should read a command byte");
        assert_ne!(read, 0, "port closed before the device read every command");
        if byte[0] != b'\n' {
            line.push(byte[0]);
            continue;
        }
        let mut reply = std::mem::take(&mut line);
        reply.extend_from_slice(b"\r\n.\r\n");
        device
            .write_all(&reply)
            .await
            .expect("device should write its reply");
        answered += 1;
    }
    device
}

/// Wait until `probe` reports its stream released, or panic after `limit`.
pub async fn wait_released(probe: &TrapProbe, limit: Duration) {
    let started = std::time::Instant::now();
    while !probe.released() {
        assert!(
            started.elapsed() < limit,
            "stream was not released within {limit:?}"
        );
        tokio::time::sleep(TICK).await;
    }
}
