//! Moving outputs between zones: the one path every drop and every "Move
//! to" choice takes.
//!
//! A move edits membership through [`StudioHistory`], so Ctrl+Z takes it
//! back, then announces where the outputs went and hands focus back to
//! whatever opened the move.

use hypercolor_types::api::scene::{MemberEdit, MemberPlacementHint, MemberState};
use hypercolor_types::scene::ZoneRole;
use hypercolor_types::spatial::{NormalizedPosition, Output};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::zones::ZoneOutcome;
use crate::api::{self, DeviceSummary};
use crate::toasts;

use super::StudioContext;
use super::device_card::{UNASSIGNED_KEY, device_card_dom_id};
use super::device_pane::pane_zone_toggle_dom_id;
use super::drag::{DragController, FocusReturn};
use super::drag_model::{
    DropAction, Footprint, MovePayload, Origin, moved_announcement, offset_to_center, origin_name,
    placed_announcement, zone_name_for_device,
};
use super::history::EditDone;
use super::surface::{SurfaceKind, surfaces_from_zones};

/// Run `action` on `payload`. `focus` names where focus goes once the move
/// lands; a drag passes `None`.
pub(crate) fn perform(
    drag: DragController,
    payload: MovePayload,
    action: DropAction,
    focus: Option<FocusReturn>,
) {
    let studio = drag.studio;
    if studio.history.busy.get_untracked() {
        toasts::toast_info("Still saving the last change");
        return;
    }
    let focus = focus.map(|mut focus| {
        if focus.fallback.is_none() {
            focus.fallback = landing_dom_id(studio, &payload, &action);
        }
        focus
    });
    match action {
        DropAction::MoveTo(zone_id) => {
            let done = announce_move(drag, &payload, &zone_id, focus);
            move_into(drag, &payload, &zone_id, None, done);
        }
        DropAction::Place { zone_id, point } => {
            let done = if payload.is_in(&zone_id) {
                let text = placed_announcement(&payload.label, &zone_name(studio, &zone_id));
                announce_after(drag, text, focus)
            } else {
                announce_move(drag, &payload, &zone_id, focus)
            };
            move_into(drag, &payload, &zone_id, Some(point), done);
        }
        DropAction::Unassign => {
            let from = origin_name(&payload.origin, |id| zone_name(studio, id));
            let text = moved_announcement(&payload.label, "Unassigned", &from);
            unassign(studio, &payload, announce_after(drag, text, focus));
        }
        DropAction::NewZone => move_to_new_zone(drag, payload),
    }
}

/// Where the payload shows in the pane once `action` lands: its card in
/// the open zone or in Unassigned, or the Other zones row that took it.
fn landing_dom_id(
    studio: StudioContext,
    payload: &MovePayload,
    action: &DropAction,
) -> Option<String> {
    let open = studio.selected_surface_id.get_untracked();
    match action {
        DropAction::MoveTo(zone_id) | DropAction::Place { zone_id, .. } => {
            Some(if open.as_deref() == Some(zone_id.as_str()) {
                device_card_dom_id(zone_id, &payload.device_id)
            } else {
                pane_zone_toggle_dom_id(zone_id)
            })
        }
        DropAction::Unassign => Some(device_card_dom_id(UNASSIGNED_KEY, &payload.device_id)),
        DropAction::NewZone => None,
    }
}

/// A zone's shown name, read at call time.
pub(crate) fn zone_name(studio: StudioContext, zone_id: &str) -> String {
    studio.active_scene.with_untracked(|scene| {
        scene
            .as_ref()
            .and_then(|scene| {
                surfaces_from_zones(&scene.zones)
                    .into_iter()
                    .find(|surface| surface.id == zone_id)
                    .map(|surface| surface.name)
            })
            .unwrap_or_else(|| "this zone".to_owned())
    })
}

/// Every LED zone of `scene` as `(zone id, shown name, identity color)`,
/// in tab order.
pub(crate) fn led_zones(scene: &api::SceneDocument) -> Vec<(String, String, Option<String>)> {
    surfaces_from_zones(&scene.zones)
        .into_iter()
        .filter(|surface| surface.kind == SurfaceKind::Light)
        .map(|surface| (surface.id, surface.name, surface.color))
        .collect()
}

fn announce_move(
    drag: DragController,
    payload: &MovePayload,
    zone_id: &str,
    focus: Option<FocusReturn>,
) -> EditDone {
    let studio = drag.studio;
    let from = origin_name(&payload.origin, |id| zone_name(studio, id));
    let text = moved_announcement(&payload.label, &zone_name(studio, zone_id), &from);
    announce_after(drag, text, focus)
}

/// Announce `text` once the write applies, then return focus. A write that
/// did not apply already raised its own toast; the announcement says so too.
fn announce_after(drag: DragController, text: String, focus: Option<FocusReturn>) -> EditDone {
    Box::new(move |applied| match applied {
        Some(revision) => {
            drag.announce(text);
            if let Some(focus) = focus {
                drag.focus_after(revision, focus);
            }
        }
        None => {
            drag.announce("The move did not apply".to_owned());
            if let Some(focus) = focus {
                drag.focus_after(0, focus);
            }
        }
    })
}

