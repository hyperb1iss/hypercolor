//! Revocable device sessions.
//!
//! A backend keeps its open connections in a [`SessionTable`]. Each connect
//! opens one [`Session`], the sole owner of that connection's protocol state
//! and tasks, and every [`SessionSink`] handed out refers to the session's
//! slot, not to the state itself. Holding a sink therefore keeps nothing
//! alive: closing the session empties the slot, so a sink retained past
//! [`SessionTable::close`] rejects every frame and holds no socket.
//!
//! When close returns `Ok`, all of this is true, in this order:
//!
//! 1. New operations are revoked. Nothing is admitted from now on.
//! 2. Owned tasks are stopped and joined.
//! 3. Admitted I/O has settled, or was cancelled once
//!    [`DeviceSession::SETTLE`] ran out.
//! 4. [`DeviceSession::finish`] ran bounded protocol cleanup.
//! 5. The session state is gone, and with it the session's transport.
//!
//! Close runs on its own task and the caller only waits for it, so a caller
//! that gives up early never leaves a half-closed session behind. An error
//! means cleanup failed or timed out; revocation and release still happen.
//!
//! A reconnect installs a new session with a new slot. Sinks from the old
//! generation stay revoked. Retiring a table closes everything in it and
//! refuses every later install, so a connect still in flight when its
//! backend is retired ends up closed instead of published.

use std::collections::HashMap;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use hypercolor_driver_api::{
    DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver, DeviceFrameSink,
    DeviceWriteOutcome,
};
use hypercolor_types::device::{DeviceError, DeviceId};
use tokio::sync::{Mutex, MutexGuard, Notify};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

/// Default [`DeviceSession::SETTLE`].
pub const DEFAULT_SETTLE: Duration = Duration::from_millis(500);

/// Default [`DeviceSession::CLEANUP`].
pub const DEFAULT_CLEANUP: Duration = Duration::from_millis(1_400);

/// Time cancelled I/O gets to release the session. Cancellation drops the
/// write at its next await, so this only covers a busy runtime.
const CANCEL_GRACE: Duration = Duration::from_millis(100);

/// Host delivery one frame write reports its transport start to.
#[derive(Clone)]
pub struct FrameDelivery {
    id: DeviceDeliveryId,
    observer: Arc<dyn DeviceDeliveryObserver>,
}

impl FrameDelivery {
    /// Delivery this write belongs to.
    #[must_use]
    pub const fn id(&self) -> DeviceDeliveryId {
        self.id
    }

    /// Report that transport I/O for this frame has begun.
    pub fn transport_started(&self) {
        self.observer.transport_started(self.id);
    }
}

/// Protocol state of one open device connection.
///
/// Implementors own everything that dies with the connection. State shared
/// across a whole backend, like one UDP socket every device sends from, is
/// held here by reference count, so closing one session leaves the others
/// on that socket running.
pub trait DeviceSession: Send + 'static {
    /// Facts fixed for the life of the session, readable without waiting
    /// for its I/O.
    type Facts: Send + Sync + 'static;

    /// How long admitted I/O may keep running once the session is revoked
    /// before close cancels it.
    const SETTLE: Duration = DEFAULT_SETTLE;

    /// How long [`Self::finish`] may run before close drops it.
    const CLEANUP: Duration = DEFAULT_CLEANUP;

    /// Write one frame to the device.
    ///
    /// The future may be dropped at any await once close runs out of
    /// patience, so it must leave nothing behind that outlives the session.
    fn write_frame(
        &mut self,
        colors: &[[u8; 3]],
        delivery: Option<&FrameDelivery>,
    ) -> impl Future<Output = Result<DeviceWriteOutcome, DeviceError>> + Send;

    /// Deliver one queue-qualified frame and acknowledge its terminal state.
    fn deliver_frame(
        &mut self,
        delivery: FrameDelivery,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> impl Future<Output = DeviceDeliveryAck> + Send {
        async move {
            let payload_bytes = colors.len().saturating_mul(3);
            let started_at = Instant::now();
            let result = self.write_frame(colors.as_slice(), Some(&delivery)).await;
            DeviceDeliveryAck::from_write_result(
                delivery.id(),
                payload_bytes,
                started_at.elapsed(),
                result,
            )
        }
    }

    /// Run protocol cleanup, consuming the session.
    ///
    /// The transport is released when this returns or when close drops it
    /// at [`Self::CLEANUP`]. Sessions with nothing to say on the way out
    /// keep the default.
    fn finish(self) -> impl Future<Output = Result<(), DeviceError>> + Send
    where
        Self: Sized,
    {
        async { Ok(()) }
    }
}

