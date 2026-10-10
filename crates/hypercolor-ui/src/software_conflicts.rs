//! Competing RGB software: which warnings the UI owes the user.
//!
//! Pure and DOM-free so the rules are unit-testable. The daemon decides what
//! is running, and [`SoftwareConflict::affects`] decides which devices a
//! program competes for. This module layers the UI's choices on top: which
//! devices carry a hint, which warnings the user has dismissed, and when a
//! dismissal expires.
//!
//! A dismissal lasts while the program keeps running. Once a conclusive scan
//! stops reporting it, the dismissal is cleared, so the warning comes back if
//! the program starts again. Some people run a vendor suite on purpose; this
//! lets them silence it without silencing the next surprise.

use std::collections::{BTreeSet, HashMap};

use hypercolor_types::api::system::{SoftwareConflict, SoftwareConflictsStatus};
use hypercolor_types::device::DriverTransportKind;

use crate::api::DeviceSummary;
use crate::label_utils::humanize_identifier_label;
use crate::vendors;

/// Event the daemon publishes whenever the set of running conflicts changes.
pub const SOFTWARE_CONFLICTS_EVENT: &str = "software_conflicts_changed";

/// `localStorage` key holding dismissed conflict ids as a JSON array.
pub const DISMISSED_STORAGE_KEY: &str = "hc-software-conflicts-dismissed";

/// Parse stored dismissals. A missing or unreadable value means none.
#[must_use]
pub fn parse_dismissed(raw: Option<&str>) -> BTreeSet<String> {
    raw.and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
        .map(|ids| ids.into_iter().collect())
        .unwrap_or_default()
}

/// Encode dismissals for storage. `None` means the key should be removed.
#[must_use]
pub fn encode_dismissed(dismissed: &BTreeSet<String>) -> Option<String> {
    if dismissed.is_empty() {
        return None;
    }
    serde_json::to_string(dismissed).ok()
}

/// Whether `status` proves what is *not* running. A host that cannot list
/// processes, or a daemon that has not finished its first scan, reports an
/// empty list that says nothing. A failed scan carries the last successful
/// result, which says nothing about what has stopped since.
#[must_use]
pub fn scan_is_conclusive(status: &SoftwareConflictsStatus) -> bool {
    status.supported && status.scanned && !status.scan_failed
}

/// Quiet note under the banner title when the list on screen is not fresh.
pub const STALE_SCAN_NOTE: &str =
    "The latest check failed, so this list is from the last check that worked.";

/// The note to show beside the conflicts in `status`, if any. A failed scan
/// keeps its last good list on screen with this note; when that list is
/// empty there is no banner at all, since an inventory hiccup alone is not
/// worth a warning.
#[must_use]
pub fn stale_note(status: &SoftwareConflictsStatus) -> Option<&'static str> {
    (status.scan_failed && !status.conflicts.is_empty()).then_some(STALE_SCAN_NOTE)
}

/// Dismissals that survive `status`: every id the scan still reports.
/// An inconclusive scan keeps them all.
#[must_use]
pub fn prune_dismissed(
    dismissed: &BTreeSet<String>,
    status: &SoftwareConflictsStatus,
) -> BTreeSet<String> {
    if !scan_is_conclusive(status) {
        return dismissed.clone();
    }
    dismissed
        .iter()
        .filter(|id| status.conflicts.iter().any(|conflict| &conflict.id == *id))
        .cloned()
        .collect()
}

/// What to tell the user after a check they asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckFeedback {
    /// The scan failed; the list on screen is from the last one that worked.
    Failed,
    /// These programs are still running and not dismissed.
    StillRunning(String),
}

impl CheckFeedback {
    /// Toast text for this outcome.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Failed => "Check failed. The list shows the last check that worked.".to_owned(),
            Self::StillRunning(names) => format!("Still running: {names}"),
        }
    }
}

/// Feedback for a check the user asked for, or `None` when it cleared
/// everything they have not dismissed.
#[must_use]
pub fn check_feedback(
    status: &SoftwareConflictsStatus,
    dismissed: &BTreeSet<String>,
) -> Option<CheckFeedback> {
    if status.scan_failed {
        return Some(CheckFeedback::Failed);
    }
    let names: Vec<String> = visible_conflicts(&status.conflicts, dismissed)
        .into_iter()
        .map(|conflict| conflict.name)
        .collect();
    (!names.is_empty()).then(|| CheckFeedback::StillRunning(names.join(", ")))
}

