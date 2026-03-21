//! Wiring panel — map physical components (fans, strips, etc.) to device channels.
//! Terminology: "wiring" = section, "component" = individual item, "channel" = device slot.

use leptos::{ev, portal::Portal, prelude::*};
use leptos_icons::Icon;
use leptos_use::{UseEventListenerOptions, use_event_listener_with_options};
use wasm_bindgen::JsCast;

use hypercolor_types::attachment::AttachmentSuggestedZone;
use hypercolor_types::spatial::{
    DeviceZone, LedTopology, NormalizedPosition, Orientation, ZoneAttachment,
};

use crate::api;
use crate::app::DevicesContext;
use crate::components::attachment_editor;
use crate::components::device_card::topology_shape_svg;
use crate::icons::*;
use crate::layout_geometry;
use crate::toasts;

// ── LocalStorage helpers for channel name overrides ─────────────────────────

fn ls_channel_key(device_id: &str, slot_id: &str) -> String {
    format!("hc-channel-name-{device_id}-{slot_id}")
}

fn load_channel_name(device_id: &str, slot_id: &str) -> Option<String> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| {
            s.get_item(&ls_channel_key(device_id, slot_id))
                .ok()
                .flatten()
        })
}

fn save_channel_name(device_id: &str, slot_id: &str, name: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(&ls_channel_key(device_id, slot_id), name);
    }
}

// ── Category shape SVGs ─────────────────────────────────────────────────────

fn category_shape_svg(category: &str, size: u32) -> String {
    let s = size;
    let half = s / 2;
    let r = half.saturating_sub(2).max(3);
    let inner_r = r / 3;
    match category {
        "fan" | "aio" | "ring" | "heatsink" => {
            format!(
                r#"<svg viewBox="0 0 {s} {s}" width="{s}" height="{s}"><circle cx="{half}" cy="{half}" r="{r}" fill="none" stroke="currentColor" stroke-width="1.5" opacity="0.6"/><circle cx="{half}" cy="{half}" r="{inner_r}" fill="currentColor" opacity="0.25"/></svg>"#
            )
        }
        "strip" | "radiator" | "case" => {
            let y = half.saturating_sub(2);
            let w = s.saturating_sub(4);
            format!(
                r#"<svg viewBox="0 0 {s} {s}" width="{s}" height="{s}"><rect x="2" y="{y}" width="{w}" height="5" rx="2" fill="none" stroke="currentColor" stroke-width="1.5" opacity="0.6"/></svg>"#
            )
        }
        "strimer" => {
            let y = half.saturating_sub(3);
            let w = s.saturating_sub(4);
            format!(
                r#"<svg viewBox="0 0 {s} {s}" width="{s}" height="{s}"><rect x="2" y="{y}" width="{w}" height="7" rx="1" fill="none" stroke="currentColor" stroke-width="1.5" opacity="0.6" stroke-dasharray="3 1.5"/></svg>"#
            )
        }
        "matrix" => {
            let p = 3_u32;
            let sz = s.saturating_sub(p * 2);
            format!(
                r#"<svg viewBox="0 0 {s} {s}" width="{s}" height="{s}"><rect x="{p}" y="{p}" width="{sz}" height="{sz}" rx="1" fill="none" stroke="currentColor" stroke-width="1.5" opacity="0.6"/></svg>"#
            )
        }
        _ => {
            format!(
                r#"<svg viewBox="0 0 {s} {s}" width="{s}" height="{s}"><circle cx="{half}" cy="{half}" r="{inner_r}" fill="currentColor" opacity="0.35"/></svg>"#
            )
        }
    }
}

// ── Wiring panel ────────────────────────────────────────────────────────────

