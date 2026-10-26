//! What the live region says once an undo or redo lands, so it never goes
//! on describing a move that was just taken back.

use std::collections::BTreeSet;

use hypercolor_types::api::scene::{MemberEdit, MemberState, PatchZoneRequest};

use super::super::drag_model::channel_label;
use super::StudioEdit;

/// Where moved outputs end up when no zone holds them.
const UNASSIGNED: &str = "Unassigned";

/// A device's shown name and light segment count, by its layout device id.
pub type DeviceLookup<'a> = dyn Fn(&str) -> Option<(String, usize)> + 'a;

/// The announcement for replaying `edit` as an undo (`redo == false`) or a
/// redo. `zone_name` names a zone by id and `device` looks a device up by
/// its layout device id, each as the scene and registry have them now.
/// `None` for a step with nothing to say, such as an empty membership
/// entry.
#[must_use]
pub fn replay_announcement(
    edit: &StudioEdit,
    redo: bool,
    zone_name: &dyn Fn(&str) -> Option<String>,
    device: &DeviceLookup<'_>,
) -> Option<String> {
    let verb = if redo { "Redid" } else { "Undid" };
    let zone = |id: &str| zone_name(id).unwrap_or_else(|| "a zone".to_owned());
    let action = match edit {
        StudioEdit::Layout { zone_id, .. } => format!("layout change on {}", zone(zone_id)),
        StudioEdit::Members(changes) => members_action(changes, redo, &zone, device)?,
        StudioEdit::ZoneCreated { name, filled, .. } if filled.is_empty() => {
            format!("created {name}")
        }
        StudioEdit::ZoneCreated { filled, .. } => {
            let label = moved_label(filled, device);
            if redo {
                format!("moved {label} to a new zone")
            } else {
                let source = filled
                    .iter()
                    .find_map(|change| change.before.as_ref())
                    .map_or_else(
                        || UNASSIGNED.to_owned(),
                        |state| zone(&state.zone_id.to_string()),
                    );
                format!("moved {label} back to {source}")
            }
        }
        StudioEdit::ZonePatched {
            zone_id,
            before,
            after,
        } => patch_action(&zone(zone_id), before, after),
        StudioEdit::ZoneDeleted(record) => format!("deleted {}", record.name),
    };
    Some(format!("{verb}: {action}"))
}

/// A membership replay: where the outputs went, or where they were placed
/// when they stayed in their zone.
fn members_action(
    changes: &[MemberEdit],
    redo: bool,
    zone: &dyn Fn(&str) -> String,
    device: &DeviceLookup<'_>,
) -> Option<String> {
    let first = changes.first()?;
    let label = moved_label(changes, device);
    let zone_of = |state: Option<&MemberState>| {
        state.map_or_else(
            || UNASSIGNED.to_owned(),
            |state| zone(&state.zone_id.to_string()),
        )
    };
    let stayed = changes.iter().all(|change| {
        change.before.as_ref().map(|state| state.zone_id)
            == change.after.as_ref().map(|state| state.zone_id)
    });
    if stayed {
        return Some(format!(
            "placed {label} on {}",
            zone_of(first.after.as_ref())
        ));
    }
    Some(if redo {
        format!("moved {label} to {}", zone_of(first.after.as_ref()))
    } else {
        format!("moved {label} back to {}", zone_of(first.before.as_ref()))
    })
}

/// What a set of membership changes moved: a whole device by its name,
/// one channel by its own, part of a device by its channel count, and
/// several devices by their count.
fn moved_label(changes: &[MemberEdit], device: &DeviceLookup<'_>) -> String {
    let outputs: Vec<_> = changes
        .iter()
        .filter_map(|change| change.after.as_ref().or(change.before.as_ref()))
        .map(|state| &state.output)
        .collect();
    let devices: BTreeSet<&str> = outputs
        .iter()
        .map(|output| output.device_id.as_str())
        .collect();
    if devices.len() > 1 {
        return format!("{} devices", devices.len());
    }
    let Some(first) = outputs.first() else {
        return "nothing".to_owned();
    };
    let count = outputs.len();
    match device(&first.device_id) {
        Some((name, segments)) if count >= segments => name,
        Some((name, _)) if count == 1 => channel_label(&first.name, &name),
        Some((name, _)) => format!("{count} channels of {name}"),
        None if count == 1 => first.name.clone(),
        None => format!("{count} outputs"),
    }
}

/// A zone patch, named for the field it changed.
fn patch_action(zone: &str, before: &PatchZoneRequest, after: &PatchZoneRequest) -> String {
    if let (Some(from), Some(to)) = (&before.name, &after.name) {
        return format!("renamed {from} to {to}");
    }
    match after.enabled {
        Some(false) => return format!("paused {zone}"),
        Some(true) => return format!("resumed {zone}"),
        None => {}
    }
    if after.color.is_some() {
        format!("recolored {zone}")
    } else if after.brightness.is_some() {
        format!("changed the brightness of {zone}")
    } else {
        format!("moved {zone} in the tabs")
    }
}
