//! One submission, one receipt: what became of a frame offered to a device.
//!
//! The host wraps every frame it offers in a [`Frame`]. Whoever owns the
//! transport moves the frame's [`Attempt`] along: [`Attempt::start`] marks the
//! first transport I/O, and exactly one terminal call settles it. [`Receipt`]
//! can only come out of those calls, so nothing can report bytes that were
//! never written or a skip that touched the wire, and an attempt can only
//! settle once because settling consumes it. An attempt dropped unsettled
//! still reports, so a cancelled delivery never leaves the host waiting.
//!
//! A frame owner that hands the attempt to a worker returns
//! [`Submission::Pending`], and the receipt reaches the observer when the
//! worker settles it. That is the only difference between a lane that writes
//! inline and one that owns a mailbox.
//!
//! # Bytes
//!
//! [`DeliveryResult::Written`] counts bytes of the submitted RGB frame the
//! write covered: three per LED, excluding protocol framing. A whole frame is
//! `colors.len() * 3` whatever the wire format. A protocol that writes only
//! what changed reports the part it wrote.
//!
//! # What "written" means
//!
//! The local transport completed the write. For a datagram that proves
//! nothing about the device: a peer's opinion, when it gives one, is the
//! optional [`PeerDisposition`].

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use hypercolor_types::device::{DeviceError, DeviceId};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// Queue-qualified identity for one device-frame delivery attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceDeliveryId {
    /// Generation of the output queue that issued this attempt.
    pub queue_generation: u64,
    /// Monotonic sequence within the queue.
    pub sequence: u64,
}

/// Whether a lane can take a frame now.
///
/// A lane with its own reasons to wait (a device quota, a transport backlog,
/// another application holding the device) answers with this, and a deferred
/// answer becomes [`SkipReason::Deferred`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// The lane can write now.
    Ready,
    /// The lane cannot take a frame until `retry_at`.
    Deferred {
        /// When the lane expects to be ready.
        retry_at: Instant,
    },
}

/// Why a frame did not reach the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The device already shows this frame.
    Unchanged,
    /// The lane cannot take a frame until `retry_at`.
    Deferred {
        /// When the lane expects to be ready.
        retry_at: Instant,
    },
    /// A newer frame took this one's place.
    Superseded,
}

/// What a peer made of a frame it received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerDisposition {
    /// The peer applied the frame.
    Accepted,
    /// The peer refused the frame and keeps showing what it showed. The
    /// session stays healthy and a later frame may be accepted.
    Declined,
}

/// Where a failed attempt stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoStage {
    /// Before any transport I/O. Nothing of the frame reached the device.
    Admission,
    /// During transport I/O. The outcome is unconfirmed: some or all of the
    /// frame may have reached the device.
    Transport,
}

/// The one terminal result of a delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryResult {
    /// Nothing was written.
    NotWritten(SkipReason),
    /// The transport completed the write.
    Written {
        /// Bytes of the frame the write covered.
        bytes: u64,
        /// What the peer reported, if it reports anything.
        peer: Option<PeerDisposition>,
    },
    /// The attempt failed.
    Failed {
        /// Where it stopped.
        stage: IoStage,
        /// What went wrong.
        error: DeviceError,
        /// Whether the lane keeps running, so one failure alone says nothing
        /// about the session's health.
        transient: bool,
    },
}

/// The terminal record of one delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    id: DeviceDeliveryId,
    result: DeliveryResult,
    queue_wait: Duration,
    transport_latency: Duration,
    total_latency: Duration,
    settled_at: Instant,
}

impl Receipt {
    /// Delivery this receipt settles.
    #[must_use]
    pub const fn id(&self) -> DeviceDeliveryId {
        self.id
    }

    /// The terminal result.
    #[must_use]
    pub const fn result(&self) -> &DeliveryResult {
        &self.result
    }

    /// Time the frame waited before its transport started. For a frame that
    /// never started, its whole life.
    #[must_use]
    pub const fn queue_wait(&self) -> Duration {
        self.queue_wait
    }

