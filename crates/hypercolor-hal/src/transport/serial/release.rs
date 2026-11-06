//! Off-runtime release of serial ports.
//!
//! Dropping a tty descriptor waits in the kernel until queued output drains,
//! for up to the driver's closing wait (30 seconds by default) when the
//! device is wedged, so a port must never be dropped on a runtime worker.
//! A transport hands its stream to the process-wide [`ReleaseService`],
//! whose dispatcher thread lives for the whole process and gives each
//! release its own worker thread, so one wedged port never delays another.
//! When a worker cannot start, the dispatcher releases the port itself:
//! slower, but still off the runtime. The service starts before a transport
//! takes ownership of a port, so no port is ever released on the caller.
//!
//! Every release also registers its port path, and [`wait_for_release`]
//! lets the next open of that path wait until the previous descriptor has
//! closed instead of racing its exclusive lock.

use std::collections::HashMap;
use std::io;
use std::panic::AssertUnwindSafe;
use std::sync::{LazyLock, Mutex, OnceLock, PoisonError, mpsc};
use std::thread;
use std::time::Duration;

use hypercolor_worker_retention::{retain_worker, spawn_worker};
use tokio::sync::{oneshot, watch};
use tracing::warn;

use super::AsyncSerialIo;
use crate::transport::TransportError;

/// Starts the work of one release on its own thread. On failure the work
/// has already run or been dropped on the calling thread.
type SpawnRelease = fn(Box<dyn FnOnce() + Send>, String) -> io::Result<()>;

/// Process-wide service that releases serial ports off the runtime.
pub(super) struct ReleaseService {
    jobs: mpsc::Sender<ReleaseJob>,
}

/// One port waiting to be released.
///
/// Dropping the job is the release: the fields drop in declaration order,
/// so the stream closes before completion is signalled, on whichever
/// thread drops the job.
struct ReleaseJob {
    /// Held only so that dropping the job closes it.
    _stream: Box<dyn AsyncSerialIo>,
    completion: ReleaseCompletion,
}

struct ReleaseCompletion {
    path: String,
    released: Option<oneshot::Sender<()>>,
}

impl Drop for ReleaseCompletion {
    fn drop(&mut self) {
        finish_pending(&self.path);
        if let Some(released) = self.released.take() {
            let _ = released.send(());
        }
    }
}

/// Ports with a release in flight, keyed by path, with how many are
/// pending for each.
static PENDING: LazyLock<Mutex<HashMap<String, watch::Sender<usize>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Return the process-wide release service, starting it on first use.
///
/// # Errors
///
/// Returns [`TransportError::IoError`] when the dispatcher thread cannot
/// start. A caller must not take ownership of a port in that case.
pub(super) fn service() -> Result<&'static ReleaseService, TransportError> {
    static SERVICE: OnceLock<ReleaseService> = OnceLock::new();
    static INITIALIZE: Mutex<()> = Mutex::new(());

    if let Some(service) = SERVICE.get() {
        return Ok(service);
    }
    let _initialize = INITIALIZE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(service) = SERVICE.get() {
        return Ok(service);
    }
    let service =
        ReleaseService::start(spawn_release_worker).map_err(|error| TransportError::IoError {
            detail: format!("failed to start the serial port release service: {error}"),
        })?;
    Ok(SERVICE.get_or_init(|| service))
}

impl ReleaseService {
    /// Start a dispatcher thread that hands each job to `spawn_release`.
    pub(super) fn start(spawn_release: SpawnRelease) -> io::Result<Self> {
        let (jobs, queue) = mpsc::channel();
        // The dispatcher runs until every sender drops. The process-wide
        // service never drops its sender, so it lives for the whole process
        // and is deliberately not retained for joining.
        let _dispatcher = spawn_worker(
            thread::Builder::new().name("hc-serial-svc".to_owned()),
            move || dispatch(&queue, spawn_release),
        )?;
        Ok(Self { jobs })
    }

    /// Release `stream` off the calling thread.
    ///
    /// The receiver resolves once the descriptor has closed. The calling
    /// thread never drops the stream.
    pub(super) fn release(
        &self,
        path: &str,
        stream: Box<dyn AsyncSerialIo>,
    ) -> oneshot::Receiver<()> {
        let (released_tx, released_rx) = oneshot::channel();
        begin_pending(path);
        let job = ReleaseJob {
            _stream: stream,
            completion: ReleaseCompletion {
                path: path.to_owned(),
                released: Some(released_tx),
            },
        };
        if let Err(mpsc::SendError(job)) = self.jobs.send(job) {
            // Unreachable while the dispatcher runs, and it never exits on
            // its own. Fall back to a dedicated thread rather than the caller.
            warn!(
                path,
                "serial port release dispatcher is gone; releasing on a dedicated thread"
            );
            let context = format!("usb serial release {path}");
            if let Err(error) = spawn_release_worker(Box::new(move || drop(job)), context) {
                warn!(
                    path,
                    error = %error,
                    "could not start a serial release thread; the port was released on the caller"
                );
            }
        }
        released_rx
    }
}

fn dispatch(queue: &mpsc::Receiver<ReleaseJob>, spawn_release: SpawnRelease) {
    for job in queue {
        let path = job.completion.path.clone();
        let context = format!("usb serial release {path}");
        let spawned = std::panic::catch_unwind(AssertUnwindSafe(|| {
            spawn_release(Box::new(move || drop(job)), context)
        }));
        match spawned {
            Ok(Ok(())) => {}
            Ok(Err(error)) => warn!(
                path,
                error = %error,
                "could not start a serial release thread; released the port on the dispatcher"
            ),
            Err(_) => warn!(path, "serial port release panicked on the dispatcher"),
        }
    }
}

fn spawn_release_worker(work: Box<dyn FnOnce() + Send>, context: String) -> io::Result<()> {
    let worker = spawn_worker(
        thread::Builder::new().name("hc-serial-close".to_owned()),
        work,
    )?;
    retain_worker(worker, context);
    Ok(())
}

fn begin_pending(path: &str) {
    let mut pending = PENDING.lock().unwrap_or_else(PoisonError::into_inner);
    pending
        .entry(path.to_owned())
        .or_insert_with(|| watch::Sender::new(0))
        .send_modify(|count| *count += 1);
}

fn finish_pending(path: &str) {
    let mut pending = PENDING.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(count) = pending.get(path) else {
        return;
    };
    count.send_modify(|count| *count = count.saturating_sub(1));
    if *count.borrow() == 0 {
        pending.remove(path);
    }
}

/// Wait until every release of `path` handed off so far has closed its
/// descriptor, for at most `timeout`.
///
/// # Errors
///
/// Returns [`TransportError::NotReady`] when a release is still running
/// after `timeout`.
pub(super) async fn wait_for_release(path: &str, timeout: Duration) -> Result<(), TransportError> {
    let pending = PENDING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(path)
        .map(watch::Sender::subscribe);
    let Some(mut pending) = pending else {
        return Ok(());
    };
    // The count only drops to zero right before its sender is removed, so a
    // closed channel also means every release finished.
    match tokio::time::timeout(timeout, pending.wait_for(|count| *count == 0)).await {
        Ok(_) => Ok(()),
        Err(_) => Err(TransportError::NotReady {
            detail: format!(
                "serial port {path} is still being released by an earlier session after {} ms",
                timeout.as_millis()
            ),
        }),
    }
}

#[cfg(test)]
mod tests;
