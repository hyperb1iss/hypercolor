//! Layout zone properties panel — horizontal editor below canvas viewport.

use hypercolor_leptos_ext::events::{Change, Input};
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::app::DevicesContext;
use crate::async_helpers::spawn_identify;
use crate::components::device_brightness_slider::DeviceBrightnessSlider;
use crate::icons::*;
use crate::layout_geometry::{self, SizeAxis};
use crate::style_utils::device_accent_colors;

mod group_panel;

use group_panel::GroupZoneProperties;

/// Zone properties editor (bottom panel of layout builder).
#[component]
pub fn LayoutZoneProperties() -> impl IntoView {
    let editor = expect_context::<crate::components::layout_builder::LayoutEditorContext>();
    let layout = editor.layout;
    let selected_zone_ids = editor.selected_zone_ids;
    let keep_aspect_ratio = editor.keep_aspect_ratio;
    let set_layout = editor.set_layout;
    let set_keep_aspect_ratio = editor.set_keep_aspect_ratio;
    let set_selected_zone_ids = editor.set_selected_zone_ids;
    let set_is_dirty = editor.set_is_dirty;
    let compound_depth = editor.compound_depth;
    let zone_display_ctx =
        expect_context::<crate::components::layout_builder::LayoutZoneDisplayContext>();

    // Brightness aggregate for the currently-selected zones — returns
    // `(value_0_to_100, mixed)`. Each `Output` carries its own
    // `brightness: Option<f32>` (None = 1.0). When the selection spans
    // zones with different brightness values the slider shows "Mixed"
    // at the average; dragging collapses them all to a shared value.
    let brightness_value = Signal::derive(move || {
        let zone_ids = selected_zone_ids.get();
        if zone_ids.is_empty() {
            return (100u8, false);
        }
        layout.with(|current| {
            let Some(l) = current.as_ref() else {
                return (100u8, false);
            };
            let values: Vec<u8> = l
                .zones
                .iter()
                .filter(|z| zone_ids.contains(&z.id))
                .map(|z| {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let pct = (z.brightness.unwrap_or(1.0).clamp(0.0, 1.0) * 100.0).round() as u8;
                    pct
                })
                .collect();
            if values.is_empty() {
                return (100u8, false);
            }
            let first = values[0];
            let mixed = values.iter().any(|v| *v != first);
            if mixed {
                let sum: u32 = values.iter().map(|v| u32::from(*v)).sum();
                #[allow(clippy::cast_possible_truncation)]
                let avg = (sum / values.len() as u32) as u8;
                (avg, true)
            } else {
                (first, false)
            }
        })
    });

    // Write handler — update `brightness` on every selected zone. Setting
    // back to 100 clears to `None` so serialized layouts stay clean.
    let set_zone_brightness = move |pct: u8| {
        let zone_ids = selected_zone_ids.get_untracked();
        if zone_ids.is_empty() {
            return;
        }
        let new_brightness = if pct >= 100 {
            None
        } else {
            Some(f32::from(pct) / 100.0)
        };
        set_layout.update(|l| {
            let Some(layout) = l.as_mut() else { return };
            for zone in &mut layout.zones {
                if zone_ids.contains(&zone.id) {
                    zone.brightness = new_brightness;
                }
            }
        });
        set_is_dirty.set(true);
    };

    // Accent color for the brightness slider — take the first selected
    // zone's device accent so it feels tied to the selection.
    let brightness_rgb = Signal::derive(move || {
        let zone_ids = selected_zone_ids.get();
        layout.with(|current| {
            current
                .as_ref()
                .and_then(|l| {
                    l.zones
                        .iter()
                        .find(|z| zone_ids.contains(&z.id))
                        .map(|z| device_accent_colors(&z.device_id).0)
                })
                .unwrap_or_else(|| "225, 53, 255".to_string())
        })
    });
    // Canvas pixel dimensions for display conversion
    let canvas_dims = Signal::derive(move || {
        layout.with(|current| {
            current
                .as_ref()
                .map(|l| (l.canvas_width.max(1) as f32, l.canvas_height.max(1) as f32))
                .unwrap_or((320.0, 200.0))
        })
    });

    // ── Group transform state (accumulated deltas, reset on selection change) ──
    let (group_rot_offset, set_group_rot_offset) = signal(0.0f32);
    let (group_scale_factor, set_group_scale_factor) = signal(1.0f32);
    // Track the previous selection set to detect changes
    let (prev_selection, set_prev_selection) = signal(std::collections::HashSet::<String>::new());

    // Derive selected zone snapshot for display (single-selection only for Phase 1)
    let zone_snapshot = Signal::derive(move || {
        let ids = selected_zone_ids.get();
        if ids.len() != 1 {
            return None;
        }
        let id = ids.iter().next()?;
        layout.with(|current| {
            let l = current.as_ref()?;
            let suppressed = crate::layout_utils::suppressed_attachment_source_zone_ids(l);
            if suppressed.contains(id) {
                return None;
            }
            l.zones.iter().find(|z| z.id == *id).cloned()
        })
    });

    // Helper to update a zone field
    let update_zone =
        move |zone_id: String, updater: Box<dyn FnOnce(&mut hypercolor_types::spatial::Output)>| {
            set_layout.update(|l| {
                if let Some(layout) = l
                    && let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zone_id)
                {
                    updater(zone);
                    zone.size = layout_geometry::normalize_zone_size_for_editor(
                        zone.position,
                        zone.size,
                        &zone.topology,
                    );
                }
            });
            set_is_dirty.set(true);
        };

    let update_zone_rotation = move |zone_id: String, rotation_radians: f32| {
        set_layout.update(|l| {
            if let Some(layout) = l {
                let changed =
                    layout_geometry::set_zone_rotation(layout, &zone_id, rotation_radians);
                if changed && let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zone_id) {
                    zone.size = layout_geometry::normalize_zone_size_for_editor(
                        zone.position,
                        zone.size,
                        &zone.topology,
                    );
                }
            }
        });
        set_is_dirty.set(true);
    };

    let move_zone_to_position =
        move |zone_id: String, desired_position: hypercolor_types::spatial::NormalizedPosition| {
            set_layout.update(|l| {
                if let Some(layout) = l {
                    let _ = layout_geometry::set_zone_position(layout, &zone_id, desired_position);
                }
            });
            set_is_dirty.set(true);
        };

    view! {
        <div class="h-full px-5 py-2.5 overflow-y-auto">
            {move || {
                let ids = selected_zone_ids.get();
                if ids.len() > 1 {
                    // Reset accumulated deltas when selection set changes
                    let prev = prev_selection.get_untracked();
                    if prev != ids {
                        set_prev_selection.set(ids.clone());
                        set_group_rot_offset.set(0.0);
                        set_group_scale_factor.set(1.0);
                    }

                    return view! {
                        <GroupZoneProperties
                            ids=ids
                            layout=layout
                            canvas_dims=canvas_dims
                            brightness_value=brightness_value
                            brightness_rgb=brightness_rgb.get()
                            on_brightness_change=Callback::new(set_zone_brightness)
                            set_layout=set_layout
                            set_is_dirty=set_is_dirty
                            compound_depth=compound_depth
                            group_rot_offset=group_rot_offset
                            set_group_rot_offset=set_group_rot_offset
                            group_scale_factor=group_scale_factor
                            set_group_scale_factor=set_group_scale_factor
                        />
                    }.into_any();
                }
                let Some(zone) = zone_snapshot.get() else {
                    return view! {
                        <div class="flex items-center justify-center h-full gap-2.5">
                            <Icon icon=LuMousePointerClick width="18px" height="18px" style="color: rgba(139, 133, 160, 0.12)" />
                            <div class="text-xs text-fg-tertiary/60">"Click an output on the canvas to edit its properties"</div>
                        </div>
                    }.into_any();
                };

                let ctx = expect_context::<DevicesContext>();
                let devices = ctx
                    .devices_resource
                    .get_untracked()
                    .and_then(Result::ok)
                    .unwrap_or_default();
                let attachment_profiles =
                    zone_display_ctx.attachment_profiles.get().unwrap_or_default();
                let zone_display =
                    crate::layout_utils::effective_zone_display(&zone, &devices, &attachment_profiles);

                let zone_id = zone.id.clone();
                let zone_name = zone_display.label.clone();
                let device_id_display = zone.device_id.clone();
                let device_id_title = zone.device_id.clone();
                let channel_name = zone.zone_name.clone();
                let (cw, ch) = canvas_dims.get_untracked();
                let transform_anchor = layout.with_untracked(|current| {
                    current.as_ref().and_then(|layout| {
                        layout_geometry::zone_transform_anchor(layout, &zone.id)
                    })
                }).unwrap_or(zone.position);
                let pos_x_px = transform_anchor.x * cw;
                let pos_y_px = transform_anchor.y * ch;
                let size_w_px = zone.size.x * cw;
                let size_h_px = zone.size.y * ch;
                let rotation_deg = zone.rotation.to_degrees();
                let scale = zone.scale;
                let led_count = zone.topology.led_count();
                let topology_label = topology_name(&zone.topology);
                let attachment = zone.attachment.clone();
                let default_name = zone_display.default_label.clone();
                let name_is_default = zone_name == default_name;
                let display_order = zone.display_order;
                let identify_target = zone_display.identify_target.clone();
                let reset_device_name =
                    crate::layout_utils::effective_device_name(&zone.device_id, &devices)
                        .unwrap_or_else(|| zone.device_id.clone());

                let zid_name = zone_id.clone();
                let zid_name_reset = zone_id.clone();
                let zid_channel = zone_id.clone();
                let zid_pos_x = zone_id.clone();
                let zid_pos_y = zone_id.clone();
                let zid_center_h = zone_id.clone();
                let zid_center_v = zone_id.clone();
                let zid_size_w = zone_id.clone();
                let zid_size_h = zone_id.clone();
                let zid_rotation = zone_id.clone();
                let zid_rotation_input = zone_id.clone();
                let zid_scale = zone_id.clone();
                let zid_scale_input = zone_id.clone();
                let zid_front = zone_id.clone();
                let zid_up = zone_id.clone();
                let zid_down = zone_id.clone();
                let identify_action = identify_target.clone();
                let zid_reset_defaults = zone_id.clone();
                let reset_device_id = zone.device_id.clone();
                let reset_zone_name = zone.zone_name.clone();
                let zid_back = zone_id.clone();
                let zid_remove = zone_id;

                view! {
                    <div class="space-y-2">
                        // ── Row 1: Identity · Metadata · Assignment · Layer · Actions ──
                        // Wraps so the toolbar reflows down instead of
                        // overflowing and overlapping as the window narrows.
                        <div class="flex flex-wrap items-center gap-x-3 gap-y-2 min-w-0">
                            // Name + Channel
                            <div class="flex items-center gap-1.5 min-w-0 shrink">
                                <input
                                    type="text"
                                    class="min-w-0 flex-1 max-w-[200px] bg-surface-sunken border border-edge-subtle rounded-md px-2.5 py-1
                                           text-sm text-fg-primary placeholder-fg-tertiary
                                           focus:outline-none focus:border-accent-muted glow-ring transition-colors"
                                    prop:value=zone_name
                                    on:change=move |ev| {
                                        let event = Change::from_event(ev);
                                        if let Some(val) = event.value_string() {
                                            let zid = zid_name.clone();
                                            update_zone(zid, Box::new(move |z| z.name = val));
                                        }
                                    }
                                />
                                {(!name_is_default).then(|| {
                                    let default = default_name.clone();
                                    view! {
                                        <button
                                            class="shrink-0 text-fg-tertiary/30 hover:text-accent transition-colors btn-press"
                                            title="Reset to default name"
                                            on:click=move |_| {
                                                let val = default.clone();
                                                let zid = zid_name_reset.clone();
                                                update_zone(zid, Box::new(move |z| z.name = val));
                                            }
                                        >
                                            <Icon icon=LuRotateCcw width="11px" height="11px" />
                                        </button>
                                    }
                                })}
                                <span class="text-[9px] text-fg-tertiary/35 font-mono uppercase ml-0.5">"Ch"</span>
                                <input
                                    type="text"
                                    placeholder="\u{2014}"
                                    class="w-24 bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1
                                           text-xs text-fg-primary font-mono placeholder-fg-tertiary/20
                                           focus:outline-none focus:border-accent-muted glow-ring transition-colors"
                                    prop:value=channel_name.clone().unwrap_or_default()
                                    on:change=move |ev| {
                                        let event = Change::from_event(ev);
                                        if let Some(val) = event.value_string() {
                                            let zid = zid_channel.clone();
                                            let zone_name = if val.trim().is_empty() { None } else { Some(val) };
                                            update_zone(zid, Box::new(move |z| z.zone_name = zone_name));
                                        }
                                    }
                                />
                            </div>

                            // Metadata — condensed inline, device id clickable tooltip
                            <div class="flex items-center gap-1.5 min-w-0 text-[10px] font-mono text-fg-tertiary/40">
                                <span class="truncate max-w-24 cursor-default" title=device_id_title>{device_id_display}</span>
                                <span class="text-fg-tertiary/20">{"\u{00b7}"}</span>
                                <span class="whitespace-nowrap">{topology_label}</span>
                                <span class="text-fg-tertiary/20">{"\u{00b7}"}</span>
                                <span class="tabular-nums">{led_count}</span>
                                {attachment.map(|att| {
                                    let label = att.template_id.clone();
                                    let detail = match att.led_count {
                                        Some(count) => format!("{label} ({count} LEDs)"),
                                        None => label.clone(),
                                    };
                                    view! {
                                        <span
                                            class="shrink-0 px-1.5 py-0.5 rounded text-[10px] truncate max-w-32"
                                            style="color: rgba(128, 255, 234, 0.7); background: rgba(128, 255, 234, 0.06)"
                                            title=detail
                                        >
                                            {label}
                                        </span>
                                    }
                                })}
                            </div>

                            // Layer controls — compact pill, pushed to the
                            // trailing edge; wraps below when the row is tight.
                            <div class="flex items-center shrink-0 rounded-md ml-auto"
                                 style="background: rgba(255, 255, 255, 0.02)">
                                <span class="text-[9px] text-fg-tertiary/40 font-mono uppercase tracking-wider shrink-0 pl-1.5 pr-0.5">
                                    "Layer"
                                </span>
                                {layer_icon_button(LuSkipForward, "Bring to front", {
                                    let zid = zid_front;
                                    move |_| {
                                        let zid = zid.clone();
                                        set_layout.update(|l| {
                                            if let Some(layout) = l {
                                                let max = layout.zones.iter().map(|z| z.display_order).max().unwrap_or(0);
                                                if let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zid) {
                                                    zone.display_order = max + 1;
                                                }
                                            }
                                        });
                                        set_is_dirty.set(true);
                                    }
                                })}
                                {layer_icon_button(LuChevronUp, "Move up one layer", {
                                    let zid = zid_up;
                                    move |_| {
                                        let zid = zid.clone();
                                        set_layout.update(|l| {
                                            if let Some(layout) = l {
                                                let current_order = layout.zones.iter()
                                                    .find(|z| z.id == zid)
                                                    .map(|z| z.display_order);
                                                if let Some(order) = current_order {
                                                    let next_up = layout.zones.iter()
                                                        .filter(|z| z.display_order > order)
                                                        .map(|z| z.display_order)
                                                        .min();
                                                    if let Some(swap_order) = next_up {
                                                        for z in &mut layout.zones {
                                                            if z.id == zid {
                                                                z.display_order = swap_order;
                                                            } else if z.display_order == swap_order {
                                                                z.display_order = order;
                                                            }
                                                        }
                                                    } else if let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zid) {
                                                        zone.display_order += 1;
                                                    }
                                                }
                                            }
                                        });
                                        set_is_dirty.set(true);
                                    }
                                })}
                                {layer_icon_button(LuChevronDown, "Move down one layer", {
                                    let zid = zid_down;
                                    move |_| {
                                        let zid = zid.clone();
                                        set_layout.update(|l| {
                                            if let Some(layout) = l {
                                                let current_order = layout.zones.iter()
                                                    .find(|z| z.id == zid)
                                                    .map(|z| z.display_order);
                                                if let Some(order) = current_order {
                                                    let next_down = layout.zones.iter()
                                                        .filter(|z| z.display_order < order)
                                                        .map(|z| z.display_order)
                                                        .max();
                                                    if let Some(swap_order) = next_down {
                                                        for z in &mut layout.zones {
                                                            if z.id == zid {
                                                                z.display_order = swap_order;
                                                            } else if z.display_order == swap_order {
                                                                z.display_order = order;
                                                            }
                                                        }
                                                    } else if let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zid) {
                                                        zone.display_order -= 1;
                                                    }
                                                }
                                            }
                                        });
                                        set_is_dirty.set(true);
                                    }
                                })}
                                {layer_icon_button(LuSkipBack, "Send to back", {
                                    let zid = zid_back;
                                    move |_| {
                                        let zid = zid.clone();
                                        set_layout.update(|l| {
                                            if let Some(layout) = l {
                                                let min = layout.zones.iter().map(|z| z.display_order).min().unwrap_or(0);
                                                if let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zid) {
                                                    zone.display_order = min - 1;
                                                }
                                            }
                                        });
                                        set_is_dirty.set(true);
                                    }
                                })}
                                <span class="text-[9px] font-mono text-fg-tertiary/25 tabular-nums px-1">
                                    {display_order}
                                </span>
                            </div>

                            // Zone actions — destructive separated by divider
                            <div class="flex items-center gap-0.5 shrink-0">
                                {identify_action.map(|target| view! {
                                    <button
                                        class="shrink-0 p-1 rounded-md text-fg-tertiary/30 hover:text-accent hover:bg-accent/8
                                               transition-colors btn-press"
                                        title="Identify output"
                                        on:click=move |_| match target.clone() {
                                            crate::layout_utils::ZoneIdentifyTarget::Device { device_id, zone_id } => {
                                                spawn_identify(
                                                    "output",
                                                    async move { crate::api::identify_zone(&device_id, &zone_id).await },
                                                );
                                            }
                                            crate::layout_utils::ZoneIdentifyTarget::Attachment {
                                                device_id,
                                                slot_id,
                                                binding_index,
                                                instance,
                                            } => {
                                                spawn_identify(
                                                    "component",
                                                    async move {
                                                        crate::api::identify_attachment(
                                                            &device_id,
                                                            &slot_id,
                                                            binding_index,
                                                            instance,
                                                        )
                                                        .await
                                                    },
                                                );
                                            }
                                        }
                                    >
                                        <Icon icon=LuZap width="12px" height="12px" />
                                    </button>
                                })}
                                <button
                                    class="shrink-0 p-1 rounded-md text-fg-tertiary/30 hover:text-accent hover:bg-accent/8
                                           transition-colors btn-press"
                                    title="Reset output to defaults"
                                    on:click=move |_| {
                                        let zid = zid_reset_defaults.clone();
                                        let did = reset_device_id.clone();
                                        let zn = reset_zone_name.clone();
                                        let dname = reset_device_name.clone();
                                        let zone_summary: Option<crate::api::ZoneSummary> = ctx
                                            .devices_resource
                                            .get_untracked()
                                            .and_then(|r| r.ok())
                                            .and_then(|devices| {
                                                devices.iter()
                                                    .find(|d| d.layout_device_id == did)
                                                    .and_then(|d| {
                                                        zn.as_ref().and_then(|name| {
                                                            d.zones.iter().find(|z| z.name == *name).cloned()
                                                        })
                                                    })
                                            });
                                        let total_leds = ctx
                                            .devices_resource
                                            .get_untracked()
                                            .and_then(|r| r.ok())
                                            .and_then(|devices| devices.iter().find(|d| d.layout_device_id == did).map(|d| d.total_leds as usize))
                                            .unwrap_or(1);
                                        let (canvas_width, canvas_height) = layout.with_untracked(|current| {
                                            current.as_ref()
                                                .map(|l| (l.canvas_width.max(1), l.canvas_height.max(1)))
                                                .unwrap_or((320, 200))
                                        });
                                        let defaults = crate::layout_geometry::default_zone_visuals(
                                            &dname,
                                            zone_summary.as_ref(),
                                            total_leds,
                                            canvas_width,
                                            canvas_height,
                                        );
                                        set_layout.update(|l| {
                                            if let Some(layout) = l
                                                && let Some(zone) = layout.zones.iter_mut().find(|z| z.id == zid) {
                                                    zone.position = hypercolor_types::spatial::NormalizedPosition::new(0.5, 0.5);
                                                    zone.size = crate::layout_geometry::normalize_zone_size_for_editor(
                                                        zone.position,
                                                        defaults.size,
                                                        &defaults.topology,
                                                    );
                                                    zone.rotation = 0.0;
                                                    zone.scale = 1.0;
                                                }
                                        });
                                        set_is_dirty.set(true);
                                    }
                                >
                                    <Icon icon=LuRotateCcw width="12px" height="12px" />
                                </button>
                                <div class="w-px h-3 bg-edge-subtle mx-1" />
                                <button
                                    class="shrink-0 p-1 rounded-md text-status-error/30 hover:text-status-error hover:bg-status-error/8
                                           transition-colors btn-press"
                                    title="Remove output from layout"
                                    on:click=move |_| {
                                        let zid = zid_remove.clone();
                                        set_layout.update(|l| {
                                            if let Some(layout) = l
                                                && let Some(pos) = layout.zones.iter().position(|z| z.id == zid) {
                                                    let removed = layout.zones.remove(pos);
                                                    let key = (removed.device_id.clone(), removed.zone_name.clone());
                                                    editor.set_removed_zone_cache.update(|c| { c.insert(key, removed); });
                                                }
                                        });
                                        set_selected_zone_ids.set(std::collections::HashSet::new());
                                        set_is_dirty.set(true);
                                    }
                                >
                                    <Icon icon=LuTrash2 width="12px" height="12px" />
                                </button>
                            </div>
                            <DeviceBrightnessSlider
                                value=brightness_value
                                on_change=Callback::new(set_zone_brightness)
                                rgb=brightness_rgb.get()
                            />
                        </div>

                        // ── Row 2: Transform controls in pill sections ──
                        // Each pill is a wrap unit, so Pos / Size / Rot / Scale
                        // flow onto the next line rather than overflowing.
                        <div class="flex flex-wrap items-center gap-1.5">
                            // Position
                            <div class="flex items-center gap-1.5 shrink-0 rounded-lg px-2 py-1"
                                 style="background: rgba(255, 255, 255, 0.02)">
                                <span class="text-[9px] text-fg-tertiary/40 font-mono uppercase tracking-wider shrink-0 w-5">"Pos"</span>
                                {zone_pixel_input("X", pos_x_px, "1", 0, cw, {
                                    let zid = zid_pos_x;
                                    move |px: f32| {
                                        let (cw, _) = canvas_dims.get_untracked();
                                        let norm = (px / cw).clamp(0.0, 1.0);
                                        let anchor = layout.with_untracked(|current| {
                                            current.as_ref().and_then(|layout| {
                                                layout_geometry::zone_transform_anchor(layout, &zid)
                                            })
                                        });
                                        let Some(anchor) = anchor else {
                                            return;
                                        };
                                        move_zone_to_position(
                                            zid.clone(),
                                            hypercolor_types::spatial::NormalizedPosition::new(norm, anchor.y),
                                        );
                                    }
                                })}
                                {zone_pixel_input("Y", pos_y_px, "1", 0, ch, {
                                    let zid = zid_pos_y;
                                    move |px: f32| {
                                        let (_, ch) = canvas_dims.get_untracked();
                                        let norm = (px / ch).clamp(0.0, 1.0);
                                        let anchor = layout.with_untracked(|current| {
                                            current.as_ref().and_then(|layout| {
                                                layout_geometry::zone_transform_anchor(layout, &zid)
                                            })
                                        });
                                        let Some(anchor) = anchor else {
                                            return;
                                        };
                                        move_zone_to_position(
                                            zid.clone(),
                                            hypercolor_types::spatial::NormalizedPosition::new(anchor.x, norm),
                                        );
                                    }
                                })}
                                <button
                                    class="shrink-0 p-0.5 rounded text-fg-tertiary/30 hover:text-accent transition-colors btn-press"
                                    title="Center horizontally"
                                    on:click=move |_| {
                                        let zid = zid_center_h.clone();
                                        let anchor = layout.with_untracked(|current| {
                                            current.as_ref().and_then(|layout| {
                                                layout_geometry::zone_transform_anchor(layout, &zid)
                                            })
                                        });
                                        if let Some(anchor) = anchor {
                                            move_zone_to_position(
                                                zid,
                                                hypercolor_types::spatial::NormalizedPosition::new(0.5, anchor.y),
                                            );
                                        }
                                    }
                                >
                                    <Icon icon=LuAlignCenterHorizontal width="11px" height="11px" />
                                </button>
                                <button
                                    class="shrink-0 p-0.5 rounded text-fg-tertiary/30 hover:text-accent transition-colors btn-press"
                                    title="Center vertically"
                                    on:click=move |_| {
                                        let zid = zid_center_v.clone();
                                        let anchor = layout.with_untracked(|current| {
                                            current.as_ref().and_then(|layout| {
                                                layout_geometry::zone_transform_anchor(layout, &zid)
                                            })
                                        });
                                        if let Some(anchor) = anchor {
                                            move_zone_to_position(
                                                zid,
                                                hypercolor_types::spatial::NormalizedPosition::new(anchor.x, 0.5),
                                            );
                                        }
                                    }
                                >
                                    <Icon icon=LuAlignCenterVertical width="11px" height="11px" />
                                </button>
                            </div>

                            // Size
                            <div class="flex items-center gap-1.5 shrink-0 rounded-lg px-2 py-1"
                                 style="background: rgba(255, 255, 255, 0.02)">
                                <span class="text-[9px] text-fg-tertiary/40 font-mono uppercase tracking-wider shrink-0 w-6">"Size"</span>
                                {zone_pixel_input("W", size_w_px, "1", 0, cw, {
                                    let zid = zid_size_w;
                                    move |px: f32| {
                                        let (cw, _) = canvas_dims.get_untracked();
                                        let norm = (px / cw).clamp(0.0, 1.0);
                                        let locked = keep_aspect_ratio.get_untracked();
                                        update_zone(zid.clone(), Box::new(move |z| {
                                            z.size = layout_geometry::update_zone_size(
                                                z.size, SizeAxis::Width, norm, locked,
                                            );
                                        }))
                                    }
                                })}
                                {zone_pixel_input("H", size_h_px, "1", 0, ch, {
                                    let zid = zid_size_h;
                                    move |px: f32| {
                                        let (_, ch) = canvas_dims.get_untracked();
                                        let norm = (px / ch).clamp(0.0, 1.0);
                                        let locked = keep_aspect_ratio.get_untracked();
                                        update_zone(zid.clone(), Box::new(move |z| {
                                            z.size = layout_geometry::update_zone_size(
                                                z.size, SizeAxis::Height, norm, locked,
                                            );
                                        }))
                                    }
                                })}
                                <button
                                    class="shrink-0 p-0.5 rounded transition-colors btn-press"
                                    title=move || if keep_aspect_ratio.get() { "Aspect ratio linked" } else { "Aspect ratio free" }
                                    style=move || {
                                        if keep_aspect_ratio.get() {
                                            "color: rgba(128, 255, 234, 0.8)".to_string()
                                        } else {
                                            "color: rgba(139, 133, 160, 0.35)".to_string()
                                        }
                                    }
                                    on:click=move |_| {
                                        set_keep_aspect_ratio.update(|locked| *locked = !*locked);
                                    }
                                >
                                    {move || if keep_aspect_ratio.get() {
                                        view! { <Icon icon=LuLink width="11px" height="11px" /> }.into_any()
                                    } else {
                                        view! { <Icon icon=LuUnlink width="11px" height="11px" /> }.into_any()
                                    }}
                                </button>
                            </div>

                            // Rotation
                            <div class="flex items-center gap-1.5 flex-1 min-w-28 rounded-lg px-2 py-1"
                                 style="background: rgba(255, 255, 255, 0.02)">
                                <span class="text-[9px] text-fg-tertiary/40 font-mono uppercase tracking-wider shrink-0">"Rot"</span>
                                <input
                                    type="range"
                                    min="0" max="360" step="1"
                                    class="min-w-0 flex-1"
                                    prop:value=format!("{rotation_deg:.0}")
                                    on:input=move |ev| {
                                        let event = Input::from_event(ev);
                                        if let Some(deg) = event.value::<f32>() {
                                            let rad = deg.to_radians();
                                            let zid = zid_rotation.clone();
                                            update_zone_rotation(zid, rad);
                                        }
                                    }
                                />
                                <div class="flex items-center gap-0.5 shrink-0">
                                    <input
                                        type="number"
                                        min="0" max="360" step="1"
                                        class="w-12 bg-surface-sunken border border-edge-subtle rounded px-1.5 py-0.5
                                               text-[11px] text-fg-primary font-mono tabular-nums text-right
                                               focus:outline-none focus:border-accent-muted glow-ring transition-colors"
                                        prop:value=format!("{rotation_deg:.0}")
                                        on:change=move |ev| {
                                            let event = Change::from_event(ev);
                                            if let Some(deg) = event.value::<f32>() {
                                                let rad = deg.to_radians();
                                                let zid = zid_rotation_input.clone();
                                                update_zone_rotation(zid, rad);
                                            }
                                        }
                                    />
                                    <span class="text-[11px] font-mono text-fg-tertiary/30">{"\u{00b0}"}</span>
                                </div>
                            </div>

                            // Scale
                            <div class="flex items-center gap-1.5 flex-1 min-w-28 rounded-lg px-2 py-1"
                                 style="background: rgba(255, 255, 255, 0.02)">
                                <span class="text-[9px] text-fg-tertiary/40 font-mono uppercase tracking-wider shrink-0">"Scale"</span>
                                <input
                                    type="range"
                                    min="0.5" max="3.0" step="0.1"
                                    class="min-w-0 flex-1"
                                    prop:value=format!("{scale:.1}")
                                    on:input=move |ev| {
                                        let event = Input::from_event(ev);
                                        if let Some(s) = event.value::<f32>() {
                                            let zid = zid_scale.clone();
                                            update_zone(zid, Box::new(move |z| z.scale = s));
                                        }
                                    }
                                />
                                <div class="flex items-center gap-0.5 shrink-0">
                                    <input
                                        type="number"
                                        min="0.5" max="3.0" step="0.1"
                                        class="w-12 bg-surface-sunken border border-edge-subtle rounded px-1.5 py-0.5
                                               text-[11px] text-fg-primary font-mono tabular-nums text-right
                                               focus:outline-none focus:border-accent-muted glow-ring transition-colors"
                                        prop:value=format!("{scale:.1}")
                                        on:change=move |ev| {
                                            let event = Change::from_event(ev);
                                            if let Some(s) = event.value::<f32>() {
                                                let zid = zid_scale_input.clone();
                                                update_zone(zid, Box::new(move |z| z.scale = s));
                                            }
                                        }
                                    />
                                    <span class="text-[11px] font-mono text-fg-tertiary/30">"x"</span>
                                </div>
                            </div>
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

