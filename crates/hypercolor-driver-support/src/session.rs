//! Revocable device sessions.
//!
//! A backend keeps its connections in a [`SessionTable`], which holds one
//! state machine per device:
//!
//! ```text
//!               open                   opened
//! (nothing) -------------> Opening ----------------> Open
//!     ^                       |                        |
//!     |  open failed          |  retired, or nobody    |  close, retire,
//!     +-----------------------+  waiting any more      |  moved device
//!     ^                       v                        v
//!     +------------------- Closing <-------------------+
//!        cleanup finished
//! ```
//!
//! At most one generation of a device is Opening or Open at a time, and a
//! Closing generation stays in the table until its transport is really gone.
//! Device state outside any one connection (a controller registration keyed
//! by device id, entertainment mode on a bridge, realtime mode on a strip)
//! therefore never sees two generations: a new connection starts after the
//! old one's cleanup.
//!
//! The table owns every open. An open runs as its own task and connects only
//! wait for it, so concurrent connects of one device share one open and one
//! result, and a connect that gives up leaves any native work the open is
//! doing in the table's hands. An open that finishes with nobody waiting ends
//! closed, so a connect that timed out leaves nothing running.
//!
//! Each [`Session`] is the sole owner of its protocol state and tasks. A
//! [`SessionSink`] refers to the session's slot, not to the state, so holding
//! a sink keeps nothing alive: a sink retained past close rejects every frame
//! and holds no socket.
//!
//! When close returns `Ok`, all of this is true, in this order:
//!
//! 1. New operations are revoked. Nothing is admitted from now on.
//! 2. Owned tasks are stopped and joined.
//! 3. Admitted I/O has settled, or was cancelled once
//!    [`DeviceSession::SETTLE`] ran out.
//! 4. [`DeviceSession::finish`] ran bounded protocol cleanup.
//! 5. [`DeviceSession::release`] gave the transport back.
//!
//! Close runs on its own task and the caller only waits for it, so a caller
//! that gives up early never leaves a half-closed session behind. An error
//! means one of those steps failed or has not finished, and a generation that
//! has not finished stays Closing, so a reconnect or a retirement waits for
//! it instead of racing a transport that is still in use. Every wait for a
//! close is bounded by what the session was allowed, so a wedged transport
//! holds its own device and never a caller.
//!
//! Retiring a table closes everything in it, refuses every later connect, and
//! waits for every Opening and Closing generation, so an open still in flight
//! ends up closed instead of published.

use std::collections::HashMap;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock as StdRwLock, RwLockReadGuard, RwLockWriteGuard, Weak};
use std::time::{Duration, Instant};

use hypercolor_driver_api::{
    DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver, DeviceFrameSink,
    DeviceWriteOutcome,
};
use hypercolor_types::device::{DeviceError, DeviceId};
use tokio::sync::{Mutex, Notify, OwnedMutexGuard, watch};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

/// Default [`DeviceSession::OPEN`], past every connect timeout a driver asks
/// the host for.
pub const DEFAULT_OPEN: Duration = Duration::from_secs(30);

/// Default [`DeviceSession::SETTLE`].
pub const DEFAULT_SETTLE: Duration = Duration::from_millis(500);

/// Default [`DeviceSession::CLEANUP`].
pub const DEFAULT_CLEANUP: Duration = Duration::from_millis(1_400);

/// Default [`DeviceSession::RELEASE`].
pub const DEFAULT_RELEASE: Duration = Duration::from_millis(100);

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

    /// How long an open may run before the table drops it.
    ///
    /// Connects stop waiting on their own timeouts, but the open carries on,
    /// so this has to outlast them. An open that awaits blocking native work
    /// sets [`Duration::MAX`]: dropping that wait would detach the work, so
    /// the open ends when the work does.
    const OPEN: Duration = DEFAULT_OPEN;

    /// How long admitted I/O may keep running once the session is revoked
    /// before close cancels it.
    const SETTLE: Duration = DEFAULT_SETTLE;

    /// How long [`Self::finish`] may run before close drops it.
    const CLEANUP: Duration = DEFAULT_CLEANUP;

    /// How long close's caller waits for [`Self::release`]. Release itself
    /// runs to the end; past this the caller gets a timeout while the
    /// generation stays Closing.
    const RELEASE: Duration = DEFAULT_RELEASE;

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

    /// Run protocol cleanup: say goodbye on the wire.
    ///
    /// Close drops this future at [`Self::CLEANUP`] and carries on, so a
    /// step that matters more than the one before it needs its own bound.
    fn finish(&mut self) -> impl Future<Output = Result<(), DeviceError>> + Send {
        async { Ok(()) }
    }

    /// Give the transport back, consuming the session.
    ///
    /// The default drops the session. A transport whose release can block
    /// awaits that here, however long it takes, because the device stays
    /// Closing until this returns.
    fn release(self) -> impl Future<Output = Result<(), DeviceError>> + Send
    where
        Self: Sized,
    {
        async { Ok(()) }
    }
}

