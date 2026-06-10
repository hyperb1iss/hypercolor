//! The device card for the Studio zone tree.
//!
//! Each physical device under a zone renders as a card carrying its
//! brand identity: a duotone accent strip, the vendor mark, the LED
//! count, and a per-channel breakdown with live component data. The
//! card body selects the parent zone; trailing actions hide every
//! output, identify the hardware, and remove it from the zone.

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use hypercolor_types::spatial::SpatialLayout;

use crate::api::zones::ZoneOutcome;
use crate::api::{self, DeviceSummary, ZoneTopologySummary};
use crate::channel_names;
use crate::components::device_card::{
    brand_colors, brand_label, brand_vendor, classify_brand, classify_device, device_class_icon,
    driver_identifier_label, topology_shape_svg,
};
use crate::icons::*;
use crate::layout_utils;
use crate::toasts;
use crate::vendors::{VendorMark, VendorMarkSize};

use super::device_grouping::ZoneDeviceRow;
use super::zone_add_device::assign_device_to_zone;
use super::{StudioContext, hidden_outputs_storage_key};

/// How a device card behaves, which decides its trailing actions and
/// whether it carries an in-zone layout (hidden state, per-output toggles).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CardMode {
    /// The device is placed in this zone: hide-all, identify, remove.
    #[default]
    Placed,
    /// The device is connected but in no zone — the single-zone fold
    /// (§3.3). Identify, plus a one-tap add into the zone named by
    /// `select`.
    Available,
    /// The multi-zone Unassigned bucket: identify only. Assignment happens
    /// in the Stage layout view.
    Unassigned,
}

/// One row in the per-channel breakdown beneath the card body.
///
/// Captures the channel's identity — slot id, zone name, and display
/// name — so the component-binding match helper can surface live
/// attachment data against the channel's aliases, plus the output id
/// when the channel has an output in this zone so the row can be
/// hidden individually.
struct ComponentRow {
    /// User-facing channel label, identical to the row's display name.
    name: String,
    shape_svg: &'static str,
    led_count: usize,
    /// `Output.id` (`Output.id`) when this channel has an output
    /// in the current zone — `None` for the Unassigned bucket or a
    /// channel with no placed output, in which case the row offers no
    /// hide toggle.
    output_id: Option<String>,
    slot_id: String,
    zone_name: Option<String>,
    display_name: String,
}

