//! Leptos-free partitioning of the Studio device pane.
//!
//! The pane is about the active tab: its zone's devices first (connected,
//! then an Offline subsection), then one row per other LED zone, then the
//! Unassigned section. A device whose outputs are split across zones is
//! listed under every zone holding some of them, and the active zone's
//! card carries a hint naming where the rest live. Search filters every
//! section at once and reports hit counts per section. Kept free of
//! `leptos` so the contract is exercised from
//! `tests/studio_pane_model_tests.rs`.

use std::collections::HashMap;

use hypercolor_types::scene::{UnassignedBehavior, ZoneId, ZoneRole};

use crate::api::ZoneResource;

use super::device_assignment::{
    DeviceMeta, ZoneDeviceRow, device_rows_for_zone, sort_device_rows, unassigned_device_rows,
};

/// Some of a device's outputs claimed by another zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElsewhereHint {
    pub zone_id: String,
    /// Outputs of the device that zone claims.
    pub outputs: usize,
}

/// One device row in a zone section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneDevice {
    pub row: ZoneDeviceRow,
    /// Other LED zones holding the rest of this device's outputs, in scene
    /// order. Filled for the active zone only.
    pub elsewhere: Vec<ElsewhereHint>,
}

/// One zone's slice of the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneSection {
    pub zone_id: String,
    /// Connected devices that pass the search, sorted.
    pub devices: Vec<PaneDevice>,
    /// Devices missing from the registry that pass the search, sorted.
    pub offline: Vec<PaneDevice>,
    /// Every device the zone holds, before search.
    pub total: usize,
}

impl ZoneSection {
    /// Devices that pass the search, connected and offline together.
    #[must_use]
    pub fn hits(&self) -> usize {
        self.devices.len() + self.offline.len()
    }
}

/// Hardware no zone claims.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnassignedSection {
    /// Rows that pass the search, connected devices first.
    pub rows: Vec<ZoneDeviceRow>,
    /// Every unassigned device, before search.
    pub total: usize,
}

/// The whole pane for one active zone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PaneModel {
    /// The active zone, or `None` when it is not an LED zone of the scene.
    pub active: Option<ZoneSection>,
    /// Every other LED zone, in scene order. While searching, only zones
    /// with hits remain.
    pub others: Vec<ZoneSection>,
    pub unassigned: UnassignedSection,
    /// Whether a search term is filtering the pane.
    pub searching: bool,
}

/// What the pane is built from.
pub struct PaneInputs<'a> {
    /// The scene's full zone list, Screens included.
    pub zones: &'a [ZoneResource],
    pub active_zone_id: &'a str,
    /// The device registry.
    pub devices: &'a [DeviceMeta],
    /// Saved controllers absent from both the registry and every zone.
    pub saved_only: Vec<ZoneDeviceRow>,
    /// The raw search box text.
    pub search: &'a str,
    /// Further searchable names per device id: channel and component
    /// names, which match alongside the device's own name.
    pub terms: &'a HashMap<String, Vec<String>>,
}