/// The outputs `payload` names as they stand in the live scene, each with
/// the zone and index that hold it.
fn held_outputs(studio: StudioContext, payload: &MovePayload) -> Vec<MemberState> {
    let Some(scene) = studio.history.current_scene() else {
        return Vec::new();
    };
    scene
        .zones
        .iter()
        .filter(|zone| zone.role != ZoneRole::Display)
        .flat_map(|zone| {
            api::zone_outputs(zone)
                .into_iter()
                .enumerate()
                .filter(|(_, output)| payload.outputs.contains(&output.id))
                .map(|(index, output)| MemberState {
                    zone_id: zone.id,
                    output,
                    index,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Move `payload` into `zone_id`. With a `point`, the outputs center on it;
/// without one, the zone's arrival rule places them.
fn move_into(
    drag: DragController,
    payload: &MovePayload,
    zone_id: &str,
    point: Option<NormalizedPosition>,
    done: EditDone,
) {
    let studio = drag.studio;
    let history = studio.history;
    let Some(scene) = history.current_scene() else {
        return;
    };
    let Some(target) = scene
        .zones
        .iter()
        .find(|zone| zone.id.to_string() == zone_id && zone.role != ZoneRole::Display)
    else {
        return;
    };

    if payload.origin == Origin::Unassigned {
        let device = registry_device(drag, &payload.device_id);
        let mut arriving = Vec::new();
        let hints = arrival_placement(studio, zone_id, device.as_ref(), &mut arriving);
        let segments = payload.segments.clone();
        match point {
            Some(point) => history.assign_device_at(
                zone_id.to_owned(),
                payload.device_id.clone(),
                segments,
                hints,
                point,
                Some(done),
            ),
            None => history.assign_device(
                zone_id.to_owned(),
                payload.device_id.clone(),
                segments,
                hints,
                Some(done),
            ),
        }
        return;
    }

    let held = held_outputs(studio, payload);
    let mut arriving: Vec<Output> = held.iter().map(|state| state.output.clone()).collect();
    match point {
        Some(point) => center_on(&mut arriving, point),
        None => {
            let device = registry_device(drag, &payload.device_id);
            arrival_placement(studio, zone_id, device.as_ref(), &mut arriving);
        }
    }
    let mut next_index = target.members.len();
    let changes: Vec<MemberEdit> = held
        .into_iter()
        .zip(arriving)
        .map(|(before, output)| {
            // An output the zone already holds keeps its slot.
            let index = if before.zone_id == target.id {
                before.index
            } else {
                next_index += 1;
                next_index - 1
            };
            MemberEdit {
                before: Some(before),
                after: Some(MemberState {
                    zone_id: target.id,
                    output,
                    index,
                }),
            }
        })
        .filter(|change| change.before != change.after)
        .collect();
    if changes.is_empty() {
        return;
    }
    history.edit_members(changes, Some(done));
}

/// Release `payload`'s outputs from their zones.
fn unassign(studio: StudioContext, payload: &MovePayload, done: EditDone) {
    let changes = held_outputs(studio, payload)
        .into_iter()
        .map(|before| MemberEdit {
            before: Some(before),
            after: None,
        })
        .collect();
    studio.history.edit_members(changes, Some(done));
}

/// Make a zone named after the device, move the payload in, then open its
/// tab with the name selected for typing over.
fn move_to_new_zone(drag: DragController, payload: MovePayload) {
    let studio = drag.studio;
    let Some(scene) = studio.history.current_scene() else {
        toasts::toast_error("No active scene is available");
        return;
    };
    let shown: Vec<String> = led_zones(&scene)
        .into_iter()
        .map(|(_, name, _)| name)
        .collect();
    let names: Vec<&str> = scene
        .zones
        .iter()
        .map(|zone| zone.name.as_str())
        .chain(shown.iter().map(String::as_str))
        .collect();
    let name = zone_name_for_device(&payload.device_name, &names);
    let revision = scene.revision;
    spawn_local(async move {
        let zone = match api::zones::create_zone(&name, None, revision).await {
            Ok(ZoneOutcome::Applied(zone)) => zone,
            Ok(ZoneOutcome::Stale { .. }) => {
                toasts::toast_error("Scene changed elsewhere. Reloaded, try again.");
                studio.refresh_scene.run(());
                return;
            }
            Err(error) => {
                toasts::toast_error(&format!("Zone create failed: {error}"));
                return;
            }
        };
        let zone_id = zone.id.to_string();
        match api::fetch_active_scene().await {
            Ok(live) => studio.history.adopt(live),
            Err(error) => {
                toasts::toast_error(&format!("Created {name}, but the move failed: {error}"));
                studio.refresh_scene.run(());
                return;
            }
        }
        let from = origin_name(&payload.origin, |id| zone_name(studio, id));
        let text = moved_announcement(&payload.label, &zone.name, &from);
        let opened = zone_id.clone();
        let zone_name = zone.name.clone();
        let done: EditDone = Box::new(move |applied| {
            drag.announce(if applied.is_some() {
                text
            } else {
                "The move did not apply".to_owned()
            });
            studio
                .zone_rename_draft
                .try_set(Some((opened.clone(), zone_name)));
            studio.selected_surface_id.try_set(Some(opened));
        });
        move_into(drag, &payload, &zone_id, None, done);
    });
}

/// Center a group of outputs on `point`, keeping their arrangement.
fn center_on(outputs: &mut [Output], point: NormalizedPosition) {
    let footprints: Vec<Footprint> = outputs
        .iter()
        .map(|output| Footprint {
            center: output.position,
            size: output.size,
        })
        .collect();
    let Some((dx, dy)) = offset_to_center(&footprints, point) else {
        return;
    };
    for output in outputs {
        output.position = NormalizedPosition::new(output.position.x + dx, output.position.y + dy);
    }
}

/// Where outputs arriving in `zone_id` without a drop point land.
///
/// Returns placement hints, keyed by segment, for outputs the daemon will
/// mint for `device`, and may adjust `arriving`, the outputs that already
/// exist and keep their geometry otherwise. Today only devices with a
/// seeded footprint get hints; placement from the rig's physical layout
/// belongs here.
fn arrival_placement(
    studio: StudioContext,
    _zone_id: &str,
    device: Option<&DeviceSummary>,
    _arriving: &mut Vec<Output>,
) -> Vec<MemberPlacementHint> {
    let Some(device) = device else {
        return Vec::new();
    };
    let (width, height) = studio.render_canvas_size.get_untracked();
    let light_segments: Vec<_> = device
        .segments
        .iter()
        .filter(|segment| {
            segment.led_count > 0
                && !matches!(
                    segment.topology_hint,
                    Some(api::SegmentTopologySummary::Display { .. })
                )
        })
        .cloned()
        .collect();
    // Seeded footprints describe editor geometry only. The daemon keeps
    // ownership of output identity, hardware topology, and binding metadata.
    crate::layout_geometry::seeded_device_layout(
        &device.layout_device_id,
        &device.name,
        &light_segments,
        width,
        height,
        0,
    )
    .map(|seed| {
        seed.zones
            .into_iter()
            .map(|output| MemberPlacementHint {
                segment: output.zone_name,
                position: output.position,
                size: output.size,
                rotation: output.rotation,
                scale: output.scale,
                orientation: output.orientation,
            })
            .collect()
    })
    .unwrap_or_default()
}

/// A device's registry record by its layout device id.
pub(crate) fn registry_device(
    drag: DragController,
    layout_device_id: &str,
) -> Option<DeviceSummary> {
    drag.devices.devices_resource.with_untracked(|devices| {
        devices
            .as_ref()
            .and_then(|devices| devices.as_ref().ok())
            .and_then(|devices| {
                devices
                    .iter()
                    .find(|device| device.layout_device_id == layout_device_id)
                    .cloned()
            })
    })
}

/// How many light segments a device would bring into a zone.
#[must_use]
pub(crate) fn light_segment_count(device: &DeviceSummary) -> usize {
    device
        .segments
        .iter()
        .filter(|segment| {
            segment.led_count > 0
                && !matches!(
                    segment.topology_hint,
                    Some(api::SegmentTopologySummary::Display { .. })
                )
        })
        .count()
}

/// The payload for a device's outputs in `zone_id`, read from the live
/// scene.
pub(crate) fn zone_device_payload(
    studio: StudioContext,
    zone_id: &str,
    device_id: &str,
    device_name: &str,
) -> MovePayload {
    let outputs = studio
        .history
        .current_scene()
        .and_then(|scene| {
            scene
                .zones
                .iter()
                .find(|zone| zone.id.to_string() == zone_id)
                .map(|zone| {
                    zone.members
                        .iter()
                        .filter(|member| member.device_id == device_id)
                        .map(|member| member.id.to_string())
                        .collect()
                })
        })
        .unwrap_or_default();
    MovePayload::zone_device(device_id, device_name, zone_id, outputs)
}

/// The payload for boxes dragged off `zone_id`'s canvas: the outputs
/// `ids` names in that zone.
pub(crate) fn canvas_payload(
    drag: DragController,
    zone_id: &str,
    ids: &[String],
) -> Option<MovePayload> {
    let scene = drag.studio.history.current_scene()?;
    let zone = scene
        .zones
        .iter()
        .find(|zone| zone.id.to_string() == zone_id)?;
    let members: Vec<_> = zone
        .members
        .iter()
        .filter(|member| ids.contains(&member.id.to_string()))
        .collect();
    let first = members.first()?;
    let device_name = registry_device(drag, &first.device_id)
        .map_or_else(|| first.name.clone(), |device| device.name);
    let one_device = members
        .iter()
        .all(|member| member.device_id == first.device_id);
    let label = match (members.len(), one_device) {
        (1, _) => first.name.clone(),
        (_, true) => device_name.clone(),
        (count, false) => format!("{count} outputs"),
    };
    Some(MovePayload::canvas(
        &first.device_id,
        &device_name,
        &label,
        zone_id,
        members.iter().map(|member| member.id.to_string()).collect(),
    ))
}
