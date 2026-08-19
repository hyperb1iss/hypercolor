use std::path::PathBuf;
use std::sync::Arc;

/// Capture configuration for one evdev session.
#[derive(Clone)]
pub struct EvdevInputConfig {
    /// Open keyboard-capable event nodes.
    pub keyboard: bool,
    /// Open relative-pointer event nodes.
    pub pointer: bool,
    /// Epoch allocated by the shared fold and echoed in every batch.
    pub epoch: u64,
    /// Shared monotonic clock sampled once for every published batch.
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl std::fmt::Debug for EvdevInputConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvdevInputConfig")
            .field("keyboard", &self.keyboard)
            .field("pointer", &self.pointer)
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

/// Device classes accepted from one evdev node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceCapabilities {
    pub keyboard: bool,
    pub pointer: bool,
}

/// Immutable identity for one opened event-node lifetime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvdevDeviceDescriptor {
    pub source_id: Arc<str>,
    pub path: Arc<str>,
    pub label: Arc<str>,
    pub capabilities: DeviceCapabilities,
    pub session_epoch: u64,
    /// Monotonic within one acquisition session. Reopening the same path gets
    /// a new generation even when the kernel recycles the node name.
    pub device_generation: u64,
}

/// Native repeat evidence carried by an evdev key report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvdevKeyState {
    Pressed,
    Released,
    Repeated,
}

/// Pointer buttons with stable cross-platform meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EvdevPointerButton {
    Left,
    Right,
    Middle,
    Side,
    Extra,
}

impl EvdevPointerButton {
    /// Canonical name used by the shared fold.
    #[must_use]
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Middle => "middle",
            Self::Side => "side",
            Self::Extra => "extra",
        }
    }
}

/// Why the shared fold must discard its assumptions for one device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvdevStateGapReason {
    SynchronizationDropped,
    DeviceRemoved,
    ReadFailed,
}

/// One ordered native edge or device-lifecycle marker.
#[derive(Debug, Clone, PartialEq)]
pub enum EvdevInputEvent {
    Key {
        device: Arc<EvdevDeviceDescriptor>,
        code: u16,
        state: EvdevKeyState,
    },
    Button {
        device: Arc<EvdevDeviceDescriptor>,
        button: EvdevPointerButton,
        state: EvdevKeyState,
    },
    /// Relative counts accumulated for one device until its `SYN_REPORT`.
    MotionRelative {
        device: Arc<EvdevDeviceDescriptor>,
        dx: i32,
        dy: i32,
    },
    /// Signed Q16.16 travel in `Line120` units.
    Scroll {
        device: Arc<EvdevDeviceDescriptor>,
        delta_x_q16_16: i64,
        delta_y_q16_16: i64,
    },
    DeviceArrived {
        device: Arc<EvdevDeviceDescriptor>,
    },
    StateGap {
        device: Arc<EvdevDeviceDescriptor>,
        reason: EvdevStateGapReason,
    },
    DeviceRemoved {
        device: Arc<EvdevDeviceDescriptor>,
    },
}

impl EvdevInputEvent {
    /// Device bucket this event belongs to.
    #[must_use]
    pub fn device(&self) -> &Arc<EvdevDeviceDescriptor> {
        match self {
            Self::Key { device, .. }
            | Self::Button { device, .. }
            | Self::MotionRelative { device, .. }
            | Self::Scroll { device, .. }
            | Self::DeviceArrived { device }
            | Self::StateGap { device, .. }
            | Self::DeviceRemoved { device } => device,
        }
    }

    /// Stable source bucket for the event-node lifetime.
    #[must_use]
    pub fn source_id(&self) -> &Arc<str> {
        &self.device().source_id
    }
}

/// One coherent publication from the acquisition worker.
#[derive(Debug)]
pub struct EvdevInputBatch<'a> {
    pub events: &'a [EvdevInputEvent],
    pub at_ms: u64,
    pub epoch: u64,
    /// Advances once for each rescan or read-failure transaction that changes
    /// the open-device set.
    pub topology_generation: u64,
}

/// Why an event node is not currently contributing input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceOpenState {
    Opened,
    PermissionDenied,
    Ignored,
    Failed(String),
}

/// Per-node result from the latest discovery pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceOpenStatus {
    pub path: PathBuf,
    pub label: String,
    pub state: DeviceOpenState,
}

/// Liveness of the acquisition worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvdevWorkerState {
    Running,
    Failed(String),
}

/// Errors from validating or starting an evdev session.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EvdevInputError {
    #[error("evdev host input is only available on Linux")]
    UnsupportedPlatform,
    #[error("no input kinds enabled for capture")]
    NothingToCapture,
    #[error("failed to spawn the evdev input worker: {0}")]
    WorkerSpawn(String),
    #[error("timed out waiting for the evdev input worker to become ready")]
    WorkerReadyTimeout,
    #[error("evdev input publication panicked during worker startup")]
    InitialPublicationPanicked,
}

pub type EvdevInputResult<T> = Result<T, EvdevInputError>;

/// Events awaiting one atomic delivery to the shared fold.
#[derive(Debug, Default)]
pub struct PendingEvents {
    events: Vec<EvdevInputEvent>,
}

impl PendingEvents {
    #[must_use]
    pub const fn new() -> Self {
        Self { events: Vec::new() }
    }

    pub fn push(&mut self, event: EvdevInputEvent) {
        self.events.push(event);
    }

    pub fn extend(&mut self, events: impl IntoIterator<Item = EvdevInputEvent>) {
        self.events.extend(events);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Deliver the pending sequence once, then clear it.
    pub fn deliver(
        &mut self,
        at_ms: u64,
        epoch: u64,
        topology_generation: u64,
        sink: &mut impl FnMut(EvdevInputBatch<'_>),
    ) -> bool {
        if self.events.is_empty() {
            return false;
        }
        sink(EvdevInputBatch {
            events: &self.events,
            at_ms,
            epoch,
            topology_generation,
        });
        self.events.clear();
        true
    }
}
