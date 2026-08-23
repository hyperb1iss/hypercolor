//! The "+ Add device" affordance on each Studio zone.
//!
//! Picking a device brings every output it has into this zone. A device
//! placed in another zone is moved (its `Output`s reassigned); a
//! device the scene has not placed at all is minted (a fresh
//! `Output` per channel). Minting prefers a seeded hardware footprint
//! when one exists, and asks the daemon to keep it; otherwise the daemon
//! grid-places the outputs.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use hypercolor_types::scene::ZoneRole;

use crate::api;
use crate::api::zones::{OutputAssignment, ZoneOutcome};
use crate::app::DevicesContext;
use crate::components::silk_select::SilkSelect;
use crate::icons::*;
use crate::layout_geometry;
use crate::layout_utils;
use crate::style_utils::uuid_v4_hex;
use crate::toasts;

use super::StudioContext;

/// Authoring canvas used to normalize a freshly seeded hardware footprint.
/// Live zone placements are normalized and carry no pixel dimensions.
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
                    .zones
                    .iter()
                    .filter(|group| group.id.to_string() != target)
                    .flat_map(|group| group.members.iter())
                    .filter(|member| member.device_id == device_layout_id)
                    .count();
                let any_output = scene.zones.iter().any(|group| {
                    group
                        .members
                        .iter()
                        .any(|member| member.device_id == device_layout_id)
                });
                // Already entirely in this zone; nothing to move.
                if any_output && outputs_outside_target == 0 {
                    return None;
                }
                let location = device_location(&scene.zones, device_layout_id, &target);
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
/// zone are moved and the daemon grid-places them; a device the scene has
/// not placed is minted fresh, keeping its seeded footprint when it has
/// one. Shared by the per-zone picker and the single-zone "available
/// device" add button on the device card.
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
    for group in &scene.zones {
        if group.id.to_string() == zone_id {
            continue;
        }
        for member in &group.members {
            if member.device_id == device.layout_device_id {
                assignments.push(OutputAssignment::Existing {
                    id: member.id.to_string(),
                });
            }
        }
    }
    let mut preserve_placement = false;
    if assignments.is_empty() {
        let minted = mint_device_zones(&device, (MINT_CANVAS_WIDTH, MINT_CANVAS_HEIGHT));
        assignments = minted.assignments;
        preserve_placement = minted.preserve_placement;
    }
    if assignments.is_empty() {
        // No existing outputs and no channels to mint from; nothing the
        // daemon can place.
        toasts::toast_error("Device has no channels to add");
        return;
    }
    let revision = scene.revision;
    let device_name = device.name.clone();
    spawn_local(async move {
        match api::zones::assign_devices(&zone_id, assignments, preserve_placement, revision).await
        {
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

/// Outputs minted for a device the scene has not placed, plus whether
/// their geometry is deliberate enough that the daemon should keep it.
struct MintedOutputs {
    assignments: Vec<OutputAssignment>,
    preserve_placement: bool,
}

/// Build a fresh `Output` per channel for a device that no scene
/// has placed: one zone per declared `SegmentSummary`, or a single zone
/// for a device with no channels.
///
/// A device with a seeded footprint (the Push 2's pads, display, and
/// touch strip sit at fixed offsets on the real hardware) mints that
/// arrangement and asks the daemon to preserve it. Everything else mints
/// topology and shape only, and the daemon grid-places it.
fn mint_device_zones(
    device: &api::DeviceSummary,
    (canvas_width, canvas_height): (u32, u32),
) -> MintedOutputs {
    let layout_id = device.layout_device_id.as_str();
    let physical_id = device.id.as_str();
    let name = device.name.as_str();
    let total_leds = device.total_leds as usize;

    if let Some(seed) = layout_geometry::seeded_device_layout(
        layout_id,
        name,
        &device.segments,
        canvas_width,
        canvas_height,
        0,
    ) {
        return MintedOutputs {
            assignments: seed
                .zones
                .into_iter()
                .map(|mut output| {
                    // seeded_device_layout derives ids by folding every
                    // non-alphanumeric character to '_', so device ids that
                    // differ only in punctuation collapse together — two Push
                    // 2s on usb paths `...-0-12` and `...-0-1-2` land on the
                    // same id. assign_output_to_zone treats an output id as a
                    // scene-global ownership key, so the second device would
                    // silently take over the first one's outputs.
                    output.id = format!("zone_{}", uuid_v4_hex());
                    OutputAssignment::New(Box::new(output))
                })
                .collect(),
            preserve_placement: true,
        };
    }

    if device.segments.is_empty() {
        return MintedOutputs {
            assignments: vec![OutputAssignment::New(Box::new(
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
            ))],
            preserve_placement: false,
        };
    }
    MintedOutputs {
        assignments: device
            .segments
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
            .collect(),
        preserve_placement: false,
    }
}

/// The non-target zone that currently owns a device's outputs, or
/// "unassigned" if no zone holds any. Drives the location hint in the
/// picker label so the user sees where the move comes from.
fn device_location(groups: &[api::ZoneResource], device_id: &str, target: &str) -> String {
    for group in groups {
        if group.role == ZoneRole::Display {
            continue;
        }
        if group.id.to_string() == target {
            continue;
        }
        if group
            .members
            .iter()
            .any(|member| member.device_id == device_id)
        {
            return format!("in {}", zone_display_name(group));
        }
    }
    "unassigned".to_owned()
}

/// Display name for a zone: the user's typed name, or "Default zone" for an
/// unnamed `Primary` group, so it never surfaces a raw role string. Shared
/// with the device card's move-to-zone picker.
pub(super) fn zone_display_name(group: &api::ZoneResource) -> String {
    let trimmed = group.name.trim();
    if group.role == ZoneRole::Primary
        && (trimmed.is_empty() || trimmed.eq_ignore_ascii_case("primary"))
    {
        "Default zone".to_owned()
    } else {
        group.name.clone()
    }
}