/// Where a session's state lives while the session is open.
struct Slot<S> {
    /// Set once; nothing is admitted afterwards.
    revoked: AtomicBool,
    /// Set when close stopped waiting for admitted I/O.
    cancelled: AtomicBool,
    /// Set when no close is coming, so the last admitted operation drops
    /// the state on its way out.
    abandoned: AtomicBool,
    cancel: Notify,
    state: Mutex<Option<S>>,
}

impl<S> Slot<S> {
    fn new(session: S) -> Self {
        Self {
            revoked: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            abandoned: AtomicBool::new(false),
            cancel: Notify::new(),
            state: Mutex::new(Some(session)),
        }
    }

    fn is_revoked(&self) -> bool {
        self.revoked.load(Ordering::Acquire)
    }

    fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.cancel.notify_waiters();
    }

    /// Drop the state as soon as nothing holds it.
    fn abandon(&self) {
        self.revoke();
        self.abandoned.store(true, Ordering::Release);
        self.cancel();
        if let Ok(mut state) = self.state.try_lock() {
            state.take();
        }
    }

    async fn cancelled(&self) {
        // Created before the check: `notify_waiters` reaches a `Notified`
        // from the moment it exists, so a cancel cannot slip between.
        let notified = self.cancel.notified();
        if !self.cancelled.load(Ordering::Acquire) {
            notified.await;
        }
    }

    async fn admit(&self) -> Option<Admitted<'_, S>> {
        if self.is_revoked() {
            return None;
        }
        let state = self.state.lock().await;
        if self.is_revoked() || state.is_none() {
            return None;
        }
        Some(Admitted { state, slot: self })
    }

    /// Run admitted I/O, giving up when close cancels it.
    async fn until_cancelled<T>(&self, io: impl Future<Output = T>) -> Option<T> {
        tokio::select! {
            biased;
            value = io => Some(value),
            () = self.cancelled() => None,
        }
    }
}

/// Exclusive access to a live session.
///
/// Close waits for this to drop, so hold it across short bookkeeping only.
/// Frame writes go through [`SessionSink::write`], which close can cancel.
pub struct Admitted<'a, S> {
    state: MutexGuard<'a, Option<S>>,
    slot: &'a Slot<S>,
}

impl<S> Deref for Admitted<'_, S> {
    type Target = S;

    fn deref(&self) -> &S {
        self.state
            .as_ref()
            .expect("an admitted session stays in its slot")
    }
}

impl<S> DerefMut for Admitted<'_, S> {
    fn deref_mut(&mut self) -> &mut S {
        self.state
            .as_mut()
            .expect("an admitted session stays in its slot")
    }
}

impl<S> Drop for Admitted<'_, S> {
    fn drop(&mut self) {
        if self.slot.abandoned.load(Ordering::Acquire) {
            self.state.take();
        }
    }
}

/// Frame sink over one session's slot. Clones share the slot.
pub struct SessionSink<S> {
    device_id: DeviceId,
    slot: Arc<Slot<S>>,
}

impl<S> Clone for SessionSink<S> {
    fn clone(&self) -> Self {
        Self {
            device_id: self.device_id,
            slot: Arc::clone(&self.slot),
        }
    }
}

impl<S> SessionSink<S> {
    /// Whether the session behind this sink is over.
    #[must_use]
    pub fn is_revoked(&self) -> bool {
        self.slot.is_revoked()
    }

    /// Take exclusive access to the session state.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError::Disconnected`] once the session is revoked.
    pub async fn admit(&self) -> Result<Admitted<'_, S>, DeviceError> {
        self.slot
            .admit()
            .await
            .ok_or_else(|| disconnected(self.device_id))
    }

    /// Take exclusive access to the session state without waiting for it,
    /// or `None` when it is busy or revoked.
    #[must_use]
    pub fn try_admit(&self) -> Option<Admitted<'_, S>> {
        if self.slot.is_revoked() {
            return None;
        }
        let state = self.slot.state.try_lock().ok()?;
        (!self.slot.is_revoked() && state.is_some()).then_some(Admitted {
            state,
            slot: &self.slot,
        })
    }
}

impl<S: DeviceSession> SessionSink<S> {
    /// Write one frame unless the session was closed.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError::Disconnected`] when the session is revoked or
    /// close cancelled this write, and whatever the write itself returns.
    pub async fn write(&self, colors: &[[u8; 3]]) -> Result<DeviceWriteOutcome, DeviceError> {
        let mut session = self.admit().await?;
        self.slot
            .until_cancelled(session.write_frame(colors, None))
            .await
            .unwrap_or_else(|| Err(disconnected(self.device_id)))
    }
}

