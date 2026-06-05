//! Online device cards — draggable palette entries with zone rows and actions.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::channel_names;
use crate::components::device_card::{
    brand_colors, brand_label, brand_vendor, classify_brand, driver_identifier_label,
};
use crate::compound_selection::{self, CompoundDepth};
use crate::icons::*;
use crate::layout_utils;
use crate::style_utils::uuid_v4_hex;
use crate::vendors::{VendorMark, VendorMarkSize};

use super::topology::topology_icon;
use super::{PaletteState, fetch_attachments_for};

mod zone_rows;

use zone_rows::render_zone_rows;

pub(super) fn render_online_devices_section(state: PaletteState) -> AnyView {
    let devices = state.stable_devices.get();
    if devices.is_empty() {
        return view! {
            <div class="flex flex-col items-center py-6 space-y-2">
                <Icon icon=LuCpu width="24px" height="24px" style="color: rgba(139, 133, 160, 0.2)" />
                <div class="text-[10px] text-fg-tertiary">"No devices connected"</div>
            </div>
        }
        .into_any();
    }

    view! {
        <div class="space-y-2">
            {devices.into_iter().enumerate().map(|(idx, dev)| {
                render_device_card(state, idx, dev)
            }).collect_view()}
        </div>
    }
    .into_any()
}

