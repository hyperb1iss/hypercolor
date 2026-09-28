//! Whether each half of a controller is taking commands, kept per controller
//! across sessions so a wedge outlives the connect that found it.
//!
//! A live dongle takes a 64-byte bulk packet in about a millisecond. The
//! vendor tool gives a write 100 ms and gives up on a dongle after five
//! failed writes in a row, resetting it through its partner
//! (`lewisgibson/FanControl.LianLi`, `Transport/WinUsbTransport.cs` and
//! `Devices/WirelessDonglePair.cs`). The bulk transport gives a write one
//! second, so a single write refused for that long is treated as the same
//! verdict: the dongle is unresponsive. A firmware hang is the suspected
//! cause; on the V1 `SLV3TX` the state has only ever cleared with a power
//! cycle, and a USB-level reset of the stuck TX made it fail re-enumeration.
//!
//! The registry is keyed by the controller's identity (the TX's USB path),
//! records which half stalled, and is cleared by the first command that half
//! takes afterwards, through any session, so the wedge and its recovery each
//! log once.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Instant;

use crate::protocol::TransferType;

/// One of the controller's two USB functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DongleHalf {
    /// The `0x8040` transmitter: RF sends and the master query.
    Tx,
    /// The `0x8041` receiver: the device table poll.
    Rx,
}

impl DongleHalf {
    /// The half a transfer type reaches through the controller's transport.
    #[must_use]
    pub const fn for_transfer(transfer_type: TransferType) -> Self {
        match transfer_type {
            TransferType::Companion => Self::Rx,
            TransferType::Primary | TransferType::Bulk | TransferType::HidReport => Self::Tx,
        }
    }

    /// The other half, which carries the reset for this one.
    #[must_use]
    pub const fn partner(self) -> Self {
        match self {
            Self::Tx => Self::Rx,
            Self::Rx => Self::Tx,
        }
    }

    /// The transfer type that reaches this half.
    #[must_use]
    pub const fn transfer_type(self) -> TransferType {
        match self {
            Self::Tx => TransferType::Primary,
            Self::Rx => TransferType::Companion,
        }
    }

    /// Short name for logs: `TX` or `RX`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tx => "TX",
            Self::Rx => "RX",
        }
    }
}

/// A half that stopped taking commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wedge {
    /// The half that stalled.
    pub half: DongleHalf,
    /// When the first stall of this wedge was seen.
    pub since: Instant,
    /// Stalled writes since the wedge began, across reconnects.
    pub stalls: u32,
    /// Resets delivered to the partner half for this wedge.
    pub resets_sent: u32,
}

/// A controller's standing, as its transports have seen it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerHealth {
    /// No stall seen, or every stalled half has taken a command since.
    Healthy,
    /// A half stopped taking commands and has not taken one since. The TX
    /// is reported first when both are wedged, since it carries the output.
    Wedged(Wedge),
}

/// The standing of the controller identified by `controller` (the TX's USB
/// path, as the transport factory was handed it).
#[must_use]
pub fn controller_health(controller: &str) -> ControllerHealth {
    let wedges = registry().lock().unwrap_or_else(PoisonError::into_inner);
    [DongleHalf::Tx, DongleHalf::Rx]
        .into_iter()
        .find_map(|half| wedges.get(&(controller.to_owned(), half)).copied())
        .map_or(ControllerHealth::Healthy, ControllerHealth::Wedged)
}

/// Whether any controller has a wedge open. The hot path checks this before
/// taking the registry lock, so a healthy rig pays one atomic load per write.
pub(crate) fn any_wedge_open() -> bool {
    OPEN_WEDGES.load(Ordering::Acquire) > 0
}

/// Count a stalled write on `half`, opening a wedge when none is open.
pub(crate) fn record_stall(controller: &str, half: DongleHalf) -> Wedge {
    let mut wedges = registry().lock().unwrap_or_else(PoisonError::into_inner);
    let wedge = match wedges.entry((controller.to_owned(), half)) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => {
            OPEN_WEDGES.fetch_add(1, Ordering::AcqRel);
            entry.insert(Wedge {
                half,
                since: Instant::now(),
                stalls: 0,
                resets_sent: 0,
            })
        }
    };
    wedge.stalls = wedge.stalls.saturating_add(1);
    *wedge
}

/// Count a reset delivered to the partner of the wedged `half`.
pub(crate) fn record_reset_sent(controller: &str, half: DongleHalf) {
    let mut wedges = registry().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(wedge) = wedges.get_mut(&(controller.to_owned(), half)) {
        wedge.resets_sent = wedge.resets_sent.saturating_add(1);
    }
}

/// `half` took a command: close its wedge, returning it when one was open.
pub(crate) fn record_alive(controller: &str, half: DongleHalf) -> Option<Wedge> {
    let mut wedges = registry().lock().unwrap_or_else(PoisonError::into_inner);
    let closed = wedges.remove(&(controller.to_owned(), half));
    if closed.is_some() {
        OPEN_WEDGES.fetch_sub(1, Ordering::AcqRel);
    }
    drop(wedges);
    closed
}

/// Wedges in the registry, maintained under its lock.
static OPEN_WEDGES: AtomicUsize = AtomicUsize::new(0);

fn registry() -> &'static Mutex<HashMap<(String, DongleHalf), Wedge>> {
    static WEDGES: OnceLock<Mutex<HashMap<(String, DongleHalf), Wedge>>> = OnceLock::new();
    WEDGES.get_or_init(|| Mutex::new(HashMap::new()))
}
