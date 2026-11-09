//! Fault injection for the serial release service.
//!
//! These tests build their own [`ReleaseService`] with a spawner that
//! always fails, the way `std::thread::Builder::spawn` fails under thread
//! exhaustion: it drops the work on the calling thread and returns an
//! error. The port stand-in blocks in its destructor, like a tty close
//! waiting on a wedged device, and records the thread that ran it.

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};

use super::super::{CLOSE_TIMEOUT, UsbSerialTransport};
use super::{ReleaseService, spawn_release_worker, wait_for_release};
use crate::transport::{Transport, TransportError};

const SLACK: Duration = Duration::from_millis(500);
const HOLD_LIMIT: Duration = Duration::from_secs(10);

fn failing_spawn(work: Box<dyn FnOnce() + Send>, _context: String) -> std::io::Result<()> {
    drop(work);
    Err(std::io::Error::other("injected thread spawn failure"))
}

fn leaked_service(spawn: super::SpawnRelease) -> &'static ReleaseService {
    Box::leak(Box::new(
        ReleaseService::start(spawn).expect("test release dispatcher should start"),
    ))
}

/// A port whose destructor blocks until the test lets go.
struct HeldPort {
    inner: DuplexStream,
    hold: Option<mpsc::Receiver<()>>,
    dropped_on: Arc<Mutex<Option<String>>>,
}

struct HeldPortHandle {
    release: mpsc::Sender<()>,
    dropped_on: Arc<Mutex<Option<String>>>,
    _device: DuplexStream,
}

impl HeldPortHandle {
    fn dropped_on(&self) -> Option<String> {
        self.dropped_on
            .lock()
            .expect("dropped_on lock should not be poisoned")
            .clone()
    }

    async fn wait_dropped(&self) -> String {
        let started = Instant::now();
        loop {
            if let Some(thread) = self.dropped_on() {
                return thread;
            }
            assert!(started.elapsed() < SLACK, "the port was never released");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

fn held_port() -> (HeldPort, HeldPortHandle) {
    let (inner, device) = tokio::io::duplex(64);
    let (release, hold) = mpsc::channel();
    let dropped_on = Arc::new(Mutex::new(None));
    let port = HeldPort {
        inner,
        hold: Some(hold),
        dropped_on: Arc::clone(&dropped_on),
    };
    let handle = HeldPortHandle {
        release,
        dropped_on,
        _device: device,
    };
    (port, handle)
}

impl AsyncRead for HeldPort {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for HeldPort {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        panic!("serial transport flushed the port");
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        panic!("serial transport shut the port down");
    }
}

impl Drop for HeldPort {
    fn drop(&mut self) {
        if let Some(hold) = self.hold.take() {
            let _ = hold.recv_timeout(HOLD_LIMIT);
        }
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_owned();
        *self
            .dropped_on
            .lock()
            .expect("dropped_on lock should not be poisoned") = Some(thread);
    }
}

fn start_ticker() -> (Arc<AtomicU64>, tokio::task::JoinHandle<()>) {
    let ticks = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&ticks);
    let task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(5)).await;
            counter.fetch_add(1, Ordering::AcqRel);
        }
    });
    (ticks, task)
}

#[tokio::test(flavor = "current_thread")]
async fn close_stays_bounded_when_no_release_thread_can_start() {
    let service = leaked_service(failing_spawn);
    let path = "fault-close-no-thread";
    let (port, handle) = held_port();
    let transport = UsbSerialTransport::with_release(path, port, service);
    transport.send(b"last frame").await.expect("send");
    let (ticks, ticker) = start_ticker();

    let started = Instant::now();
    transport.close().await.expect("close should succeed");
    let elapsed = started.elapsed();

    assert!(
        elapsed + Duration::from_millis(10) >= CLOSE_TIMEOUT,
        "close should give the release its whole bound: {elapsed:?}"
    );
    assert!(
        elapsed < CLOSE_TIMEOUT + SLACK,
        "close outlived its bound: {elapsed:?}"
    );
    assert!(
        ticks.load(Ordering::Acquire) >= 10,
        "the blocked release parked the runtime thread"
    );
    assert!(handle.dropped_on().is_none(), "the release is still held");
    assert!(matches!(
        wait_for_release(path, Duration::from_millis(20)).await,
        Err(TransportError::NotReady { .. })
    ));

    handle
        .release
        .send(())
        .expect("release should still be held");
    let thread = handle.wait_dropped().await;
    assert_eq!(
        thread, "hc-serial-svc",
        "with no release thread available the dispatcher releases the port"
    );
    wait_for_release(path, SLACK)
        .await
        .expect("the barrier should clear once the port is released");
    ticker.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn drop_stays_off_the_runtime_when_no_release_thread_can_start() {
    let service = leaked_service(failing_spawn);
    let (port, handle) = held_port();
    let transport = UsbSerialTransport::with_release("fault-drop-no-thread", port, service);

    let started = Instant::now();
    drop(transport);
    assert!(
        started.elapsed() < SLACK,
        "drop blocked on the port release"
    );
    assert!(handle.dropped_on().is_none());

    handle
        .release
        .send(())
        .expect("release should still be held");
    assert_eq!(handle.wait_dropped().await, "hc-serial-svc");
}

#[tokio::test(flavor = "current_thread")]
async fn dispatcher_keeps_releasing_after_a_failed_spawn() {
    let service = leaked_service(failing_spawn);
    for index in 0..3 {
        let (port, handle) = held_port();
        let transport =
            UsbSerialTransport::with_release(format!("fault-repeat-{index}"), port, service);
        handle.release.send(()).expect("pre-release the hold");
        transport.close().await.expect("close should succeed");
        assert_eq!(handle.wait_dropped().await, "hc-serial-svc");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn each_release_gets_its_own_thread_when_one_can_start() {
    let service = leaked_service(spawn_release_worker);
    let (port, handle) = held_port();
    let transport = UsbSerialTransport::with_release("release-own-thread", port, service);
    handle.release.send(()).expect("pre-release the hold");

    transport.close().await.expect("close should succeed");

    assert_eq!(
        handle.dropped_on().as_deref(),
        Some("hc-serial-close"),
        "close returns once the release thread has closed the port"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_held_release_does_not_delay_another_port() {
    let service = leaked_service(spawn_release_worker);
    let (wedged_port, wedged) = held_port();
    let wedged_transport = UsbSerialTransport::with_release("release-wedged", wedged_port, service);
    drop(wedged_transport);

    let (port, healthy) = held_port();
    let transport = UsbSerialTransport::with_release("release-healthy", port, service);
    healthy.release.send(()).expect("pre-release the hold");
    let started = Instant::now();
    transport.close().await.expect("close should succeed");

    assert!(started.elapsed() < SLACK);
    assert_eq!(healthy.dropped_on().as_deref(), Some("hc-serial-close"));
    assert!(wedged.dropped_on().is_none(), "the wedged release is held");

    wedged
        .release
        .send(())
        .expect("release should still be held");
    wedged.wait_dropped().await;
}
