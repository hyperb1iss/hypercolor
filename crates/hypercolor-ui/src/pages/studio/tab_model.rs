//! Leptos-free rules behind the Studio zone tab strip.
//!
//! Which tabs fit and which fold into the overflow menu, where a reordered
//! zone lands in the scene's full zone order, how the keyboard moves
//! between tabs, which zones may be deleted, and how identical Screen
//! names are told apart. Kept free of `leptos` so the contract is
//! exercised from `tests/studio_tab_model_tests.rs`.

use std::collections::HashMap;

use hypercolor_types::scene::ZoneRole;

use crate::api::{ApiError, ZoneResource};

/// The one optional status glyph a zone tab carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabStatus {
    /// Nothing to report.
    Clear,
    /// A layer failed to render, or the zone has no devices.
    Warning,
    /// The zone is disabled.
    Paused,
}

/// Pick a tab's status glyph. A paused zone says so before anything else,
/// because a disabled zone renders nothing whatever its layers report.
#[must_use]
pub fn tab_status(enabled: bool, degraded: bool, member_count: usize) -> TabStatus {
    if !enabled {
        TabStatus::Paused
    } else if degraded || member_count == 0 {
        TabStatus::Warning
    } else {
        TabStatus::Clear
    }
}

/// Why a zone cannot be deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteBlock {
    /// The primary zone is permanent.
    Primary,
    /// A scene keeps at least one LED zone.
    LastZone,
    /// The zone is not an LED zone of this scene.
    Missing,
}

impl DeleteBlock {
    /// Plain-words reason, shown on the disabled menu item.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Primary => "The main zone can't be deleted",
            Self::LastZone => "A scene keeps at least one zone",
            Self::Missing => "This zone is no longer in the scene",
        }
    }
}

/// Whether the LED zone `zone_id` may be deleted.
///
/// # Errors
/// Returns the [`DeleteBlock`] that forbids the delete.
pub fn deletable(zones: &[ZoneResource], zone_id: &str) -> Result<(), DeleteBlock> {
    let led_zones = zones
        .iter()
        .filter(|zone| zone.role != ZoneRole::Display)
        .count();
    let zone = zones
        .iter()
        .find(|zone| zone.role != ZoneRole::Display && zone.id.to_string() == zone_id)
        .ok_or(DeleteBlock::Missing)?;
    if zone.role == ZoneRole::Primary {
        Err(DeleteBlock::Primary)
    } else if led_zones <= 1 {
        Err(DeleteBlock::LastZone)
    } else {
        Ok(())
    }
}

/// Distinct physical devices a zone claims outputs from.
#[must_use]
pub fn zone_device_count(zone: &ZoneResource) -> usize {
    let mut seen: Vec<&str> = Vec::new();
    for member in &zone.members {
        if !seen.contains(&member.device_id.as_str()) {
            seen.push(member.device_id.as_str());
        }
    }
    seen.len()
}

/// The confirmation shown before deleting a zone that holds devices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletePrompt {
    /// The question: "Delete 'Keyboard'?"
    pub title: String,
    /// The consequence: "1 device becomes unassigned."
    pub detail: String,
}

/// The confirmation for deleting a zone, or `None` when the zone is empty
/// and deletes straight away (with undo).
#[must_use]
pub fn delete_prompt(zone_name: &str, device_count: usize) -> Option<DeletePrompt> {
    let detail = match device_count {
        0 => return None,
        1 => "1 device becomes unassigned.".to_owned(),
        count => format!("{count} devices become unassigned."),
    };
    Some(DeletePrompt {
        title: format!("Delete \u{2018}{zone_name}\u{2019}?"),
        detail,
    })
}

/// Where a reordered zone lands among the other LED zones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReorderTarget {
    /// Immediately before this LED zone.
    Before(String),
    /// After the last LED zone.
    AfterLast,
}

/// The target for nudging `moving` one tab left (`delta < 0`) or right
/// (`delta > 0`). `None` at either edge, where the move is a no-op.
#[must_use]
pub fn nudge_target(led_ids: &[String], moving: &str, delta: isize) -> Option<ReorderTarget> {
    let position = led_ids.iter().position(|id| id == moving)?;
    let next = position.checked_add_signed(delta)?;
    if next >= led_ids.len() || next == position {
        return None;
    }
    let others: Vec<&String> = led_ids.iter().filter(|id| *id != moving).collect();
    Some(others.get(next).map_or(ReorderTarget::AfterLast, |id| {
        ReorderTarget::Before((*id).clone())
    }))
}

/// The `PatchZoneRequest::index` that realizes `target` for `moving`.
///
/// `order` is the scene's FULL zone list as `(zone id, is an LED zone)`
/// pairs, Screens included. The daemon removes the zone and reinserts it
/// at the index, so the index is computed against the list without it.
/// `None` when either zone is unknown or the move changes nothing.
#[must_use]
pub fn full_zone_index(
    order: &[(&str, bool)],
    moving: &str,
    target: &ReorderTarget,
) -> Option<usize> {
    let current = order.iter().position(|(id, _)| *id == moving)?;
    let without: Vec<&(&str, bool)> = order.iter().filter(|(id, _)| *id != moving).collect();
    let index = match target {
        ReorderTarget::Before(anchor) => without.iter().position(|(id, _)| *id == anchor)?,
        ReorderTarget::AfterLast => without
            .iter()
            .rposition(|(_, is_led)| *is_led)
            .map_or(0, |last| last + 1),
    };
    (index != current).then_some(index)
}

