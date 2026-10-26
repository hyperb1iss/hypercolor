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
use super::placement;
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

/// Move `payload` into `zone_id`. Outputs land per the zone's arrival
/// rule ([`placement`]): where they sit in the rig, carried into the zone
/// alongside its members, or fitted to an empty zone. With a `point`, the
/// arriving group then centers on it: the drop point decides where, the
/// arrival rule still decides how large.
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
        done(None);
        return;
    };
    let Some(target) = scene
        .zones
        .iter()
        .find(|zone| zone.id.to_string() == zone_id && zone.role != ZoneRole::Display)
    else {
        done(None);
        return;
    };

    let canvas = studio.render_canvas_size.get_untracked();
    if payload.origin == Origin::Unassigned {
        let segments = payload.segments.clone();
        // A channel brings only its own segment, so only its hint counts.
        let mut hints = registry_device(drag, &payload.device_id)
            .map(|device| {
                studio.rig_layout.with_untracked(|rig| {
                    placement::assignment_hints(
                        &scene,
                        rig.as_ref(),
                        &device,
                        zone_id,
                        &segments,
                        canvas,
                    )
                })
            })
            .unwrap_or_default();
        // Hints already carry the geometry, so a drop point only moves
        // them; without hints the arrivals are placed once minted.
        let point = point.filter(|point| !center_hints_on(&mut hints, *point));
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
    // Outputs from other zones land per the arrival rule; outputs the zone
    // already holds stay where they are until a point moves them.
    let landings = studio.rig_layout.with_untracked(|rig| {
        placement::move_edits(&scene, rig.as_ref(), zone_id, &payload.outputs, canvas)
    });
    let mut arriving: Vec<Output> = held
        .iter()
        .map(|state| {
            landings
                .iter()
                .filter_map(|edit| edit.after.as_ref())
                .find(|after| after.output.id == state.output.id)
                .map_or_else(|| state.output.clone(), |after| after.output.clone())
        })
        .collect();
    if let Some(point) = point {
        center_on(&mut arriving, point);
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
    // Wait out any write in flight, then hold the scene until the zone
    // exists and the move into it goes first, so no other edit slips in.
    drag.studio
        .history
        .run_when_free(move || create_zone_and_move(drag, payload));
}

fn create_zone_and_move(drag: DragController, payload: MovePayload) {
    let studio = drag.studio;
    let history = studio.history;
    let Some(scene) = history.current_scene() else {
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
    let from = origin_name(&payload.origin, |id| zone_name(studio, id));
    let revision = scene.revision;
    let Some(generation) = history.begin() else {
        toasts::toast_info("Still saving the last change");
        return;
    };
    spawn_local(async move {
        let created = api::zones::create_zone(&name, None, revision).await;
        if !history.is_current(generation) {
            return;
        }
        let zone = match created {
            Ok(ZoneOutcome::Applied(zone)) => zone,
            Ok(ZoneOutcome::Stale { .. }) => {
                toasts::toast_error("Scene changed elsewhere. Reloaded, try again.");
                studio.refresh_scene.run(());
                history.finish(generation);
                return;
            }
            Err(error) => {
                toasts::toast_error(&format!("Zone create failed: {error}"));
                history.finish(generation);
                return;
            }
        };
        let live = api::fetch_active_scene().await;
        if !history.is_current(generation) {
            return;
        }
        match live {
            Ok(live) => history.adopt(live),
            Err(error) => {
                toasts::toast_error(&format!("Created {name}, but the move failed: {error}"));
                studio.refresh_scene.run(());
                history.finish(generation);
                return;
            }
        }
        let zone_id = zone.id.to_string();
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
        history.hand_over(generation, move || {
            move_into(drag, &payload, &zone_id, None, done);
        });
    });
}

/// Center seeded placement hints on `point`, keeping their arrangement.
/// False when there are none to move.
fn center_hints_on(hints: &mut [MemberPlacementHint], point: NormalizedPosition) -> bool {
    let footprints: Vec<Footprint> = hints
        .iter()
        .map(|hint| Footprint {
            center: hint.position,
            size: hint.size,
        })
        .collect();
    let Some((dx, dy)) = offset_to_center(&footprints, point) else {
        return false;
    };
    for hint in hints {
        hint.position = NormalizedPosition::new(hint.position.x + dx, hint.position.y + dy);
    }
    true
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
