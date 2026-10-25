//! The device card for the Studio device pane.
//!
//! Each physical device renders as a card carrying its brand identity: a
//! duotone accent strip, the vendor mark, the LED count, and a
//! per-channel breakdown with live component data. In the active zone
//! the card body lights the device's outputs on the canvas, and two
//! trailing icons hide every output and open the operations menu. A
//! device split across zones lists only this zone's channels, plus a
//! quiet line naming where the rest live. An Unassigned card offers
//! identify and a one-tap add into the active zone instead.

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use hypercolor_types::scene::ZoneRole;
use hypercolor_types::spatial::Output;

use crate::api::{self, DeviceSummary, SegmentTopologySummary};
use crate::channel_names;
use crate::components::device_card::{
    TopologyShape, TopologyShapeKind, brand_colors, brand_label, brand_vendor, classify_brand,
    classify_device, device_class_icon, driver_identifier_label, topology_shape_kind,
};
use crate::icons::*;
use crate::layout_utils;
use crate::toasts;
use crate::vendors::{VendorMark, VendorMarkSize};

use super::device_assignment::ZoneDeviceRow;
use super::device_pane::reveal_pane_zone;
use super::pane_model::elsewhere_label;
use super::surface::surfaces_from_zones;
use super::{StudioContext, hidden_outputs_storage_key, zone_id_from_storage_key};

/// How a device card behaves, which decides its trailing actions and
/// whether it carries an in-zone layout (hidden state, per-output toggles).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CardMode {
    /// The device has outputs in the zone named by `select`: hide-all and
    /// the operations kebab (identify, move, remove).
    #[default]
    Placed,
    /// The device is in no zone: identify, plus a one-tap add into the
    /// zone named by `select`, the active tab.
    Unassigned,
}

/// Outputs of a split device that another zone claims, as the card's
/// "more outputs" line names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elsewhere {
    pub zone_id: String,
    pub zone_name: String,
    pub outputs: usize,
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
    shape: TopologyShapeKind,
    led_count: usize,
    /// `Output.id` (`Output.id`) when this channel has an output
    /// in the current zone — `None` for an Unassigned card or a
    /// channel with no placed output, in which case the row offers no
    /// hide toggle.
    output_id: Option<String>,
    slot_id: String,
    zone_name: Option<String>,
    display_name: String,
}