/// [`full_zone_index`] over the scene's zone resources.
#[must_use]
pub fn zone_move_index(
    zones: &[ZoneResource],
    moving: &str,
    target: &ReorderTarget,
) -> Option<usize> {
    let ids: Vec<String> = zones.iter().map(|zone| zone.id.to_string()).collect();
    let order: Vec<(&str, bool)> = zones
        .iter()
        .zip(&ids)
        .map(|(zone, id)| (id.as_str(), zone.role != ZoneRole::Display))
        .collect();
    full_zone_index(&order, moving, target)
}

/// How many tab centers sit left of `pointer_x`: the insertion slot a drag
/// release points at, among the tabs other than the dragged one.
#[must_use]
pub fn drop_slot(pointer_x: f64, centers: &[f64]) -> usize {
    centers.iter().filter(|center| **center < pointer_x).count()
}

/// The reorder a tab drag describes. `visible` is the rendered tab order
/// (overflowed tabs absent), `slot` the insertion point among the visible
/// tabs other than `dragged`, and `led_ids` every LED zone in order.
/// A drop past the last visible tab lands right after it, ahead of any
/// tabs folded into the overflow menu.
#[must_use]
pub fn drag_target(
    visible: &[String],
    dragged: &str,
    slot: usize,
    led_ids: &[String],
) -> Option<ReorderTarget> {
    let others: Vec<&String> = visible.iter().filter(|id| *id != dragged).collect();
    if let Some(anchor) = others.get(slot) {
        return Some(ReorderTarget::Before((*anchor).clone()));
    }
    let Some(last) = others.last() else {
        return None;
    };
    let rest: Vec<&String> = led_ids.iter().filter(|id| *id != dragged).collect();
    let after_last = rest.iter().position(|id| id == last)? + 1;
    Some(rest.get(after_last).map_or(ReorderTarget::AfterLast, |id| {
        ReorderTarget::Before((*id).clone())
    }))
}

/// The tab a navigation key moves to over `len` tabs: arrows wrap, Home
/// and End jump. Any other key returns `None`.
#[must_use]
pub fn next_tab_index(current: Option<usize>, len: usize, key: &str) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let last = len - 1;
    match key {
        "ArrowRight" | "]" => Some(current.map_or(0, |i| if i >= last { 0 } else { i + 1 })),
        "ArrowLeft" | "[" => Some(current.map_or(last, |i| if i == 0 { last } else { i - 1 })),
        "Home" => Some(0),
        "End" => Some(last),
        _ => None,
    }
}

/// How many zone tabs fit in `available` pixels before the tail folds
/// into an overflow button `more_button` wide. Never below one, so the
/// active tab always shows.
#[must_use]
pub fn fit_tab_count(
    available: f64,
    count: usize,
    min_tab: f64,
    gap: f64,
    more_button: f64,
) -> usize {
    if count == 0 {
        return 0;
    }
    let step = min_tab + gap;
    let all = count as f64 * step - gap;
    if all <= available {
        return count;
    }
    // Each kept tab costs its width plus the gap after it, the last of
    // those gaps separating it from the overflow button.
    let room = (available - more_button).max(0.0);
    let fit = (room / step).floor() as usize;
    fit.clamp(1, count.saturating_sub(1).max(1))
}

/// Split `count` tabs into the rendered ones and the overflowed tail. The
/// active tab always renders: when it falls in the tail it takes the last
/// visible slot, and the tab it displaces joins the overflow menu.
#[must_use]
pub fn split_overflow(
    count: usize,
    active: Option<usize>,
    visible: usize,
) -> (Vec<usize>, Vec<usize>) {
    if visible >= count {
        return ((0..count).collect(), Vec::new());
    }
    let visible = visible.max(1);
    let mut shown: Vec<usize> = (0..visible).collect();
    if let Some(active) = active.filter(|active| *active >= visible && *active < count)
        && let Some(slot) = shown.last_mut()
    {
        *slot = active;
    }
    let hidden = (0..count).filter(|index| !shown.contains(index)).collect();
    (shown, hidden)
}