#[allow(clippy::too_many_lines)]
fn render_device_card(state: PaletteState, idx: usize, dev: api::DeviceSummary) -> AnyView {
    let layout = state.layout;
    let selected_zone_ids = state.selected_zone_ids;
    let set_selected_zone_ids = state.set_selected_zone_ids;
    let set_compound_depth = state.set_compound_depth;
    let collapsed_devices = state.collapsed_devices;
    let set_collapsed_devices = state.set_collapsed_devices;

    let physical_device_id = dev.id.clone();
    let device_id = dev.layout_device_id.clone();
    let device_name = dev.name.clone();
    let connection_label = dev
        .connection
        .label
        .clone()
        .or(dev.connection.endpoint.clone());
    let brand = classify_brand(&dev);
    let (primary_rgb, secondary_rgb) = brand_colors(&brand);
    let vendor = brand_vendor(&brand);
    let driver_label = brand_label(&brand).unwrap_or_else(|| {
        let identifier = if dev.origin.driver_id.trim().is_empty() {
            &dev.origin.backend_id
        } else {
            &dev.origin.driver_id
        };
        driver_identifier_label(identifier).unwrap_or_else(|| identifier.to_string())
    });
    let fallback_leds = dev.total_leds;
    let has_multi_zones = dev.zones.len() > 1;
    let zone_count = dev.zones.len();

    // --- Single-zone handling ---
    let single_zone_summary = (!has_multi_zones)
        .then(|| dev.zones.first().cloned())
        .flatten();
    let single_topo = (!has_multi_zones).then(|| topology_icon(dev.zones.first()));

    // Layout membership for single-zone device
    let single_zone_in_layout = {
        let did = device_id.clone();
        let zone_name_key = single_zone_summary.as_ref().map(|z| z.name.clone());
        Signal::derive(move || {
            if has_multi_zones {
                return false;
            }
            layout.with(|current| {
                current
                    .as_ref()
                    .map(|l| {
                        l.zones.iter().any(|z| {
                            if z.device_id != did {
                                return false;
                            }
                            match zone_name_key.as_deref() {
                                Some(name) => layout_utils::zone_name_matches_slot_alias(
                                    z.zone_name.as_deref(),
                                    Some(name),
                                ),
                                None => z.zone_name.is_none(),
                            }
                        })
                    })
                    .unwrap_or(false)
            })
        })
    };

    // Is any zone for this device currently selected on canvas?
    let device_is_active = {
        let did = device_id.clone();
        Signal::derive(move || {
            selected_zone_ids.with(|ids| {
                if ids.is_empty() {
                    return false;
                }
                layout.with(|current| {
                    current.as_ref().is_some_and(|l| {
                        l.zones
                            .iter()
                            .any(|z| z.device_id == did && ids.contains(&z.id))
                    })
                })
            })
        })
    };

    // Find first zone_id for this device in the layout
    let first_zone_id_in_layout = {
        let did = device_id.clone();
        Signal::derive(move || {
            layout.with(|current| {
                current
                    .as_ref()
                    .and_then(|l| layout_utils::representative_zone_id_for_device(l, &did))
            })
        })
    };

    // --- Whole-device layout membership ---
    let any_zone_in_layout = {
        let did = device_id.clone();
        Signal::derive(move || {
            layout.with(|current| {
                current
                    .as_ref()
                    .is_some_and(|l| l.zones.iter().any(|z| z.device_id == did))
            })
        })
    };

    // --- Multi-zone handling ---
    let collapse_key = dev.layout_device_id.clone();
    let collapse_key2 = dev.layout_device_id.clone();
    let is_collapsed = {
        let key = collapse_key.clone();
        Signal::derive(move || collapsed_devices.get().contains(&key))
    };

    let rgb_for_indicator = primary_rgb.clone();
    let rgb_for_zones = primary_rgb.clone();
    let rgb = primary_rgb.clone();
    let rgb2 = secondary_rgb.clone();
    let header_device_id = device_id.clone();
    let header_physical_device_id = physical_device_id.clone();
    let channel_override_device_id = physical_device_id.clone();
    let mut entries: Vec<(Option<api::ZoneSummary>, String, usize)> = if has_multi_zones {
        dev.zones
            .iter()
            .cloned()
            .map(|zone| {
                let leds = zone.led_count;
                let display_name = channel_names::effective_channel_name(
                    &channel_override_device_id,
                    &zone.id,
                    &zone.name,
                );
                (Some(zone), display_name, leds)
            })
            .collect()
    } else {
        vec![]
    };
    entries.sort_by(|left, right| left.1.cmp(&right.1));

    let stagger = format!("animation-delay: {}ms", idx * 40);

    view! {
        <div
            class="rounded-lg overflow-hidden transition-all animate-enter-up"
            draggable=move || {
                if !has_multi_zones && single_zone_in_layout.get() { "true" } else { "false" }
            }
            on:dragstart=move |ev: web_sys::DragEvent| {
                // Single-zone devices: drag the whole card
                if !has_multi_zones
                    && let Some(zid) = first_zone_id_in_layout.get_untracked()
                        && let Some(dt) = ev.data_transfer() {
                            let _ = dt.set_data("application/x-hypercolor-zone", &zid);
                            dt.set_effect_allowed("move");
                        }
            }
            style=move || {
                let active = device_is_active.get();
                let border_opacity = if active { 0.5 } else { 0.15 };
                let bg_opacity = if active { 0.08 } else { 0.03 };
                let bg_opacity2 = if active { 0.04 } else { 0.01 };
                let shadow = if active {
                    format!(
                        "box-shadow: 0 0 16px rgba({rgb}, 0.15), 0 2px 8px rgba(0,0,0,0.2); "
                    )
                } else {
                    "box-shadow: 0 1px 4px rgba(0,0,0,0.15); ".to_string()
                };
                format!(
                    "--glow-rgb: {rgb}; {stagger}; \
                     border: 1px solid rgba({rgb}, {border_opacity}); \
                     background: linear-gradient(135deg, rgba({rgb}, {bg_opacity}), rgba({rgb2}, {bg_opacity2})); \
                     {shadow}"
                )
            }
        >
            // Device header
            <button
                class="w-full flex items-center gap-2 px-2.5 py-2 text-left transition-colors
                       hover:bg-white/[0.03]"
                on:click=move |_| {
                    if has_multi_zones {
                        // Select all zones from this device as a compound
                        set_compound_depth.set(CompoundDepth::Root);
                        layout.with_untracked(|l| {
                            if let Some(l) = l.as_ref() {
                                let ids = compound_selection::device_compound_ids(l, &header_device_id);
                                if !ids.is_empty() {
                                    set_selected_zone_ids.set(ids);
                                }
                            }
                        });
                        // Toggle collapse + fetch attachments on expand
                        let was_collapsed = collapsed_devices.get_untracked().contains(&collapse_key2);
                        set_collapsed_devices.update(|set| {
                            if set.contains(&collapse_key2) {
                                set.remove(&collapse_key2);
                            } else {
                                set.insert(collapse_key2.clone());
                            }
                        });
                        if was_collapsed {
                            fetch_attachments_for(state, header_physical_device_id.clone());
                        }
                    } else {
                        // Single-zone device: select as compound
                        set_compound_depth.set(CompoundDepth::Root);
                        layout.with_untracked(|l| {
                            if let Some(l) = l.as_ref() {
                                let ids = compound_selection::device_compound_ids(l, &header_device_id);
                                if !ids.is_empty() {
                                    set_selected_zone_ids.set(ids);
                                }
                            }
                        });
                    }
                }
            >
                // Device accent gradient strip
                <div
                    class="w-1 self-stretch rounded-full shrink-0"
                    style=format!(
                        "background: linear-gradient(180deg, rgb({primary_rgb}), rgb({secondary_rgb}))"
                    )
                />
                <div class="flex-1 min-w-0">
                    <div class="flex items-center gap-1.5">
                        {vendor.map(|v| view! { <VendorMark vendor=v size=VendorMarkSize::Xs /> })}
                        <span class="text-[11px] font-medium text-fg-primary truncate">
                            {device_name}
                        </span>
                        {vendor.is_none().then(|| view! {
                            <span
                                class="text-[8px] font-mono uppercase tracking-wider px-1 py-0.5 rounded border shrink-0"
                                style=format!(
                                    "color: rgba({primary_rgb}, 0.8); border-color: rgba({primary_rgb}, 0.2); background: rgba({primary_rgb}, 0.06)"
                                )
                            >
                                {driver_label}
                            </span>
                        })}
                    </div>
                    {connection_label.as_ref().map(|label| {
                        view! {
                            <div class="text-[9px] font-mono text-fg-tertiary/80 truncate mt-0.5">
                                {label.clone()}
                            </div>
                        }
                    })}
                    <div class="text-[10px] text-fg-tertiary font-mono flex items-center gap-1.5 mt-0.5">
                        <span>
                            {fallback_leds} " LEDs"
                        </span>
                        {has_multi_zones.then(|| {
                            view! {
                                <>
                                    <span class="opacity-40">"·"</span>
                                    <span>{zone_count} " outputs"</span>
                                </>
                            }
                        })}
                    </div>
                </div>

                // Right side: device actions — uniform button strip
                {if has_multi_zones {
                    render_multizone_header_actions(
                        state,
                        MultizoneHeaderActions {
                            dev: &dev,
                            layout_device_id: &device_id,
                            channel_device_id: &physical_device_id,
                            toggle_all_rgb: rgb_for_indicator.clone(),
                            any_zone_in_layout,
                            is_collapsed,
                            fallback_leds,
                        },
                    )
                } else {
                    render_singlezone_header_actions(
                        state,
                        &dev,
                        &device_id,
                        &physical_device_id,
                        rgb_for_indicator.clone(),
                        single_topo,
                        single_zone_in_layout,
                        single_zone_summary.clone(),
                        fallback_leds,
                    )
                }}
            </button>

            // Zone rows (multi-zone only)
            {has_multi_zones.then(|| {
                render_zone_rows(
                    state,
                    entries,
                    device_id.clone(),
                    physical_device_id.clone(),
                    dev.name.clone(),
                    rgb_for_zones,
                    is_collapsed,
                    fallback_leds,
                )
            })}
        </div>
    }
    .into_any()
}

