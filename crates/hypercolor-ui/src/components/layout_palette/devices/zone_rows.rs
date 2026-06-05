use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::compound_selection;
use crate::icons::*;
use crate::layout_utils;
use crate::style_utils::uuid_v4_hex;

use super::super::PaletteState;
use super::super::topology::topology_icon;

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub(super) fn render_zone_rows(
    state: PaletteState,
    entries: Vec<(Option<api::ZoneSummary>, String, usize)>,
    layout_device_id: String,
    channel_device_id: String,
    device_name: String,
    rgb_for_zones: String,
    is_collapsed: Signal<bool>,
    fallback_leds: usize,
) -> AnyView {
    let zone_border_rgb = rgb_for_zones.clone();
    let layout = state.layout;
    let selected_zone_ids = state.selected_zone_ids;
    let hidden_zones = state.hidden_zones;
    let set_layout = state.set_layout;
    let set_selected_zone_ids = state.set_selected_zone_ids;
    let set_is_dirty = state.set_is_dirty;
    let set_hidden_zones = state.set_hidden_zones;
    let compound_depth = state.compound_depth;
    let removed_zone_cache = state.removed_zone_cache;
    let set_removed_zone_cache = state.set_removed_zone_cache;
    let attachment_cache = state.attachment_cache;

    view! {
        <div
            class="border-t px-1.5 py-1 space-y-0.5 overflow-hidden transition-all duration-200"
            style=move || {
                if is_collapsed.get() {
                    "max-height: 0; opacity: 0; padding: 0; border: none".to_string()
                } else {
                    format!("max-height: 500px; opacity: 1; border-color: rgba({zone_border_rgb}, 0.1)")
                }
            }
        >
            {entries.into_iter().map(|(zone_summary, display_name, led_count)| {
                let zone_name_key = zone_summary.as_ref().map(|z| z.name.clone());
                let in_layout = {
                    let did = layout_device_id.clone();
                    let zone_name = zone_name_key.clone();
                    Signal::derive(move || {
                        layout.with(|current| {
                            current.as_ref().is_some_and(|l| {
                                layout_utils::representative_zone_id_for_device_slot(
                                    l,
                                    &did,
                                    zone_name.as_deref(),
                                )
                                .is_some()
                            })
                        })
                    })
                };

                // Is this specific zone selected?
                let zone_is_selected = {
                    let did = layout_device_id.clone();
                    let zn = zone_name_key.clone();
                    Signal::derive(move || {
                        selected_zone_ids.with(|ids| {
                            if ids.is_empty() {
                                return false;
                            }
                            layout.with(|current| {
                                current.as_ref().is_some_and(|l| {
                                    ids.iter().any(|sel| {
                                        layout_utils::selected_zone_matches_device_slot(
                                            l,
                                            sel,
                                            &did,
                                            zn.as_deref(),
                                        )
                                    })
                                })
                            })
                        })
                    })
                };

                // Find zone_id to select on click
                let zone_id_for_select = {
                    let did = layout_device_id.clone();
                    let zn = zone_name_key.clone();
                    Signal::derive(move || {
                        layout.with(|current| {
                            current.as_ref().and_then(|l| {
                                layout_utils::representative_zone_id_for_device_slot(
                                    l,
                                    &did,
                                    zn.as_deref(),
                                )
                            })
                        })
                    })
                };

                let topo_icon = topology_icon(zone_summary.as_ref());
                let zone_for_toggle = zone_summary.clone();
                let did_for_toggle = layout_device_id.clone();
                let channel_did_for_toggle = channel_device_id.clone();
                let dname_for_toggle = device_name.clone();
                let zone_rgb2 = rgb_for_zones.clone();
                let zone_rgb3 = rgb_for_zones.clone();
                let toggle_zone_name = zone_name_key.clone();

                let zone_id_for_drag = zone_id_for_select;

                // Attachment binding for this zone/slot
                let binding_slot_id = zone_summary
                    .as_ref()
                    .map(|zone| zone.id.clone())
                    .unwrap_or_else(|| display_name.clone());
                let binding_zone_name = zone_summary.as_ref().map(|zone| zone.name.clone());
                let binding_device_id = channel_device_id.clone();
                let binding_display_name = display_name.clone();
                let zone_bindings = Signal::derive(move || {
                    let cache = attachment_cache.get();
                    cache.get(&binding_device_id)
                        .map(|bindings| {
                            bindings
                                .iter()
                                .filter(|binding| {
                                    layout_utils::attachment_binding_matches_slot_alias(
                                        &binding.slot_id,
                                        Some(&binding_slot_id),
                                        binding_zone_name.as_deref(),
                                        &binding_display_name,
                                    )
                                })
                                .cloned()
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                });

                view! {
                    <div
                        class="flex items-center gap-1.5 px-2 py-1.5 rounded-lg
                                cursor-pointer hover:bg-white/[0.04] transition-all group/zone"
                        draggable=move || if in_layout.get() { "true" } else { "false" }
                        style=move || {
                            if zone_is_selected.get() {
                                format!(
                                    "background: rgba({zone_rgb2}, 0.08); \
                                     box-shadow: inset 2px 0 0 rgb({zone_rgb2})"
                                )
                            } else {
                                String::new()
                            }
                        }
                        on:dragstart=move |ev: web_sys::DragEvent| {
                            if let Some(zid) = zone_id_for_drag.get_untracked()
                                && let Some(dt) = ev.data_transfer() {
                                    let _ = dt.set_data("application/x-hypercolor-zone", &zid);
                                    dt.set_effect_allowed("move");
                                }
                        }
                        on:click=move |_| {
                            if let Some(zid) = zone_id_for_select.get_untracked() {
                                let depth = compound_depth.get_untracked();
                                layout.with_untracked(|l| {
                                    if let Some(l) = l.as_ref() {
                                        let ids = compound_selection::resolve_click(l, &zid, &depth);
                                        set_selected_zone_ids.set(ids);
                                    }
                                });
                            }
                        }
                    >
                        // Topology icon
                        <div class="text-fg-tertiary/50 shrink-0">
                            {topo_icon}
                        </div>
                        <div class="flex-1 min-w-0">
                            <div class="text-[11px] text-fg-primary truncate">
                                {display_name}
                            </div>
                            <div class="flex items-center gap-1.5">
                                <span class="text-[8px] text-fg-tertiary/60 font-mono tabular-nums">
                                    {led_count} " LEDs"
                                </span>
                                {move || {
                                    let bindings = zone_bindings.get();
                                    let attachment_count: u32 = bindings
                                        .iter()
                                        .map(|binding| binding.instances.max(1))
                                        .sum();
                                    if attachment_count == 0 {
                                        return None;
                                    }

                                    let title = bindings
                                        .iter()
                                        .map(|binding| {
                                            let name = binding
                                                .name
                                                .clone()
                                                .unwrap_or_else(|| binding.template_name.clone());
                                            if binding.instances > 1 {
                                                format!(
                                                    "{name} ×{} ({} LEDs)",
                                                    binding.instances,
                                                    binding.effective_led_count
                                                )
                                            } else {
                                                format!(
                                                    "{name} ({} LEDs)",
                                                    binding.effective_led_count
                                                )
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                        .join(", ");

                                    let label = if attachment_count == 1 {
                                        bindings[0]
                                            .name
                                            .clone()
                                            .unwrap_or_else(|| bindings[0].template_name.clone())
                                    } else {
                                        format!("{attachment_count} components")
                                    };

                                    Some(view! {
                                        <span class="text-[8px] font-mono px-1 py-0.5 rounded truncate max-w-[100px]"
                                            style="color: rgb(128, 255, 234); background: rgba(128, 255, 234, 0.08); border: 1px solid rgba(128, 255, 234, 0.12)"
                                            title=title
                                        >
                                            <Icon icon=LuCable width="8px" height="8px" style="display: inline; vertical-align: -1px; margin-right: 2px" />
                                            {label}
                                        </span>
                                    })
                                }}
                            </div>
                        </div>
                        // Zone action buttons — uniform sizing
                        <div class="flex items-center gap-0.5 shrink-0">
                            // Visibility toggle
                            {move || {
                                if !in_layout.get() { return None; }
                                let zone_id = zone_id_for_select.get()?;
                                let zid_toggle = zone_id.clone();
                                let is_zone_hidden = hidden_zones.with(|s| s.contains(&zone_id));
                                Some(view! {
                                    <button
                                        class="w-6 h-6 flex items-center justify-center rounded-md
                                               transition-all shrink-0 btn-press"
                                        style=if is_zone_hidden {
                                            "color: var(--color-text-tertiary); opacity: 0.3"
                                        } else {
                                            "color: var(--color-text-tertiary); opacity: 0.5"
                                        }
                                        title=if is_zone_hidden { "Show output" } else { "Hide output" }
                                        on:click=move |ev: web_sys::MouseEvent| {
                                            ev.stop_propagation();
                                            state.set_master_hidden_snapshot.set(None);
                                            let zid = zid_toggle.clone();
                                            set_hidden_zones.update(|set| {
                                                if !set.remove(&zid) {
                                                    set.insert(zid);
                                                }
                                            });
                                        }
                                    >
                                        {if is_zone_hidden {
                                            view! { <Icon icon=LuEyeOff width="12px" height="12px" /> }.into_any()
                                        } else {
                                            view! { <Icon icon=LuEye width="12px" height="12px" /> }.into_any()
                                        }}
                                    </button>
                                })
                            }}
                            // Add or remove zone
                            {move || {
                                let did = did_for_toggle.clone();
                                let channel_did = channel_did_for_toggle.clone();
                                let zn = toggle_zone_name.clone();
                                let zone_entry = zone_for_toggle.clone();
                                let dname = dname_for_toggle.clone();
                                if in_layout.get() {
                                    view! {
                                        <button
                                            class="w-6 h-6 flex items-center justify-center rounded-md
                                                   transition-all shrink-0 btn-press"
                                            style="color: rgba(255, 99, 99, 0.4)"
                                            title="Remove output"
                                            on:click=move |ev| {
                                                ev.stop_propagation();
                                                layout_utils::remove_device_zone(
                                                    &did,
                                                    zn.as_deref(),
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
                                                "background: rgba({zone_rgb3}, 0.08); border-color: rgba({zone_rgb3}, 0.2); color: rgb({zone_rgb3})"
                                            )
                                            title="Add output"
                                            on:click=move |ev| {
                                                ev.stop_propagation();
                                                let cache_key = (did.clone(), zone_entry.as_ref().map(|z| z.name.clone()));
                                                let cached = removed_zone_cache.with_untracked(|c| c.get(&cache_key).cloned());
                                                let new_zone = if let Some(mut restored) = cached {
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
                                                        zone_entry.as_ref(),
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
                    </div>
                }
            }).collect_view()}
        </div>
    }
    .into_any()
}