/// Wiring panel — configure which components are connected to each channel.
#[component]
pub fn WiringPanel(
    #[prop(into)] device_id: Signal<String>,
    #[prop(into)] device: Signal<Option<api::DeviceSummary>>,
) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();
    let (expanded_slot, set_expanded_slot) = signal(Option::<String>::None);
    let (save_in_flight, set_save_in_flight) = signal(false);
    let (refetch_tick, set_refetch_tick) = signal(0_u32);

    let attachments = LocalResource::new(move || {
        let id = device_id.get();
        refetch_tick.get();
        async move {
            if id.is_empty() {
                return Ok(api::DeviceAttachmentsResponse {
                    device_id: String::new(),
                    device_name: String::new(),
                    slots: Vec::new(),
                    bindings: Vec::new(),
                    suggested_zones: Vec::new(),
                });
            }
            api::fetch_device_attachments(&id).await
        }
    });

    let (templates_tick, set_templates_tick) = signal(0_u32);
    let templates = LocalResource::new(move || {
        templates_tick.get();
        async move {
            api::fetch_attachment_templates(None)
                .await
                .unwrap_or_default()
        }
    });

    view! {
        <div class="rounded-xl bg-surface-raised border border-edge-subtle overflow-visible edge-glow">
            <div class="flex items-center justify-between px-4 py-2.5 border-b border-edge-subtle">
                <div class="flex items-center gap-2">
                    <Icon icon=LuLayers width="12px" height="12px" style="color: rgba(128, 255, 234, 0.7)" />
                    <h3 class="text-[10px] font-mono uppercase tracking-[0.12em] text-fg-tertiary">"Channels"</h3>
                </div>
            </div>

            <div class="p-2">
                <Suspense fallback=|| view! {
                    <div class="text-[10px] text-fg-tertiary animate-pulse py-2 px-1">"Loading..."</div>
                }>
                    {move || {
                        let all_templates = templates.get().map(|loaded| loaded.to_vec()).unwrap_or_default();
                        let did = device_id.get();
                        let device_zones = device.get().map(|d| d.zones.clone()).unwrap_or_default();

                        attachments.get().map(|result| match result {
                            Ok(profile) => {
                                let slots = profile.slots.clone();
                                let bindings = profile.bindings.clone();

                                if slots.is_empty() {
                                    return view! {
                                        <div class="text-[10px] text-fg-tertiary/50 text-center py-2">
                                            "No channels"
                                        </div>
                                    }.into_any();
                                }

                                view! {
                                    <div class="space-y-1">
                                        {slots.into_iter().map(|slot| {
                                            let slot_id = slot.id.clone();
                                            // Match this slot to a device zone for topology info
                                            let zone_match = device_zones.iter()
                                                .find(|z| z.name == slot.name || z.id == slot.id)
                                                .cloned();
                                            let zone_topology_svg = zone_match.as_ref()
                                                .map(|z| topology_shape_svg(&z.topology))
                                                .unwrap_or_else(|| topology_shape_svg("strip"));
                                            let zone_id_for_identify = zone_match.as_ref().map(|z| z.id.clone());
                                            let slot_bindings = bindings
                                                .iter()
                                                .filter(|b| b.slot_id == slot.id)
                                                .cloned()
                                                .collect::<Vec<_>>();
                                            let attachment_count: u32 = slot_bindings.iter().map(|b| b.instances.max(1)).sum();

                                            // Editable channel name (localStorage override)
                                            let default_name = slot.name.clone();
                                            let stored_name = load_channel_name(&did, &slot.id);
                                            let display_name = stored_name.unwrap_or_else(|| default_name.clone());
                                            let (channel_name, set_channel_name) = signal(display_name);
                                            let (editing_channel, set_editing_channel) = signal(false);
                                            let (channel_input, set_channel_input) = signal(String::new());

                                            let save_channel = {
                                                let did = did.clone();
                                                let slot_id = slot_id.clone();
                                                let default_name = default_name.clone();
                                                move || {
                                                    set_editing_channel.set(false);
                                                    let new_name = channel_input.get();
                                                    let name = if new_name.trim().is_empty() || new_name.trim() == default_name {
                                                        default_name.clone()
                                                    } else {
                                                        new_name.trim().to_string()
                                                    };
                                                    save_channel_name(&did, &slot_id, &name);
                                                    set_channel_name.set(name);
                                                }
                                            };

                                            let is_expanded = {
                                                let slot_id = slot_id.clone();
                                                move || expanded_slot.get().as_deref() == Some(&slot_id)
                                            };
                                            let toggle_slot = {
                                                let slot_id = slot_id.clone();
                                                move |_: web_sys::MouseEvent| {
                                                    set_expanded_slot.update(|current| {
                                                        if current.as_deref() == Some(&slot_id) {
                                                            *current = None;
                                                        } else {
                                                            *current = Some(slot_id.clone());
                                                        }
                                                    });
                                                }
                                            };

                                            let initial_rows = attachment_editor::expand_slot_bindings(&slot.id, &bindings);
                                            let initial_rows_store = StoredValue::new(initial_rows.clone());
                                            let (draft_rows, set_draft_rows) = signal(initial_rows.clone());

                                            let slot_categories = slot.suggested_categories.clone();
                                            let relevant_templates = if slot_categories.is_empty() {
                                                all_templates.clone()
                                            } else {
                                                let matching = all_templates.iter()
                                                    .filter(|t| slot_categories.iter().any(|c| c.matches_category(&t.category)))
                                                    .cloned().collect::<Vec<_>>();
                                                let matching_ids = matching.iter().map(|t| t.id.clone()).collect::<std::collections::HashSet<_>>();
                                                let mut ordered = matching;
                                                ordered.extend(all_templates.iter().filter(|t| !matching_ids.contains(&t.id)).cloned());
                                                ordered
                                            };
                                            let category_count = relevant_templates.iter()
                                                .take_while(|t| slot_categories.is_empty() || slot_categories.iter().any(|c| c.matches_category(&t.category)))
                                                .count();
                                            let templates_store = StoredValue::new(relevant_templates.clone());

                                            let draft_summary = Signal::derive({
                                                let slot = slot.clone();
                                                move || templates_store.with_value(|ts| attachment_editor::summarize_slot_rows(&slot, &draft_rows.get(), ts))
                                            });
                                            let draft_is_dirty = Signal::derive(move || initial_rows_store.with_value(|saved| draft_rows.get() != *saved));

                                            let add_row = move |_: web_sys::MouseEvent| {
                                                set_draft_rows.update(|rows| rows.push(attachment_editor::AttachmentDraftRow::empty()));
                                            };

                                            // Collapsed summary shapes
                                            let bound_shapes: Vec<(String, String, u32)> = slot_bindings.iter().map(|b| {
                                                let cat = all_templates.iter().find(|t| t.id == b.template_id)
                                                    .map(|t| t.category.as_str().to_string())
                                                    .unwrap_or_else(|| "other".to_string());
                                                let name = b.name.clone().unwrap_or_else(|| b.template_name.clone());
                                                (cat, name, b.instances)
                                            }).collect();

                                            view! {
                                                <div class="rounded-lg border border-edge-subtle bg-surface-overlay/15 overflow-visible transition-all group/slot">
                                                    // Channel header — topology icon + name + LED count + identify + expand
                                                    <button
                                                        class="w-full px-2.5 py-2 text-left hover:bg-surface-hover/20 transition-colors"
                                                        on:click=toggle_slot
                                                    >
                                                        <div class="flex items-center gap-2">
                                                            // Topology shape icon
                                                            <div class="w-4 h-4 shrink-0" style="color: rgba(128, 255, 234, 0.5)"
                                                                 inner_html=format!(r#"<svg viewBox="0 0 16 16" width="16" height="16">{zone_topology_svg}</svg>"#) />

                                                            // Editable channel name (single-click to edit)
                                                            <div class="flex-1 min-w-0">
                                                                {move || if editing_channel.get() {
                                                                    view! {
                                                                        <input
                                                                            type="text"
                                                                            class="bg-surface-overlay border border-edge-subtle rounded px-1.5 py-0.5 text-[11px] text-fg-primary
                                                                                   focus:outline-none focus:border-accent-muted w-full"
                                                                            prop:value=move || channel_input.get()
                                                                            on:input=move |ev| {
                                                                                let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                                                                if let Some(el) = target { set_channel_input.set(el.value()); }
                                                                            }
                                                                            on:keydown={
                                                                                let save = save_channel.clone();
                                                                                move |ev: web_sys::KeyboardEvent| {
                                                                                    ev.stop_propagation();
                                                                                    if ev.key() == "Enter" { save(); }
                                                                                    if ev.key() == "Escape" { set_editing_channel.set(false); }
                                                                                }
                                                                            }
                                                                            on:blur={
                                                                                let save = save_channel.clone();
                                                                                move |_| save()
                                                                            }
                                                                            on:click=move |ev: web_sys::MouseEvent| ev.stop_propagation()
                                                                        />
                                                                    }.into_any()
                                                                } else {
                                                                    view! {
                                                                        <span
                                                                            class="text-[11px] font-medium text-fg-primary flex items-center gap-1 cursor-text"
                                                                            on:click={
                                                                                move |ev: web_sys::MouseEvent| {
                                                                                    ev.stop_propagation();
                                                                                    set_channel_input.set(channel_name.get_untracked());
                                                                                    set_editing_channel.set(true);
                                                                                }
                                                                            }
                                                                        >
                                                                            {move || channel_name.get()}
                                                                            <span class="w-2.5 h-2.5 text-fg-tertiary/25 shrink-0">
                                                                                <Icon icon=LuPencil width="10px" height="10px" />
                                                                            </span>
                                                                        </span>
                                                                    }.into_any()
                                                                }}
                                                            </div>

                                                            // LED count
                                                            <span class="text-[9px] font-mono text-fg-tertiary/40 tabular-nums shrink-0">
                                                                {slot.led_count} " LEDs"
                                                            </span>

                                                            // Identify channel button
                                                            {zone_id_for_identify.clone().map(|zid| {
                                                                let dev_id = did.clone();
                                                                view! {
                                                                    <button
                                                                        class="w-5 h-5 flex items-center justify-center rounded shrink-0
                                                                               opacity-0 group-hover/slot:opacity-100 transition-opacity
                                                                               text-fg-tertiary/40 hover:text-accent btn-press"
                                                                        title="Identify channel"
                                                                        on:click={
                                                                            let dev_id = dev_id.clone();
                                                                            let zid = zid.clone();
                                                                            move |ev: web_sys::MouseEvent| {
                                                                                ev.stop_propagation();
                                                                                let did = dev_id.clone();
                                                                                let zid = zid.clone();
                                                                                leptos::task::spawn_local(async move {
                                                                                    if let Err(e) = api::identify_zone(&did, &zid).await {
                                                                                        toasts::toast_error(&format!("Identify failed: {e}"));
                                                                                    } else {
                                                                                        toasts::toast_success("Flashing channel");
                                                                                    }
                                                                                });
                                                                            }
                                                                        }
                                                                    >
                                                                        <Icon icon=LuZap width="10px" height="10px" />
                                                                    </button>
                                                                }
                                                            })}

                                                            // Expand/collapse chevron
                                                            <div class="shrink-0">
                                                                {if is_expanded() {
                                                                    view! { <Icon icon=LuChevronUp width="11px" height="11px" style="color: rgba(139, 133, 160, 0.5)" /> }.into_any()
                                                                } else {
                                                                    view! { <Icon icon=LuChevronDown width="11px" height="11px" style="color: rgba(139, 133, 160, 0.5)" /> }.into_any()
                                                                }}
                                                            </div>
                                                        </div>

                                                        // Collapsed: attached component shapes
                                                        {(!is_expanded() && attachment_count > 0).then(|| {
                                                            let shapes = bound_shapes.clone();
                                                            view! {
                                                                <div class="flex items-center gap-2 mt-1 pl-6" style="color: rgba(128, 255, 234, 0.5)">
                                                                    {shapes.into_iter().map(|(cat, name, instances)| {
                                                                        let svg = category_shape_svg(&cat, 16);
                                                                        let label = if instances > 1 { format!("{name} \u{00d7}{instances}") } else { name };
                                                                        view! {
                                                                            <div class="flex items-center gap-1">
                                                                                <div class="w-4 h-4 shrink-0" inner_html=svg />
                                                                                <span class="text-[10px] text-fg-tertiary/60">{label}</span>
                                                                            </div>
                                                                        }
                                                                    }).collect_view()}
                                                                </div>
                                                            }
                                                        })}
                                                    </button>

                                                    // Expanded: component rows with searchable picker
                                                    {
                                                    let slot_for_save = StoredValue::new(slot.clone());
                                                    let slot_id_for_save = StoredValue::new(slot_id.clone());
                                                    let did_for_identify = StoredValue::new(did.clone());
                                                    let slot_id_for_identify = StoredValue::new(slot_id.clone());
                                                    view! {
                                                    <Show when=is_expanded>
                                                        <div class="px-2.5 pb-2 space-y-1.5 border-t border-edge-subtle bg-surface-sunken/20 animate-fade-in">
                                                            <div class="pt-1.5 space-y-1">
                                                                {move || {
                                                                    let rows = draft_rows.get();
                                                                    let summary = draft_summary.get();

                                                                    if rows.is_empty() {
                                                                        return view! {
                                                                            <div class="text-[10px] text-fg-tertiary/40 text-center py-1.5">"No components"</div>
                                                                        }.into_any();
                                                                    }

                                                                    rows.into_iter().enumerate().map(|(index, row)| {
                                                                        let placement = summary.rows.get(index).cloned().flatten();
                                                                        let template_info = templates_store.with_value(|ts| ts.iter().find(|t| t.id == row.template_id).cloned());
                                                                        let options = templates_store.with_value(|ts| ts.clone());
                                                                        let cat_str = template_info.as_ref().map(|t| t.category.as_str().to_string()).unwrap_or_else(|| "other".to_string());
                                                                        let shape_svg = category_shape_svg(&cat_str, 18);
                                                                        let is_user_template = template_info.as_ref()
                                                                            .map(|t| t.origin == Some(hypercolor_types::attachment::AttachmentOrigin::User))
                                                                            .unwrap_or(false);
                                                                        let template_led_count = template_info.as_ref().map(|t| t.led_count).unwrap_or(0);
                                                                        let template_id_for_edit = row.template_id.clone();
                                                                        let selected_name = template_info.as_ref()
                                                                            .map(|t| format!("{} \u{2014} {} LEDs", t.name, t.led_count))
                                                                            .unwrap_or_default();

                                                                        view! {
                                                                            <div class="flex items-center gap-1.5 group/row">
                                                                                <div class="w-4.5 h-4.5 shrink-0" style="color: rgba(128, 255, 234, 0.4)" inner_html=shape_svg />

                                                                                <ComponentCombobox
                                                                                    components=options
                                                                                    selected_id=row.template_id.clone()
                                                                                    selected_label=selected_name
                                                                                    category_count=category_count
                                                                                    on_select=Callback::new({
                                                                                        let set_rows = set_draft_rows;
                                                                                        move |value: String| {
                                                                                            set_rows.update(|rows| {
                                                                                                if let Some(row) = rows.get_mut(index) {
                                                                                                    row.template_id = value;
                                                                                                }
                                                                                            });
                                                                                        }
                                                                                    })
                                                                                    on_refresh_templates=Callback::new(move |()| {
                                                                                        set_templates_tick.update(|t| *t += 1);
                                                                                    })
                                                                                />

                                                                                // LED count: editable for user templates, static for built-in
                                                                                {if is_user_template {
                                                                                    let tid = template_id_for_edit.clone();
                                                                                    view! {
                                                                                        <input
                                                                                            type="number" min="1" max="2000"
                                                                                            class="w-12 bg-surface-base/40 border border-edge-subtle rounded px-1 py-0.5
                                                                                                   text-[9px] font-mono tabular-nums text-neon-cyan/70
                                                                                                   focus:outline-none focus:border-neon-cyan/30 shrink-0 text-right"
                                                                                            title="Edit LED count"
                                                                                            prop:value=template_led_count.to_string()
                                                                                            on:change={
                                                                                                let tid = tid.clone();
                                                                                                move |ev| {
                                                                                                    let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                                                                                    if let Some(el) = target {
                                                                                                        if let Ok(new_count) = el.value().parse::<u32>() {
                                                                                                            let new_count = new_count.clamp(1, 2000);
                                                                                                            let tid = tid.clone();
                                                                                                            leptos::task::spawn_local(async move {
                                                                                                                match api::fetch_attachment_template(&tid).await {
                                                                                                                    Ok(mut tmpl) => {
                                                                                                                        // Update the topology's LED count
                                                                                                                        tmpl.topology = match tmpl.topology {
                                                                                                                            hypercolor_types::spatial::LedTopology::Strip { direction, .. } =>
                                                                                                                                hypercolor_types::spatial::LedTopology::Strip { count: new_count, direction },
                                                                                                                            hypercolor_types::spatial::LedTopology::Matrix { serpentine, start_corner, .. } =>
                                                                                                                                // For matrix, interpret new_count as total and keep aspect
                                                                                                                                hypercolor_types::spatial::LedTopology::Matrix {
                                                                                                                                    width: new_count, height: 1, serpentine, start_corner,
                                                                                                                                },
                                                                                                                            other => other,
                                                                                                                        };
                                                                                                                        tmpl.name = format!("Custom Strip - {new_count} LEDs");
                                                                                                                        if let Err(e) = api::update_attachment_template(&tmpl).await {
                                                                                                                            toasts::toast_error(&format!("Update failed: {e}"));
                                                                                                                        } else {
                                                                                                                            set_templates_tick.update(|t| *t += 1);
                                                                                                                        }
                                                                                                                    }
                                                                                                                    Err(e) => toasts::toast_error(&format!("Fetch failed: {e}")),
                                                                                                                }
                                                                                                            });
                                                                                                        }
                                                                                                    }
                                                                                                }
                                                                                            }
                                                                                            on:click=move |ev: web_sys::MouseEvent| ev.stop_propagation()
                                                                                        />
                                                                                    }.into_any()
                                                                                } else {
                                                                                    placement.map(|p| view! {
                                                                                        <span class="text-[8px] font-mono text-fg-tertiary/30 tabular-nums shrink-0 w-10 text-right">
                                                                                            {p.led_offset} "-" {p.led_end.saturating_sub(1)}
                                                                                        </span>
                                                                                    }).into_any()
                                                                                }}

                                                                                // Identify component
                                                                                <button
                                                                                    class="w-4 h-4 flex items-center justify-center rounded shrink-0
                                                                                           opacity-0 group-hover/row:opacity-100 transition-opacity
                                                                                           text-fg-tertiary/40 hover:text-accent btn-press"
                                                                                    title="Identify component"
                                                                                    on:click={
                                                                                        let did = did_for_identify.get_value();
                                                                                        let sid = slot_id_for_identify.get_value();
                                                                                        move |ev: web_sys::MouseEvent| {
                                                                                            ev.stop_propagation();
                                                                                            let did = did.clone();
                                                                                            let sid = sid.clone();
                                                                                            leptos::task::spawn_local(async move {
                                                                                                if let Err(e) = api::identify_attachment(&did, &sid, Some(index)).await {
                                                                                                    toasts::toast_error(&format!("Identify failed: {e}"));
                                                                                                } else {
                                                                                                    toasts::toast_success("Flashing component");
                                                                                                }
                                                                                            });
                                                                                        }
                                                                                    }
                                                                                >
                                                                                    <Icon icon=LuZap width="9px" height="9px" />
                                                                                </button>

                                                                                // Delete component
                                                                                <button
                                                                                    class="w-4 h-4 flex items-center justify-center rounded shrink-0
                                                                                           opacity-0 group-hover/row:opacity-100 transition-opacity
                                                                                           text-fg-tertiary/40 hover:text-error-red"
                                                                                    on:click={
                                                                                        let set_rows = set_draft_rows;
                                                                                        move |ev: web_sys::MouseEvent| {
                                                                                            ev.stop_propagation();
                                                                                            set_rows.update(|rows| { if index < rows.len() { rows.remove(index); } });
                                                                                        }
                                                                                    }
                                                                                >
                                                                                    <Icon icon=LuX width="9px" height="9px" />
                                                                                </button>
                                                                            </div>
                                                                        }
                                                                    }).collect_view().into_any()
                                                                }}
                                                            </div>

                                                            <div class="flex items-center justify-between">
                                                                <button
                                                                    class="text-[9px] font-medium px-1.5 py-0.5 rounded transition-all btn-press flex items-center gap-1"
                                                                    style="color: rgba(128, 255, 234, 0.6)"
                                                                    on:click=add_row
                                                                >
                                                                    <Icon icon=LuPlus width="9px" height="9px" />
                                                                    "Add"
                                                                </button>
                                                                <div class="flex items-center gap-2">
                                                                    {move || {
                                                                        let summary = draft_summary.get();
                                                                        (summary.overflow_leds > 0).then(|| view! {
                                                                            <span class="text-[9px] font-mono" style="color: rgb(255, 99, 99)">{summary.overflow_leds} " over"</span>
                                                                        })
                                                                    }}
                                                                    <Show when=move || draft_is_dirty.get()>
                                                                        <button
                                                                            class="text-[9px] font-medium px-1.5 py-0.5 rounded transition-all btn-press disabled:opacity-30"
                                                                            style="color: rgba(128, 255, 234, 0.7); background: rgba(128, 255, 234, 0.08)"
                                                                            disabled=move || { let s = draft_summary.get(); save_in_flight.get() || !s.is_valid() }
                                                                            on:click={
                                                                                move |_: web_sys::MouseEvent| {
                                                                                let slot = slot_for_save.get_value();
                                                                                let slot_id = slot_id_for_save.get_value();
                                                                                let packed_rows = match templates_store.with_value(|ts| {
                                                                                    attachment_editor::pack_slot_rows(&slot, &draft_rows.get_untracked(), ts)
                                                                                }) {
                                                                                    Ok(r) => r,
                                                                                    Err(e) => { toasts::toast_error(&e); return; }
                                                                                };
                                                                                set_save_in_flight.set(true);
                                                                                let did = device_id.get_untracked();
                                                                                let slot_id = slot_id.clone();
                                                                                let layouts_resource = ctx.layouts_resource;
                                                                                let device_for_sync = device.get_untracked();
                                                                                leptos::task::spawn_local(async move {
                                                                                    let result = async {
                                                                                        let current = api::fetch_device_attachments(&did).await?;
                                                                                        let mut api_bindings = current.bindings.iter()
                                                                                            .filter(|b| b.slot_id != slot_id)
                                                                                            .map(|b| api::AttachmentBindingRequest {
                                                                                                slot_id: b.slot_id.clone(), template_id: b.template_id.clone(),
                                                                                                name: b.name.clone(), enabled: b.enabled, instances: b.instances, led_offset: b.led_offset,
                                                                                            }).collect::<Vec<_>>();
                                                                                        api_bindings.extend(packed_rows.into_iter().map(|b| api::AttachmentBindingRequest {
                                                                                            slot_id: slot_id.clone(), template_id: b.template_id,
                                                                                            name: b.name, enabled: true, instances: 1, led_offset: b.led_offset,
                                                                                        }));
                                                                                        api::update_device_attachments(&did, &api::UpdateAttachmentsRequest { bindings: api_bindings }).await
                                                                                    }.await;
                                                                                    set_save_in_flight.set(false);
                                                                                    match result {
                                                                                        Ok(updated) => {
                                                                                            let saved = attachment_editor::expand_slot_bindings(&slot_id, &updated.bindings);
                                                                                            initial_rows_store.set_value(saved.clone());
                                                                                            set_draft_rows.set(saved);
                                                                                            toasts::toast_success("Saved");
                                                                                            set_refetch_tick.update(|t| *t += 1);
                                                                                            if let Some(dev) = device_for_sync {
                                                                                                if !updated.suggested_zones.is_empty() {
                                                                                                    sync_wiring_to_layout(dev, updated.suggested_zones, layouts_resource);
                                                                                                }
                                                                                            }
                                                                                        }
                                                                                        Err(e) => toasts::toast_error(&format!("Save failed: {e}")),
                                                                                    }
                                                                                });
                                                                            }}
                                                                        >
                                                                            <Icon icon=LuCheck width="9px" height="9px" style="color: inherit" />
                                                                            " Save"
                                                                        </button>
                                                                    </Show>
                                                                </div>
                                                            </div>
                                                        </div>
                                                    </Show>
                                                    }}
                                                </div>
                                            }
                                        }).collect_view()}
                                    </div>
                                }.into_any()
                            }
                            Err(error) => view! {
                                <div class="text-[10px] text-error-red py-2">{error}</div>
                            }.into_any(),
                        })
                    }}
                </Suspense>
            </div>
        </div>
    }
}