    /// Time spent in transport I/O, excluding queue wait.
    #[must_use]
    pub const fn transport_latency(&self) -> Duration {
        self.transport_latency
    }

    /// Time from production to this receipt.
    #[must_use]
    pub const fn total_latency(&self) -> Duration {
        self.total_latency
    }

    /// When the attempt settled.
    #[must_use]
    pub const fn settled_at(&self) -> Instant {
        self.settled_at
    }

    /// The failure, for a caller that only cares whether the frame failed.
    ///
    /// # Errors
    ///
    /// Returns the attempt's error when it failed. A skipped frame is not a
    /// failure.
    pub fn into_result(self) -> Result<(), DeviceError> {
        match self.result {
            DeliveryResult::Failed { error, .. } => Err(error),
            DeliveryResult::NotWritten(_) | DeliveryResult::Written { .. } => Ok(()),
        }
    }
}

/// Watches delivery attempts cross transport boundaries.
pub trait FrameObserver: Send + Sync {
    /// The attempt's first transport I/O is about to run.
    fn transport_started(&self, id: DeviceDeliveryId);

    /// The attempt settled. Called exactly once per attempt, on whichever
    /// task owned it.
    fn settled(&self, receipt: &Receipt);
}

/// What became of a submission, as far as the submitter can know yet.
#[derive(Debug)]
#[must_use]
pub enum Submission {
    /// The attempt settled before the submission returned. The observer
    /// already has this receipt.
    Settled(Receipt),
    /// The attempt is with a worker, and the observer gets the receipt when
    /// the worker settles it.
    Pending,
}

impl From<Receipt> for Submission {
    fn from(receipt: Receipt) -> Self {
        Self::Settled(receipt)
    }
}

struct Tracked {
    device_id: DeviceId,
    id: DeviceDeliveryId,
    observer: Option<Arc<dyn FrameObserver>>,
    /// A frame has at most two waiters: whoever submitted it and a sink that
    /// blocks on the worker it handed the frame to.
    waiters: [Option<oneshot::Sender<Receipt>>; 2],
    produced_at: Instant,
    frame_bytes: u64,
}

impl Tracked {
    fn settle(self, started_at: Option<Instant>, result: DeliveryResult) -> Receipt {
        let settled_at = Instant::now();
        let receipt = Receipt {
            id: self.id,
            result,
            queue_wait: started_at
                .unwrap_or(settled_at)
                .saturating_duration_since(self.produced_at),
            transport_latency: started_at.map_or(Duration::ZERO, |at| {
                settled_at.saturating_duration_since(at)
            }),
            total_latency: settled_at.saturating_duration_since(self.produced_at),
            settled_at,
        };
        if let Some(observer) = &self.observer {
            observer.settled(&receipt);
        }
        for waiter in self.waiters.into_iter().flatten() {
            let _ = waiter.send(receipt.clone());
        }
        receipt
    }

    fn disconnected(&self) -> DeviceError {
        DeviceError::Disconnected {
            device: self.device_id.to_string(),
        }
    }
}

/// A delivery attempt whose transport has not started.
pub struct Attempt {
    tracked: Option<Tracked>,
}

impl Attempt {
    fn take(&mut self) -> Tracked {
        self.tracked
            .take()
            .expect("an attempt settles once because settling consumes it")
    }

    /// Mark the first transport I/O of this attempt.
    #[must_use]
    pub fn start(mut self) -> Transmission {
        let tracked = self.take();
        if let Some(observer) = &tracked.observer {
            observer.transport_started(tracked.id);
        }
        Transmission {
            tracked: Some(tracked),
            started_at: Instant::now(),
        }
    }

    /// Settle without writing.
    pub fn skip(mut self, reason: SkipReason) -> Receipt {
        self.take().settle(None, DeliveryResult::NotWritten(reason))
    }

    /// Settle as failed before any transport I/O.
    pub fn reject(mut self, error: DeviceError) -> Receipt {
        self.take().settle(
            None,
            DeliveryResult::Failed {
                stage: IoStage::Admission,
                error,
                transient: false,
            },
        )
    }