/// The state itself. Only its session holds a lasting reference; admitted
/// operations hold one just for their own length, so when the session is gone
/// the last of them to finish drops the state, with no handoff to miss.
type SessionState<S> = Arc<Mutex<Option<S>>>;

/// What a sink needs to reach a session's state.
struct Slot<S> {
    /// Set once; nothing is admitted afterwards.
    revoked: AtomicBool,
    /// Set when close stopped waiting for admitted I/O.
    cancelled: AtomicBool,
    cancel: Notify,
    state: Weak<Mutex<Option<S>>>,
}

impl<S> Slot<S> {
    fn new(state: &SessionState<S>) -> Self {
        Self {
            revoked: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            cancel: Notify::new(),
            state: Arc::downgrade(state),
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

    async fn cancelled(&self) {
        // Created before the check: `notify_waiters` reaches a `Notified`
        // from the moment it exists, so a cancel cannot slip between.
        let notified = self.cancel.notified();
        if !self.cancelled.load(Ordering::Acquire) {
            notified.await;
        }
    }

    async fn admit(&self) -> Option<Admitted<S>> {
        if self.is_revoked() {
            return None;
        }
        let state = self.state.upgrade()?.lock_owned().await;
        (!self.is_revoked() && state.is_some()).then_some(Admitted { state })
    }

    fn try_admit(&self) -> Option<Admitted<S>> {
        if self.is_revoked() {
            return None;
        }
        let state = self.state.upgrade()?.try_lock_owned().ok()?;
        (!self.is_revoked() && state.is_some()).then_some(Admitted { state })
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
/// Device I/O goes through [`SessionSink::write`] or
/// [`SessionSink::transact`], which close can cancel.
pub struct Admitted<S> {
    state: OwnedMutexGuard<Option<S>>,
}

impl<S> Deref for Admitted<S> {
    type Target = S;

    fn deref(&self) -> &S {
        self.state
            .as_ref()
            .expect("an admitted session stays in its slot")
    }
}

impl<S> DerefMut for Admitted<S> {
    fn deref_mut(&mut self) -> &mut S {
        self.state
            .as_mut()
            .expect("an admitted session stays in its slot")
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
    pub async fn admit(&self) -> Result<Admitted<S>, DeviceError> {
        self.slot
            .admit()
            .await
            .ok_or_else(|| disconnected(self.device_id))
    }

    /// Take exclusive access to the session state without waiting for it,
    /// or `None` when it is busy or revoked.
    #[must_use]
    pub fn try_admit(&self) -> Option<Admitted<S>> {
        self.slot.try_admit()
    }
}

impl<S> SessionSink<S> {
    /// Run device I/O against the session state, unless it was closed.
    ///
    /// Close can cancel this, which is what separates it from holding
    /// [`Self::admit`] across an await.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError::Disconnected`] when the session is revoked or
    /// close cancelled the I/O.
    pub async fn transact<T>(&self, io: impl AsyncFnOnce(&mut S) -> T) -> Result<T, DeviceError> {
        let mut session = self.admit().await?;
        self.slot
            .until_cancelled(io(&mut session))
            .await
            .ok_or_else(|| disconnected(self.device_id))
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
                DeviceDeliveryAck::failed(id, true, Duration::ZERO, disconnected(self.device_id))
            })
    }
}

/// Owner of one device connection.
pub struct Session<S: DeviceSession> {
    device_id: DeviceId,
    slot: Arc<Slot<S>>,
    state: SessionState<S>,
    facts: Arc<S::Facts>,
    tasks: Vec<JoinHandle<()>>,
    closing: bool,
}

impl<S: DeviceSession> Session<S> {
    /// Open a session for `device_id` around `state`.
    #[must_use]
    pub fn new(device_id: DeviceId, facts: S::Facts, state: S) -> Self {
        let state = Arc::new(Mutex::new(Some(state)));
        Self {
            device_id,
            slot: Arc::new(Slot::new(&state)),
            state,
            facts: Arc::new(facts),
            tasks: Vec::new(),
            closing: false,
        }
    }

    /// Facts this session was opened with.
    #[must_use]
    pub fn facts(&self) -> Arc<S::Facts> {
        Arc::clone(&self.facts)
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
    ///
    /// The table must already hold this device as Closing. The task clears
    /// that when it ends, however it ends.
    fn begin_close(mut self, shared: Arc<TableShared<S>>) -> JoinHandle<Result<(), DeviceError>> {
        self.closing = true;
        self.slot.revoke();
        let tasks = std::mem::take(&mut self.tasks);
        for task in &tasks {
            task.abort();
        }
        let device_id = self.device_id;
        let slot = Arc::clone(&self.slot);
        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            let closing = ClosingEntry { shared, device_id };
            let result = finish_close(device_id, &slot, state, tasks).await;
            if let Err(error) = &result {
                closing.record_failure(error);
            }
            result
        })
    }
}

impl<S: DeviceSession> Drop for Session<S> {
    /// A session dropped without closing still ends: it transmits nothing
    /// further, and the state goes with the last operation holding it,
    /// skipping only protocol cleanup.
    fn drop(&mut self) {
        if self.closing {
            return;
        }
        self.slot.revoke();
        self.slot.cancel();
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Keeps a device Closing for exactly as long as its close task lives.
struct ClosingEntry<S: DeviceSession> {
    shared: Arc<TableShared<S>>,
    device_id: DeviceId,
}

impl<S: DeviceSession> ClosingEntry<S> {
    fn record_failure(&self, error: &DeviceError) {
        let mut state = self.shared.write();
        if state.retired {
            state.failure.get_or_insert_with(|| error.clone());
        }
    }
}

impl<S: DeviceSession> Drop for ClosingEntry<S> {
    fn drop(&mut self) {
        self.shared.write().devices.remove(&self.device_id);
        self.shared.changed.notify_waiters();
    }
}

/// Join owned tasks, keeping the handle of any that has not finished, so a
/// deadline can interrupt this and a later call can pick up where it stopped.
async fn join_tasks(tasks: &mut Vec<JoinHandle<()>>) {
    while let Some(task) = tasks.last_mut() {
        let _ = task.await;
        tasks.pop();
    }
}

async fn finish_close<S: DeviceSession>(
    device_id: DeviceId,
    slot: &Slot<S>,
    state: SessionState<S>,
    mut tasks: Vec<JoinHandle<()>>,
) -> Result<(), DeviceError> {
    // A task that ignores its abort must not hold protocol cleanup hostage,
    // but it does keep this close from reporting success.
    let tasks_joined = tokio::time::timeout(S::SETTLE, join_tasks(&mut tasks))
        .await
        .is_ok();
    if !tasks_joined {
        warn!(%device_id, "a session task outlived its abort");
    }

    let mut guard = if let Ok(guard) = tokio::time::timeout(S::SETTLE, state.lock()).await {
        guard
    } else {
        debug!(%device_id, "cancelling device I/O that outlived its session");
        slot.cancel();
        state.lock().await
    };
    let session = guard.take();
    drop(guard);

    let mut result = Ok(());
    if let Some(mut session) = session {
        result = tokio::time::timeout(S::CLEANUP, session.finish())
            .await
            .unwrap_or(Err(DeviceError::Timeout { after: S::CLEANUP }));
        let released = session.release().await;
        if result.is_ok() {
            result = released;
        }
    }

    join_tasks(&mut tasks).await;
    if !tasks_joined && result.is_ok() {
        result = Err(DeviceError::Timeout { after: S::SETTLE });
    }
    result
}

/// Everything a close is allowed before a caller stops waiting for it. The
/// close keeps running and the device stays Closing.
fn close_budget<S: DeviceSession>() -> Duration {
    S::SETTLE
        .saturating_add(S::SETTLE)
        .saturating_add(CANCEL_GRACE)
        .saturating_add(S::CLEANUP)
        .saturating_add(S::RELEASE)
}

/// Wait for a close task.
async fn join_close(close: JoinHandle<Result<(), DeviceError>>) -> Result<(), DeviceError> {
    close
        .await
        .unwrap_or_else(|error| Err(DeviceError::protocol("device session", error)))
}

/// How an open ended, once it has. Every connect waiting on it gets a copy.
type OpenOutcome = Option<Result<(), DeviceError>>;

/// Wait for an open and share its result.
async fn wait_for_open(mut outcome: watch::Receiver<OpenOutcome>) -> Result<(), DeviceError> {
    match outcome.wait_for(Option::is_some).await {
        Ok(outcome) => outcome.clone().expect("wait_for saw an outcome"),
        Err(_) => Err(DeviceError::protocol(
            "device session",
            "an open ended without a result",
        )),
    }
}

/// Where one device stands in its table.
enum Generation<S: DeviceSession> {
    /// An open task is running. Connects subscribe to learn how it ends.
    Opening(Arc<watch::Sender<OpenOutcome>>),
    /// The session is live.
    Open(Session<S>),
    /// A close task is running.
    Closing,
}

struct TableState<S: DeviceSession> {
    retired: bool,
    /// First close that failed since retirement began.
    failure: Option<DeviceError>,
    devices: HashMap<DeviceId, Generation<S>>,
}

struct TableShared<S: DeviceSession> {
    state: StdRwLock<TableState<S>>,
    /// Wakes everyone waiting for a device or the table to change.
    changed: Notify,
}

impl<S: DeviceSession> TableShared<S> {
    fn read(&self) -> RwLockReadGuard<'_, TableState<S>> {
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, TableState<S>> {
        self.state.write().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<S: DeviceSession> TableState<S> {
    /// Move an open device to Closing, handing back its session.
    fn begin_closing(&mut self, id: DeviceId) -> Option<Session<S>> {
        let generation = self.devices.get_mut(&id)?;
        if !matches!(generation, Generation::Open(_)) {
            return None;
        }
        match std::mem::replace(generation, Generation::Closing) {
            Generation::Open(session) => Some(session),
            Generation::Opening(_) | Generation::Closing => None,
        }
    }
}

/// Keeps a device Opening for exactly as long as its open task lives.
struct OpeningEntry<S: DeviceSession> {
    shared: Arc<TableShared<S>>,
    device_id: DeviceId,
    outcome: Arc<watch::Sender<OpenOutcome>>,
    held: bool,
}

impl<S: DeviceSession> OpeningEntry<S> {
    /// Publish what the open produced, unless it no longer belongs.
    ///
    /// A retired table refuses the session, and one nobody is waiting for is
    /// nobody's: the connect behind it gave up, so nothing would ever use or
    /// close it. Either way it closes through normal cleanup, so nothing it
    /// opened survives. The checks and the publication share one lock, so a
    /// retirement or a connect joining this open lands on one side of them.
    async fn settle(mut self, opened: Result<Session<S>, DeviceError>) -> Result<(), DeviceError> {
        let session = opened?;
        let device_id = self.device_id;
        let refused = {
            let mut state = self.shared.write();
            self.held = false;
            if state.retired {
                state.devices.insert(device_id, Generation::Closing);
                Some((session, retired(device_id)))
            } else if self.outcome.receiver_count() == 0 {
                state.devices.insert(device_id, Generation::Closing);
                Some((session, abandoned(device_id)))
            } else {
                state.devices.insert(device_id, Generation::Open(session));
                None
            }
        };
        self.shared.changed.notify_waiters();
        let Some((session, refusal)) = refused else {
            return Ok(());
        };
        let budget = close_budget::<S>();
        let close = session.begin_close(Arc::clone(&self.shared));
        let closed = tokio::time::timeout(budget, join_close(close))
            .await
            .unwrap_or(Err(DeviceError::Timeout { after: budget }));
        if let Err(error) = closed {
            debug!(%device_id, %error, "cleanup of a refused session failed");
        }
        Err(refusal)
    }
}

impl<S: DeviceSession> Drop for OpeningEntry<S> {
    fn drop(&mut self) {
        if self.held {
            self.shared.write().devices.remove(&self.device_id);
            self.shared.changed.notify_waiters();
        }
    }
}

/// Run one open to its end, whoever is still waiting for it.
async fn run_open<S: DeviceSession>(
    opening: OpeningEntry<S>,
    open: impl Future<Output = Result<Session<S>, DeviceError>>,
) {
    let outcome = Arc::clone(&opening.outcome);
    let opened = tokio::time::timeout(S::OPEN, open)
        .await
        .unwrap_or(Err(DeviceError::Timeout { after: S::OPEN }));
    let result = opening.settle(opened).await;
    outcome.send_replace(Some(result));
}

/// Every session of one backend.
pub struct SessionTable<S: DeviceSession> {
    shared: Arc<TableShared<S>>,
}

impl<S: DeviceSession> Default for SessionTable<S> {
    fn default() -> Self {
        Self {
            shared: Arc::new(TableShared {
                state: StdRwLock::new(TableState {
                    retired: false,
                    failure: None,
                    devices: HashMap::new(),
                }),
                changed: Notify::new(),
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

    fn open_session<T>(&self, id: &DeviceId, read: impl FnOnce(&Session<S>) -> T) -> Option<T> {
        match self.shared.read().devices.get(id) {
            Some(Generation::Open(session)) => Some(read(session)),
            _ => None,
        }
    }

    /// Whether `id` has an open session.
    #[must_use]
    pub fn is_open(&self, id: &DeviceId) -> bool {
        self.open_session(id, |_| ()).is_some()
    }

    /// Facts of the session open for `id`.
    #[must_use]
    pub fn facts(&self, id: &DeviceId) -> Option<Arc<S::Facts>> {
        self.open_session(id, Session::facts)
    }

    /// Sink over the session open for `id`.
    #[must_use]
    pub fn sink(&self, id: &DeviceId) -> Option<SessionSink<S>> {
        self.open_session(id, Session::sink)
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

    /// Open `id` unless it is open already.
    ///
    /// `open` builds the future that does the work, and runs only when this
    /// call starts an open. The future runs on a task the table owns, so it
    /// outlives this call: dropping this call stops the wait, never the open.
    /// A connect that arrives during an open shares its result, and one that
    /// arrives during a close waits it out.
    ///
    /// # Errors
    ///
    /// Returns what the open returned, [`DeviceError::ConnectionFailed`] once
    /// the table is retired, and [`DeviceError::Timeout`] when a close in the
    /// way runs past its budget.
    pub async fn open<F, Fut>(&self, id: &DeviceId, open: F) -> Result<(), DeviceError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Session<S>, DeviceError>> + Send + 'static,
    {
        self.open_unless(id, |_| true, open).await
    }

    /// Like [`Self::open`], for a device whose target can move: an open
    /// session `keep` turns down is closed, cleanup included, before the new
    /// open starts.
    ///
    /// # Errors
    ///
    /// As [`Self::open`].
    pub async fn open_unless<F, Fut>(
        &self,
        id: &DeviceId,
        keep: impl Fn(&S::Facts) -> bool,
        open: F,
    ) -> Result<(), DeviceError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Session<S>, DeviceError>> + Send + 'static,
    {
        enum Step<S: DeviceSession> {
            Start(
                Arc<watch::Sender<OpenOutcome>>,
                watch::Receiver<OpenOutcome>,
            ),
            Join(watch::Receiver<OpenOutcome>),
            CloseMoved(Session<S>),
            WaitForClose,
        }

        let budget = close_budget::<S>();
        let mut close_seen_at = None;
        let mut open = Some(open);
        loop {
            // Created before reading the table: `notify_waiters` reaches a
            // `Notified` from the moment it exists, so no change can slip
            // between the read and the wait.
            let changed = self.shared.changed.notified();
            let step = {
                let mut state = self.shared.write();
                if state.retired {
                    return Err(retired(*id));
                }
                match state.devices.get(id) {
                    None => {
                        let (outcome, waiting) = watch::channel(None);
                        let outcome = Arc::new(outcome);
                        state
                            .devices
                            .insert(*id, Generation::Opening(Arc::clone(&outcome)));
                        Step::Start(outcome, waiting)
                    }
                    Some(Generation::Open(session)) if keep(&session.facts) => return Ok(()),
                    Some(Generation::Open(_)) => match state.begin_closing(*id) {
                        Some(session) => Step::CloseMoved(session),
                        None => Step::WaitForClose,
                    },
                    Some(Generation::Opening(outcome)) => Step::Join(outcome.subscribe()),
                    Some(Generation::Closing) => Step::WaitForClose,
                }
            };
            match step {
                Step::Start(outcome, waiting) => {
                    let opening = OpeningEntry {
                        shared: Arc::clone(&self.shared),
                        device_id: *id,
                        outcome,
                        held: true,
                    };
                    let open = open.take().expect("a connect starts one open and returns");
                    tokio::spawn(run_open(opening, open()));
                    return wait_for_open(waiting).await;
                }
                // The open may have been for a target `keep` turns down, so
                // a success goes around again.
                Step::Join(waiting) => wait_for_open(waiting).await?,
                Step::CloseMoved(session) => {
                    let close = session.begin_close(Arc::clone(&self.shared));
                    match tokio::time::timeout(budget, join_close(close)).await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            debug!(device_id = %id, %error, "closing a moved session failed");
                        }
                        Err(_) => return Err(DeviceError::Timeout { after: budget }),
                    }
                }
                Step::WaitForClose => {
                    let close_seen_at = *close_seen_at.get_or_insert_with(Instant::now);
                    let left = budget.saturating_sub(close_seen_at.elapsed());
                    if tokio::time::timeout(left, changed).await.is_err() {
                        return Err(DeviceError::Timeout { after: budget });
                    }
                }
            }
        }
    }

    /// Close the session open for `id`, once any open in flight settles.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError::Disconnected`] when no session is open, the
    /// cleanup failure when protocol cleanup fails or times out, and
    /// [`DeviceError::Timeout`] when the device has not closed within what
    /// its close was allowed. The session is revoked either way, and an
    /// unfinished close keeps the device Closing.
    pub async fn close(&self, id: &DeviceId) -> Result<(), DeviceError> {
        let budget = close_budget::<S>();
        let close = async {
            loop {
                let changed = self.shared.changed.notified();
                let session = {
                    let mut state = self.shared.write();
                    match state.devices.get(id) {
                        None => return Err(disconnected(*id)),
                        Some(Generation::Open(_)) => state.begin_closing(*id),
                        Some(Generation::Opening(_) | Generation::Closing) => None,
                    }
                };
                if let Some(session) = session {
                    return join_close(session.begin_close(Arc::clone(&self.shared))).await;
                }
                changed.await;
            }
        };
        tokio::time::timeout(budget, close)
            .await
            .unwrap_or(Err(DeviceError::Timeout { after: budget }))
    }

    /// Close every session, refuse all later connects, and wait for every
    /// generation still Opening or Closing.
    ///
    /// # Errors
    ///
    /// Returns the first cleanup failure since retirement began, and
    /// [`DeviceError::Timeout`] when a generation is still running after what
    /// it was allowed. Every session is revoked either way, and an open or a
    /// close still running finishes under the table.
    pub async fn retire(&self) -> Result<(), DeviceError> {
        let closing: Vec<_> = {
            let mut state = self.shared.write();
            state.retired = true;
            let open: Vec<DeviceId> = state
                .devices
                .iter()
                .filter(|(_, generation)| matches!(generation, Generation::Open(_)))
                .map(|(id, _)| *id)
                .collect();
            open.into_iter()
                .filter_map(|id| state.begin_closing(id))
                .collect()
        };
        self.shared.changed.notify_waiters();
        for session in closing {
            drop(session.begin_close(Arc::clone(&self.shared)));
        }
        let budget = close_budget::<S>();
        let budget = budget.saturating_add(S::OPEN.min(budget));
        let settled = async {
            loop {
                let changed = self.shared.changed.notified();
                {
                    let state = self.shared.read();
                    if state.devices.is_empty() {
                        return state.failure.clone().map_or(Ok(()), Err);
                    }
                }
                changed.await;
            }
        };
        tokio::time::timeout(budget, settled)
            .await
            .unwrap_or(Err(DeviceError::Timeout { after: budget }))
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

fn abandoned(device_id: DeviceId) -> DeviceError {
    DeviceError::connection(device_id, "every connect waiting on this open gave up")
}