/// One physical device under a zone. The card body selects the parent zone
/// (or the Unassigned entry) named by `select`; trailing actions identify
/// the hardware and, inside a real zone, remove it.
#[component]
pub fn StudioDeviceCard(
    row: ZoneDeviceRow,
    device: Option<DeviceSummary>,
    select: String,
    /// Placed by default; the single-zone fold passes `Available`, the
    /// multi-zone Unassigned bucket passes `Unassigned`.
    #[prop(optional)]
    mode: CardMode,
) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let row_device_id = row.device_id.clone();

    let Some(device) = device else {
        // Offline or removed: still placed in the layout, but the device
        // registry has no entry — a muted row, no brand identity. Its raw
        // backend id must never reach the user (§4), so it reads as a
        // plain vendor word with an offline tag. It can still be removed
        // from the zone; it cannot be identified.
        let vendor = friendly_offline_label(&row.device_id);
        let leds = led_label(row.led_count);
        let select_body = select.clone();
        return view! {
            <div
                class="flex w-full items-center rounded-lg border border-dashed border-edge-subtle/45 transition-colors duration-150"
                title="Offline — placed in the layout but not currently connected"
            >
                <button
                    type="button"
                    class="flex min-w-0 flex-1 items-center gap-2 px-2.5 py-2 text-left"
                    on:click=move |_| studio.selected_surface_id.set(Some(select_body.clone()))
                >
                    <Icon
                        icon=LuCpu
                        width="12px"
                        height="12px"
                        style="color: rgba(139, 133, 160, 0.5)"
                    />
                    <span class="min-w-0 flex-1 truncate text-[11px] text-fg-tertiary/65">
                        {vendor}
                    </span>
                    <span class="shrink-0 rounded bg-surface-sunken/70 px-1 py-[1px] text-[8px] font-medium uppercase tracking-wide text-fg-tertiary/50">
                        "Offline"
                    </span>
                    <span class="shrink-0 font-mono text-[9px] tabular-nums text-fg-tertiary/45">
                        {leds}
                    </span>
                </button>
                {card_actions(CardActionsArgs {
                    studio,
                    mode,
                    select,
                    device_id: row_device_id,
                    physical_id: None,
                    output_ids: Vec::new(),
                    scene_key: None,
                    add_device: None,
                })}
            </div>
        }
        .into_any();
    };

    let brand = classify_brand(&device);
    let (primary, secondary) = brand_colors(&brand);
    let vendor = brand_vendor(&brand);
    // A daemon-reported display topology hint is the authoritative
    // "this is a screen" signal — and it carries the real resolution.
    let resolution = display_resolution(&device);
    let class_icon = if resolution.is_some() {
        LuMonitor
    } else {
        device_class_icon(&classify_device(&device))
    };
    // The driver label is the card's identity only when there is no
    // vendor mark to carry it.
    let driver_label = vendor.is_none().then(|| {
        brand_label(&brand).unwrap_or_else(|| {
            let id = if device.origin.driver_id.trim().is_empty() {
                device.origin.backend_id.as_str()
            } else {
                device.origin.driver_id.as_str()
            };
            driver_identifier_label(id).unwrap_or_else(|| id.to_owned())
        })
    });
    let transport = transport_label(&device.connection.transport);
    let device_name = device.name.clone();
    // A screen has no addressable LED tally — its layout topology is a
    // pixel grid. Show the real resolution; LED devices show the count.
    let leds = match resolution {
        Some((width, height)) => format!("{width} \u{d7} {height}"),
        None => led_label(row.led_count),
    };

    // Snapshot the current zone's layout once — used both to resolve
    // each channel's output id and to gather the bulk-hide id set.
    // The Unassigned bucket has no layout (its devices are by
    // definition placed in no zone), so the snapshot is `None` there.
    let placed = mode == CardMode::Placed;
    let layout_snapshot: Option<SpatialLayout> = if placed {
        studio.active_scene.with_untracked(|scene| {
            scene.as_ref().and_then(|scene| {
                scene
                    .groups
                    .iter()
                    .find(|group| group.id.to_string() == select)
                    .map(|group| group.layout.clone())
            })
        })
    } else {
        None
    };
    let scene_key: Option<String> = placed
        .then(|| {
            studio.active_scene.with_untracked(|scene| {
                scene
                    .as_ref()
                    .map(|scene| hidden_outputs_storage_key(&scene.id, &select))
            })
        })
        .flatten();
    let device_output_ids: Vec<String> = layout_snapshot
        .as_ref()
        .map(|layout| {
            layout
                .zones
                .iter()
                .filter(|output| output.device_id == device.layout_device_id)
                .map(|output| output.id.clone())
                .collect()
        })
        .unwrap_or_default();

    let total_components = device.zones.len();
    let component_rows: Vec<ComponentRow> = device
        .zones
        .iter()
        .map(|channel| {
            let display_name =
                channel_names::effective_channel_name(&device.id, &channel.id, &channel.name);
            let output_id = layout_snapshot.as_ref().and_then(|layout| {
                layout_utils::representative_zone_id_for_device_slot(
                    layout,
                    &device.layout_device_id,
                    Some(channel.name.as_str()),
                )
            });
            ComponentRow {
                name: display_name.clone(),
                shape_svg: topology_shape_svg(&channel.topology),
                led_count: channel.led_count as usize,
                output_id,
                slot_id: channel.id.clone(),
                zone_name: Some(channel.name.clone()),
                display_name,
            }
        })
        .collect();
    let show_components = total_components > 1;

    // Live component bindings for this device — fetched once per card
    // mount, shared via the Studio context cache so each row reads it
    // reactively without re-fetching.
    let physical_id_for_fetch = device.id.clone();
    Effect::new(move |_| {
        fetch_attachments_if_needed(studio, &physical_id_for_fetch);
    });
    let physical_id_for_rows = device.id.clone();

    // A single smooth diagonal brand wash — the duotone left strip carries
    // the hardware identity. Low-alpha radial gradients banded and
    // pixelated against the dark surface, so the card stays linear.
    let card_style = format!(
        "border: 1px solid rgba({primary}, 0.20); \
         background: linear-gradient(135deg, rgba({primary}, 0.11), \
         rgba({secondary}, 0.045) 58%, transparent 90%); \
         box-shadow: 0 1px 2px rgba(0, 0, 0, 0.22)"
    );
    let strip_style =
        format!("background: linear-gradient(180deg, rgb({primary}), rgb({secondary}))");
    let icon_style = format!("color: rgba({primary}, 0.9)");
    let list_style = format!("border-color: rgba({primary}, 0.12)");
    let shape_style = format!("color: rgba({primary}, 0.7)");

    let select_body = select.clone();
    let physical_id = device.id.clone();
    // An Available card carries the device record so its add action can
    // mint outputs and assign them into the zone.
    let add_device = matches!(mode, CardMode::Available).then(|| device.clone());

    // The device's outputs in this zone drive the canvas highlight: clicking
    // selects them, hovering previews them. Empty for an unplaced (Available)
    // device, which owns no box on the spatial canvas.
    let click_outputs = device_output_ids.clone();
    let enter_outputs = device_output_ids.clone();
    let sel_check_ids = device_output_ids.clone();
    let is_card_selected = Signal::derive(move || {
        !sel_check_ids.is_empty()
            && studio
                .selected_output_ids
                .with(|sel| sel_check_ids.iter().all(|id| sel.contains(id)))
    });
    let card_outline_style = move || {
        if is_card_selected.get() {
            format!(
                "{card_style}; outline: 1.5px solid rgba(225, 53, 255, 0.55); outline-offset: -1px"
            )
        } else {
            card_style.clone()
        }
    };

    view! {
        <div
            class="group/card relative w-full overflow-hidden rounded-lg transition-[border-color,box-shadow,outline-color] duration-150"
            style=card_outline_style
        >
            // A flat low-alpha hover wash — clean and bandless, no scale,
            // no brightness pump. Replaces card-hover, which squished the
            // whole card on click and pumped a janky radial glow.
            <div class="pointer-events-none absolute inset-0 bg-white/0 transition-colors duration-150 group-hover/card:bg-white/[0.03]" />
            <div class="flex items-stretch">
                <button
                    type="button"
                    class="flex min-w-0 flex-1 items-stretch gap-2.5 px-2.5 py-2 text-left"
                    on:click=move |_| {
                        studio.selected_surface_id.set(Some(select_body.clone()));
                        studio.selected_output_ids.set(click_outputs.iter().cloned().collect());
                    }
                    on:mouseenter=move |_| {
                        studio.hovered_output_ids.set(enter_outputs.iter().cloned().collect());
                    }
                    on:mouseleave=move |_| studio.hovered_output_ids.set(HashSet::new())
                >
                    <div class="w-1 shrink-0 self-stretch rounded-full" style=strip_style />
                    <div class="min-w-0 flex-1 space-y-1">
                        <div class="flex items-center gap-1.5">
                            {match vendor {
                                Some(v) => {
                                    view! { <VendorMark vendor=v size=VendorMarkSize::Xs /> }
                                        .into_any()
                                }
                                None => {
                                    view! {
                                        <Icon
                                            icon=class_icon
                                            width="13px"
                                            height="13px"
                                            style=icon_style
                                        />
                                    }
                                        .into_any()
                                }
                            }}
                            <span class="min-w-0 flex-1 truncate text-[12px] font-medium text-fg-primary">
                                {device_name}
                            </span>
                        </div>
                        <div class="flex items-center gap-1.5 font-mono text-[10px] text-fg-tertiary/70">
                            <span class="tabular-nums">{leds}</span>
                            {transport
                                .map(|t| {
                                    view! {
                                        <>
                                            <span class="text-fg-tertiary/30">"\u{b7}"</span>
                                            <span>{t}</span>
                                        </>
                                    }
                                })}
                            {driver_label
                                .map(|label| {
                                    view! {
                                        <>
                                            <span class="text-fg-tertiary/30">"\u{b7}"</span>
                                            <span class="truncate uppercase tracking-wide">
                                                {label}
                                            </span>
                                        </>
                                    }
                                })}
                        </div>
                    </div>
                </button>
                {card_actions(CardActionsArgs {
                    studio,
                    mode,
                    select,
                    device_id: row_device_id,
                    physical_id: Some(physical_id),
                    output_ids: device_output_ids,
                    scene_key: scene_key.clone(),
                    add_device,
                })}
            </div>
            {show_components
                .then(move || {
                    let scene_key = scene_key.clone();
                    view! {
                        <div class="space-y-0.5 border-t px-1.5 py-1.5" style=list_style>
                            {component_rows
                                .into_iter()
                                .map(|row| {
                                    component_row_view(
                                        studio,
                                        scene_key.clone(),
                                        physical_id_for_rows.clone(),
                                        row,
                                        shape_style.clone(),
                                    )
                                })
                                .collect_view()}
                        </div>
                    }
                })}
        </div>
    }
    .into_any()
}