/// One physical device. `select` names the zone the card speaks for: the
/// zone holding its outputs, or, for an Unassigned card, the active tab
/// its "+" adds into.
#[component]
pub fn StudioDeviceCard(
    row: ZoneDeviceRow,
    device: Option<DeviceSummary>,
    select: String,
    /// Placed by default; the Unassigned section passes `Unassigned`.
    #[prop(optional)]
    mode: CardMode,
    /// Other zones holding the rest of a split device's outputs.
    #[prop(optional)]
    elsewhere: Vec<Elsewhere>,
) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let row_device_id = row.device_id.clone();

    let Some(device) = device else {
        return view! {
            <super::offline_device_card::OfflineDeviceCard row=row select=select placed=matches!(mode, CardMode::Placed) />
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

    // Project the current zone's placements once to resolve channel ids and
    // gather the bulk-hide id set, plus the outputs other zones hold so a
    // split device lists only this zone's channels. An Unassigned card has
    // no placements.
    let placed = mode == CardMode::Placed;
    let (zone_outputs, outputs_elsewhere): (Option<Vec<Output>>, Vec<Output>) = if placed {
        studio.active_scene.with_untracked(|scene| {
            let Some(scene) = scene.as_ref() else {
                return (None, Vec::new());
            };
            let here = scene
                .zones
                .iter()
                .find(|zone| zone.id.to_string() == select)
                .map(api::zone_outputs);
            let elsewhere = scene
                .zones
                .iter()
                .filter(|zone| zone.id.to_string() != select && zone.role != ZoneRole::Display)
                .flat_map(api::zone_outputs)
                .filter(|output| output.device_id == device.layout_device_id)
                .collect();
            (here, elsewhere)
        })
    } else {
        (None, Vec::new())
    };
    let scene_key: Option<String> = placed
        .then(|| {
            studio.active_scene.with_untracked(|scene| {
                scene
                    .as_ref()
                    .map(|scene| hidden_outputs_storage_key(&scene.id.to_string(), &select))
            })
        })
        .flatten();
    let device_output_ids: Vec<String> = zone_outputs
        .as_ref()
        .map(|outputs| {
            outputs
                .iter()
                .filter(|output| output.device_id == device.layout_device_id)
                .map(|output| output.id.clone())
                .collect()
        })
        .unwrap_or_default();

    let total_components = device.segments.len();
    let claimed_elsewhere = |channel_name: &str| {
        outputs_elsewhere.iter().any(|output| {
            layout_utils::channel_name_matches_slot_alias(
                output.zone_name.as_deref(),
                Some(channel_name),
            )
        })
    };
    let component_rows: Vec<ComponentRow> = device
        .segments
        .iter()
        .filter_map(|channel| {
            let display_name =
                channel_names::effective_channel_name(&device.id, &channel.id, &channel.name);
            let output_id = zone_outputs.as_ref().and_then(|outputs| {
                outputs
                    .iter()
                    .find(|output| {
                        output.device_id == device.layout_device_id
                            && layout_utils::channel_name_matches_slot_alias(
                                output.zone_name.as_deref(),
                                Some(channel.name.as_str()),
                            )
                    })
                    .map(|output| output.id.clone())
            });
            // A channel another zone holds belongs to that zone's card; one
            // no zone holds still shows, as it always has.
            if output_id.is_none() && claimed_elsewhere(&channel.name) {
                return None;
            }
            Some(ComponentRow {
                name: display_name.clone(),
                shape: topology_shape_kind(&channel.topology),
                led_count: channel.led_count as usize,
                output_id,
                slot_id: channel.id.clone(),
                zone_name: Some(channel.name.clone()),
                display_name,
            })
        })
        .collect();
    let show_components = total_components > 1;
    let channels_open = RwSignal::new(mode == CardMode::Placed);

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
    let hint_style = list_style.clone();
    let shape_style = format!("color: rgba({primary}, 0.7)");

    let select_body = select.clone();
    // A display device's home surface is its Screen entry: the card sits
    // under an LED zone but owns no output boxes on the spatial canvas,
    // so a zone-select from here is a dead click. Clicking it jumps to
    // the screen surface instead.
    let is_display = resolution.is_some();
    let click_device_id = device.id.clone();
    let physical_id = device.id.clone();
    // The Placed card's operations menu (identify / move / remove) expands
    // inline below the body and is toggled by the action cluster's kebab.
    // Capture what it needs before `select` and `device_output_ids` move
    // into `card_actions`.
    let ops_open = super::keyed_disclosure(
        studio.rail_disclosure,
        format!("device-menu::{select}::{row_device_id}"),
    );
    let menu_select = select.clone();
    let menu_physical_id = device.id.clone();
    let menu_output_ids = device_output_ids.clone();
    // An Unassigned card carries the device record so its add action can
    // mint outputs and assign them into the zone.
    let add_device = (mode == CardMode::Unassigned).then(|| device.clone());

    // The device's outputs in this zone drive the canvas highlight: clicking
    // selects them, hovering previews them. Empty for an Unassigned device,
    // which owns no box on the spatial canvas.
    let click_outputs = device_output_ids.clone();
    let enter_outputs = device_output_ids.clone();
    let sel_check_ids = device_output_ids.clone();
    release_hover_on_unmount(studio, device_output_ids.clone());
    let is_card_selected = Signal::derive(move || {
        !sel_check_ids.is_empty()
            && studio
                .selected_output_ids
                .with(|sel| sel_check_ids.iter().all(|id| sel.contains(id)))
    });
    // Canvas hover, mirrored back: the pointer resting on one of this
    // device's boxes softly lights the card, the counterpart of rail
    // hover lifting the box.
    let pointer_check_ids = device_output_ids.clone();
    let is_under_canvas_pointer = Memo::new(move |_| {
        studio.pointer_output_id.with(|under| {
            under
                .as_ref()
                .is_some_and(|id| pointer_check_ids.contains(id))
        })
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
            // whole card on click and pumped a janky radial glow. The same
            // wash answers the canvas: pointing at a box lights its card.
            <div
                class="pointer-events-none absolute inset-0 bg-white/0 transition-colors duration-150 group-hover/card:bg-white/[0.03]"
                class=("bg-white/[0.04]", move || is_under_canvas_pointer.get())
            />
            <div class="flex items-stretch">
                <button
                    type="button"
                    class="flex min-w-0 flex-1 items-stretch gap-2.5 px-2.5 py-2 text-left"
                    on:click=move |_| {
                        // A display with no boxes in this zone lives on its
                        // Screen surface; one with boxes is selected here like
                        // any other card, so the eye and channel rows beneath
                        // it keep acting on the zone that stays on the Stage.
                        if is_display
                            && click_outputs.is_empty()
                            && let Some(screen_id) =
                                screen_surface_id_for_device(studio, &click_device_id)
                        {
                            studio.selected_surface_id.set(Some(screen_id));
                            return;
                        }
                        if studio.selected_surface_id.get_untracked().as_deref() != Some(select_body.as_str()) {
                            studio.selected_surface_id.set(Some(select_body.clone()));
                        }
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
                    physical_id: Some(physical_id),
                    output_ids: device_output_ids,
                    scene_key: scene_key.clone(),
                    add_device,
                    ops_open,
                })}
            </div>
            {(mode == CardMode::Placed)
                .then(move || {
                    view! {
                        <div class=("hidden", move || !ops_open.get())>
                            {card_ops_menu(
                                studio,
                                ops_open,
                                menu_physical_id,
                                menu_select,
                                row_device_id,
                                menu_output_ids,
                            )}
                        </div>
                    }
                })}
            {show_components
                .then(move || {
                    let scene_key = scene_key.clone();
                    let channel_count = component_rows.len();
                    view! {
                        // An Unassigned card folds its channels away until
                        // asked; they matter once the device is in a zone.
                        {(mode == CardMode::Unassigned)
                            .then(|| {
                                view! {
                                    <button
                                        type="button"
                                        class="flex w-full items-center gap-1.5 border-t px-2.5 py-1 text-left text-[10px] text-fg-tertiary/80 transition-colors hover:text-fg-secondary"
                                        style=list_style.clone()
                                        aria-expanded=move || channels_open.get().to_string()
                                        on:click=move |_| channels_open.update(|open| *open = !*open)
                                    >
                                        <Icon
                                            icon=LuChevronRight
                                            width="10px"
                                            height="10px"
                                            {..}
                                            class=("rotate-90", move || channels_open.get())
                                        />
                                        {format!("{channel_count} channels")}
                                    </button>
                                }
                            })}
                        <div
                            class="space-y-0.5 border-t px-1.5 py-1.5"
                            class=("hidden", move || !channels_open.get())
                            style=list_style.clone()
                        >
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
            {(!elsewhere.is_empty())
                .then(move || {
                    view! {
                        <div class="space-y-0.5 border-t px-2.5 py-1" style=hint_style>
                            {elsewhere
                                .into_iter()
                                .map(|hint| {
                                    let zone_id = hint.zone_id.clone();
                                    view! {
                                        <button
                                            type="button"
                                            class="flex w-full items-center gap-1 text-left text-[10px] text-fg-tertiary/80 transition-colors hover:text-fg-secondary"
                                            on:click=move |_| reveal_pane_zone(studio, &zone_id)
                                        >
                                            <span class="min-w-0 truncate">
                                                {elsewhere_label(hint.outputs, &hint.zone_name)}
                                            </span>
                                            <Icon icon=LuChevronRight width="10px" height="10px" />
                                        </button>
                                    }
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
    physical_id: Option<String>,
    output_ids: Vec<String>,
    scene_key: Option<String>,
    add_device: Option<DeviceSummary>,
    ops_open: RwSignal<bool>,
}

/// The trailing-edge action cluster, capped at two icons. A placed device
/// gets hide-all, which toggles every output in unison, and a kebab that
/// opens the inline operations menu (identify / move / remove). An
/// Unassigned device gets identify and a one-tap add into the active tab.
fn card_actions(args: CardActionsArgs) -> impl IntoView {
    let CardActionsArgs {
        studio,
        mode,
        select,
        physical_id,
        output_ids,
        scene_key,
        add_device,
        ops_open,
    } = args;
    let (identifying, set_identifying) = signal(false);
    // Only an Unassigned card shows identify, and it carries the device.
    let identify_label = add_device.as_ref().map_or_else(
        || "Identify".to_owned(),
        |device| format!("Identify {}", device.name),
    );
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
                .filter(|_| !matches!(mode, CardMode::Placed))
                .map(|id| {
                    let identify_id = id.clone();
                    view! {
                        <button
                            type="button"
                            class="btn-press flex h-6 w-6 items-center justify-center rounded-md text-fg-tertiary/55 transition-colors hover:text-fg-secondary disabled:cursor-wait disabled:opacity-60"
                            title="Identify (flash the hardware)"
                            aria-label=identify_label.clone()
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
                CardMode::Placed => Some(
                    view! {
                        <button
                            type="button"
                            class="btn-press flex h-6 w-6 items-center justify-center rounded-md text-fg-tertiary/55 transition-colors hover:text-fg-secondary"
                            class=("text-fg-secondary", move || ops_open.get())
                            title="Device options"
                            on:click=move |ev: web_sys::MouseEvent| {
                                ev.stop_propagation();
                                ops_open.update(|open| *open = !*open);
                            }
                        >
                            <Icon icon=LuEllipsis width="14px" height="14px" />
                        </button>
                    }
                    .into_any(),
                ),
                CardMode::Unassigned => add_device.map(|device| {
                    let target = zone_label(studio, &select);
                    view! {
                        <button
                            type="button"
                            class="btn-press flex h-6 w-6 items-center justify-center rounded-md text-accent transition-colors hover:bg-accent-subtle disabled:opacity-40"
                            title=format!("Add to {target}")
                            aria-label=format!("Add {} to {target}", device.name)
                            disabled=move || studio.history.busy.get()
                            on:click=move |ev: web_sys::MouseEvent| {
                                ev.stop_propagation();
                                assign_device_to_zone(studio, device.clone(), select.clone());
                            }
                        >
                            <Icon icon=LuPlus width="13px" height="13px" />
                        </button>
                    }
                    .into_any()
                }),
            }}
        </div>
    }
}

/// The Placed device card's operations menu — identify, move-to-zone, and
/// remove — folded behind the card's kebab so the action cluster stays at
/// two icons. Expands inline below the card body. Every row closes the
/// menu, then runs its action. The
/// move rows are one per other LED zone (none in a single-zone scene), so a
/// move is a single click rather than a nested picker.
fn card_ops_menu(
    studio: StudioContext,
    ops_open: RwSignal<bool>,
    physical_id: String,
    current_zone_id: String,
    device_id: String,
    output_ids: Vec<String>,
) -> impl IntoView {
    let (identifying, set_identifying) = signal(false);
    let identify_id = StoredValue::new(physical_id);
    let current = StoredValue::new(current_zone_id);
    let remove_device_id = StoredValue::new(device_id);
    let ids = StoredValue::new(output_ids);

    // Every LED zone except the one this device already sits in.
    let zone_options = Memo::new(move |_| {
        studio.active_scene.with(|scene| {
            scene
                .as_ref()
                .map(|scene| {
                    surfaces_from_zones(&scene.zones)
                        .into_iter()
                        .filter(|surface| surface.kind == super::surface::SurfaceKind::Light)
                        .filter(|surface| surface.id != current.get_value())
                        .map(|surface| (surface.id, surface.name))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });

    let row = "btn-press flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left \
               text-[11px] transition-colors";

    view! {
        <div class="space-y-0.5 border-t border-edge-subtle/45 bg-surface-sunken/60 px-1.5 py-1.5">
            <button
                type="button"
                class=format!("{row} text-fg-secondary hover:bg-surface-hover/40 disabled:opacity-60")
                disabled=move || identifying.get()
                on:click=move |ev: web_sys::MouseEvent| {
                    ev.stop_propagation();
                    ops_open.set(false);
                    identify_device_now(&identify_id.get_value(), set_identifying);
                }
            >
                <Icon icon=LuZap width="12px" height="12px" />
                <span>"Identify"</span>
            </button>
            {move || {
                zone_options
                    .get()
                    .into_iter()
                    .map(|(zone_id, zone_name)| {
                        view! {
                            <button
                                type="button"
                                class=format!("{row} text-fg-secondary hover:bg-surface-hover/40")
                                on:click=move |ev: web_sys::MouseEvent| {
                                    ev.stop_propagation();
                                    ops_open.set(false);
                                    move_outputs_to_zone(studio, zone_id.clone(), ids.get_value());
                                }
                                disabled=move || studio.history.busy.get()
                                class=("opacity-40", move || studio.history.busy.get())
                            >
                                <Icon icon=LuArrowRightLeft width="12px" height="12px" />
                                <span>{format!("Move to {zone_name}")}</span>
                            </button>
                        }
                    })
                    .collect_view()
            }}
            <button
                type="button"
                class=format!("{row} hover:bg-surface-hover/40")
                style="color: rgba(255, 99, 99, 0.85)"
                on:click=move |ev: web_sys::MouseEvent| {
                    ev.stop_propagation();
                    ops_open.set(false);
                    remove_device_from_zone(studio, current.get_value(), remove_device_id.get_value());
                }
                disabled=move || studio.history.busy.get()
                class=("opacity-40", move || studio.history.busy.get())
            >
                <Icon icon=LuTrash2 width="12px" height="12px" />
                <span>"Remove from zone"</span>
            </button>
        </div>
    }
}

/// Move a set of device outputs into `target_zone_id` by reassigning them
/// as existing outputs in one call. The daemon moves each output out of
/// whatever zone currently holds it.
fn move_outputs_to_zone(studio: StudioContext, target_zone_id: String, output_ids: Vec<String>) {
    use hypercolor_types::api::scene::{MemberEdit, MemberState};
    let history = studio.history;
    let Some(scene) = history.current_scene() else {
        return;
    };
    let Some(target) = scene
        .zones
        .iter()
        .find(|zone| zone.id.to_string() == target_zone_id)
    else {
        return;
    };
    let mut changes = Vec::new();
    for zone in &scene.zones {
        if zone.id == target.id {
            continue;
        }
        for (index, output) in api::zone_outputs(zone).into_iter().enumerate() {
            if output_ids.contains(&output.id) {
                changes.push(MemberEdit {
                    before: Some(MemberState {
                        zone_id: zone.id,
                        output: output.clone(),
                        index,
                    }),
                    after: Some(MemberState {
                        zone_id: target.id,
                        output,
                        index: target.members.len() + changes.len(),
                    }),
                });
            }
        }
    }
    history.edit_members(changes);
}

/// Render one per-channel row beneath the card body.
///
/// Each row shows the channel's topology shape and name, a live
/// component badge listing what is bound to this channel, the channel's
/// LED count, and a per-output hide toggle when the channel has an output
/// in the current zone. Hidden state is per-(scene, zone) client UI state,
/// not the daemon's `layout_auto_exclusions`. Moving a single channel
/// between zones is not surfaced here; the card's operations menu moves the
/// whole device.
fn component_row_view(
    studio: StudioContext,
    scene_key: Option<String>,
    physical_device_id: String,
    row: ComponentRow,
    shape_style: String,
) -> impl IntoView {
    let ComponentRow {
        name,
        shape,
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
    // (an Unassigned card, an unplaced segment) just reads as a label.
    let interactive = output_id.is_some();
    let click_output = output_id.clone();
    let key_output = output_id.clone();
    let enter_output = output_id.clone();
    let row_selected_id = output_id.clone();
    // The row's zone is the scene_key's zone half; a click from a card
    // expanded under another zone brings that zone onto the Stage first.
    let row_zone_id = scene_key.as_deref().and_then(zone_id_from_storage_key);
    let key_zone_id = row_zone_id.clone();
    release_hover_on_unmount(studio, output_id.iter().cloned().collect());
    let pointer_row_id = output_id.clone();
    let is_row_under_canvas_pointer = Memo::new(move |_| {
        pointer_row_id.as_ref().is_some_and(|id| {
            studio
                .pointer_output_id
                .with(|under| under.as_deref() == Some(id.as_str()))
        })
    });
    let is_row_selected = Signal::derive(move || {
        row_selected_id
            .as_ref()
            .is_some_and(|id| studio.selected_output_ids.with(|sel| sel.contains(id)))
    });

    view! {
        <div
            class="flex items-center gap-2 rounded px-1 py-1 transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent/60"
            class=("cursor-pointer", move || interactive)
            class=("bg-accent/12", move || is_row_selected.get())
            class=("bg-surface-hover/30", move || is_row_under_canvas_pointer.get())
            class=("hover:bg-surface-hover/30", move || interactive)
            role=interactive.then_some("button")
            tabindex=interactive.then_some("0")
            on:click=move |_| {
                if let Some(id) = click_output.clone() {
                    select_output_in_zone(studio, row_zone_id.as_deref(), id);
                }
            }
            on:keydown=move |ev: web_sys::KeyboardEvent| {
                if matches!(ev.key().as_str(), "Enter" | " ")
                    && let Some(id) = key_output.clone()
                {
                    ev.prevent_default();
                    select_output_in_zone(studio, key_zone_id.as_deref(), id);
                }
            }
            on:mouseenter=move |_| {
                if let Some(id) = enter_output.clone() {
                    studio.hovered_output_ids.set(HashSet::from([id]));
                }
            }
            on:mouseleave=move |_| studio.hovered_output_ids.set(HashSet::new())
        >
            <div class="h-3 w-3 shrink-0" style=shape_style>
                <TopologyShape kind=shape size=12 />
            </div>
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
/// Cards trigger this once on mount, and the pane's search for every
/// device once. The check on the cache lets the same device's card
/// unmount and remount without re-fetching, and lets every card share one
/// cache entry per physical device.
pub(super) fn fetch_attachments_if_needed(studio: StudioContext, physical_device_id: &str) {
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

/// A zone's shown name, read at click time.
fn zone_label(studio: StudioContext, zone_id: &str) -> String {
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
/// sequence because each one bumps the scene revision; threading the new
/// revision into the next call lets a multi-output controller leave the
/// zone in a single user action.
pub(super) fn remove_device_from_zone(studio: StudioContext, zone_id: String, device_id: String) {
    use hypercolor_types::api::scene::{MemberEdit, MemberState};
    let history = studio.history;
    let Some(scene) = history.current_scene() else {
        return;
    };
    let Some(zone) = scene
        .zones
        .iter()
        .find(|zone| zone.id.to_string() == zone_id)
    else {
        return;
    };
    let changes = api::zone_outputs(zone)
        .into_iter()
        .enumerate()
        .filter(|(_, output)| output.device_id == device_id)
        .map(|(index, output)| MemberEdit {
            before: Some(MemberState {
                zone_id: zone.id,
                output,
                index,
            }),
            after: None,
        })
        .collect();
    history.edit_members(changes);
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

/// A device's display resolution in pixels, from the first segment the
/// daemon tags with a `Display` topology hint. Ordinary LED segments are
/// strips, rings, and matrices.
fn display_resolution(device: &DeviceSummary) -> Option<(u32, u32)> {
    device
        .segments
        .iter()
        .find_map(|zone| match zone.topology_hint {
            Some(SegmentTopologySummary::Display { width, height, .. }) => Some((width, height)),
            _ => None,
        })
}

/// Vendor stand-in word for a device that is placed in the layout but
/// absent from the registry. The raw backend id (`razer:1532:…`) is never
/// shown (§4); this maps its leading backend token to a vendor name, or
/// the neutral "Device" when the token is unrecognized.
pub(super) fn friendly_offline_label(device_id: &str) -> &'static str {
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

/// The Screen surface backed by a physical display device, if the active
/// scene carries one. Looked up at click time so the redirect always
/// follows the current scene.
/// Select one output on the canvas, bringing its zone onto the Stage
/// first when the row was clicked under a zone that is not the current
/// surface. The surface switch clears the selection, so the output is
/// selected after it.
fn select_output_in_zone(studio: StudioContext, zone_id: Option<&str>, output_id: String) {
    match zone_id {
        Some(zone_id) => select_outputs_in_zone(studio, zone_id, HashSet::from([output_id])),
        None => studio.selected_output_ids.set(HashSet::from([output_id])),
    }
}

/// Select outputs on the canvas, bringing their zone onto the Stage first
/// when it is not the current surface. The switch's effects (which clear
/// the selection and reload the canvas) run after the calling handler, so
/// the outputs land a frame later, and only if nothing newer claimed the
/// selection in between.
fn select_outputs_in_zone(studio: StudioContext, zone_id: &str, outputs: HashSet<String>) {
    if studio.selected_surface_id.get_untracked().as_deref() != Some(zone_id) {
        studio.selected_surface_id.set(Some(zone_id.to_owned()));
        request_animation_frame(move || {
            let untouched = studio
                .selected_output_ids
                .try_with_untracked(HashSet::is_empty)
                .unwrap_or(false);
            if untouched {
                studio.selected_output_ids.try_set(outputs);
            }
        });
        return;
    }
    studio.selected_output_ids.set(outputs);
}

/// A hovered card or row can unmount without a `mouseleave` (collapse,
/// search, scene refresh); drop its hover so the canvas does not stay
/// dimmed around a phantom highlight.
fn release_hover_on_unmount(studio: StudioContext, outputs: Vec<String>) {
    on_cleanup(move || {
        let held = studio
            .hovered_output_ids
            .try_with_untracked(|hovered| {
                !hovered.is_empty() && outputs.iter().any(|id| hovered.contains(id))
            })
            .unwrap_or(false);
        if held {
            studio.hovered_output_ids.try_set(HashSet::new());
        }
    });
}

fn screen_surface_id_for_device(studio: StudioContext, device_id: &str) -> Option<String> {
    studio.active_scene.with_untracked(|scene| {
        scene.as_ref().and_then(|scene| {
            scene
                .zones
                .iter()
                .find(|zone| {
                    zone.role == ZoneRole::Display
                        && zone
                            .display_target
                            .as_ref()
                            .is_some_and(|target| target.device_id.to_string() == device_id)
                })
                .map(|zone| zone.id.to_string())
        })
    })
}