struct MultizoneHeaderActions<'a> {
    dev: &'a api::DeviceSummary,
    layout_device_id: &'a str,
    channel_device_id: &'a str,
    toggle_all_rgb: String,
    any_zone_in_layout: Signal<bool>,
    is_collapsed: Signal<bool>,
    fallback_leds: usize,
}

fn render_multizone_header_actions(
    state: PaletteState,
    actions: MultizoneHeaderActions<'_>,
) -> AnyView {
    let layout = state.layout;
    let hidden_zones = state.hidden_zones;
    let set_layout = state.set_layout;
    let set_selected_zone_ids = state.set_selected_zone_ids;
    let set_is_dirty = state.set_is_dirty;
    let set_hidden_zones = state.set_hidden_zones;
    let removed_zone_cache = state.removed_zone_cache;
    let set_removed_zone_cache = state.set_removed_zone_cache;

    let toggle_all_did = actions.layout_device_id.to_owned();
    let toggle_all_channel_did = actions.channel_device_id.to_owned();
    let toggle_all_dname = actions.dev.name.clone();
    let toggle_all_zones = actions.dev.zones.clone();
    let vis_did = actions.layout_device_id.to_owned();

    // Device-level visibility: are ALL zones for this device hidden?
    let device_all_hidden = {
        let did = actions.layout_device_id.to_owned();
        Signal::derive(move || {
            let hidden = hidden_zones.get();
            layout.with(|current| {
                current
                    .as_ref()
                    .map(|l| {
                        let device_zones: Vec<_> =
                            l.zones.iter().filter(|z| z.device_id == did).collect();
                        !device_zones.is_empty()
                            && device_zones.iter().all(|z| hidden.contains(&z.id))
                    })
                    .unwrap_or(false)
            })
        })
    };

    view! {
        <div class="shrink-0 flex items-center gap-1">
            // Visibility toggle (device-level)
            {move || {
                if !actions.any_zone_in_layout.get() { return None; }
                let did = vis_did.clone();
                let all_hidden = device_all_hidden.get();
                Some(view! {
                    <button
                        class="w-6 h-6 flex items-center justify-center rounded-md
                               transition-all shrink-0 btn-press"
                        style=if all_hidden {
                            "color: var(--color-text-tertiary); opacity: 0.3"
                        } else {
                            "color: var(--color-text-tertiary); opacity: 0.5"
                        }
                        title=if all_hidden { "Show all outputs" } else { "Hide all outputs" }
                        on:click=move |ev: web_sys::MouseEvent| {
                            ev.stop_propagation();
                            state.set_master_hidden_snapshot.set(None);
                            let zone_ids: Vec<String> = layout.with_untracked(|current| {
                                current.as_ref().map(|l| {
                                    l.zones.iter()
                                        .filter(|z| z.device_id == did)
                                        .map(|z| z.id.clone())
                                        .collect()
                                }).unwrap_or_default()
                            });
                            set_hidden_zones.update(|set| {
                                for zid in &zone_ids {
                                    if all_hidden { set.remove(zid); } else { set.insert(zid.clone()); }
                                }
                            });
                        }
                    >
                        {if all_hidden {
                            view! { <Icon icon=LuEyeOff width="12px" height="12px" /> }.into_any()
                        } else {
                            view! { <Icon icon=LuEye width="12px" height="12px" /> }.into_any()
                        }}
                    </button>
                })
            }}
            // Add-all / remove-all toggle
            {move || {
                let did = toggle_all_did.clone();
                let channel_did = toggle_all_channel_did.clone();
                let dname = toggle_all_dname.clone();
                let zones = toggle_all_zones.clone();
                if actions.any_zone_in_layout.get() {
                    view! {
                        <button
                            class="w-6 h-6 flex items-center justify-center rounded-md
                                   transition-all shrink-0 btn-press"
                            style="color: rgba(255, 99, 99, 0.4)"
                            title="Remove all outputs"
                            on:click=move |ev| {
                                ev.stop_propagation();
                                layout_utils::remove_all_device_zones(
                                    &did,
                                    &set_layout,
                                    &set_selected_zone_ids,
                                    &set_is_dirty,
                                    &set_removed_zone_cache,
                                );
                            }
                        >
                            <Icon icon=LuTrash2 width="12px" height="12px" />
                        </button>
                    }.into_any()
                } else {
                    view! {
                        <button
                            class="w-6 h-6 flex items-center justify-center rounded-md
                                   border transition-all shrink-0 btn-press"
                            style=format!(
                                "background: rgba({}, 0.08); border-color: rgba({}, 0.2); color: rgb({})",
                                actions.toggle_all_rgb,
                                actions.toggle_all_rgb,
                                actions.toggle_all_rgb,
                            )
                            title="Add all outputs"
                            on:click=move |ev| {
                                ev.stop_propagation();
                                layout_utils::add_all_device_zones(
                                    &did,
                                    &channel_did,
                                    &dname,
                                    &zones,
                                    actions.fallback_leds,
                                    &layout,
                                    &set_layout,
                                    &set_selected_zone_ids,
                                    &set_is_dirty,
                                    &removed_zone_cache,
                                    &set_removed_zone_cache,
                                );
                            }
                        >
                            <Icon icon=LuPlus width="12px" height="12px" />
                        </button>
                    }.into_any()
                }
            }}
            // Expand/collapse chevron
            <div
                class="text-fg-tertiary/60"
                style=move || {
                    if actions.is_collapsed.get() {
                        "transform: rotate(-90deg); transition: transform 0.2s ease"
                    } else {
                        "transition: transform 0.2s ease"
                    }
                }
            >
                <Icon
                    icon=LuChevronDown
                    width="14px"
                    height="14px"
                />
            </div>
        </div>
    }
    .into_any()
}