struct CardActionsArgs {
    studio: StudioContext,
    mode: CardMode,
    select: String,
    device_id: String,
    physical_id: Option<String>,
    output_ids: Vec<String>,
    scene_key: Option<String>,
    add_device: Option<DeviceSummary>,
}

/// The trailing-edge action cluster. Hide-all toggles every output of a
/// placed device in unison; identify flashes the hardware whenever it is
/// online (`physical_id` is `Some`). The final action depends on `mode`:
/// a placed device offers remove, an available device offers a one-tap
/// add into the zone, and an Unassigned-bucket row offers neither.
fn card_actions(args: CardActionsArgs) -> impl IntoView {
    let CardActionsArgs {
        studio,
        mode,
        select,
        device_id,
        physical_id,
        output_ids,
        scene_key,
        add_device,
    } = args;
    let (identifying, set_identifying) = signal(false);
    // Hide-all is only meaningful when the card sits in a real zone
    // (so it has a scene_key) and the device actually owns outputs
    // there (otherwise there is nothing to toggle).
    let hide_all_pair: Option<(String, Vec<String>)> = scene_key
        .filter(|_| !output_ids.is_empty())
        .map(|key| (key, output_ids));
    view! {
        <div class="flex shrink-0 items-center gap-0.5 self-center pr-1.5">
            {hide_all_pair
                .map(|(key, ids)| {
                    let probe_key = key.clone();
                    let probe_ids = ids.clone();
                    let all_hidden = Signal::derive(move || {
                        studio.hidden_outputs.with(|map| {
                            map.get(&probe_key).is_some_and(|hidden| {
                                probe_ids.iter().all(|id| hidden.contains(id))
                            })
                        })
                    });
                    let toggle_key = key;
                    let toggle_ids = ids;
                    view! {
                        <button
                            type="button"
                            class="btn-press flex h-6 w-6 items-center justify-center rounded-md text-fg-tertiary/55 transition-colors hover:text-fg-secondary"
                            title=move || {
                                if all_hidden.get() {
                                    "Show every output for this device"
                                } else {
                                    "Hide every output for this device"
                                }
                            }
                            on:click=move |ev: web_sys::MouseEvent| {
                                ev.stop_propagation();
                                let currently_all = all_hidden.get();
                                let key = toggle_key.clone();
                                let ids = toggle_ids.clone();
                                studio
                                    .hidden_outputs
                                    .update(|map| {
                                        let entry = map.entry(key).or_default();
                                        for id in ids {
                                            if currently_all {
                                                entry.remove(&id);
                                            } else {
                                                entry.insert(id);
                                            }
                                        }
                                    });
                            }
                        >
                            {move || {
                                if all_hidden.get() {
                                    view! { <Icon icon=LuEyeOff width="12px" height="12px" /> }
                                        .into_any()
                                } else {
                                    view! { <Icon icon=LuEye width="12px" height="12px" /> }
                                        .into_any()
                                }
                            }}
                        </button>
                    }
                })}
            {physical_id
                .map(|id| {
                    let identify_id = id.clone();
                    view! {
                        <button
                            type="button"
                            class="btn-press flex h-6 w-6 items-center justify-center rounded-md text-fg-tertiary/55 transition-colors hover:text-fg-secondary disabled:cursor-wait disabled:opacity-60"
                            title="Identify (flash the hardware)"
                            disabled=move || identifying.get()
                            on:click=move |ev: web_sys::MouseEvent| {
                                ev.stop_propagation();
                                if identifying.get_untracked() {
                                    return;
                                }
                                identify_device_now(&identify_id, set_identifying);
                            }
                        >
                            <span class=move || if identifying.get() { "animate-pulse" } else { "" }>
                                <Icon icon=LuZap width="12px" height="12px" />
                            </span>
                        </button>
                    }
                })}
            {match mode {
                CardMode::Placed => {
                    let select = select.clone();
                    Some(
                        view! {
                            <button
                                type="button"
                                class="btn-press flex h-6 w-6 items-center justify-center rounded-md transition-colors"
                                style="color: rgba(255, 99, 99, 0.5)"
                                title="Remove from this zone"
                                on:click=move |ev: web_sys::MouseEvent| {
                                    ev.stop_propagation();
                                    remove_device_from_zone(studio, select.clone(), device_id.clone());
                                }
                            >
                                <Icon icon=LuTrash2 width="12px" height="12px" />
                            </button>
                        }
                        .into_any(),
                    )
                }
                CardMode::Available => {
                    add_device
                        .map(|device| {
                            let select = select.clone();
                            view! {
                                <button
                                    type="button"
                                    class="btn-press flex h-6 w-6 items-center justify-center rounded-md transition-colors"
                                    style="color: rgba(80, 250, 123, 0.78)"
                                    title="Add to this zone"
                                    on:click=move |ev: web_sys::MouseEvent| {
                                        ev.stop_propagation();
                                        assign_device_to_zone(studio, device.clone(), select.clone());
                                    }
                                >
                                    <Icon icon=LuPlus width="13px" height="13px" />
                                </button>
                            }
                            .into_any()
                        })
                }
                CardMode::Unassigned => None,
            }}
        </div>
    }
}