// ── Searchable component combobox ───────────────────────────────────────────

fn install_component_combobox_outside_handler(set_open: WriteSignal<bool>) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };

    // Use BUBBLE phase (not capture) so the panel's own stop_propagation()
    // prevents this handler from firing when clicking inside the portaled panel.
    let _ = use_event_listener_with_options(
        doc,
        ev::mousedown,
        move |ev: leptos::ev::MouseEvent| {
            let inside = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .map(|el| {
                    el.closest(".component-combobox, .component-combobox-panel")
                        .ok()
                        .flatten()
                        .is_some()
                })
                .unwrap_or(false);

            if !inside {
                set_open.set(false);
            }
        },
        UseEventListenerOptions::default(),
    );
}

// NOTE: Scroll-close handler intentionally removed. It was the root cause of the
// dropdown repeatedly breaking — it listened for ANY scroll event on the window
// in capture phase, which fires on sidebar scroll, dropdown internal scroll,
// layout reflows from slot expansion, and autofocus adjustments. The outside-click
// handler above is sufficient for dismissal.

fn component_dropdown_panel_style(trigger: Option<web_sys::HtmlButtonElement>) -> String {
    trigger
        .map(|el| {
            let rect = el.get_bounding_client_rect();
            let Some(window) = web_sys::window() else {
                return String::new();
            };

            let viewport_width = window
                .inner_width()
                .ok()
                .and_then(|value| value.as_f64())
                .unwrap_or(rect.right());
            let viewport_height = window
                .inner_height()
                .ok()
                .and_then(|value| value.as_f64())
                .unwrap_or(rect.bottom());

            let horizontal_margin = 12.0;
            let vertical_margin = 12.0;
            let desired_max_height = 340.0;
            let width = rect.width().max(300.0);
            let max_left = (viewport_width - width - horizontal_margin).max(horizontal_margin);
            let left = rect.left().clamp(horizontal_margin, max_left);
            let available_below = (viewport_height - rect.bottom() - vertical_margin).max(0.0);
            let available_above = (rect.top() - vertical_margin).max(0.0);
            let open_upward = available_below < 180.0 && available_above > available_below;
            let max_height = if open_upward {
                available_above
            } else {
                available_below
            }
            .min(desired_max_height)
            .max(120.0);

            if open_upward {
                let bottom = (viewport_height - rect.top() + 4.0).max(vertical_margin);
                format!(
                    "left: {left}px; bottom: {bottom}px; width: {width}px; max-height: {max_height}px; z-index: 9999"
                )
            } else {
                let top = (rect.bottom() + 4.0).max(vertical_margin);
                format!(
                    "top: {top}px; left: {left}px; width: {width}px; max-height: {max_height}px; z-index: 9999"
                )
            }
        })
        .unwrap_or_default()
}