    /// Settle as failed before any transport I/O, with the lane still
    /// running.
    pub fn reject_transient(mut self, error: DeviceError) -> Receipt {
        self.take().settle(
            None,
            DeliveryResult::Failed {
                stage: IoStage::Admission,
                error,
                transient: true,
            },
        )
    }
}

impl Drop for Attempt {
    /// An attempt nobody settled was cancelled with the session or the queue
    /// that owned it.
    fn drop(&mut self) {
        if let Some(tracked) = self.tracked.take() {
            let error = tracked.disconnected();
            tracked.settle(
                None,
                DeliveryResult::Failed {
                    stage: IoStage::Admission,
                    error,
                    transient: false,
                },
            );
        }
    }
}

/// A delivery attempt whose transport has started.
pub struct Transmission {
    tracked: Option<Tracked>,
    started_at: Instant,
}

impl Transmission {
    fn take(&mut self) -> Tracked {
        self.tracked
            .take()
            .expect("a transmission settles once because settling consumes it")
    }

    /// Settle with the whole frame written.
    pub fn written(mut self) -> Receipt {
        let tracked = self.take();
        let bytes = tracked.frame_bytes;
        tracked.settle(
            Some(self.started_at),
            DeliveryResult::Written { bytes, peer: None },
        )
    }

    /// Settle with `bytes` of the frame written, for a protocol that writes
    /// only part of it.
    pub fn written_part(mut self, bytes: usize) -> Receipt {
        let tracked = self.take();
        tracked.settle(
            Some(self.started_at),
            DeliveryResult::Written {
                bytes: u64::try_from(bytes).unwrap_or(u64::MAX),
                peer: None,
            },
        )
    }

    /// Settle with the whole frame written and the peer's answer.
    pub fn answered(mut self, peer: PeerDisposition) -> Receipt {
        let tracked = self.take();
        let bytes = tracked.frame_bytes;
        tracked.settle(
            Some(self.started_at),
            DeliveryResult::Written {
                bytes,
                peer: Some(peer),
            },
        )
    }

    /// Settle as replaced by a newer frame after part of this one went out,
    /// for a protocol that applies a frame only once all of it arrives.
    pub fn superseded(mut self) -> Receipt {
        let tracked = self.take();
        tracked.settle(
            Some(self.started_at),
            DeliveryResult::NotWritten(SkipReason::Superseded),
        )
    }

    /// Settle as failed during transport I/O.
    pub fn failed(mut self, error: DeviceError) -> Receipt {
        let tracked = self.take();
        tracked.settle(
            Some(self.started_at),
            DeliveryResult::Failed {
                stage: IoStage::Transport,
                error,
                transient: false,
            },
        )
    }

    /// Settle as failed during transport I/O, with the lane still running.
    ///
    /// The host rebuilds the session only after several of these arrive in a
    /// row, so report this only when the lane keeps running.
    pub fn failed_transient(mut self, error: DeviceError) -> Receipt {
        let tracked = self.take();
        tracked.settle(
            Some(self.started_at),
            DeliveryResult::Failed {
                stage: IoStage::Transport,
                error,
                transient: true,
            },
        )
    }
}

impl Drop for Transmission {
    /// A transmission nobody settled was cancelled mid-write. Blocking I/O
    /// behind the write can outlive that and still finish, so this says what
    /// every [`IoStage::Transport`] failure says: the outcome is unconfirmed,
    /// not that the frame failed to land.
    fn drop(&mut self) {
        if let Some(tracked) = self.tracked.take() {
            let error = tracked.disconnected();
            tracked.settle(
                Some(self.started_at),
                DeliveryResult::Failed {
                    stage: IoStage::Transport,
                    error,
                    transient: false,
                },
            );
        }
    }
}

/// The receipt of a frame, for a submitter that waits for it.
pub struct ReceiptWait {
    receipt: oneshot::Receiver<Receipt>,
}

impl ReceiptWait {
    /// Wait for the attempt to settle. An attempt always settles, even when
    /// it is dropped, so `None` only follows a panic while settling.
    pub async fn received(self) -> Option<Receipt> {
        self.receipt.await.ok()
    }

