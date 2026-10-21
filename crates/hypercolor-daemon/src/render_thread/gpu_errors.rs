//! Process-wide accounting for wgpu errors no error scope captured.
//!
//! wgpu's default uncaptured-error handler panics the calling thread, which
//! on the render thread takes the whole pipeline down for a validation
//! mistake. Every device the daemon opens routes those errors here instead:
//! each one is counted, the latest is kept for diagnostics, and they are
//! logged at error level under a burst limit so a cascade of follow-on
//! errors from one invalid resource cannot flood the log. Device loss is
//! counted and logged too, and each device raises its own
//! [`DeviceLossSignal`], which the render supervisor waits on for the
//! device its pipeline draws with.
//!
//! The accounting has no wgpu types in it, so builds without the `wgpu`
//! feature still report (all zero) totals.
#![cfg_attr(
    not(feature = "wgpu"),
    allow(
        dead_code,
        reason = "only wgpu devices report errors; builds without wgpu still report zero totals"
    )
)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;

/// Errors logged per [`LOG_WINDOW`] before the rest are only counted.
const LOG_BURST: u32 = 10;

/// Window the [`LOG_BURST`] limit applies to.
const LOG_WINDOW: Duration = Duration::from_secs(1);

/// Which kind of uncaptured error wgpu raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GpuErrorKind {
    Validation,
    OutOfMemory,
    Internal,
}

impl GpuErrorKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::OutOfMemory => "out_of_memory",
            Self::Internal => "internal",
        }
    }
}

/// What a device does for the daemon, reported with its loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GpuDeviceRole {
    /// A compositor device a render pipeline draws on.
    Compositor,
    /// Any other logical device, such as the interactive preview executor's.
    Auxiliary,
}

/// The most recent uncaptured error or device loss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuErrorSample {
    /// Label of the device that reported it.
    pub device: String,
    /// `validation`, `out_of_memory`, `internal`, or `device_lost`.
    pub kind: String,
    /// wgpu's description of the error.
    pub message: String,
    /// Wall-clock time it was recorded, in Unix milliseconds.
    pub at_unix_ms: u64,
}

/// Counts of every uncaptured error and device loss since process start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GpuErrorTotals {
    pub validation: u64,
    pub out_of_memory: u64,
    pub internal: u64,
    pub device_lost: u64,
    pub last: Option<GpuErrorSample>,
}

impl GpuErrorTotals {
    /// Uncaptured errors of every kind, device loss excluded.
    #[must_use]
    pub const fn uncaptured(&self) -> u64 {
        self.validation
            .saturating_add(self.out_of_memory)
            .saturating_add(self.internal)
    }
}

#[derive(Debug, Default)]
struct LogWindow {
    started_at: Option<Instant>,
    logged: u32,
    suppressed: u64,
}

struct GpuErrorTelemetry {
    validation: AtomicU64,
    out_of_memory: AtomicU64,
    internal: AtomicU64,
    device_lost: AtomicU64,
    last: Mutex<Option<GpuErrorSample>>,
    log_window: Mutex<LogWindow>,
}

static TELEMETRY: GpuErrorTelemetry = GpuErrorTelemetry {
    validation: AtomicU64::new(0),
    out_of_memory: AtomicU64::new(0),
    internal: AtomicU64::new(0),
    device_lost: AtomicU64::new(0),
    last: Mutex::new(None),
    log_window: Mutex::new(LogWindow {
        started_at: None,
        logged: 0,
        suppressed: 0,
    }),
};

/// Sticky "this device was lost" flag for one wgpu device.
///
/// Clones share the flag. It is set once, by the device-lost callback, and
/// stays set: a lost device never comes back, so anything that checks it
/// later, such as a pipeline rebuilt on the same device, sees the loss too.
#[derive(Debug, Clone)]
pub(crate) struct DeviceLossSignal {
    sender: Arc<watch::Sender<Option<Arc<str>>>>,
}

impl Default for DeviceLossSignal {
    fn default() -> Self {
        Self {
            sender: Arc::new(watch::Sender::new(None)),
        }
    }
}

impl DeviceLossSignal {
    /// Mark the device lost, keeping the first description.
    pub(crate) fn raise(&self, description: String) {
        self.sender.send_if_modified(|lost| {
            if lost.is_some() {
                return false;
            }
            *lost = Some(Arc::from(description));
            true
        });
    }

    /// The loss description, once the device is lost.
    pub(crate) fn lost(&self) -> Option<Arc<str>> {
        self.sender.borrow().clone()
    }

    /// Wait until the device is lost, and return the description.
    pub(crate) async fn wait(&self) -> String {
        let mut receiver = self.sender.subscribe();
        let lost = receiver
            .wait_for(Option::is_some)
            .await
            .map(|lost| lost.as_deref().unwrap_or("device lost").to_owned());
        match lost {
            Ok(lost) => lost,
            // This signal holds its own sender, so the channel cannot close
            // while the wait borrows it.
            Err(_) => std::future::pending().await,
        }
    }
}