/// Searchable dropdown for selecting a component (fan, strip, etc.).
/// Uses `position: fixed` to escape overflow clipping from parent containers.
#[component]
fn ComponentCombobox(
    components: Vec<api::TemplateSummary>,
    selected_id: String,
    selected_label: String,
    category_count: usize,
    #[prop(into)] on_select: Callback<String>,
    #[prop(into)] on_refresh_templates: Callback<()>,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let (search, set_search) = signal(String::new());
    let (creating, set_creating) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Button>::new();
    let components_store = StoredValue::new(components);
    let has_selection = !selected_id.is_empty();

    install_component_combobox_outside_handler(set_open);

    let filtered = Memo::new(move |_| {
        let term = search.get().to_lowercase();
        components_store.with_value(|components| {
            let mut results: Vec<_> = if term.is_empty() {
                components.clone()
            } else {
                components
                    .iter()
                    .filter(|t| {
                        t.name.to_lowercase().contains(&term)
                            || t.vendor.to_lowercase().contains(&term)
                            || t.category.as_str().to_lowercase().contains(&term)
                            || t.description.to_lowercase().contains(&term)
                            || t.tags.iter().any(|tag| tag.to_lowercase().contains(&term))
                    })
                    .cloned()
                    .collect()
            };
            // Sort alphabetically within vendor groups (Generic first, then by name)
            results.sort_by(|a, b| {
                let a_generic = a.vendor.to_lowercase() == "generic" || a.vendor.to_lowercase() == "custom";
                let b_generic = b.vendor.to_lowercase() == "generic" || b.vendor.to_lowercase() == "custom";
                b_generic.cmp(&a_generic)
                    .then_with(|| a.vendor.cmp(&b.vendor))
                    .then_with(|| a.name.cmp(&b.name))
            });
            results
        })
    });

    let open_dropdown = move |ev: web_sys::MouseEvent| {
        ev.stop_propagation();
        if open.get_untracked() {
            set_open.set(false);
            return;
        }
        set_search.set(String::new());
        set_open.set(true);
    };

    view! {
        <div class="component-combobox flex-1 min-w-0 relative">
            <button
                type="button"
                node_ref=trigger_ref
                class="w-full flex items-center gap-1 px-2 py-1 rounded border text-left text-[11px] transition-all min-w-0"
                style=move || {
                    if open.get() {
                        "background: rgba(128, 255, 234, 0.06); border-color: rgba(128, 255, 234, 0.3); color: var(--text-primary)"
                    } else if has_selection {
                        "background: rgba(255, 255, 255, 0.03); border-color: rgba(139, 133, 160, 0.15); color: var(--text-primary)"
                    } else {
                        "background: rgba(255, 255, 255, 0.02); border-color: rgba(139, 133, 160, 0.1); color: rgba(139, 133, 160, 0.5)"
                    }
                }
                on:click=open_dropdown
            >
                <span class="flex-1 min-w-0" style="overflow: hidden; text-overflow: ellipsis; white-space: nowrap">
                    {if has_selection { selected_label.clone() } else { "Select component...".to_string() }}
                </span>
                <span class="w-3 h-3 shrink-0 transition-transform duration-200 flex items-center justify-center"
                      class:rotate-180=move || open.get()>
                    <Icon icon=LuChevronDown width="10px" height="10px" style="color: rgba(139, 133, 160, 0.4)" />
                </span>
            </button>

            // Fixed-position dropdown (escapes overflow clipping)
            {move || open.get().then(|| {
                let cat_count = category_count;

                view! {
                    <Portal>
                        <div
                            class="component-combobox-panel fixed flex flex-col rounded-xl border border-edge-subtle
                                   bg-surface-overlay shadow-xl dropdown-glow animate-fade-in overflow-hidden"
                            style=move || component_dropdown_panel_style(trigger_ref.get())
                            on:mousedown=|ev: leptos::ev::MouseEvent| ev.stop_propagation()
                        >
                            <div class="p-1.5 border-b border-edge-subtle">
                                <div class="relative">
                                    <span class="absolute left-2 top-1/2 -translate-y-1/2 pointer-events-none text-fg-tertiary/40">
                                        <Icon icon=LuSearch width="11px" height="11px" />
                                    </span>
                                    <input
                                        type="text"
                                        placeholder="Search components..."
                                        class="w-full bg-surface-base/60 border border-edge-subtle rounded-lg pl-6 pr-2 py-1
                                               text-[11px] text-fg-primary placeholder-fg-tertiary/40
                                               focus:outline-none focus:border-accent-muted search-glow"
                                        prop:value=move || search.get()
                                        on:input=move |ev| {
                                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                            if let Some(el) = target { set_search.set(el.value()); }
                                        }
                                        on:click=move |ev| ev.stop_propagation()
                                    />
                                </div>
                            </div>

                            <div class="flex-1 overflow-y-auto scrollbar-dropdown">
                                {move || {
                                    let results = filtered.get();
                                    let is_searching = !search.get().is_empty();
                                    let show_custom_at_top = !is_searching && (cat_count == 0 || results.is_empty());

                                    // Helper to render a template option
                                    let render_option = |t: api::TemplateSummary| {
                                        let svg = category_shape_svg(t.category.as_str(), 16);
                                        view! { <ComponentOption component=t shape_svg=svg on_click=Callback::new({
                                            let on_select = on_select; let set_open = set_open;
                                            move |id: String| { on_select.run(id); set_open.set(false); }
                                        }) /> }
                                    };

                                    // Custom creation buttons (appear as list items)
                                    let custom_items = move || {
                                        let is_creating = creating.get();
                                        view! {
                                            <button
                                                type="button"
                                                class="w-full flex items-center gap-2 px-2 py-1.5 mx-1 rounded-lg
                                                       hover:bg-neon-cyan/5 transition-colors text-left disabled:opacity-30"
                                                style="width: calc(100% - 8px); color: rgb(128, 255, 234)"
                                                disabled=is_creating
                                                on:click={
                                                    let on_select = on_select;
                                                    let on_refresh = on_refresh_templates;
                                                    move |ev: web_sys::MouseEvent| {
                                                        ev.stop_propagation();
                                                        set_creating.set(true);
                                                        let id = format!("custom-strip-60-{}", js_sys::Date::now() as u64);
                                                        let template = hypercolor_types::attachment::AttachmentTemplate {
                                                            id: id.clone(),
                                                            name: "Custom Strip - 60 LEDs".to_string(),
                                                            category: hypercolor_types::attachment::AttachmentCategory::Strip,
                                                            origin: hypercolor_types::attachment::AttachmentOrigin::User,
                                                            description: "Custom LED strip, 60 LEDs".to_string(),
                                                            vendor: "Custom".to_string(),
                                                            default_size: hypercolor_types::attachment::AttachmentCanvasSize { width: 0.24, height: 0.06 },
                                                            topology: hypercolor_types::spatial::LedTopology::Strip {
                                                                count: 60,
                                                                direction: hypercolor_types::spatial::StripDirection::LeftToRight,
                                                            },
                                                            compatible_slots: Vec::new(),
                                                            tags: vec!["custom".to_string(), "strip".to_string()],
                                                            led_names: None, led_mapping: None, image_url: None, physical_size_mm: None,
                                                        };
                                                        leptos::task::spawn_local(async move {
                                                            match api::create_attachment_template(&template).await {
                                                                Ok(_) => {
                                                                    on_refresh.run(());
                                                                    on_select.run(id);
                                                                    set_open.set(false);
                                                                    toasts::toast_success("Custom strip created \u{2014} edit LED count in the row");
                                                                }
                                                                Err(e) => toasts::toast_error(&format!("Create failed: {e}")),
                                                            }
                                                            set_creating.set(false);
                                                        });
                                                    }
                                                }
                                            >
                                                <div class="w-4 h-4 shrink-0 flex items-center justify-center">
                                                    <Icon icon=LuPlus width="12px" height="12px" />
                                                </div>
                                                <div class="flex-1 min-w-0">
                                                    <div class="text-[11px] font-medium leading-tight">"New Custom Strip"</div>
                                                    <div class="text-[9px] text-neon-cyan/40">"Define your own LED count"</div>
                                                </div>
                                            </button>
                                            <button
                                                type="button"
                                                class="w-full flex items-center gap-2 px-2 py-1.5 mx-1 rounded-lg
                                                       hover:bg-neon-cyan/5 transition-colors text-left disabled:opacity-30"
                                                style="width: calc(100% - 8px); color: rgb(128, 255, 234)"
                                                disabled=is_creating
                                                on:click={
                                                    let on_select = on_select;
                                                    let on_refresh = on_refresh_templates;
                                                    move |ev: web_sys::MouseEvent| {
                                                        ev.stop_propagation();
                                                        set_creating.set(true);
                                                        let id = format!("custom-matrix-8x8-{}", js_sys::Date::now() as u64);
                                                        let template = hypercolor_types::attachment::AttachmentTemplate {
                                                            id: id.clone(),
                                                            name: "Custom Matrix - 8\u{00d7}8".to_string(),
                                                            category: hypercolor_types::attachment::AttachmentCategory::Matrix,
                                                            origin: hypercolor_types::attachment::AttachmentOrigin::User,
                                                            description: "Custom 8\u{00d7}8 LED matrix, 64 LEDs".to_string(),
                                                            vendor: "Custom".to_string(),
                                                            default_size: hypercolor_types::attachment::AttachmentCanvasSize { width: 0.24, height: 0.24 },
                                                            topology: hypercolor_types::spatial::LedTopology::Matrix {
                                                                width: 8, height: 8, serpentine: true,
                                                                start_corner: hypercolor_types::spatial::Corner::TopLeft,
                                                            },
                                                            compatible_slots: Vec::new(),
                                                            tags: vec!["custom".to_string(), "matrix".to_string()],
                                                            led_names: None, led_mapping: None, image_url: None, physical_size_mm: None,
                                                        };
                                                        leptos::task::spawn_local(async move {
                                                            match api::create_attachment_template(&template).await {
                                                                Ok(_) => {
                                                                    on_refresh.run(());
                                                                    on_select.run(id);
                                                                    set_open.set(false);
                                                                    toasts::toast_success("Custom matrix created \u{2014} edit dimensions in the row");
                                                                }
                                                                Err(e) => toasts::toast_error(&format!("Create failed: {e}")),
                                                            }
                                                            set_creating.set(false);
                                                        });
                                                    }
                                                }
                                            >
                                                <div class="w-4 h-4 shrink-0 flex items-center justify-center">
                                                    <Icon icon=LuPlus width="12px" height="12px" />
                                                </div>
                                                <div class="flex-1 min-w-0">
                                                    <div class="text-[11px] font-medium leading-tight">"New Custom Matrix"</div>
                                                    <div class="text-[9px] text-neon-cyan/40">"Define rows \u{00d7} columns"</div>
                                                </div>
                                            </button>
                                        }
                                    };

                                    if results.is_empty() && !show_custom_at_top {
                                        return view! {
                                            <div class="px-3 py-4 text-center text-[10px] text-fg-tertiary/40">"No components found"</div>
                                        }.into_any();
                                    }

                                    if show_custom_at_top {
                                        // No suggestions — custom creation first, then all templates
                                        view! {
                                            <div class="px-2 pt-1.5 pb-0.5">
                                                <div class="text-[9px] font-mono uppercase tracking-wider text-neon-cyan/40 px-1">"Create"</div>
                                            </div>
                                            {custom_items()}
                                            {(!results.is_empty()).then(|| view! {
                                                <div class="h-px bg-border-subtle/20 mx-2 my-1" />
                                                <div class="px-2 pt-0.5 pb-0.5">
                                                    <div class="text-[9px] font-mono uppercase tracking-wider text-fg-tertiary/25 px-1">"Templates"</div>
                                                </div>
                                                {results.clone().into_iter().map(render_option).collect_view()}
                                            })}
                                        }.into_any()
                                    } else if is_searching || cat_count >= results.len() {
                                        // Searching or all are suggestions — flat sorted list
                                        results.into_iter().map(render_option).collect_view().into_any()
                                    } else {
                                        // Has suggestions — show suggested first, then create, then all
                                        let suggested = results[..cat_count].to_vec();
                                        let others = results[cat_count..].to_vec();
                                        view! {
                                            <div class="px-2 pt-1.5 pb-0.5">
                                                <div class="text-[9px] font-mono uppercase tracking-wider text-fg-tertiary/35 px-1">"Suggested"</div>
                                            </div>
                                            {suggested.into_iter().map(render_option).collect_view()}
                                            <div class="h-px bg-border-subtle/20 mx-2 my-0.5" />
                                            <div class="px-2 pt-1 pb-0.5">
                                                <div class="text-[9px] font-mono uppercase tracking-wider text-neon-cyan/30 px-1">"Create"</div>
                                            </div>
                                            {custom_items()}
                                            <div class="h-px bg-border-subtle/20 mx-2 my-0.5" />
                                            <div class="px-2 pt-1 pb-0.5">
                                                <div class="text-[9px] font-mono uppercase tracking-wider text-fg-tertiary/25 px-1">"All"</div>
                                            </div>
                                            {others.into_iter().map(render_option).collect_view()}
                                        }.into_any()
                                    }
                                }}
                            </div>

                            {has_selection.then(|| view! {
                                <div class="border-t border-edge-subtle">
                                    <button
                                        type="button"
                                        class="w-full px-3 py-1 text-[10px] text-fg-tertiary/50 hover:text-fg-tertiary hover:bg-surface-hover/30 transition-colors text-left"
                                        on:click=move |_| { on_select.run(String::new()); set_open.set(false); }
                                    >
                                        "Clear"
                                    </button>
                                </div>
                            })}
                        </div>
                    </Portal>
                }
            })}
        </div>
    }
}