#[async_trait::async_trait]
impl<S: DeviceSession> DeviceFrameSink for SessionSink<S> {
    fn is_closed(&self) -> bool {
        self.is_revoked()
    }

    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<(), DeviceError> {
        self.write(colors.as_slice()).await.map(|_| ())
    }

    async fn write_colors_shared_outcome(
        &self,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        self.write(colors.as_slice()).await
    }

    async fn deliver_colors_shared_observed(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        let Some(mut session) = self.slot.admit().await else {
            return DeviceDeliveryAck::rejected(id, disconnected(self.device_id));
        };
        let delivery = FrameDelivery { id, observer };
        self.slot
            .until_cancelled(session.deliver_frame(delivery, colors))
            .await
            .unwrap_or_else(|| {
                DeviceDeliveryAck::failed(
                    id,
                    true,
                    Duration::ZERO,
                    disconnected(self.device_id),
                )
            })
    }
}

/// Owner of one open device connection.
pub struct Session<S: DeviceSession> {
    device_id: DeviceId,
    slot: Arc<Slot<S>>,
    facts: Arc<S::Facts>,
    tasks: Vec<JoinHandle<()>>,
    closing: bool,
}

impl<S: DeviceSession> Session<S> {
    /// Open a session for `device_id` around `state`.
    #[must_use]
    pub fn new(device_id: DeviceId, facts: S::Facts, state: S) -> Self {
        Self {
            device_id,
            slot: Arc::new(Slot::new(state)),
            facts: Arc::new(facts),
            tasks: Vec::new(),
            closing: false,
        }
    }

    /// Sink over this session, for tasks that need to reach its state.
    #[must_use]
    pub fn sink(&self) -> SessionSink<S> {
        SessionSink {
            device_id: self.device_id,
            slot: Arc::clone(&self.slot),
        }
    }

    /// Run `task` for the life of the session.
    ///
    /// Close aborts the task and joins it before settling admitted I/O, so
    /// it must be safe to stop at any await.
    pub fn spawn(&mut self, task: impl Future<Output = ()> + Send + 'static) {
        self.tasks.push(tokio::spawn(task));
    }

    /// Revoke the session now and finish closing it on its own task.
    fn begin_close(mut self) -> JoinHandle<Result<(), DeviceError>> {
        self.closing = true;
        self.slot.revoke();
        let tasks = std::mem::take(&mut self.tasks);
        for task in &tasks {
            task.abort();
        }
        tokio::spawn(finish_close(self.device_id, Arc::clone(&self.slot), tasks))
    }
}

