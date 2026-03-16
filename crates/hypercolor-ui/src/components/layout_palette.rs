//! Layout palette — available devices + zone group management.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::api::{self, ZoneTopologySummary};
use crate::app::DevicesContext;
use crate::icons::*;
use crate::layout_utils;
use crate::style_utils::{device_accent_colors, hex_to_rgb, uuid_v4_hex};
use hypercolor_types::spatial::ZoneGroup;

/// Group color presets — works in both dark and light themes.
const GROUP_COLORS: &[&str] = &[
    "#e135ff", "#80ffea", "#ff6ac1", "#f1fa8c", "#50fa7b", "#82AAFF", "#ff9e64", "#c792ea",
];

/// Device palette for adding zones to the layout, with group management.
#[component]
pub fn LayoutPalette() -> impl IntoView {
    let devices_ctx = expect_context::<DevicesContext>();
    let ctx = expect_context::<crate::components::layout_builder::LayoutEditorContext>();
    let layout = ctx.layout;
    let selected_zone_id = ctx.selected_zone_id;
    let hidden_zones = ctx.hidden_zones;
    let set_layout = ctx.set_layout;
    let set_selected_zone_id = ctx.set_selected_zone_id;
    let set_is_dirty = ctx.set_is_dirty;
    let set_hidden_zones = ctx.set_hidden_zones;
    let removed_zone_cache = ctx.removed_zone_cache;
    let set_removed_zone_cache = ctx.set_removed_zone_cache;

    let stable_devices = Memo::new(move |_| {
        devices_ctx
            .devices_resource
            .get()
            .and_then(|r| r.ok())
            .unwrap_or_default()
    });

    let (collapsed_devices, set_collapsed_devices) =
        signal(std::collections::HashSet::<String>::new());

    let (attachment_cache, set_attachment_cache) = signal(std::collections::HashMap::<
        String,
        Vec<api::AttachmentBindingSummary>,
    >::new());
    let (import_in_flight, set_import_in_flight) = signal(false);

    // Fetch attachments for a device (if not already cached).
    let fetch_attachments = move |device_id: String| {
        if attachment_cache.get_untracked().contains_key(&device_id) {
            return;
        }
        let did = device_id.clone();
        leptos::task::spawn_local(async move {
            if let Ok(profile) = api::fetch_device_attachments(&did).await {
                set_attachment_cache.update(|cache| {
                    cache.insert(did, profile.bindings);
                });
            }
        });
    };

    // Auto-fetch attachments for multi-zone devices (they start expanded).
    Effect::new(move |_| {
        let devices = stable_devices.get();
        let collapsed = collapsed_devices.get();
        for dev in &devices {
            if dev.zones.len() > 1 && !collapsed.contains(&dev.layout_device_id) {
                fetch_attachments(dev.id.clone());
            }
        }
    });

    // Derive group list from layout
    let groups = Signal::derive(move || {
        layout.with(|current| {
            current
                .as_ref()
                .map(|l| l.groups.clone())
                .unwrap_or_default()
        })
    });

    // Count zones per group
    let group_zone_counts = Signal::derive(move || {
        layout.with(|current| {
            let Some(l) = current.as_ref() else {
                return std::collections::HashMap::new();
            };
            let mut counts = std::collections::HashMap::new();
            for zone in &l.zones {
                if let Some(gid) = &zone.group_id {
                    *counts.entry(gid.clone()).or_insert(0usize) += 1;
                }
            }
            counts
        })
    });

    // Group being renamed (double-click to edit)
    let (editing_group_id, set_editing_group_id) = signal(None::<String>);

    // Group currently hovered during zone drag
    let (drag_over_group_id, set_drag_over_group_id) = signal(None::<String>);

    // Assign a zone to a group (or ungroup if group_id is None)
    let assign_zone_to_group = move |zone_id: String, group_id: Option<String>| {
        set_layout.update(|l| {
            if let Some(layout) = l {
                if let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zone_id) {
                    zone.group_id = group_id;
                }
            }
        });
        set_is_dirty.set(true);
    };

    // Rename a group
    let rename_group = move |group_id: String, new_name: String| {
        let name = new_name.trim().to_string();
        if name.is_empty() {
            return;
        }
        set_layout.update(|l| {
            if let Some(layout) = l {
                if let Some(group) = layout.groups.iter_mut().find(|g| g.id == group_id) {
                    group.name = name;
                }
            }
        });
        set_is_dirty.set(true);
        set_editing_group_id.set(None);
    };

    // Create a new group
    let create_group = move || {
        let current_groups = groups.get();
        let color_idx = current_groups.len() % GROUP_COLORS.len();
        let new_group = ZoneGroup {
            id: format!("group_{}", uuid_v4_hex()),
            name: format!("Group {}", current_groups.len() + 1),
            color: Some(GROUP_COLORS[color_idx].to_string()),
        };
        set_layout.update(|l| {
            if let Some(layout) = l {
                layout.groups.push(new_group);
            }
        });
        set_is_dirty.set(true);
    };

    // Delete a group (ungroups all zones)
    let delete_group = move |group_id: String| {
        set_layout.update(|l| {
            if let Some(layout) = l {
                layout.groups.retain(|g| g.id != group_id);
                for zone in &mut layout.zones {
                    if zone.group_id.as_deref() == Some(&group_id) {
                        zone.group_id = None;
                    }
                }
            }
        });
        set_is_dirty.set(true);
    };

    view! {
        <div class="p-3 space-y-4">
            // Groups section
            <div class="space-y-2">
                <div class="flex items-center justify-between">
                    <h3 class="text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary flex items-center gap-1.5">
                        <Icon icon=LuGroup width="12px" height="12px" />
                        "Groups"
                    </h3>
                    <button
                        class="flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] font-medium border whitespace-nowrap transition-all btn-press"
                        style="background: rgba(225, 53, 255, 0.06); border-color: rgba(225, 53, 255, 0.15); color: rgb(225, 53, 255)"
                        on:click=move |_| create_group()
                    >
                        <Icon icon=LuPlus width="10px" height="10px" />
                        "New"
                    </button>
                </div>

                {move || {
                    let current_groups = groups.get();
                    let counts = group_zone_counts.get();
                    let has_groups = !current_groups.is_empty();
                    if !has_groups {
                        view! {
                            <div class="text-[10px] text-fg-tertiary/50 italic">"No groups yet"</div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="space-y-1.5">
                                <div class="flex flex-wrap gap-1.5">
                                    {current_groups.into_iter().map(|group| {
                                        let gid = group.id.clone();
                                        let gid_delete = group.id.clone();
                                        let gid_visibility = group.id.clone();
                                        let gid_rename = group.id.clone();
                                        let gid_rename2 = group.id.clone();
                                        let gid_drop = group.id.clone();
                                        let gid_dragover = group.id.clone();
                                        let gid_dragleave = group.id.clone();
                                        let group_name = group.name.clone();
                                        let color = group.color.clone().unwrap_or_else(|| "#e135ff".to_string());
                                        let rgb = hex_to_rgb(&color);
                                        let count = counts.get(&group.id).copied().unwrap_or(0);

                                        // Is this group being renamed?
                                        let is_editing = Signal::derive(move || {
                                            editing_group_id.get().as_deref() == Some(&gid)
                                        });

                                        // Is a zone being dragged over this group?
                                        let is_drag_over = {
                                            let gid = gid_dragover.clone();
                                            Signal::derive(move || {
                                                drag_over_group_id.get().as_deref() == Some(&gid)
                                            })
                                        };

                                        // Check if all zones in this group are hidden
                                        let group_all_hidden = {
                                            let gid = group.id.clone();
                                            Signal::derive(move || {
                                                let hidden = hidden_zones.get();
                                                layout.with(|current| {
                                                    current.as_ref().map(|l| {
                                                        let member_zones: Vec<_> = l.zones.iter()
                                                            .filter(|z| z.group_id.as_deref() == Some(&gid))
                                                            .collect();
                                                        !member_zones.is_empty() && member_zones.iter().all(|z| hidden.contains(&z.id))
                                                    }).unwrap_or(false)
                                                })
                                            })
                                        };

                                        view! {
                                            <div
                                                class="flex items-center gap-1 px-2 py-1 rounded-full text-[13px] font-medium border
                                                       chip-interactive cursor-pointer group/chip transition-all"
                                                style=move || {
                                                    let drag = is_drag_over.get();
                                                    let bg_opacity = if drag { 0.2 } else { 0.08 };
                                                    let border_opacity = if drag { 0.6 } else { 0.25 };
                                                    let shadow = if drag {
                                                        format!("box-shadow: 0 0 12px rgba({rgb}, 0.3); ")
                                                    } else {
                                                        String::new()
                                                    };
                                                    format!(
                                                        "color: rgb({rgb}); border-color: rgba({rgb}, {border_opacity}); \
                                                         background: rgba({rgb}, {bg_opacity}); --glow-rgb: {rgb}; {shadow}"
                                                    )
                                                }
                                                on:dragover=move |ev: web_sys::DragEvent| {
                                                    ev.prevent_default();
                                                    set_drag_over_group_id.set(Some(gid_dragover.clone()));
                                                }
                                                on:dragleave=move |_: web_sys::DragEvent| {
                                                    // Only clear if we're still the hovered group
                                                    if drag_over_group_id.get_untracked().as_deref() == Some(&gid_dragleave) {
                                                        set_drag_over_group_id.set(None);
                                                    }
                                                }
                                                on:drop=move |ev: web_sys::DragEvent| {
                                                    ev.prevent_default();
                                                    set_drag_over_group_id.set(None);
                                                    if let Some(dt) = ev.data_transfer() {
                                                        if let Ok(zone_id) = dt.get_data("application/x-hypercolor-zone") {
                                                            if !zone_id.is_empty() {
                                                                assign_zone_to_group(zone_id, Some(gid_drop.clone()));
                                                            }
                                                        }
                                                    }
                                                }
                                            >
                                                <div
                                                    class="w-2 h-2 rounded-full shrink-0"
                                                    style=format!("background: rgb({rgb})")
                                                />
                                                // Name: inline edit on double-click, plain text otherwise
                                                {move || {
                                                    if is_editing.get() {
                                                        let gid = gid_rename.clone();
                                                        let gid2 = gid_rename2.clone();
                                                        let current_name = group_name.clone();
                                                        view! {
                                                            <input
                                                                type="text"
                                                                class="w-20 bg-transparent border-b border-current outline-none text-[13px] font-medium px-0 py-0"
                                                                style="color: inherit"
                                                                prop:value=current_name
                                                                autofocus=true
                                                                on:blur=move |ev| {
                                                                    let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                                                    if let Some(el) = target {
                                                                        rename_group(gid.clone(), el.value());
                                                                    }
                                                                }
                                                                on:keydown=move |ev: web_sys::KeyboardEvent| {
                                                                    if ev.key() == "Enter" {
                                                                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                                                        if let Some(el) = target {
                                                                            rename_group(gid2.clone(), el.value());
                                                                        }
                                                                    } else if ev.key() == "Escape" {
                                                                        set_editing_group_id.set(None);
                                                                    }
                                                                }
                                                                on:click=move |ev: web_sys::MouseEvent| {
                                                                    ev.stop_propagation();
                                                                }
                                                            />
                                                        }.into_any()
                                                    } else {
                                                        let gid = group.id.clone();
                                                        let name = group.name.clone();
                                                        view! {
                                                            <span
                                                                class="cursor-text select-none"
                                                                title="Double-click to rename"
                                                                on:dblclick=move |ev: web_sys::MouseEvent| {
                                                                    ev.stop_propagation();
                                                                    set_editing_group_id.set(Some(gid.clone()));
                                                                }
                                                            >
                                                                {name}
                                                            </span>
                                                        }.into_any()
                                                    }
                                                }}
                                                <span class="text-[9px] opacity-60">{count}</span>
                                                // Visibility toggle for group
                                                <button
                                                    class="ml-0.5 transition-opacity btn-press"
                                                    style=move || if group_all_hidden.get() {
                                                        "opacity: 0.35"
                                                    } else {
                                                        "opacity: 0.6"
                                                    }
                                                    title=move || if group_all_hidden.get() { "Show group" } else { "Hide group" }
                                                    on:click={
                                                        let gid = gid_visibility.clone();
                                                        move |ev: web_sys::MouseEvent| {
                                                            ev.stop_propagation();
                                                            let all_hidden = group_all_hidden.get_untracked();
                                                            let zone_ids: Vec<String> = layout.with_untracked(|current| {
                                                                current.as_ref().map(|l| {
                                                                    l.zones.iter()
                                                                        .filter(|z| z.group_id.as_deref() == Some(&gid))
                                                                        .map(|z| z.id.clone())
                                                                        .collect()
                                                                }).unwrap_or_default()
                                                            });
                                                            set_hidden_zones.update(|set| {
                                                                for zid in &zone_ids {
                                                                    if all_hidden {
                                                                        set.remove(zid);
                                                                    } else {
                                                                        set.insert(zid.clone());
                                                                    }
                                                                }
                                                            });
                                                        }
                                                    }
                                                >
                                                    {move || if group_all_hidden.get() {
                                                        view! { <Icon icon=LuEyeOff width="10px" height="10px" /> }.into_any()
                                                    } else {
                                                        view! { <Icon icon=LuEye width="10px" height="10px" /> }.into_any()
                                                    }}
                                                </button>
                                                <button
                                                    class="ml-0.5 opacity-0 group-hover/chip:opacity-60 hover:opacity-100 transition-opacity"
                                                    on:click=move |ev| {
                                                        ev.stop_propagation();
                                                        delete_group(gid_delete.clone());
                                                    }
                                                >
                                                    <Icon icon=LuX width="10px" height="10px" />
                                                </button>
                                            </div>
                                        }
                                    }).collect_view()}
                                </div>
                                // "Ungrouped" drop target — visible when groups exist
                                <div
                                    class="flex items-center gap-1 px-2 py-1 rounded-full text-[10px] border border-dashed transition-all"
                                    style=move || {
                                        let drag = drag_over_group_id.get().as_deref() == Some("__ungrouped__");
                                        if drag {
                                            "color: var(--color-text-secondary); border-color: var(--color-text-tertiary); \
                                             background: rgba(139, 133, 160, 0.1)".to_string()
                                        } else {
                                            "color: var(--color-text-tertiary); border-color: rgba(139, 133, 160, 0.15); \
                                             background: transparent; opacity: 0.6".to_string()
                                        }
                                    }
                                    on:dragover=move |ev: web_sys::DragEvent| {
                                        ev.prevent_default();
                                        set_drag_over_group_id.set(Some("__ungrouped__".to_string()));
                                    }
                                    on:dragleave=move |_: web_sys::DragEvent| {
                                        if drag_over_group_id.get_untracked().as_deref() == Some("__ungrouped__") {
                                            set_drag_over_group_id.set(None);
                                        }
                                    }
                                    on:drop=move |ev: web_sys::DragEvent| {
                                        ev.prevent_default();
                                        set_drag_over_group_id.set(None);
                                        if let Some(dt) = ev.data_transfer() {
                                            if let Ok(zone_id) = dt.get_data("application/x-hypercolor-zone") {
                                                if !zone_id.is_empty() {
                                                    assign_zone_to_group(zone_id, None);
                                                }
                                            }
                                        }
                                    }
                                >
                                    <Icon icon=LuUnlink width="10px" height="10px" />
                                    "Ungrouped"
                                </div>
                            </div>
                        }.into_any()
                    }
                }}
            </div>

            // Separator
            <div class="h-px bg-edge-subtle" />

            // Devices section
            <div class="space-y-2">
                <h3 class="text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary flex items-center gap-1.5">
                    <Icon icon=LuCpu width="12px" height="12px" />
                    "Devices"
                </h3>

                {move || {
                    let devices = stable_devices.get();
                    if devices.is_empty() {
                        return view! {
                            <div class="flex flex-col items-center py-6 space-y-2">
                                <Icon icon=LuCpu width="24px" height="24px" style="color: rgba(139, 133, 160, 0.2)" />
                                <div class="text-[10px] text-fg-tertiary">"No devices connected"</div>
                            </div>
                        }.into_any();
                    }

                    view! {
                        <div class="space-y-2">
                            {devices.into_iter().enumerate().map(|(idx, dev)| {
                                        let device_id = dev.layout_device_id.clone();
                                        let device_name = dev.name.clone();
                                        let connection_label = dev.connection_label.clone();
                                        let backend = dev.backend.clone();
                                        let (primary_rgb, secondary_rgb) = device_accent_colors(&device_id);
                                        let fallback_leds = dev.total_leds;
                                        let has_multi_zones = dev.zones.len() > 1;
                                        let zone_count = dev.zones.len();

                                        // --- Single-zone handling ---
                                        let single_zone_summary = (!has_multi_zones)
                                            .then(|| dev.zones.first().cloned())
                                            .flatten();
                                        let single_topo = (!has_multi_zones)
                                            .then(|| topology_icon(dev.zones.first()));

                                        // Layout membership for single-zone device
                                        let single_zone_in_layout = {
                                            let did = device_id.clone();
                                            let zone_name_key = single_zone_summary
                                                .as_ref()
                                                .map(|z| z.name.clone());
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
                                                                    Some(name) => {
                                                                        z.zone_name.as_deref()
                                                                            == Some(name)
                                                                    }
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
                                                let Some(sel_zone) = selected_zone_id.get() else {
                                                    return false;
                                                };
                                                layout.with(|current| {
                                                    current
                                                        .as_ref()
                                                        .and_then(|l| l.zones.iter().find(|z| z.id == sel_zone))
                                                        .is_some_and(|z| z.device_id == did)
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
                                                        .and_then(|l| {
                                                            l.zones.iter().find(|z| z.device_id == did).map(|z| z.id.clone())
                                                        })
                                                })
                                            })
                                        };

                                        // --- Whole-device layout membership ---
                                        let any_zone_in_layout = {
                                            let did = device_id.clone();
                                            Signal::derive(move || {
                                                layout.with(|current| {
                                                    current.as_ref().is_some_and(|l| {
                                                        l.zones.iter().any(|z| z.device_id == did)
                                                    })
                                                })
                                            })
                                        };

                                        // --- Multi-zone handling ---
                                        let collapse_key = dev.layout_device_id.clone();
                                        let collapse_key2 = dev.layout_device_id.clone();
                                        let is_collapsed = {
                                            let key = collapse_key.clone();
                                            Signal::derive(move || {
                                                collapsed_devices.get().contains(&key)
                                            })
                                        };

                                        let rgb_for_indicator = primary_rgb.clone();
                                        let rgb_for_zones = primary_rgb.clone();
                                        let rgb = primary_rgb.clone();
                                        let rgb2 = secondary_rgb.clone();
                                        let mut entries: Vec<(
                                            Option<api::ZoneSummary>,
                                            String,
                                            usize,
                                        )> = if has_multi_zones {
                                            dev.zones
                                                .iter()
                                                .cloned()
                                                .map(|zone| {
                                                    let leds = zone.led_count;
                                                    (Some(zone.clone()), zone.name, leds)
                                                })
                                                .collect()
                                        } else {
                                            vec![]
                                        };
                                        entries.sort_by(|left, right| left.1.cmp(&right.1));

                                        let stagger =
                                            format!("animation-delay: {}ms", idx * 40);

                                        view! {
                                            <div
                                                class="rounded-lg overflow-hidden transition-all animate-fade-in-up"
                                                draggable=move || {
                                                    if !has_multi_zones && single_zone_in_layout.get() { "true" } else { "false" }
                                                }
                                                on:dragstart=move |ev: web_sys::DragEvent| {
                                                    // Single-zone devices: drag the whole card
                                                    if !has_multi_zones {
                                                        if let Some(zid) = first_zone_id_in_layout.get_untracked() {
                                                            if let Some(dt) = ev.data_transfer() {
                                                                let _ = dt.set_data("application/x-hypercolor-zone", &zid);
                                                                dt.set_effect_allowed("move");
                                                            }
                                                        }
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
                                                            // Select first zone in layout (if any)
                                                            if let Some(zid) = first_zone_id_in_layout.get_untracked() {
                                                                set_selected_zone_id.set(Some(zid));
                                                            }
                                                            // Toggle collapse + fetch attachments on expand
                                                            let was_collapsed = collapsed_devices.get_untracked().contains(&collapse_key2);
                                                            set_collapsed_devices.update(
                                                                |set| {
                                                                    if set.contains(
                                                                        &collapse_key2,
                                                                    ) {
                                                                        set.remove(
                                                                            &collapse_key2,
                                                                        );
                                                                    } else {
                                                                        set.insert(
                                                                            collapse_key2
                                                                                .clone(),
                                                                        );
                                                                    }
                                                                },
                                                            );
                                                            if was_collapsed {
                                                                fetch_attachments(collapse_key.clone());
                                                            }
                                                        } else {
                                                            // Select zone on canvas if it's in layout
                                                            if let Some(zid) = first_zone_id_in_layout.get_untracked() {
                                                                set_selected_zone_id.set(Some(zid));
                                                            }
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
                                                            <span class="text-[11px] font-medium text-fg-primary truncate">
                                                                {device_name}
                                                            </span>
                                                            <span
                                                                class="text-[8px] font-mono uppercase tracking-wider px-1 py-0.5 rounded border shrink-0"
                                                                style=format!(
                                                                    "color: rgba({primary_rgb}, 0.8); border-color: rgba({primary_rgb}, 0.2); background: rgba({primary_rgb}, 0.06)"
                                                                )
                                                            >
                                                                {backend}
                                                            </span>
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
                                                            {has_multi_zones
                                                                .then(|| {
                                                                    view! {
                                                                        <>
                                                                            <span class="opacity-40">
                                                                                "·"
                                                                            </span>
                                                                            <span>
                                                                                {zone_count}
                                                                                " zones"
                                                                            </span>
                                                                        </>
                                                                    }
                                                                })}
                                                        </div>
                                                    </div>

                                                    // Right side: device actions — uniform button strip
                                                    {if has_multi_zones {
                                                        let toggle_all_rgb = rgb_for_indicator.clone();
                                                        let toggle_all_did = device_id.clone();
                                                        let toggle_all_dname = dev.name.clone();
                                                        let toggle_all_zones = dev.zones.clone();
                                                        let vis_did = device_id.clone();

                                                        // Device-level visibility: are ALL zones for this device hidden?
                                                        let device_all_hidden = {
                                                            let did = device_id.clone();
                                                            Signal::derive(move || {
                                                                let hidden = hidden_zones.get();
                                                                layout.with(|current| {
                                                                    current.as_ref().map(|l| {
                                                                        let device_zones: Vec<_> = l.zones.iter()
                                                                            .filter(|z| z.device_id == did)
                                                                            .collect();
                                                                        !device_zones.is_empty() && device_zones.iter().all(|z| hidden.contains(&z.id))
                                                                    }).unwrap_or(false)
                                                                })
                                                            })
                                                        };

                                                        view! {
                                                            <div class="shrink-0 flex items-center gap-1">
                                                                // Visibility toggle (device-level)
                                                                {move || {
                                                                    if !any_zone_in_layout.get() { return None; }
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
                                                                            title=if all_hidden { "Show all zones" } else { "Hide all zones" }
                                                                            on:click=move |ev: web_sys::MouseEvent| {
                                                                                ev.stop_propagation();
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
                                                                    let dname = toggle_all_dname.clone();
                                                                    let zones = toggle_all_zones.clone();
                                                                    if any_zone_in_layout.get() {
                                                                        view! {
                                                                            <button
                                                                                class="w-6 h-6 flex items-center justify-center rounded-md
                                                                                       transition-all shrink-0 btn-press"
                                                                                style="color: rgba(255, 99, 99, 0.4)"
                                                                                title="Remove all zones"
                                                                                on:click=move |ev| {
                                                                                    ev.stop_propagation();
                                                                                    layout_utils::remove_all_device_zones(
                                                                                        &did,
                                                                                        &set_layout,
                                                                                        &set_selected_zone_id,
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
                                                                                    "background: rgba({toggle_all_rgb}, 0.08); border-color: rgba({toggle_all_rgb}, 0.2); color: rgb({toggle_all_rgb})"
                                                                                )
                                                                                title="Add all zones"
                                                                                on:click=move |ev| {
                                                                                    ev.stop_propagation();
                                                                                    layout_utils::add_all_device_zones(
                                                                                        &did,
                                                                                        &dname,
                                                                                        &zones,
                                                                                        fallback_leds,
                                                                                        &layout,
                                                                                        &set_layout,
                                                                                        &set_selected_zone_id,
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
                                                                        if is_collapsed.get() {
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
                                                    } else {
                                                        let toggle_rgb = rgb_for_indicator.clone();
                                                        let toggle_did = device_id.clone();
                                                        let toggle_dname = dev.name.clone();
                                                        let toggle_zone = single_zone_summary.clone();
                                                        let vis_single_did = device_id.clone();

                                                        // Single-zone device visibility
                                                        let single_zone_hidden = {
                                                            let did = device_id.clone();
                                                            Signal::derive(move || {
                                                                let hidden = hidden_zones.get();
                                                                layout.with(|current| {
                                                                    current.as_ref().map(|l| {
                                                                        l.zones.iter()
                                                                            .filter(|z| z.device_id == did)
                                                                            .all(|z| hidden.contains(&z.id))
                                                                            && l.zones.iter().any(|z| z.device_id == did)
                                                                    }).unwrap_or(false)
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
                                                                                        &set_selected_zone_id,
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
                                                                                    set_selected_zone_id.set(Some(zone_id));
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
                                                    }}
                                                </button>

                                                // Zone rows (multi-zone only)
                                                {has_multi_zones.then(|| {
                                                    let device_id = device_id.clone();
                                                    let zone_border_rgb = primary_rgb.clone();
                                                    let import_border_rgb = primary_rgb.clone();
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
                                                            {entries
                                                                .into_iter()
                                                                .map(
                                                                    |(
                                                                        zone_summary,
                                                                        display_name,
                                                                        led_count,
                                                                    )| {
                                                                        let zone_name_key =
                                                                            zone_summary
                                                                                .as_ref()
                                                                                .map(|z| {
                                                                                    z.name.clone()
                                                                                });
                                                                        let in_layout = {
                                                                            let did =
                                                                                device_id.clone();
                                                                            let zone_name =
                                                                                zone_name_key
                                                                                    .clone();
                                                                            Signal::derive(
                                                                                move || {
                                                                                    layout.with(
                                                                                    |current| {
                                                                                        current
                                                                                            .as_ref()
                                                                                            .map(
                                                                                                |l| {
                                                                                                    l.zones.iter().any(|z| {
                                                                                                        if z.device_id != did {
                                                                                                            return false;
                                                                                                        }
                                                                                                        match zone_name.as_deref() {
                                                                                                            Some(name) => z.zone_name.as_deref() == Some(name),
                                                                                                            None => z.zone_name.is_none(),
                                                                                                        }
                                                                                                    })
                                                                                                },
                                                                                            )
                                                                                            .unwrap_or(
                                                                                                false,
                                                                                            )
                                                                                    },
                                                                                )
                                                                                },
                                                                            )
                                                                        };

                                                                        // Is this specific zone selected?
                                                                        let zone_is_selected = {
                                                                            let did = device_id.clone();
                                                                            let zn = zone_name_key.clone();
                                                                            Signal::derive(move || {
                                                                                let Some(sel) = selected_zone_id.get() else {
                                                                                    return false;
                                                                                };
                                                                                layout.with(|current| {
                                                                                    current.as_ref()
                                                                                        .and_then(|l| l.zones.iter().find(|z| z.id == sel))
                                                                                        .is_some_and(|z| {
                                                                                            z.device_id == did && z.zone_name.as_deref() == zn.as_deref()
                                                                                        })
                                                                                })
                                                                            })
                                                                        };

                                                                        // Find zone_id to select on click
                                                                        let zone_id_for_select = {
                                                                            let did = device_id.clone();
                                                                            let zn = zone_name_key.clone();
                                                                            Signal::derive(move || {
                                                                                layout.with(|current| {
                                                                                    current.as_ref().and_then(|l| {
                                                                                        l.zones.iter().find(|z| {
                                                                                            z.device_id == did && z.zone_name.as_deref() == zn.as_deref()
                                                                                        }).map(|z| z.id.clone())
                                                                                    })
                                                                                })
                                                                            })
                                                                        };

                                                                        let topo_icon =
                                                                            topology_icon(
                                                                                zone_summary
                                                                                    .as_ref(),
                                                                            );
                                                                        let zone_for_toggle =
                                                                            zone_summary.clone();
                                                                        let did_for_toggle =
                                                                            device_id.clone();
                                                                        let dname_for_toggle =
                                                                            dev.name.clone();
                                                                        let zone_rgb2 = rgb_for_zones.clone();
                                                                        let zone_rgb3 = rgb_for_zones.clone();
                                                                        let toggle_zone_name = zone_name_key.clone();

                                                                        // Zone ID for drag-to-group
                                                                        let zone_id_for_drag = zone_id_for_select;

                                                                        // Group color for this zone (if assigned to a group)
                                                                        let zone_group_color = {
                                                                            let did = device_id.clone();
                                                                            let zn = zone_name_key.clone();
                                                                            Signal::derive(move || {
                                                                                layout.with(|current| {
                                                                                    let l = current.as_ref()?;
                                                                                    let zone = l.zones.iter().find(|z| {
                                                                                        z.device_id == did && z.zone_name.as_deref() == zn.as_deref()
                                                                                    })?;
                                                                                    let gid = zone.group_id.as_ref()?;
                                                                                    let group = l.groups.iter().find(|g| &g.id == gid)?;
                                                                                    group.color.clone()
                                                                                })
                                                                            })
                                                                        };

                                                                        // Attachment binding for this zone/slot
                                                                        let binding_zone_name = display_name.clone();
                                                                        let binding_device_id = device_id.clone();
                                                                        let zone_bindings = Signal::derive(move || {
                                                                            let cache = attachment_cache.get();
                                                                            cache.get(&binding_device_id)
                                                                                .map(|bindings| {
                                                                                    bindings
                                                                                        .iter()
                                                                                        .filter(|binding| {
                                                                                            layout_utils::slot_id_matches_zone_name(
                                                                                                &binding.slot_id,
                                                                                                &binding_zone_name,
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
                                                                                    if let Some(zid) = zone_id_for_drag.get_untracked() {
                                                                                        if let Some(dt) = ev.data_transfer() {
                                                                                            let _ = dt.set_data("application/x-hypercolor-zone", &zid);
                                                                                            dt.set_effect_allowed("move");
                                                                                        }
                                                                                    }
                                                                                }
                                                                                on:click=move |_| {
                                                                                    if let Some(zid) = zone_id_for_select.get_untracked() {
                                                                                        set_selected_zone_id.set(Some(zid));
                                                                                    }
                                                                                }
                                                                            >
                                                                                // Group membership dot
                                                                                {move || {
                                                                                    zone_group_color.get().map(|color| {
                                                                                        let rgb = hex_to_rgb(&color);
                                                                                        view! {
                                                                                            <div
                                                                                                class="w-1.5 h-1.5 rounded-full shrink-0"
                                                                                                style=format!("background: rgb({rgb})")
                                                                                                title="Drag to a group chip to reassign"
                                                                                            />
                                                                                        }
                                                                                    })
                                                                                }}
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
                                                                                                format!("{attachment_count} attached")
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
                                                                                            title=if is_zone_hidden { "Show zone" } else { "Hide zone" }
                                                                                            on:click=move |ev: web_sys::MouseEvent| {
                                                                                                ev.stop_propagation();
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
                                                                                    let zn = toggle_zone_name.clone();
                                                                                    let zone_entry = zone_for_toggle.clone();
                                                                                    let dname = dname_for_toggle.clone();
                                                                                    if in_layout.get() {
                                                                                        view! {
                                                                                            <button
                                                                                                class="w-6 h-6 flex items-center justify-center rounded-md
                                                                                                       transition-all shrink-0 btn-press"
                                                                                                style="color: rgba(255, 99, 99, 0.4)"
                                                                                                title="Remove zone"
                                                                                                on:click=move |ev| {
                                                                                                    ev.stop_propagation();
                                                                                                    layout_utils::remove_device_zone(
                                                                                                        &did,
                                                                                                        zn.as_deref(),
                                                                                                        &set_layout,
                                                                                                        &set_selected_zone_id,
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
                                                                                                title="Add zone"
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
                                                                                                    set_selected_zone_id.set(Some(zone_id));
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
                                                                    },
                                                                )
                                                                .collect_view()}

                                                            // Import attachments button (if device has bindings)
                                                            {move || {
                                                                let did = device_id.clone();
                                                                let has_bindings = attachment_cache.get()
                                                                    .get(&did)
                                                                    .is_some_and(|b| !b.is_empty());
                                                                has_bindings.then(|| {
                                                                    let did = did.clone();
                                                                    view! {
                                                                        <div class="mt-1 pt-1.5 border-t" style=format!("border-color: rgba({import_border_rgb}, 0.08)")>
                                                                            <button
                                                                                class="w-full flex items-center justify-center gap-1.5 px-2 py-1.5 rounded-md text-[10px] font-medium transition-all btn-press disabled:opacity-40 disabled:cursor-not-allowed"
                                                                                style="background: rgba(128, 255, 234, 0.06); border: 1px solid rgba(128, 255, 234, 0.12); color: rgb(128, 255, 234)"
                                                                                disabled=move || import_in_flight.get()
                                                                                on:click=move |ev| {
                                                                                    ev.stop_propagation();
                                                                                    layout_utils::import_device_attachments(
                                                                                        did.clone(),
                                                                                        set_import_in_flight,
                                                                                        devices_ctx.layouts_resource,
                                                                                    );
                                                                                }
                                                                            >
                                                                                <Icon icon=LuLayoutTemplate width="10px" height="10px" style="color: inherit" />
                                                                                {move || if import_in_flight.get() { "Importing..." } else { "Import Attachments" }}
                                                                            </button>
                                                                        </div>
                                                                    }
                                                                })
                                                            }}
                                                        </div>
                                                    }
                                                })}
                                            </div>
                                        }
                                    })
                                    .collect_view()}
                                </div>
                    }
                        .into_any()
                }}
            </div>

            // Offline devices — zones in the layout whose device is not currently connected
            {move || {
                let devices = stable_devices.get();
                let connected_ids: std::collections::HashSet<String> = devices.iter()
                    .map(|d| d.layout_device_id.clone())
                    .collect();

                // Collect unique offline device IDs from the layout
                let offline_devices: Vec<(String, Vec<hypercolor_types::spatial::DeviceZone>)> = layout.with(|current| {
                    let Some(l) = current.as_ref() else { return Vec::new(); };
                    let mut by_device: std::collections::BTreeMap<String, Vec<hypercolor_types::spatial::DeviceZone>> =
                        std::collections::BTreeMap::new();
                    for zone in &l.zones {
                        if !connected_ids.contains(&zone.device_id) {
                            by_device.entry(zone.device_id.clone()).or_default().push(zone.clone());
                        }
                    }
                    by_device.into_iter().collect()
                });

                if offline_devices.is_empty() { return None; }

                Some(view! {
                    <div class="h-px bg-edge-subtle" />
                    <div class="space-y-2">
                        <h3 class="text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary/60 flex items-center gap-1.5">
                            <Icon icon=LuWifi width="12px" height="12px" />
                            "Offline"
                        </h3>
                        <div class="space-y-1.5">
                            {offline_devices.into_iter().map(|(device_id, zones)| {
                                let (primary_rgb, secondary_rgb) = device_accent_colors(&device_id);
                                let zone_count = zones.len();
                                let total_leds: u32 = zones.iter().map(|z| z.topology.led_count()).sum();
                                let device_name = zones[0].name.clone();
                                let did_remove = device_id.clone();
                                let did_vis = device_id.clone();

                                // Device-level visibility for offline device
                                let offline_all_hidden = {
                                    let did = device_id.clone();
                                    Signal::derive(move || {
                                        let hidden = hidden_zones.get();
                                        layout.with(|current| {
                                            current.as_ref().map(|l| {
                                                let dzones: Vec<_> = l.zones.iter().filter(|z| z.device_id == did).collect();
                                                !dzones.is_empty() && dzones.iter().all(|z| hidden.contains(&z.id))
                                            }).unwrap_or(false)
                                        })
                                    })
                                };

                                view! {
                                    <div
                                        class="rounded-lg overflow-hidden"
                                        style=format!(
                                            "border: 1px solid rgba({primary_rgb}, 0.12); \
                                             background: linear-gradient(135deg, rgba({primary_rgb}, 0.03), rgba({secondary_rgb}, 0.01)); \
                                             opacity: 0.7"
                                        )
                                    >
                                        <div class="flex items-center gap-2 px-2.5 py-2">
                                            <div
                                                class="w-1 self-stretch rounded-full shrink-0 opacity-40"
                                                style=format!("background: linear-gradient(180deg, rgb({primary_rgb}), rgb({secondary_rgb}))")
                                            />
                                            <div class="flex-1 min-w-0">
                                                <div class="flex items-center gap-1.5">
                                                    <span class="text-[11px] font-medium text-fg-secondary truncate">{device_name}</span>
                                                    <span class="text-[8px] font-mono uppercase tracking-wider px-1 py-0.5 rounded border shrink-0"
                                                        style="color: rgba(255, 99, 99, 0.6); border-color: rgba(255, 99, 99, 0.15); background: rgba(255, 99, 99, 0.04)"
                                                    >"offline"</span>
                                                </div>
                                                <div class="text-[10px] text-fg-tertiary/60 font-mono mt-0.5">
                                                    {total_leds} " LEDs"
                                                    {(zone_count > 1).then(|| view! {
                                                        <><span class="opacity-40">" · "</span>{zone_count}" zones"</>
                                                    })}
                                                </div>
                                            </div>
                                            // Visibility toggle
                                            {move || {
                                                let did = did_vis.clone();
                                                let all_hidden = offline_all_hidden.get();
                                                Some(view! {
                                                    <button
                                                        class="w-6 h-6 flex items-center justify-center rounded-md transition-all shrink-0 btn-press"
                                                        style=if all_hidden {
                                                            "color: var(--color-text-tertiary); opacity: 0.3"
                                                        } else {
                                                            "color: var(--color-text-tertiary); opacity: 0.5"
                                                        }
                                                        title=if all_hidden { "Show device" } else { "Hide device" }
                                                        on:click=move |ev: web_sys::MouseEvent| {
                                                            ev.stop_propagation();
                                                            let zone_ids: Vec<String> = layout.with_untracked(|current| {
                                                                current.as_ref().map(|l| {
                                                                    l.zones.iter().filter(|z| z.device_id == did)
                                                                        .map(|z| z.id.clone()).collect()
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
                                            // Remove all zones for this offline device
                                            <button
                                                class="w-6 h-6 flex items-center justify-center rounded-md transition-all shrink-0 btn-press"
                                                style="color: rgba(255, 99, 99, 0.4)"
                                                title="Remove all zones for this device"
                                                on:click=move |ev| {
                                                    ev.stop_propagation();
                                                    let did = did_remove.clone();
                                                    set_layout.update(|l| {
                                                        if let Some(layout) = l {
                                                            layout.zones.retain(|z| z.device_id != did);
                                                        }
                                                    });
                                                    set_selected_zone_id.set(None);
                                                    set_is_dirty.set(true);
                                                }
                                            >
                                                <Icon icon=LuTrash2 width="12px" height="12px" />
                                            </button>
                                        </div>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    </div>
                })
            }}
        </div>
    }
}

/// Return an appropriate icon view based on zone topology.
fn topology_icon(zone: Option<&api::ZoneSummary>) -> leptos::prelude::AnyView {
    match zone.and_then(|z| z.topology_hint.as_ref()) {
        Some(ZoneTopologySummary::Strip) => {
            view! { <Icon icon=LuMinus width="12px" height="12px" /> }.into_any()
        }
        Some(ZoneTopologySummary::Matrix { .. }) => {
            view! { <Icon icon=LuGrid2x2 width="12px" height="12px" /> }.into_any()
        }
        Some(ZoneTopologySummary::Ring { .. }) => {
            view! { <Icon icon=LuCircle width="12px" height="12px" /> }.into_any()
        }
        Some(ZoneTopologySummary::Point) => {
            view! { <Icon icon=LuCircleDot width="12px" height="12px" /> }.into_any()
        }
        _ => view! { <Icon icon=LuMinus width="12px" height="12px" /> }.into_any(),
    }
}
