//! The "+ Add device" affordance on each Studio zone.
//!
//! Picking a device brings every output it has into this zone. A device
//! placed in another zone is moved (its `Output`s reassigned); a
//! device the scene has not placed at all is minted (a fresh
//! `Output` per channel; the daemon resets placement on assign).

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use hypercolor_types::scene::{Zone, ZoneRole};

use crate::api;
use crate::api::zones::{OutputAssignment, ZoneOutcome};
use crate::app::DevicesContext;
use crate::components::silk_select::SilkSelect;
use crate::icons::*;
use crate::layout_utils;
use crate::toasts;

use super::StudioContext;

/// Canvas dimensions used when minting a `Output` for an unassigned
/// device. The daemon resets position and size on assign, so these only
/// seed the topology defaults `create_default_zone` derives from them.
const MINT_CANVAS_WIDTH: u32 = 640;
const MINT_CANVAS_HEIGHT: u32 = 480;

/// The "+ Add device" control. Collapsed to a button until clicked,
/// then a picker of every device not currently in `zone_id`.
#[component]
pub fn ZoneAddDevice(zone_id: String) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let devices = expect_context::<DevicesContext>();
    let picking = RwSignal::new(false);
    let zone_id = StoredValue::new(zone_id);

    let options = Memo::new(move |_| {
        let Some(scene) = studio.active_scene.get() else {
            return Vec::new();
        };
        let target = zone_id.get_value();
        let registry = devices
            .devices_resource
            .get()
            .and_then(Result::ok)
            .unwrap_or_default();
        registry
            .into_iter()
            .filter_map(|device| {
                let device_layout_id = device.layout_device_id.as_str();
                let outputs_outside_target = scene
                    .groups
                    .iter()
                    .filter(|group| group.id.to_string() != target)
                    .flat_map(|group| group.layout.zones.iter())
                    .filter(|output| output.device_id == device_layout_id)
                    .count();
                let any_output = scene.groups.iter().any(|group| {
                    group
                        .layout
                        .zones
                        .iter()
                        .any(|output| output.device_id == device_layout_id)
                });
                // Already entirely in this zone; nothing to move.
                if any_output && outputs_outside_target == 0 {
                    return None;
                }
                let location = device_location(&scene.groups, device_layout_id, &target);
                Some((
                    device.layout_device_id.clone(),
                    format!("{} ({location})", device.name),
                ))
            })
            .collect()
    });

    let on_pick = Callback::new(move |layout_device_id: String| {
        picking.set(false);
        if layout_device_id.is_empty() {
            return;
        }
        let registry = devices
            .devices_resource
            .get_untracked()
            .and_then(Result::ok)
            .unwrap_or_default();
        let Some(device) = registry
            .into_iter()
            .find(|candidate| candidate.layout_device_id == layout_device_id)
        else {
            toasts::toast_error("Device is no longer in the registry");
            return;
        };
        assign_device_to_zone(studio, device, zone_id.get_value());
    });

    view! {
        {move || {
            if picking.get() {
                let opts = options.get();
                if opts.is_empty() {
                    view! {
                        <div class="rounded-lg border border-dashed border-edge-subtle/55 px-3 py-2 text-center text-[10px] text-fg-tertiary/55">
                            "No devices to add"
                        </div>
                    }
                        .into_any()
                } else {
                    view! {
                        <div class="flex items-center gap-1">
                            <SilkSelect
                                value=Signal::derive(String::new)
                                options=options
                                on_change=on_pick
                                placeholder="Pick a device…".to_string()
                                class="flex-1 border border-accent-muted bg-surface-sunken/60 px-2.5 py-1.5 text-[12px]"
                            />
                            <button
                                type="button"
                                class="chip-interactive inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-fg-tertiary hover:text-fg-secondary"
                                title="Cancel"
                                on:click=move |_| picking.set(false)
                            >
                                <Icon icon=LuX width="11px" height="11px" />
                            </button>
                        </div>
                    }
                        .into_any()
                }
            } else {
                view! {
                    <button
                        type="button"
                        class="chip-interactive flex w-full items-center justify-center gap-1.5 rounded-lg border border-dashed border-edge-subtle/55 px-3 py-1.5 text-[11px] font-medium text-fg-tertiary hover:border-accent-muted hover:text-fg-secondary"
                        on:click=move |_| picking.set(true)
                    >
                        <Icon icon=LuPlus width="11px" height="11px" />
                        "Add device"
                    </button>
                }
                    .into_any()
            }
        }}
    }
}