/// Normalize a search box value: trimmed and lowercased. Empty means no
/// search.
#[must_use]
pub fn normalize_search(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// Whether a row matches a normalized query by its device name, its id,
/// or any of its extra `terms`. An empty query matches everything.
#[must_use]
pub fn row_matches(row: &ZoneDeviceRow, query: &str, terms: &HashMap<String, Vec<String>>) -> bool {
    if query.is_empty() {
        return true;
    }
    row.name.to_lowercase().contains(query)
        || terms
            .get(&row.device_id)
            .is_some_and(|names| names.iter().any(|name| name.to_lowercase().contains(query)))
}

/// Build the pane for `inputs.active_zone_id`.
#[must_use]
pub fn build_pane(inputs: PaneInputs<'_>) -> PaneModel {
    let query = normalize_search(inputs.search);
    let searching = !query.is_empty();
    let led_zones: Vec<&ZoneResource> = inputs
        .zones
        .iter()
        .filter(|zone| zone.role != ZoneRole::Display)
        .collect();

    let active = led_zones
        .iter()
        .find(|zone| zone.id.to_string() == inputs.active_zone_id)
        .map(|zone| {
            zone_section(zone, inputs.devices, &query, inputs.terms, |row| {
                elsewhere_hints(&led_zones, zone.id, &row.device_id)
            })
        });

    let others = led_zones
        .iter()
        .filter(|zone| zone.id.to_string() != inputs.active_zone_id)
        .map(|zone| zone_section(zone, inputs.devices, &query, inputs.terms, |_| Vec::new()))
        .filter(|section| !searching || section.hits() > 0)
        .collect();

    let mut unassigned = unassigned_device_rows(inputs.zones, inputs.devices);
    unassigned.extend(inputs.saved_only);
    sort_device_rows(&mut unassigned);
    let total = unassigned.len();
    unassigned.retain(|row| row_matches(row, &query, inputs.terms));

    PaneModel {
        active,
        others,
        unassigned: UnassignedSection {
            rows: unassigned,
            total,
        },
        searching,
    }
}

fn zone_section(
    zone: &ZoneResource,
    devices: &[DeviceMeta],
    query: &str,
    terms: &HashMap<String, Vec<String>>,
    hints: impl Fn(&ZoneDeviceRow) -> Vec<ElsewhereHint>,
) -> ZoneSection {
    let mut rows = device_rows_for_zone(zone, devices);
    sort_device_rows(&mut rows);
    let total = rows.len();
    let (connected, offline): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .filter(|row| row_matches(row, query, terms))
        .map(|row| PaneDevice {
            elsewhere: hints(&row),
            row,
        })
        .partition(|device| device.row.resolved);
    ZoneSection {
        zone_id: zone.id.to_string(),
        devices: connected,
        offline,
        total,
    }
}

fn elsewhere_hints(
    led_zones: &[&ZoneResource],
    home: ZoneId,
    device_id: &str,
) -> Vec<ElsewhereHint> {
    led_zones
        .iter()
        .filter(|zone| zone.id != home)
        .filter_map(|zone| {
            let outputs = zone
                .members
                .iter()
                .filter(|member| member.device_id == device_id)
                .count();
            (outputs > 0).then(|| ElsewhereHint {
                zone_id: zone.id.to_string(),
                outputs,
            })
        })
        .collect()
}

/// "1 more output in Push 2" / "2 more outputs in Push 2".
#[must_use]
pub fn elsewhere_label(outputs: usize, zone_name: &str) -> String {
    if outputs == 1 {
        format!("1 more output in {zone_name}")
    } else {
        format!("{outputs} more outputs in {zone_name}")
    }
}

/// A section header's count: the total, or "3 of 19" while searching.
#[must_use]
pub fn section_count(hits: usize, total: usize, searching: bool) -> String {
    if searching {
        format!("{hits} of {total}")
    } else {
        total.to_string()
    }
}

/// Whether the Unassigned section shows at all. A multi-zone scene keeps
/// it as a standing place to unassign into; a single-zone scene shows it
/// only when something is actually unassigned.
#[must_use]
pub fn shows_unassigned(led_zone_count: usize, total: usize) -> bool {
    led_zone_count > 1 || total > 0
}

/// Encode an [`UnassignedBehavior`] as a select value: `off`, `hold`, or
/// `fallback:<zone id>`.
#[must_use]
pub fn unassigned_policy_value(behavior: &UnassignedBehavior) -> String {
    match behavior {
        UnassignedBehavior::Off => "off".to_owned(),
        UnassignedBehavior::Hold => "hold".to_owned(),
        UnassignedBehavior::Fallback(zone_id) => format!("fallback:{zone_id}"),
    }
}

/// Decode a select value back into an [`UnassignedBehavior`].
#[must_use]
pub fn parse_unassigned_policy(value: &str) -> Option<UnassignedBehavior> {
    match value {
        "off" => Some(UnassignedBehavior::Off),
        "hold" => Some(UnassignedBehavior::Hold),
        other => other
            .strip_prefix("fallback:")
            .and_then(|raw| raw.parse::<uuid::Uuid>().ok())
            .map(|uuid| UnassignedBehavior::Fallback(ZoneId(uuid))),
    }
}

/// The policy select's options: Off, Hold last colors, then one "Follow"
/// per LED zone, named by `zone_names` (`(zone id, display name)` pairs).
#[must_use]
pub fn unassigned_policy_options(zone_names: &[(String, String)]) -> Vec<(String, String)> {
    let mut options = vec![
        ("off".to_owned(), "Off".to_owned()),
        ("hold".to_owned(), "Hold last colors".to_owned()),
    ];
    options.extend(
        zone_names
            .iter()
            .map(|(id, name)| (format!("fallback:{id}"), format!("Follow {name}"))),
    );
    options
}
