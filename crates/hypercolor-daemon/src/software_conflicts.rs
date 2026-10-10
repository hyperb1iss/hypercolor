//! Keeps the record of competing RGB software current.
//!
//! The watch scans at startup, every [`RESCAN_INTERVAL`], after each
//! discovery pass, whenever a device reports an error, and when asked (a
//! device just failed to open). Each scan reads the host inventory on a
//! blocking thread, because procfs and WMI both block. A program that
//! starts or stops running is logged once, so the daemon log in a
//! diagnostics bundle says what was competing and when.

use std::time::Duration;

use hypercolor_core::bus::{EventFilter, HypercolorBus};
use hypercolor_core::device::{ConflictChanges, SoftwareConflictStore};
use hypercolor_types::api::system::SoftwareConflict;
use hypercolor_types::device::{DeviceInfo, DriverTransportKind};
use hypercolor_types::event::{EventCategory, HypercolorEvent};
use hypercolor_types::host_software::HostInventory;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tracing::{debug, info, warn};

/// How often the watch rescans with no other trigger. Users quit a vendor
/// suite and expect the warning to clear without restarting anything.
pub(crate) const RESCAN_INTERVAL: Duration = Duration::from_secs(30);

/// How long a diagnostics report waits on its own scan before it reports
/// the stored result instead.
pub(crate) const DIAGNOSE_SCAN_BUDGET: Duration = Duration::from_secs(5);

/// Take one host inventory, record it, and log what changed.
pub(crate) async fn scan_now(store: &SoftwareConflictStore) {
    let ticket = store.begin_scan();
    let inventory = match tokio::task::spawn_blocking(crate::session::host_inventory).await {
        Ok(inventory) => inventory,
        Err(error) => {
            warn!(%error, "software inventory scan did not finish");
            HostInventory::Failed
        }
    };
    log_changes(&store.finish_scan(ticket, &inventory));
}

/// Scan only when the latest successful scan is older than `max_age`, and
/// give that scan at most `budget`. Diagnostics use this so a report
/// reuses what the watch scanned moments ago, and a hung WMI query (its
/// connect alone may wait two minutes) can't stall the report. A scan that
/// runs over budget keeps going on its blocking thread; its result is
/// dropped and the stored one is reported.
pub(crate) async fn scan_if_stale(
    store: &SoftwareConflictStore,
    max_age: Duration,
    budget: Duration,
) {
    if store.age().is_none_or(|age| age > max_age)
        && tokio::time::timeout(budget, scan_now(store)).await.is_err()
    {
        debug!(
            ?budget,
            "software scan ran over budget; reporting the stored result"
        );
    }
}

fn log_changes(changes: &ConflictChanges) {
    if changes.failure_started {
        warn!("could not list running software; keeping the last known competing programs");
    }
    if changes.failure_ended {
        info!("listing running software works again");
    }
    for conflict in &changes.appeared {
        warn!(
            software = %conflict.name,
            matched = ?conflict.matched,
            remedy = %conflict.remedy,
            "competing RGB software is running"
        );
    }
    for conflict in &changes.cleared {
        info!(software = %conflict.name, "competing RGB software stopped");
    }
}

/// Run the scan loop until the event bus closes.
pub(crate) fn spawn_watch(store: SoftwareConflictStore, bus: &HypercolorBus) -> JoinHandle<()> {
    let mut events =
        bus.subscribe_filtered(EventFilter::new().categories(vec![EventCategory::Device]));
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(RESCAN_INTERVAL);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                () = store.scan_requested() => {}
                received = events.recv() => match received {
                    Ok(event) if rescan_after(&event.event) => {}
                    Ok(_) => continue,
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                },
            }
            scan_now(&store).await;
        }
    })
}

/// Device events after which the picture may have changed.
fn rescan_after(event: &HypercolorEvent) -> bool {
    matches!(
        event,
        HypercolorEvent::DeviceDiscoveryCompleted { .. } | HypercolorEvent::DeviceError { .. }
    )
}

/// Running programs that compete for `device`, from the latest scan.
pub(crate) fn conflicts_for_device(
    store: &SoftwareConflictStore,
    device: &DeviceInfo,
) -> Vec<SoftwareConflict> {
    store.affecting(
        &device.origin.driver_id,
        device.origin.transport == DriverTransportKind::Smbus,
    )
}

/// One sentence naming the programs that may hold a device, for logs and
/// error messages. `None` when nothing relevant is running.
pub(crate) fn device_hint(conflicts: &[SoftwareConflict]) -> Option<String> {
    let names: Vec<&str> = conflicts
        .iter()
        .map(|conflict| conflict.name.as_str())
        .collect();
    let (verb, list) = match names.as_slice() {
        [] => return None,
        [one] => ("is", (*one).to_owned()),
        [first, second] => ("are", format!("{first} and {second}")),
        [rest @ .., last] => ("are", format!("{}, and {last}", rest.join(", "))),
    };
    Some(format!(
        "{list} {verb} running and may be holding this device"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> SoftwareConflict {
        SoftwareConflict {
            id: name.to_ascii_lowercase(),
            name: name.to_owned(),
            matched: vec![format!("{name}.exe")],
            driver_ids: Vec::new(),
            all_drivers: true,
            smbus: false,
            remedy: String::new(),
        }
    }

    #[test]
    fn hints_read_as_sentences() {
        assert_eq!(device_hint(&[]), None);
        assert_eq!(
            device_hint(&[named("SignalRGB")]).as_deref(),
            Some("SignalRGB is running and may be holding this device")
        );
        assert_eq!(
            device_hint(&[named("SignalRGB"), named("iCUE")]).as_deref(),
            Some("SignalRGB and iCUE are running and may be holding this device")
        );
        assert_eq!(
            device_hint(&[named("A"), named("B"), named("C")]).as_deref(),
            Some("A, B, and C are running and may be holding this device")
        );
    }

    #[test]
    fn only_discovery_and_device_errors_trigger_a_rescan() {
        assert!(rescan_after(&HypercolorEvent::DeviceError {
            device_id: "d".to_owned(),
            error: "e".to_owned(),
            recoverable: true,
        }));
        assert!(rescan_after(&HypercolorEvent::DeviceDiscoveryCompleted {
            found: Vec::new(),
            duration_ms: 1,
        }));
        assert!(
            !rescan_after(&HypercolorEvent::SoftwareConflictsChanged { count: 1 }),
            "a scan's own event must not trigger another scan"
        );
    }
}