/// Bring a device's outputs into `zone_id`. Existing outputs in another
/// zone are moved; a device the scene has not placed is minted fresh (the
/// daemon resets placement on assign). Shared by the per-zone picker and
/// the single-zone "available device" add button on the device card.
pub(super) fn assign_device_to_zone(
    studio: StudioContext,
    device: api::DeviceSummary,
    zone_id: String,
) {
    let Some(scene) = studio.active_scene.get_untracked() else {
        toasts::toast_error("No active scene is available");
        return;
    };
    let mut assignments: Vec<OutputAssignment> = Vec::new();
    for group in &scene.groups {
        if group.id.to_string() == zone_id {
            continue;
        }
        for output in &group.layout.zones {
            if output.device_id == device.layout_device_id {
                assignments.push(OutputAssignment::Existing {
                    id: output.id.clone(),
                });
            }
        }
    }
    if assignments.is_empty() {
        assignments = mint_device_zones(&device);
    }
    if assignments.is_empty() {
        // No existing outputs and no channels to mint from; nothing the
        // daemon can place.
        toasts::toast_error("Device has no channels to add");
        return;
    }
    let scene_id = scene.id.clone();
    let revision = scene.groups_revision;
    let device_name = device.name.clone();
    spawn_local(async move {
        match api::zones::assign_devices(&scene_id, &zone_id, assignments, Some(revision)).await {
            Ok(ZoneOutcome::Applied(_)) => {
                toasts::toast_success(&format!("{device_name} added to the zone"));
                studio.refresh_scene.run(());
            }
            Ok(ZoneOutcome::Stale { .. }) => {
                toasts::toast_error("Scene changed elsewhere — reloaded, try again");
                studio.refresh_scene.run(());
            }
            Err(error) => toasts::toast_error(&format!("Add failed: {error}")),
        }
    });
}

/// Build a fresh `Output` per channel for a device that no scene
/// has placed: one zone per declared `ZoneSummary`, or a single zone
/// for a device with no channels. The daemon resets position and size
/// on assign, so these defaults only seed topology and shape.
fn mint_device_zones(device: &api::DeviceSummary) -> Vec<OutputAssignment> {
    let layout_id = device.layout_device_id.as_str();
    let physical_id = device.id.as_str();
    let name = device.name.as_str();
    let total_leds = device.total_leds as usize;
    if device.zones.is_empty() {
        return vec![OutputAssignment::New(Box::new(
            layout_utils::create_default_zone(
                layout_id,
                physical_id,
                name,
                None,
                total_leds,
                MINT_CANVAS_WIDTH,
                MINT_CANVAS_HEIGHT,
                0,
            ),
        ))];
    }
    device
        .zones
        .iter()
        .enumerate()
        .map(|(order, channel)| {
            OutputAssignment::New(Box::new(layout_utils::create_default_zone(
                layout_id,
                physical_id,
                name,
                Some(channel),
                total_leds,
                MINT_CANVAS_WIDTH,
                MINT_CANVAS_HEIGHT,
                i32::try_from(order).unwrap_or(i32::MAX),
            )))
        })
        .collect()
}

/// The non-target zone that currently owns a device's outputs, or
/// "unassigned" if no zone holds any. Drives the location hint in the
/// picker label so the user sees where the move comes from.
fn device_location(groups: &[Zone], device_id: &str, target: &str) -> String {
    for group in groups {
        if group.role == ZoneRole::Display {
            continue;
        }
        if group.id.to_string() == target {
            continue;
        }
        if group
            .layout
            .zones
            .iter()
            .any(|output| output.device_id == device_id)
        {
            return format!("in {}", zone_display_name(group));
        }
    }
    "unassigned".to_owned()
}

/// Display name for a zone. Copies the rule used in `zone_assignment`
/// so the Default zone reads as "Default zone" rather than its raw role
/// string.
fn zone_display_name(group: &Zone) -> String {
    let trimmed = group.name.trim();
    if group.role == ZoneRole::Primary
        && (trimmed.is_empty() || trimmed.eq_ignore_ascii_case("primary"))
    {
        "Default zone".to_owned()
    } else {
        group.name.clone()
    }
}