impl<S: DeviceSession> Drop for Session<S> {
    /// A session dropped without closing still ends: it transmits nothing
    /// further and releases its transport, skipping only protocol cleanup.
    fn drop(&mut self) {
        if self.closing {
            return;
        }
        self.slot.abandon();
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn finish_close<S: DeviceSession>(
    device_id: DeviceId,
    slot: Arc<Slot<S>>,
    tasks: Vec<JoinHandle<()>>,
) -> Result<(), DeviceError> {
    let join_tasks = async {
        for task in tasks {
            let _ = task.await;
        }
    };
    if tokio::time::timeout(S::SETTLE, join_tasks).await.is_err() {
        warn!(%device_id, "a session task outlived its abort");
    }

    let mut settled = tokio::time::timeout(S::SETTLE, slot.state.lock())
        .await
        .ok();
    if settled.is_none() {
        debug!(%device_id, "cancelling device I/O that outlived its session");
        slot.cancel();
        settled = tokio::time::timeout(CANCEL_GRACE, slot.state.lock())
            .await
            .ok();
    }
    let Some(mut state) = settled else {
        warn!(%device_id, "device session state stayed busy past close");
        slot.abandon();
        return Err(DeviceError::Timeout {
            after: S::SETTLE.saturating_add(CANCEL_GRACE),
        });
    };
    let session = state.take();
    drop(state);

    let Some(session) = session else {
        return Ok(());
    };
    tokio::time::timeout(S::CLEANUP, session.finish())
        .await
        .unwrap_or(Err(DeviceError::Timeout { after: S::CLEANUP }))
}

async fn join_close(close: JoinHandle<Result<(), DeviceError>>) -> Result<(), DeviceError> {
    close
        .await
        .unwrap_or_else(|error| Err(DeviceError::protocol("device session", error)))
}

struct TableState<S: DeviceSession> {
    retired: bool,
    sessions: HashMap<DeviceId, Session<S>>,
}

/// Every open session of one backend.
pub struct SessionTable<S: DeviceSession> {
    state: StdRwLock<TableState<S>>,
}

impl<S: DeviceSession> Default for SessionTable<S> {
    fn default() -> Self {
        Self {
            state: StdRwLock::new(TableState {
                retired: false,
                sessions: HashMap::new(),
            }),
        }
    }
}

impl<S: DeviceSession> SessionTable<S> {
    /// Create an empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `id` has an open session.
    #[must_use]
    pub fn is_open(&self, id: &DeviceId) -> bool {
        self.read().sessions.contains_key(id)
    }

    /// Facts of the session open for `id`.
    #[must_use]
    pub fn facts(&self, id: &DeviceId) -> Option<Arc<S::Facts>> {
        self.read()
            .sessions
            .get(id)
            .map(|session| Arc::clone(&session.facts))
    }

    /// Sink over the session open for `id`.
    #[must_use]
    pub fn sink(&self, id: &DeviceId) -> Option<SessionSink<S>> {
        self.read().sessions.get(id).map(Session::sink)
    }

    /// Sink over the session open for `id`.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError::Disconnected`] when no session is open.
    pub fn require(&self, id: &DeviceId) -> Result<SessionSink<S>, DeviceError> {
        self.sink(id).ok_or_else(|| disconnected(*id))
    }

    /// Host-facing sink over the session open for `id`.
    #[must_use]
    pub fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.sink(id)
            .map(|sink| Arc::new(sink) as Arc<dyn DeviceFrameSink>)
    }

    /// Fail early when this table can never hold a session again.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError::ConnectionFailed`] once the table is retired.
    pub fn ensure_accepting(&self, id: &DeviceId) -> Result<(), DeviceError> {
        if self.read().retired {
            return Err(retired(*id));
        }
        Ok(())
    }

    /// Publish a new session, closing any session it displaces.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError::ConnectionFailed`] when the table is retired.
    /// The session is closed before this returns, so nothing it opened
    /// survives the refusal.
    pub async fn install(&self, session: Session<S>) -> Result<(), DeviceError> {
        let device_id = session.device_id;
        let published = {
            let mut state = self.write();
            if state.retired {
                Err(session)
            } else {
                Ok(state.sessions.insert(device_id, session))
            }
        };
        match published {
            Ok(None) => Ok(()),
            Ok(Some(displaced)) => {
                if let Err(error) = join_close(displaced.begin_close()).await {
                    debug!(%device_id, %error, "cleanup of a displaced session failed");
                }
                Ok(())
            }
            Err(refused) => {
                if let Err(error) = join_close(refused.begin_close()).await {
                    debug!(%device_id, %error, "cleanup of a refused session failed");
                }
                Err(retired(device_id))
            }
        }
    }

    /// Close the session open for `id`.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError::Disconnected`] when no session is open, and
    /// the cleanup failure when protocol cleanup fails or times out. The
    /// session is over either way.
    pub async fn close(&self, id: &DeviceId) -> Result<(), DeviceError> {
        let session = self
            .write()
            .sessions
            .remove(id)
            .ok_or_else(|| disconnected(*id))?;
        join_close(session.begin_close()).await
    }

    /// Close every session and refuse all later installs.
    ///
    /// # Errors
    ///
    /// Returns the first cleanup failure. Every session is over either way.
    pub async fn retire(&self) -> Result<(), DeviceError> {
        let closing: Vec<_> = {
            let mut state = self.write();
            state.retired = true;
            state.sessions.drain().collect()
        };
        let closing: Vec<_> = closing
            .into_iter()
            .map(|(device_id, session)| (device_id, session.begin_close()))
            .collect();
        let mut result = Ok(());
        for (device_id, close) in closing {
            if let Err(error) = join_close(close).await {
                debug!(%device_id, %error, "cleanup of a retired session failed");
                if result.is_ok() {
                    result = Err(error);
                }
            }
        }
        result
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, TableState<S>> {
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, TableState<S>> {
        self.state.write().unwrap_or_else(PoisonError::into_inner)
    }
}

fn disconnected(device_id: DeviceId) -> DeviceError {
    DeviceError::Disconnected {
        device: device_id.to_string(),
    }
}

fn retired(device_id: DeviceId) -> DeviceError {
    DeviceError::connection(device_id, "the backend serving this device was retired")
}
