//! The "+ Add device" affordance on each Studio zone.
//!
//! Picking a device brings its canonical light outputs into this zone.
//! The daemon resolves current bindings and hardware topology, and returns
//! the complete reversible assignment for Studio history.

use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_types::scene::ZoneRole;

use crate::api;
use crate::app::DevicesContext;
use crate::components::silk_select::SilkSelect;
use crate::icons::*;
use crate::toasts;

use super::StudioContext;

/// The "+ Add device" control. Collapsed to a button until clicked,
/// then a picker of every device not currently in `zone_id`.
#[component]
pub fn ZoneAddDevice(zone_id: String) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let devices = expect_context::<DevicesContext>();
    let picking = super::keyed_disclosure(studio.rail_disclosure, format!("add-picker::{zone_id}"));
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
                    .filter(|zone| zone.id.to_string() != target)
                    .flat_map(|zone| zone.members.iter())
                    .filter(|member| member.device_id == device_layout_id)
                    .count();
                let any_output = scene.zones.iter().any(|zone| {
                    zone.members
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
                                disabled=Signal::derive(move || studio.history.busy.get())
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
                        disabled=move || studio.history.busy.get()
                        class=("opacity-40", move || studio.history.busy.get())
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

/// Assign the device through the daemon's canonical hardware-output factory.
/// The response records one reversible operation for the complete device.
pub(super) fn assign_device_to_zone(
    studio: StudioContext,
    device: api::DeviceSummary,
    zone_id: String,
) {
    let (width, height) = studio.render_canvas_size.get_untracked();
    let light_segments = device
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
        .collect::<Vec<_>>();
    // Seeded footprints describe editor geometry only. The daemon keeps
    // ownership of output identity, hardware topology, and binding metadata.
    let placements = crate::layout_geometry::seeded_device_layout(
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
            .map(|output| hypercolor_types::api::scene::MemberPlacementHint {
                segment: output.zone_name,
                position: output.position,
                size: output.size,
                rotation: output.rotation,
                scale: output.scale,
                orientation: output.orientation,
            })
            .collect()
    })
    .unwrap_or_default();
    studio
        .history
        .assign_device(zone_id, device.layout_device_id, Vec::new(), placements);
}

/// The non-target zone that currently owns a device's outputs, or
/// "unassigned" if no zone holds any. Drives the location hint in the
/// picker label so the user sees where the move comes from.
fn device_location(zones: &[api::ZoneResource], device_id: &str, target: &str) -> String {
    for zone in zones {
        if zone.role == ZoneRole::Display {
            continue;
        }
        if zone.id.to_string() == target {
            continue;
        }
        if zone
            .members
            .iter()
            .any(|member| member.device_id == device_id)
        {
            return format!("in {}", zone_display_name(zone));
        }
    }
    "unassigned".to_owned()
}

/// Display name for a zone: the user's typed name, or "Default zone" for an
/// unnamed `Primary` zone, so it never surfaces a raw role string. Shared
/// with the device card's move-to-zone picker.
pub(super) fn zone_display_name(zone: &api::ZoneResource) -> String {
    let trimmed = zone.name.trim();
    if zone.role == ZoneRole::Primary
        && (trimmed.is_empty() || trimmed.eq_ignore_ascii_case("primary"))
    {
        "Default zone".to_owned()
    } else {
        zone.name.clone()
    }
}
