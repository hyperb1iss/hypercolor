//! Standing watch on blocksd's device events.
//!
//! Discovery otherwise reaches blocksd only on the periodic scan, so a block
//! plugged in between scans keeps its firmware animation until the next one,
//! and a block unplugged stays connected until then. The watcher follows
//! blocksd's `device` events on its own connection for as long as the
//! backend lives, and asks the host for a blocks-scoped discovery whenever
//! the set of blocks may have changed.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hypercolor_driver_api::DriverRuntimeActions;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{debug, warn};

use super::input::{BlocksEventConnection, RECONNECT_INITIAL, RECONNECT_MAX, StreamItem};
use super::types::BlocksEvent;

const SUBSCRIBE_REQUEST: &[u8] = b"{\"type\":\"subscribe\",\"events\":[\"device\"]}\n";

/// How long a discovery request waits after the change that prompted it.
///
/// A plug-in reports every block of the new topology within a few
/// milliseconds, so one window gathers them into a single scan.
const COALESCE_WINDOW: Duration = Duration::from_millis(150);

/// Background task that turns blocksd device changes into discovery
/// requests. Dropping it stops the task.
pub(super) struct BlocksDeviceWatcher {
    task: JoinHandle<()>,
}

impl BlocksDeviceWatcher {
    /// Start watching `socket_path`, or return `None` outside a Tokio runtime.
    pub(super) fn spawn(
        socket_path: PathBuf,
        runtime: Arc<dyn DriverRuntimeActions>,
        driver_id: Arc<str>,
    ) -> Option<Self> {
        let handle = tokio::runtime::Handle::try_current().ok()?;
        // One slot: a change noted while a request is already pending merges
        // into it instead of queueing another scan.
        let (changes, pending) = mpsc::channel(1);
        let task = handle.spawn(async move {
            tokio::join!(
                follow_events(socket_path, changes),
                request_discoveries(pending, runtime, driver_id),
            );
        });
        Some(Self { task })
    }
}

impl Drop for BlocksDeviceWatcher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Follow blocksd's device events, reconnecting whenever blocksd goes away.
async fn follow_events(socket_path: PathBuf, changes: mpsc::Sender<()>) {
    let mut backoff = RECONNECT_INITIAL;
    // blocksd may never run on this host, so an outage is logged once rather
    // than on every retry.
    let mut outage_logged = false;
    loop {
        match BlocksEventConnection::subscribe(&socket_path, SUBSCRIBE_REQUEST).await {
            Ok(mut connection) => {
                debug!(path = %socket_path.display(), "blocksd device watcher connected");
                backoff = RECONNECT_INITIAL;
                outage_logged = false;
                loop {
                    match connection.next_item().await {
                        Ok(Some(StreamItem::Event(BlocksEvent::Error { message }))) => {
                            warn!(%message, "blocksd closed the device watcher");
                        }
                        Ok(Some(StreamItem::Event(event))) => {
                            if may_change_devices(&event) {
                                note_change(&changes);
                            }
                        }
                        // An unreadable line could have been an arrival or a
                        // departure.
                        Ok(Some(StreamItem::Undecodable(_) | StreamItem::Oversized)) => {
                            note_change(&changes);
                        }
                        Ok(None) => break,
                        Err(error) => {
                            debug!(%error, "blocksd device watcher read failed");
                            break;
                        }
                    }
                }
            }
            Err(error) => {
                if !outage_logged {
                    debug!(%error, "blocksd device watcher unavailable; retrying");
                    outage_logged = true;
                }
            }
        }
        sleep(backoff).await;
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

/// Whether `event` can mean the set of blocks differs from the last scan.
///
/// The subscription acknowledgement counts: blocks that came or went while
/// the watcher was disconnected sent their events to nobody.
fn may_change_devices(event: &BlocksEvent) -> bool {
    matches!(
        event,
        BlocksEvent::Subscribed { .. }
            | BlocksEvent::DeviceAdded { .. }
            | BlocksEvent::DeviceRemoved { .. }
    )
}

/// Mark a discovery request pending. A full slot already holds one that
/// covers this change.
fn note_change(changes: &mpsc::Sender<()>) {
    if changes.try_send(()).is_err() {
        debug!("blocksd device change merged into a pending discovery request");
    }
}

/// Send one discovery request per coalescing window of changes.
async fn request_discoveries(
    mut pending: mpsc::Receiver<()>,
    runtime: Arc<dyn DriverRuntimeActions>,
    driver_id: Arc<str>,
) {
    while pending.recv().await.is_some() {
        sleep(COALESCE_WINDOW).await;
        // Changes noted during the window are covered by this request.
        while pending.try_recv().is_ok() {}
        match runtime.request_discovery(&driver_id).await {
            Ok(()) => debug!(%driver_id, "requested discovery after a blocksd device change"),
            Err(error) => debug!(%driver_id, %error, "host refused the blocks discovery request"),
        }
    }
}