/// Running conflicts the user has not dismissed, in catalog order.
#[must_use]
pub fn visible_conflicts(
    conflicts: &[SoftwareConflict],
    dismissed: &BTreeSet<String>,
) -> Vec<SoftwareConflict> {
    conflicts
        .iter()
        .filter(|conflict| !dismissed.contains(&conflict.id))
        .cloned()
        .collect()
}

/// Whether another program could be holding `device` at all. Disabled
/// devices are ones the user switched off, and virtual devices
/// (simulators) live inside Hypercolor where nothing else can reach them.
#[must_use]
pub fn device_can_be_held(device: &DeviceSummary) -> bool {
    !device.status.eq_ignore_ascii_case("disabled")
        && device.origin.transport != DriverTransportKind::Virtual
}

/// Conflicts that compete for `device`, in catalog order.
#[must_use]
pub fn conflicts_for_device<'a>(
    device: &DeviceSummary,
    conflicts: &'a [SoftwareConflict],
) -> Vec<&'a SoftwareConflict> {
    if !device_can_be_held(device) {
        return Vec::new();
    }
    let smbus = device.origin.transport == DriverTransportKind::Smbus;
    conflicts
        .iter()
        .filter(|conflict| conflict.affects(&device.origin.driver_id, smbus))
        .collect()
}

/// One sentence naming the programs that may hold a device, matching the
/// daemon's wording in logs and device errors. `None` when nothing applies.
#[must_use]
pub fn device_hint(conflicts: &[&SoftwareConflict]) -> Option<String> {
    let names: Vec<&str> = conflicts
        .iter()
        .map(|conflict| conflict.name.as_str())
        .collect();
    let verb = match names.len() {
        0 => return None,
        1 => "is",
        _ => "are",
    };
    Some(format!(
        "{} {verb} running and may be holding this device",
        join_names(&names)
    ))
}

/// The hint for every device some conflict competes for, keyed by device id.
#[must_use]
pub fn device_hints(
    devices: &[DeviceSummary],
    conflicts: &[SoftwareConflict],
) -> HashMap<String, String> {
    devices
        .iter()
        .filter_map(|device| {
            device_hint(&conflicts_for_device(device, conflicts))
                .map(|hint| (device.id.clone(), hint))
        })
        .collect()
}

/// Banner headline for one program: "SignalRGB is running."
#[must_use]
pub fn banner_headline(conflict: &SoftwareConflict) -> String {
    format!("{} is running.", conflict.name)
}

/// What the program competes for, in one sentence: "It competes with
/// Hypercolor for your devices."
#[must_use]
pub fn banner_scope(conflict: &SoftwareConflict) -> String {
    const SMBUS_LIGHTING: &str = "motherboard, RAM, and GPU lighting";
    if conflict.all_drivers || (conflict.driver_ids.is_empty() && !conflict.smbus) {
        return "It competes with Hypercolor for your devices.".to_owned();
    }
    let brands: Vec<String> = conflict
        .driver_ids
        .iter()
        .map(|id| driver_brand(id))
        .collect();
    let brands: Vec<&str> = brands.iter().map(String::as_str).collect();
    let target = match (brands.is_empty(), conflict.smbus) {
        (true, _) => SMBUS_LIGHTING.to_owned(),
        (false, false) => format!("{} devices", join_names(&brands)),
        (false, true) => format!("{} devices and for {SMBUS_LIGHTING}", join_names(&brands)),
    };
    format!("It competes with Hypercolor for {target}.")
}

/// Warning for the Windows SMBus support card, naming every running program
/// that drives SMBus lighting. `None` when there is nothing to warn about.
#[must_use]
pub fn smbus_conflict_warning(conflicts: &[SoftwareConflict]) -> Option<String> {
    let names: Vec<&str> = conflicts
        .iter()
        .filter(|conflict| conflict.smbus)
        .map(|conflict| conflict.name.as_str())
        .collect();
    (!names.is_empty()).then(|| {
        format!(
            "Other RGB software is running: {}. Quit it first to avoid SMBus conflicts.",
            names.join(", ")
        )
    })
}

/// Display name for a driver id: the vendor registry's brand when it knows
/// the id, otherwise the humanized id.
fn driver_brand(driver_id: &str) -> String {
    vendors::lookup(driver_id).map_or_else(
        || humanize_identifier_label(driver_id),
        |vendor| vendor.display_name.to_owned(),
    )
}

/// "A", "A and B", or "A, B, and C".
fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}