    /// Wait for the attempt to settle, as a submission.
    pub async fn settled(self) -> Submission {
        self.received()
            .await
            .map_or(Submission::Pending, Into::into)
    }
}

/// One frame offered to a device, with the attempt that tracks it.
pub struct Frame {
    colors: Arc<Vec<[u8; 3]>>,
    attempt: Attempt,
}

impl Frame {
    fn tracked(
        device_id: DeviceId,
        id: DeviceDeliveryId,
        observer: Option<Arc<dyn FrameObserver>>,
        produced_at: Instant,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Self {
        let frame_bytes = u64::try_from(colors.len().saturating_mul(3)).unwrap_or(u64::MAX);
        Self {
            colors,
            attempt: Attempt {
                tracked: Some(Tracked {
                    device_id,
                    id,
                    observer,
                    waiters: [None, None],
                    produced_at,
                    frame_bytes,
                }),
            },
        }
    }

    /// A frame the host produced at `produced_at`, reporting to `observer`.
    #[must_use]
    pub fn new(
        device_id: DeviceId,
        id: DeviceDeliveryId,
        observer: Arc<dyn FrameObserver>,
        produced_at: Instant,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> Self {
        Self::tracked(device_id, id, Some(observer), produced_at, colors)
    }

    /// A frame outside any host queue, with nobody observing it.
    #[must_use]
    pub fn direct(device_id: DeviceId, colors: Arc<Vec<[u8; 3]>>) -> Self {
        let id = DeviceDeliveryId {
            queue_generation: 0,
            sequence: 0,
        };
        Self::tracked(device_id, id, None, Instant::now(), colors)
    }

    /// The LED colors.
    #[must_use]
    pub fn colors(&self) -> &[[u8; 3]] {
        self.colors.as_slice()
    }

    /// Have the receipt delivered here too, for a submitter that blocks on a
    /// worker it handed the frame to.
    ///
    /// # Panics
    ///
    /// Panics on a third waiter: one submitter and one sink is the most a
    /// frame ever has.
    #[must_use]
    pub fn wait_for_receipt(&mut self) -> ReceiptWait {
        let (waiter, receipt) = oneshot::channel();
        if let Some(tracked) = &mut self.attempt.tracked {
            let slot = tracked
                .waiters
                .iter_mut()
                .find(|slot| slot.is_none())
                .expect("a frame has at most two waiters");
            *slot = Some(waiter);
        }
        ReceiptWait { receipt }
    }

    /// Split the frame into its colors and its attempt.
    #[must_use]
    pub fn into_parts(self) -> (Arc<Vec<[u8; 3]>>, Attempt) {
        (self.colors, self.attempt)
    }
}

/// A frame waiting in a latest-value mailbox.
///
/// A worker claims the attempt to write the frame. A newer frame replacing
/// this one before that settles it as superseded, and the two can race, so
/// the attempt goes to exactly one of them.
pub struct MailboxFrame {
    colors: Arc<Vec<[u8; 3]>>,
    attempt: Mutex<Option<Attempt>>,
}

impl MailboxFrame {
    /// Post `frame`.
    #[must_use]
    pub fn new(frame: Frame) -> Self {
        let (colors, attempt) = frame.into_parts();
        Self {
            colors,
            attempt: Mutex::new(Some(attempt)),
        }
    }

    /// The LED colors.
    #[must_use]
    pub fn colors(&self) -> &[[u8; 3]] {
        self.colors.as_slice()
    }

    /// Take the attempt, or `None` when somebody already did.
    #[must_use]
    pub fn claim(&self) -> Option<Attempt> {
        self.attempt
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Settle as replaced by a newer frame, unless a worker claimed it.
    pub fn supersede(&self) {
        if let Some(attempt) = self.claim() {
            attempt.skip(SkipReason::Superseded);
        }
    }

    /// Settle as failed before any transport I/O, unless a worker claimed it.
    pub fn reject(&self, error: DeviceError) {
        if let Some(attempt) = self.claim() {
            attempt.reject(error);
        }
    }
}