/// Single component option row.
#[component]
fn ComponentOption(
    component: api::TemplateSummary,
    shape_svg: String,
    #[prop(into)] on_click: Callback<String>,
) -> impl IntoView {
    let tid = component.id.clone();
    let name = component.name.clone();
    let vendor = component.vendor.clone();
    let led_count = component.led_count;
    let category = component.category.as_str().to_string();

    view! {
        <button
            type="button"
            class="w-full flex items-center gap-2 px-2 py-1 mx-1 rounded-lg
                   hover:bg-surface-hover/40 transition-colors text-left"
            style="width: calc(100% - 8px)"
            on:click=move |ev| { ev.stop_propagation(); on_click.run(tid.clone()); }
        >
            <div class="w-4 h-4 shrink-0 flex items-center justify-center" style="color: rgba(128, 255, 234, 0.4)" inner_html=shape_svg />
            <div class="flex-1 min-w-0">
                <div class="text-[11px] text-fg-primary leading-tight">{name}</div>
                <div class="text-[9px] text-fg-tertiary/40">
                    {vendor} " \u{b7} " <span class="capitalize">{category}</span>
                </div>
            </div>
            <span class="text-[9px] font-mono tabular-nums text-fg-tertiary/40 shrink-0 px-1 py-0.5 rounded bg-surface-overlay/20">
                {led_count}
            </span>
        </button>
    }
}