#[allow(clippy::too_many_arguments)]
fn render_singlezone_header_actions(
    state: PaletteState,
    dev: &api::DeviceSummary,
    layout_device_id: &str,
    channel_device_id: &str,
    toggle_rgb: String,
    single_topo: Option<AnyView>,
    single_zone_in_layout: Signal<bool>,
    single_zone_summary: Option<api::ZoneSummary>,
    fallback_leds: usize,
) -> AnyView {
    let layout = state.layout;
    let hidden_zones = state.hidden_zones;
    let set_layout = state.set_layout;
    let set_selected_zone_ids = state.set_selected_zone_ids;
    let set_is_dirty = state.set_is_dirty;
    let set_hidden_zones = state.set_hidden_zones;
    let removed_zone_cache = state.removed_zone_cache;
    let set_removed_zone_cache = state.set_removed_zone_cache;

    let toggle_did = layout_device_id.to_owned();
    let toggle_channel_did = channel_device_id.to_owned();
    let toggle_dname = dev.name.clone();
    let toggle_zone = single_zone_summary.clone();
    let vis_single_did = layout_device_id.to_owned();

    // Single-zone device visibility
    let single_zone_hidden = {
        let did = layout_device_id.to_owned();
        Signal::derive(move || {
            let hidden = hidden_zones.get();
            layout.with(|current| {
                current
                    .as_ref()
                    .map(|l| {
                        l.zones
                            .iter()
                            .filter(|z| z.device_id == did)
                            .all(|z| hidden.contains(&z.id))
                            && l.zones.iter().any(|z| z.device_id == did)
                    })
                    .unwrap_or(false)
            })
        })
    };

    view! {
        <div class="shrink-0 flex items-center gap-1">
            {single_topo}
            // Visibility toggle (single-zone device)
            {move || {
                if !single_zone_in_layout.get() { return None; }
                let did = vis_single_did.clone();
                let is_hidden = single_zone_hidden.get();
                Some(view! {
                    <button
                        class="w-6 h-6 flex items-center justify-center rounded-md
                               transition-all shrink-0 btn-press"
                        style=if is_hidden {
                            "color: var(--color-text-tertiary); opacity: 0.3"
                        } else {
                            "color: var(--color-text-tertiary); opacity: 0.5"
                        }
                        title=if is_hidden { "Show device" } else { "Hide device" }
                        on:click=move |ev: web_sys::MouseEvent| {
                            ev.stop_propagation();
                            state.set_master_hidden_snapshot.set(None);
                            let zone_ids: Vec<String> = layout.with_untracked(|current| {
                                current.as_ref().map(|l| {
                                    l.zones.iter()
                                        .filter(|z| z.device_id == did)
                                        .map(|z| z.id.clone())
                                        .collect()
                                }).unwrap_or_default()
                            });
                            set_hidden_zones.update(|set| {
                                for zid in &zone_ids {
                                    if is_hidden { set.remove(zid); } else { set.insert(zid.clone()); }
                                }
                            });
                        }
                    >
                        {if is_hidden {
                            view! { <Icon icon=LuEyeOff width="12px" height="12px" /> }.into_any()
                        } else {
                            view! { <Icon icon=LuEye width="12px" height="12px" /> }.into_any()
                        }}
                    </button>
                })
            }}
            // Add / remove toggle
            {move || {
                let did = toggle_did.clone();
                let channel_did = toggle_channel_did.clone();
                let zone = toggle_zone.clone();
                let dname = toggle_dname.clone();
                if single_zone_in_layout.get() {
                    view! {
                        <button
                            class="w-6 h-6 flex items-center justify-center rounded-md
                                   transition-all shrink-0 btn-press"
                            style="color: rgba(255, 99, 99, 0.4)"
                            title="Remove from layout"
                            on:click=move |ev| {
                                ev.stop_propagation();
                                layout_utils::remove_device_zone(
                                    &did,
                                    zone.as_ref().map(|z| z.name.as_str()),
                                    &set_layout,
                                    &set_selected_zone_ids,
                                    &set_is_dirty,
                                    &set_removed_zone_cache,
                                );
                            }
                        >
                            <Icon icon=LuTrash2 width="12px" height="12px" />
                        </button>
                    }.into_any()
                } else {
                    view! {
                        <button
                            class="w-6 h-6 flex items-center justify-center rounded-md
                                   border transition-all shrink-0 btn-press"
                            style=format!(
                                "background: rgba({toggle_rgb}, 0.08); border-color: rgba({toggle_rgb}, 0.2); color: rgb({toggle_rgb})"
                            )
                            title="Add to layout"
                            on:click=move |ev| {
                                ev.stop_propagation();
                                let cache_key = (did.clone(), zone.as_ref().map(|z| z.name.clone()));
                                let cached = removed_zone_cache.with_untracked(|c| c.get(&cache_key).cloned());
                                let new_zone = if let Some(mut restored) = cached {
                                    // Restore from cache with a fresh ID
                                    restored.id = format!("zone_{}", uuid_v4_hex());
                                    set_removed_zone_cache.update(|c| { c.remove(&cache_key); });
                                    restored
                                } else {
                                    let (canvas_width, canvas_height) =
                                        layout_utils::current_canvas_dimensions(&layout);
                                    let order = layout_utils::next_display_order(&layout);
                                    layout_utils::create_default_zone(
                                        &did,
                                        &channel_did,
                                        &dname,
                                        zone.as_ref(),
                                        fallback_leds,
                                        canvas_width,
                                        canvas_height,
                                        order,
                                    )
                                };
                                let zone_id = new_zone.id.clone();
                                set_layout.update(|l| {
                                    if let Some(layout) = l {
                                        layout.zones.push(new_zone);
                                    }
                                });
                                set_selected_zone_ids.set(std::collections::HashSet::from([zone_id]));
                                set_is_dirty.set(true);
                            }
                        >
                            <Icon icon=LuPlus width="12px" height="12px" />
                        </button>
                    }.into_any()
                }
            }}
        </div>
    }
    .into_any()
}