/// A fresh zone's name: "New zone", then "New zone 2" and up, skipping any
/// name already in the scene (compared case-insensitively).
#[must_use]
pub fn next_zone_name(existing: &[&str]) -> String {
    let taken = |candidate: &str| {
        existing
            .iter()
            .any(|name| name.trim().eq_ignore_ascii_case(candidate))
    };
    if !taken("New zone") {
        return "New zone".to_owned();
    }
    (2_usize..)
        .map(|n| format!("New zone {n}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| "New zone".to_owned())
}

/// One Screen as the strip's pane lists it, before disambiguation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenName {
    /// The display zone's stored name.
    pub name: String,
    /// The per-device label the canvas shows for this display's output
    /// ("TL LCD 2"), when the rig places one.
    pub device_label: Option<String>,
}

/// Labels for a scene's Screens, in order. A unique name stands. Repeated
/// names take their device label when every repeat has a distinct one,
/// and otherwise an ordinal ("Lian Li LCD 2") counted within the repeats.
#[must_use]
pub fn screen_labels(screens: &[ScreenName]) -> Vec<String> {
    let base = |screen: &ScreenName| screen.name.trim().to_owned();
    screens
        .iter()
        .enumerate()
        .map(|(index, screen)| {
            let name = base(screen);
            let repeats: Vec<&ScreenName> =
                screens.iter().filter(|other| base(other) == name).collect();
            if repeats.len() < 2 {
                return name;
            }
            let labels: Vec<&str> = repeats
                .iter()
                .filter_map(|other| other.device_label.as_deref().map(str::trim))
                .filter(|label| !label.is_empty())
                .collect();
            let distinct = labels.len() == repeats.len()
                && labels
                    .iter()
                    .enumerate()
                    .all(|(i, label)| !labels[..i].contains(label));
            if distinct && let Some(label) = screen.device_label.as_deref() {
                return label.trim().to_owned();
            }
            let ordinal = screens[..=index]
                .iter()
                .filter(|other| base(other) == name)
                .count();
            format!("{name} {ordinal}")
        })
        .collect()
}

/// [`screen_labels`] for a scene's display zones, keyed by zone id. A
/// Screen's device label is the name its display output carries in an
/// LED zone of the scene, found through `layout_ids` (physical display
/// id to the layout device id outputs use).
#[must_use]
pub fn screen_label_map(
    zones: &[ZoneResource],
    layout_ids: &HashMap<String, String>,
) -> HashMap<String, String> {
    let screens: Vec<&ZoneResource> = zones
        .iter()
        .filter(|zone| zone.role == ZoneRole::Display)
        .collect();
    let names: Vec<ScreenName> = screens
        .iter()
        .map(|zone| {
            let device_label = zone
                .display_target
                .as_ref()
                .and_then(|target| layout_ids.get(&target.device_id.to_string()))
                .and_then(|layout_id| {
                    zones
                        .iter()
                        .filter(|zone| zone.role != ZoneRole::Display)
                        .flat_map(|zone| zone.members.iter())
                        .find(|member| member.device_id == *layout_id)
                        .map(|member| member.name.clone())
                });
            ScreenName {
                name: zone.name.clone(),
                device_label,
            }
        })
        .collect();
    screens
        .iter()
        .map(|zone| zone.id.to_string())
        .zip(screen_labels(&names))
        .collect()
}

/// The toast for a zone write the daemon refused, in words: `what` is the
/// action ("Couldn't reorder zones"). A daemon that predates zone moves
/// rejects `index` as an unknown field, which gets its own explanation;
/// other request-shape rejections never surface serde's text.
#[must_use]
pub fn describe_zone_failure(what: &str, error: &ApiError) -> String {
    match error {
        ApiError::Http {
            message: Some(message),
            ..
        } if message.contains("unknown field `index`") => {
            "This Hypercolor daemon can't reorder zones yet. Update Hypercolor to reorder."
                .to_owned()
        }
        ApiError::Http {
            message: Some(message),
            ..
        } if message.starts_with("Failed to deserialize") => {
            format!("{what}: the daemon didn't accept the request")
        }
        ApiError::Http {
            message: Some(message),
            ..
        } => format!("{what}: {}", message.trim_end_matches('.')),
        ApiError::Http { status, .. } => format!("{what} (HTTP {status})"),
        ApiError::Network(_) => format!("{what}: the daemon could not be reached"),
        ApiError::Parse(_) | ApiError::Serialize(_) => {
            format!("{what}: the daemon sent an unexpected reply")
        }
    }
}

/// Identity swatches offered by a tab's Color menu. Zone colors are data
/// the scene stores as hex; they tint a tab's dot and never its chrome.
pub const ZONE_COLOR_PRESETS: [(&str, &str); 8] = [
    ("Purple", "#e135ff"),
    ("Cyan", "#80ffea"),
    ("Coral", "#ff6ac1"),
    ("Green", "#50fa7b"),
    ("Yellow", "#f1fa8c"),
    ("Blue", "#82aaff"),
    ("Orange", "#ff9e64"),
    ("Red", "#ff6363"),
];

/// The hover and title text for a tab's status glyph.
#[must_use]
pub const fn tab_status_note(status: TabStatus, degraded: bool) -> Option<&'static str> {
    match status {
        TabStatus::Clear => None,
        TabStatus::Paused => Some("Paused"),
        TabStatus::Warning if degraded => Some("A layer on this zone failed to render"),
        TabStatus::Warning => Some("No devices in this zone yet"),
    }
}