// ── Auto-sync wiring to layout ──────────────────────────────────────────────

fn sync_wiring_to_layout(
    device: api::DeviceSummary,
    suggested_zones: Vec<AttachmentSuggestedZone>,
    layouts_resource: LocalResource<Result<Vec<api::LayoutSummary>, String>>,
) {
    leptos::task::spawn_local(async move {
        let result: Result<usize, String> = async {
            let mut layout = api::fetch_active_layout().await?;
            let layout_id = layout.id.clone();
            let imported_zones = build_attachment_layout_zones(&device, &suggested_zones);
            let imported_count = imported_zones.len();
            layout
                .zones
                .retain(|z| !(z.device_id == device.layout_device_id && z.attachment.is_some()));
            layout.zones.extend(imported_zones);
            let req = api::UpdateLayoutApiRequest {
                name: None,
                description: None,
                canvas_width: None,
                canvas_height: None,
                zones: Some(layout.zones),
                groups: None,
            };
            api::update_layout(&layout_id, &req).await?;
            api::apply_layout(&layout_id).await?;
            Ok(imported_count)
        }
        .await;

        if let Ok(count) = result {
            if count > 0 {
                layouts_resource.refetch();
                let noun = if count == 1 { "zone" } else { "zones" };
                toasts::toast_info(&format!("Layout synced ({count} {noun})"));
            }
        }
    });
}