/// Icon-only button for layer ordering controls.
fn layer_icon_button(
    icon: icondata_core::Icon,
    title: &'static str,
    on_click: impl Fn(web_sys::MouseEvent) + 'static,
) -> impl IntoView {
    view! {
        <button
            class="p-1 rounded text-fg-tertiary/35 hover:text-accent transition-colors btn-press"
            title=title
            on:click=on_click
        >
            <Icon icon=icon width="12px" height="12px" />
        </button>
    }
}

/// Icon-only button for group align/distribute/pack/mirror operations.
/// Slightly larger than layer buttons so alignment icons read at a glance.
fn group_op_button(
    icon: icondata_core::Icon,
    title: &'static str,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        <button
            class="p-1 rounded text-fg-tertiary/40 hover:text-accent hover:bg-accent/8 transition-colors btn-press"
            title=title
            on:click=move |_| on_click()
        >
            <Icon icon=icon width="13px" height="13px" />
        </button>
    }
}

/// Inline labeled pixel input for zone position/size properties.
fn zone_pixel_input(
    label: &'static str,
    value_px: f32,
    step: &'static str,
    precision: usize,
    max_px: f32,
    on_change: impl Fn(f32) + Clone + 'static,
) -> impl IntoView {
    let max_str = format!("{max_px:.0}");
    view! {
        <div class="flex items-center gap-1">
            <span class="text-[9px] text-fg-tertiary/40 font-mono w-3">{label}</span>
            <input
                type="number"
                step=step
                min="0"
                max=max_str
                class="w-16 bg-surface-sunken border border-edge-subtle rounded px-1.5 py-0.5
                       text-[11px] text-fg-primary font-mono tabular-nums
                       focus:outline-none focus:border-accent-muted glow-ring transition-colors"
                prop:value=format!("{value_px:.precision$}")
                on:change=move |ev| {
                    let on_change = on_change.clone();
                    let event = Change::from_event(ev);
                    if let Some(v) = event.value::<f32>() {
                        on_change(v);
                    }
                }
            />
        </div>
    }
}

/// Human-readable topology name.
fn topology_name(topology: &hypercolor_types::spatial::LedTopology) -> &'static str {
    match topology {
        hypercolor_types::spatial::LedTopology::Strip { .. } => "Strip",
        hypercolor_types::spatial::LedTopology::Matrix { .. } => "Matrix",
        hypercolor_types::spatial::LedTopology::Ring { .. } => "Ring",
        hypercolor_types::spatial::LedTopology::ConcentricRings { .. } => "Concentric Rings",
        hypercolor_types::spatial::LedTopology::PerimeterLoop { .. } => "Perimeter Loop",
        hypercolor_types::spatial::LedTopology::Point => "Point",
        hypercolor_types::spatial::LedTopology::Custom { .. } => "Custom",
    }
}
