//! Leptos-free grouping of a zone's device outputs by physical device.
//!
//! A zone's `Zone.layout.zones` is a flat list of `Output`
//! outputs — a multi-segment controller contributes several. The Studio
//! zone tree shows each physical device once under its zone, so this
//! module collapses those outputs by `device_id` and resolves display
//! names against the device registry. It also derives the Unassigned
//! group: devices the scene places in no zone at all.
//!
//! Deliberately free of `leptos` and `crate::` paths so the contract is
//! exercisable from `tests/studio_device_grouping_tests.rs` via a
//! `#[path]` include, mirroring `surface.rs`.

use std::collections::HashSet;

use hypercolor_types::scene::Zone;
use hypercolor_types::spatial::Output;

/// Device-registry metadata the grouping needs. The caller builds this
/// from `DeviceSummary`, keeping this module free of `crate::` paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceMeta {
    /// `DeviceSummary.layout_device_id` — the key a `Output.device_id`
    /// is matched against.
    pub layout_device_id: String,
    /// Display name from the device registry.
    pub name: String,
    /// LED count across the whole device.
    pub total_leds: u32,
}

/// One physical device's presence within a single zone — the unit the
/// zone tree renders under a zone node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneDeviceRow {
    /// Backend device id (`"<backend>:<id>"`) shared by these outputs.
    pub device_id: String,
    /// Resolved display name, or the bare `device_id` when the device is
    /// not in the registry (offline or removed).
    pub name: String,
    /// LEDs across this device's outputs within this zone.
    pub led_count: u32,
    /// Output count — a multi-segment controller contributes more than one.
    pub output_count: usize,
    /// Whether `device_id` resolved to a known registry device.
    pub resolved: bool,
}

/// Group a zone's `Output` outputs by `device_id` — one row per
/// physical device, in first-seen output order. LED counts sum each
/// output's topology; names resolve against `devices`.
#[must_use]
pub fn device_rows_for_zone(outputs: &[Output], devices: &[DeviceMeta]) -> Vec<ZoneDeviceRow> {
    let mut rows: Vec<ZoneDeviceRow> = Vec::new();
    for output in outputs {
        let leds = output.topology.led_count();
        if let Some(row) = rows
            .iter_mut()
            .find(|row| row.device_id == output.device_id)
        {
            row.led_count = row.led_count.saturating_add(leds);
            row.output_count += 1;
        } else {
            rows.push(resolve_row(&output.device_id, leds, devices));
        }
    }
    rows
}

/// Order a zone's device rows for display: connected devices first, then
/// case-insensitively by name, with offline (unresolved) rows sunk to the
/// bottom. A stable sort so the rail reads in a predictable, repeatable
/// order rather than raw layout sequence. Manual reordering layers on top
/// of this at the call site.
pub fn sort_device_rows(rows: &mut [ZoneDeviceRow]) {
    rows.sort_by(|a, b| {
        b.resolved
            .cmp(&a.resolved)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.device_id.cmp(&b.device_id))
    });
}

/// Rows for devices the scene places in no zone — the Unassigned group.
/// Every registry device that has LEDs and whose id is the `device_id`
/// of no placed `Output` anywhere in the scene.
#[must_use]
pub fn unassigned_device_rows(groups: &[Zone], devices: &[DeviceMeta]) -> Vec<ZoneDeviceRow> {
    let placed: HashSet<&str> = groups
        .iter()
        .flat_map(|group| group.layout.zones.iter())
        .map(|zone| zone.device_id.as_str())
        .collect();
    devices
        .iter()
        .filter(|device| device.total_leds > 0)
        .filter(|device| !placed.contains(device.layout_device_id.as_str()))
        .map(|device| ZoneDeviceRow {
            device_id: device.layout_device_id.clone(),
            name: device.name.clone(),
            led_count: device.total_leds,
            output_count: 0,
            resolved: true,
        })
        .collect()
}

/// Build a fresh row for a device's first output, resolving its id
/// against the registry. `output_count` starts at one; callers fold in
/// any further outputs of the same device.
fn resolve_row(device_id: &str, led_count: u32, devices: &[DeviceMeta]) -> ZoneDeviceRow {
    let meta = devices
        .iter()
        .find(|device| device.layout_device_id == device_id);
    ZoneDeviceRow {
        device_id: device_id.to_owned(),
        name: meta.map_or_else(|| device_id.to_owned(), |device| device.name.clone()),
        led_count,
        output_count: 1,
        resolved: meta.is_some(),
    }
}