#[allow(clippy::cast_precision_loss)]
pub fn build_attachment_layout_zones(
    device: &api::DeviceSummary,
    suggested_zones: &[AttachmentSuggestedZone],
) -> Vec<DeviceZone> {
    if suggested_zones.is_empty() {
        return Vec::new();
    }

    let columns = suggested_zones.len().clamp(1, 3);
    let rows = suggested_zones.len().div_ceil(columns);
    let cell_width = 0.78 / columns as f32;
    let cell_height = 0.68 / rows as f32;

    suggested_zones
        .iter()
        .enumerate()
        .map(|(index, suggested)| {
            let row = index / columns;
            let column = index % columns;
            let position = NormalizedPosition::new(
                0.12 + cell_width * (column as f32 + 0.5),
                0.18 + cell_height * (row as f32 + 0.5),
            );
            let max_size = NormalizedPosition::new(cell_width * 0.82, cell_height * 0.82);
            let shape = layout_geometry::attachment_zone_shape(&suggested.category);
            let size = layout_geometry::normalize_zone_size_for_editor(
                position,
                layout_geometry::attachment_zone_size(suggested, max_size),
                &suggested.topology,
            );

            DeviceZone {
                id: attachment_zone_id(&device.layout_device_id, suggested),
                name: suggested.name.clone(),
                device_id: device.layout_device_id.clone(),
                zone_name: Some(suggested.slot_id.clone()),
                group_id: None,
                position,
                size,
                rotation: 0.0,
                scale: 1.0,
                orientation: if matches!(shape, Some(hypercolor_types::spatial::ZoneShape::Ring)) {
                    Some(Orientation::Radial)
                } else {
                    orientation_for_topology(&suggested.topology)
                },
                topology: suggested.topology.clone(),
                led_positions: Vec::new(),
                led_mapping: suggested.led_mapping.clone(),
                sampling_mode: None,
                edge_behavior: None,
                shape,
                shape_preset: None,
                display_order: 0,
                attachment: Some(ZoneAttachment {
                    template_id: suggested.template_id.clone(),
                    slot_id: suggested.slot_id.clone(),
                    instance: suggested.instance,
                    led_start: Some(suggested.led_start),
                    led_count: Some(suggested.led_count),
                    led_mapping: suggested.led_mapping.clone(),
                }),
            }
        })
        .collect()
}

fn orientation_for_topology(topology: &LedTopology) -> Option<Orientation> {
    match topology {
        LedTopology::Strip { .. } => Some(Orientation::Horizontal),
        LedTopology::Ring { .. } | LedTopology::ConcentricRings { .. } | LedTopology::Point => {
            Some(Orientation::Radial)
        }
        LedTopology::Matrix { .. }
        | LedTopology::PerimeterLoop { .. }
        | LedTopology::Custom { .. } => None,
    }
}

fn attachment_zone_id(layout_device_id: &str, suggested: &AttachmentSuggestedZone) -> String {
    format!(
        "attachment-{}-{}-{}-{}",
        slugify(layout_device_id),
        slugify(&suggested.slot_id),
        suggested.led_start,
        suggested.instance
    )
}

fn slugify(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut previous_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            previous_dash = false;
            continue;
        }
        if !out.is_empty() && !previous_dash {
            out.push('-');
            previous_dash = true;
        }
    }
    out.trim_matches('-').to_owned()
}