/// Record one uncaptured error from `device`.
pub(crate) fn record_uncaptured_error(device: &str, kind: GpuErrorKind, message: &str) {
    let counter = match kind {
        GpuErrorKind::Validation => &TELEMETRY.validation,
        GpuErrorKind::OutOfMemory => &TELEMETRY.out_of_memory,
        GpuErrorKind::Internal => &TELEMETRY.internal,
    };
    let total = counter.fetch_add(1, Ordering::Relaxed).saturating_add(1);
    store_last(device, kind.as_str(), message);

    if let Some(suppressed) = admit_log() {
        tracing::error!(
            device,
            kind = kind.as_str(),
            thread = std::thread::current().name().unwrap_or("<unnamed>"),
            total,
            suppressed_since_last_log = suppressed,
            error = message,
            "uncaptured wgpu error"
        );
    }
}

/// Count and log that `device` was lost.
pub(crate) fn record_device_lost(device: &str, role: GpuDeviceRole, reason: &str, message: &str) {
    let total = TELEMETRY
        .device_lost
        .fetch_add(1, Ordering::Relaxed)
        .saturating_add(1);
    let detail = if message.is_empty() {
        reason.to_owned()
    } else {
        format!("{reason}: {message}")
    };
    store_last(device, "device_lost", &detail);
    tracing::error!(
        device,
        ?role,
        reason,
        total,
        error = message,
        "wgpu device lost"
    );
}

/// Snapshot the totals since process start.
#[must_use]
pub fn gpu_error_totals() -> GpuErrorTotals {
    GpuErrorTotals {
        validation: TELEMETRY.validation.load(Ordering::Relaxed),
        out_of_memory: TELEMETRY.out_of_memory.load(Ordering::Relaxed),
        internal: TELEMETRY.internal.load(Ordering::Relaxed),
        device_lost: TELEMETRY.device_lost.load(Ordering::Relaxed),
        last: TELEMETRY
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone(),
    }
}

fn store_last(device: &str, kind: &str, message: &str) {
    let at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        });
    let sample = GpuErrorSample {
        device: device.to_owned(),
        kind: kind.to_owned(),
        message: message.to_owned(),
        at_unix_ms,
    };
    *TELEMETRY
        .last
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(sample);
}

/// Admit one log line under the burst limit, returning how many errors
/// went unlogged since the last admitted one.
fn admit_log() -> Option<u64> {
    let mut window = TELEMETRY
        .log_window
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let now = Instant::now();
    if window
        .started_at
        .is_none_or(|started| now.saturating_duration_since(started) >= LOG_WINDOW)
    {
        window.started_at = Some(now);
        window.logged = 0;
    }
    if window.logged >= LOG_BURST {
        window.suppressed = window.suppressed.saturating_add(1);
        return None;
    }
    window.logged += 1;
    Some(std::mem::take(&mut window.suppressed))
}

#[cfg(test)]
mod tests {
    use super::{
        DeviceLossSignal, GpuDeviceRole, GpuErrorKind, gpu_error_totals, record_device_lost,
        record_uncaptured_error,
    };

    #[test]
    fn uncaptured_errors_count_by_kind_and_keep_the_latest() {
        let before = gpu_error_totals();

        record_uncaptured_error("unit test device", GpuErrorKind::Validation, "bad usage");
        record_uncaptured_error(
            "unit test device",
            GpuErrorKind::OutOfMemory,
            "Out of Memory",
        );

        let after = gpu_error_totals();
        assert!(after.validation > before.validation);
        assert!(after.out_of_memory > before.out_of_memory);
        assert!(after.uncaptured() >= before.uncaptured() + 2);
        assert!(after.last.is_some(), "the latest error should be kept");
    }

    #[test]
    fn device_loss_is_counted() {
        let before = gpu_error_totals();

        record_device_lost(
            "preview device",
            GpuDeviceRole::Auxiliary,
            "unknown",
            "reset",
        );

        assert!(gpu_error_totals().device_lost > before.device_lost);
    }

    #[tokio::test]
    async fn a_raised_loss_wakes_a_waiter_and_stays_raised() {
        let signal = DeviceLossSignal::default();
        assert!(signal.lost().is_none());
        let waiter = tokio::spawn({
            let signal = signal.clone();
            async move { signal.wait().await }
        });

        signal.raise("compositor lost (unknown: reset)".to_owned());
        signal.raise("a later loss report".to_owned());

        assert_eq!(
            waiter.await.expect("waiter should finish"),
            "compositor lost (unknown: reset)"
        );
        assert_eq!(
            signal.lost().as_deref(),
            Some("compositor lost (unknown: reset)")
        );
        assert_eq!(signal.wait().await, "compositor lost (unknown: reset)");
    }
}