/// Render one per-channel row beneath the card body.
///
/// Each row shows the channel's topology shape and name, a live
/// component badge listing what is bound to this channel, the channel's
/// LED count, and a per-output hide toggle when the channel has an
/// output in the current zone. Hidden state is per-(scene, zone) client
/// UI state, not the daemon's `layout_auto_exclusions`.
fn component_row_view(
    studio: StudioContext,
    scene_key: Option<String>,
    physical_device_id: String,
    row: ComponentRow,
    shape_style: String,
) -> impl IntoView {
    let ComponentRow {
        name,
        shape_svg,
        led_count,
        output_id,
        slot_id,
        zone_name,
        display_name,
    } = row;

    // Bindings that target THIS channel, filtered through the alias
    // helper the palette card uses so slot id, zone name, and display
    // name are all accepted as matches.
    let binding_device = physical_device_id;
    let binding_slot_id = slot_id;
    let binding_zone_name = zone_name;
    let binding_display = display_name;
    let bindings = Signal::derive(move || {
        studio.attachment_cache.with(|cache| {
            cache
                .get(&binding_device)
                .map(|all| {
                    all.iter()
                        .filter(|binding| {
                            layout_utils::attachment_binding_matches_slot_alias(
                                &binding.slot_id,
                                Some(&binding_slot_id),
                                binding_zone_name.as_deref(),
                                &binding_display,
                            )
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });

    // The eye toggle only renders when both the scene_key and the
    // output id are known — i.e. the row sits in a real zone and the
    // channel has a placed output to hide.
    let hide_pair: Option<(String, String)> = match (&scene_key, &output_id) {
        (Some(key), Some(id)) => Some((key.clone(), id.clone())),
        _ => None,
    };
    let row_probe_pair = hide_pair.clone();
    let is_hidden = Signal::derive(move || {
        let Some((key, id)) = row_probe_pair.as_ref() else {
            return false;
        };
        studio
            .hidden_outputs
            .with(|map| map.get(key).is_some_and(|hidden| hidden.contains(id)))
    });

    // A channel with a placed output is interactive: click highlights it on
    // the canvas, hover previews it. A channel with no output in this zone
    // (Unassigned bucket, unplaced segment) just reads as a label.
    let interactive = output_id.is_some();
    let click_output = output_id.clone();
    let enter_output = output_id.clone();
    let row_selected_id = output_id.clone();
    let is_row_selected = Signal::derive(move || {
        row_selected_id
            .as_ref()
            .is_some_and(|id| studio.selected_output_ids.with(|sel| sel.contains(id)))
    });

    view! {
        <div
            class="flex items-center gap-2 rounded px-1 py-1 transition-colors"
            class=("cursor-pointer", move || interactive)
            class=("bg-accent/12", move || is_row_selected.get())
            class=("hover:bg-surface-hover/30", move || interactive)
            on:click=move |_| {
                if let Some(id) = click_output.clone() {
                    studio.selected_output_ids.set(HashSet::from([id]));
                }
            }
            on:mouseenter=move |_| {
                if let Some(id) = enter_output.clone() {
                    studio.hovered_output_ids.set(HashSet::from([id]));
                }
            }
            on:mouseleave=move |_| studio.hovered_output_ids.set(HashSet::new())
        >
            <div
                class="h-3 w-3 shrink-0"
                style=shape_style
                inner_html=format!(
                    r#"<svg viewBox="0 0 16 16" width="12" height="12">{shape_svg}</svg>"#,
                )
            />
            <div class="min-w-0 flex-1">
                <div class="truncate text-[10px] text-fg-tertiary">{name}</div>
                {move || {
                    let bindings = bindings.get();
                    if bindings.is_empty() {
                        return None;
                    }
                    let total: u32 = bindings.iter().map(|b| b.instances.max(1)).sum();
                    let label = if total == 1 {
                        bindings[0]
                            .name
                            .clone()
                            .unwrap_or_else(|| bindings[0].template_name.clone())
                    } else {
                        format!("{total} components")
                    };
                    let title = bindings
                        .iter()
                        .map(|b| {
                            let name = b
                                .name
                                .clone()
                                .unwrap_or_else(|| b.template_name.clone());
                            if b.instances > 1 {
                                format!(
                                    "{name} \u{d7}{} ({} LEDs)",
                                    b.instances, b.effective_led_count,
                                )
                            } else {
                                format!("{name} ({} LEDs)", b.effective_led_count)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    Some(
                        view! {
                            <span
                                class="mt-0.5 inline-flex max-w-[140px] items-center gap-1 truncate rounded px-1 py-0.5 font-mono text-[8px]"
                                style="color: rgb(128, 255, 234); background: rgba(128, 255, 234, 0.08); border: 1px solid rgba(128, 255, 234, 0.12)"
                                title=title
                            >
                                <Icon icon=LuCable width="8px" height="8px" />
                                <span class="truncate">{label}</span>
                            </span>
                        },
                    )
                }}
            </div>
            <span class="shrink-0 font-mono text-[9px] tabular-nums text-fg-tertiary/55">
                {led_count}
            </span>
            {hide_pair
                .map(|(key, id)| {
                    view! {
                        <button
                            type="button"
                            class="btn-press flex h-5 w-5 shrink-0 items-center justify-center rounded text-fg-tertiary/50 transition-colors hover:text-fg-secondary"
                            title=move || {
                                if is_hidden.get() { "Show output" } else { "Hide output" }
                            }
                            on:click=move |ev: web_sys::MouseEvent| {
                                ev.stop_propagation();
                                let key = key.clone();
                                let id = id.clone();
                                studio
                                    .hidden_outputs
                                    .update(|map| {
                                        let entry = map.entry(key).or_default();
                                        if !entry.remove(&id) {
                                            entry.insert(id);
                                        }
                                    });
                            }
                        >
                            {move || {
                                if is_hidden.get() {
                                    view! { <Icon icon=LuEyeOff width="10px" height="10px" /> }
                                        .into_any()
                                } else {
                                    view! { <Icon icon=LuEye width="10px" height="10px" /> }
                                        .into_any()
                                }
                            }}
                        </button>
                    }
                })}
        </div>
    }
}

/// Lazily populate the Studio-wide attachment cache for one device.
///
/// Cards trigger this once on mount. The check on the cache lets the
/// same device's card unmount and remount (zone collapsed and reopened)
/// without re-fetching, and lets every card across the tree share one
/// cache entry per physical device.
fn fetch_attachments_if_needed(studio: StudioContext, physical_device_id: &str) {
    if physical_device_id.is_empty() {
        return;
    }
    if studio
        .attachment_cache
        .with_untracked(|cache| cache.contains_key(physical_device_id))
    {
        return;
    }
    let id = physical_device_id.to_owned();
    spawn_local(async move {
        if let Ok(profile) = api::fetch_device_attachments(&id).await {
            studio.attachment_cache.update(|cache| {
                cache.insert(id, profile.bindings);
            });
        }
    });
}

/// Flash a device's LEDs so the user can locate it physically.
fn identify_device_now(device_id: &str, set_identifying: WriteSignal<bool>) {
    let device_id = device_id.to_owned();
    set_identifying.set(true);
    spawn_local(async move {
        match api::identify_device(&device_id).await {
            Ok(()) => toasts::toast_success("Flashing device"),
            Err(error) => toasts::toast_error(&format!("Identify failed: {error}")),
        }
        set_identifying.set(false);
    });
}

/// Unassign every output this device has in `zone_id`. The removals run in
/// sequence because each one bumps `groups_revision`; threading the new
/// revision into the next call lets a multi-output controller leave the
/// zone in a single user action.
fn remove_device_from_zone(studio: StudioContext, zone_id: String, device_id: String) {
    let Some(scene) = studio.active_scene.get_untracked() else {
        toasts::toast_error("No active scene is available");
        return;
    };
    let output_ids: Vec<String> = scene
        .groups
        .iter()
        .find(|group| group.id.to_string() == zone_id)
        .map(|group| {
            group
                .layout
                .zones
                .iter()
                .filter(|output| output.device_id == device_id)
                .map(|output| output.id.clone())
                .collect()
        })
        .unwrap_or_default();
    if output_ids.is_empty() {
        return;
    }
    let scene_id = scene.id.clone();
    let mut revision = scene.groups_revision;
    spawn_local(async move {
        for output_id in output_ids {
            match api::zones::unassign_device(&scene_id, &zone_id, &output_id, Some(revision)).await
            {
                Ok(ZoneOutcome::Applied(next)) => revision = next,
                Ok(ZoneOutcome::Stale { .. }) => {
                    toasts::toast_error("Scene changed elsewhere — reloaded, try again");
                    studio.refresh_scene.run(());
                    return;
                }
                Err(error) => {
                    toasts::toast_error(&format!("Remove failed: {error}"));
                    studio.refresh_scene.run(());
                    return;
                }
            }
        }
        toasts::toast_success("Device removed from zone");
        studio.refresh_scene.run(());
    });
}

/// Group a number's digits in threes: `230400` → `"230,400"`.
fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && (bytes.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(char::from(*byte));
    }
    out
}

/// A device's display resolution in pixels, from the first zone the
/// daemon tags with a `Display` topology hint — `None` for an ordinary
/// LED device, whose zones are strips, rings, and matrices.
fn display_resolution(device: &DeviceSummary) -> Option<(u32, u32)> {
    device
        .zones
        .iter()
        .find_map(|zone| match zone.topology_hint {
            Some(ZoneTopologySummary::Display { width, height, .. }) => Some((width, height)),
            _ => None,
        })
}

/// Vendor stand-in word for a device that is placed in the layout but
/// absent from the registry. The raw backend id (`razer:1532:…`) is never
/// shown (§4); this maps its leading backend token to a vendor name, or
/// the neutral "Device" when the token is unrecognized.
fn friendly_offline_label(device_id: &str) -> &'static str {
    let token = device_id.split(':').next().unwrap_or("");
    match token.to_ascii_lowercase().as_str() {
        "razer" => "Razer",
        "corsair" => "Corsair",
        "asus" | "aura" => "ASUS",
        "nzxt" => "NZXT",
        "lianli" | "ene" => "Lian Li",
        "wled" => "WLED",
        "hue" | "philips" => "Philips Hue",
        "govee" => "Govee",
        "nanoleaf" => "Nanoleaf",
        "dygma" => "Dygma",
        "nollie" => "Nollie",
        "prismrgb" | "prism" => "PrismRGB",
        "qmk" => "QMK",
        "ableton" | "push" => "Ableton",
        _ => "Device",
    }
}

/// "1 LED" / "1,406 LEDs".
fn led_label(count: u32) -> String {
    if count == 1 {
        "1 LED".to_owned()
    } else {
        format!("{} LEDs", group_digits(u64::from(count)))
    }
}

/// Short transport name for the card's meta line, or `None` to omit it.
fn transport_label(transport: &str) -> Option<&'static str> {
    match transport.trim() {
        "network" => Some("Network"),
        "usb" => Some("USB"),
        "smbus" => Some("SMBus"),
        "bridge" => Some("Bridge"),
        "midi" => Some("MIDI"),
        "serial" => Some("Serial"),
        _ => None,
    }
}
