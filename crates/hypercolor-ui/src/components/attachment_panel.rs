//! Channel panel — device channel listing with inline component editors.
//!
//! Each channel (hardware slot) shows its topology, name, LED count, identify button,
//! and a `ChannelEditor` for managing its components.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use hypercolor_types::attachment::AttachmentSuggestedZone;

use crate::api;
use crate::app::DevicesContext;
use crate::components::attachment_editor;
use crate::components::component_picker::ComponentPicker;
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

// ── Channel panel ───────────────────────────────────────────────────────────

/// Channel panel — lists device channels with inline component editing.
#[component]
pub fn WiringPanel(
    #[prop(into)] device_id: Signal<String>,
    #[prop(into)] device: Signal<Option<api::DeviceSummary>>,
) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();
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

    let templates = LocalResource::new(move || {
        refetch_tick.get();
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
                        let all_templates = templates.get().map(|t| t.to_vec()).unwrap_or_default();
                        let device_zones = device.get().map(|d| d.zones.clone()).unwrap_or_default();
                        let did = device_id.get();

                        attachments.get().map(|result| match result {
                            Ok(profile) => {
                                let slots = profile.slots.clone();
                                let bindings = profile.bindings.clone();

                                if slots.is_empty() {
                                    return view! {
                                        <div class="text-[10px] text-fg-tertiary/50 text-center py-3">
                                            "No channels"
                                        </div>
                                    }.into_any();
                                }

                                view! {
                                    <div class="space-y-2">
                                        {slots.into_iter().map(|slot| {
                                            let slot_id = slot.id.clone();

                                            // Match zone for topology + identify
                                            let zone_match = device_zones.iter()
                                                .find(|z| z.name == slot.name || z.id == slot.id)
                                                .cloned();
                                            let zone_svg = zone_match.as_ref()
                                                .map(|z| topology_shape_svg(&z.topology))
                                                .unwrap_or_else(|| topology_shape_svg("strip"));
                                            let zone_id = zone_match.as_ref().map(|z| z.id.clone());

                                            // Channel name (localStorage override)
                                            let default_name = slot.name.clone();
                                            let stored_name = load_channel_name(&did, &slot.id);
                                            let display_name = stored_name.unwrap_or_else(|| default_name.clone());
                                            let (channel_name, set_channel_name) = signal(display_name);
                                            let (editing, set_editing) = signal(false);
                                            let (name_input, set_name_input) = signal(String::new());

                                            let save_name = {
                                                let did = did.clone();
                                                let slot_id = slot_id.clone();
                                                let default_name = default_name.clone();
                                                move || {
                                                    set_editing.set(false);
                                                    let new_name = name_input.get();
                                                    let name = if new_name.trim().is_empty() || new_name.trim() == default_name {
                                                        default_name.clone()
                                                    } else {
                                                        new_name.trim().to_string()
                                                    };
                                                    save_channel_name(&did, &slot_id, &name);
                                                    set_channel_name.set(name);
                                                }
                                            };

                                            // Expand bindings into new draft rows
                                            let initial_drafts = attachment_editor::expand_bindings_to_drafts(
                                                &slot.id, &bindings, &all_templates,
                                            );

                                            let accent = "128, 255, 234";
                                            let did_for_identify = did.clone();

                                            view! {
                                                <div class="rounded-lg border border-edge-subtle bg-surface-overlay/10 overflow-visible">
                                                    // Channel header
                                                    <div class="px-3 py-2 flex items-center gap-2 group/slot">
                                                        // Topology icon
                                                        <div class="w-4 h-4 shrink-0" style=format!("color: rgba({accent}, 0.5)")
                                                             inner_html=format!(r#"<svg viewBox="0 0 16 16" width="16" height="16">{zone_svg}</svg>"#) />

                                                        // Editable name
                                                        <div class="flex-1 min-w-0">
                                                            {move || if editing.get() {
                                                                view! {
                                                                    <input
                                                                        type="text"
                                                                        class="bg-surface-overlay border border-edge-subtle rounded px-1.5 py-0.5 text-[11px] text-fg-primary
                                                                               focus:outline-none focus:border-accent-muted w-full"
                                                                        prop:value=move || name_input.get()
                                                                        on:input=move |ev| {
                                                                            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                                                            if let Some(el) = target { set_name_input.set(el.value()); }
                                                                        }
                                                                        on:keydown={
                                                                            let save = save_name.clone();
                                                                            move |ev: web_sys::KeyboardEvent| {
                                                                                if ev.key() == "Enter" { save(); }
                                                                                if ev.key() == "Escape" { set_editing.set(false); }
                                                                            }
                                                                        }
                                                                        on:blur={
                                                                            let save = save_name.clone();
                                                                            move |_| save()
                                                                        }
                                                                    />
                                                                }.into_any()
                                                            } else {
                                                                view! {
                                                                    <span
                                                                        class="text-[12px] font-medium text-fg-primary flex items-center gap-1 cursor-text"
                                                                        on:click=move |_| {
                                                                            set_name_input.set(channel_name.get_untracked());
                                                                            set_editing.set(true);
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
                                                        <span class="text-[10px] font-mono text-fg-tertiary/50 tabular-nums shrink-0">
                                                            {slot.led_count} " LEDs"
                                                        </span>

                                                        // Identify
                                                        {zone_id.map(|zid| {
                                                            let did = did_for_identify.clone();
                                                            view! {
                                                                <button
                                                                    class="w-5 h-5 flex items-center justify-center rounded shrink-0
                                                                           opacity-0 group-hover/slot:opacity-100 transition-opacity
                                                                           text-fg-tertiary/40 hover:text-accent btn-press"
                                                                    title="Identify channel"
                                                                    on:click={
                                                                        let did = did.clone();
                                                                        let zid = zid.clone();
                                                                        move |_| {
                                                                            let did = did.clone();
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
                                                    </div>

                                                    // Inline component editor
                                                    {
                                                        let initial_store = StoredValue::new(initial_drafts.clone());
                                                        let (drafts, set_drafts) = signal(initial_drafts);
                                                        let templates_for_summary = StoredValue::new(all_templates.clone());
                                                        let slot_for_save = StoredValue::new(slot.clone());
                                                        let slot_id_for_save = StoredValue::new(slot_id.clone());
                                                        let did_for_save = StoredValue::new(did.clone());
                                                        let did_for_identify = StoredValue::new(did.clone());
                                                        let slot_id_for_identify = StoredValue::new(slot_id.clone());
                                                        let (save_in_flight, set_save_in_flight) = signal(false);

                                                        let is_dirty = Signal::derive(move || {
                                                            initial_store.with_value(|saved| drafts.get() != *saved)
                                                        });
                                                        let summary = Signal::derive(move || {
                                                            let s = slot_for_save.get_value();
                                                            let rows = drafts.get();
                                                            templates_for_summary.with_value(|ts| attachment_editor::summarize_channel(&s, &rows, ts))
                                                        });

                                                        let add_strip = move |_: web_sys::MouseEvent| {
                                                            set_drafts.update(|rows| rows.push(attachment_editor::DraftRow::new_strip(60)));
                                                        };
                                                        let add_matrix = move |_: web_sys::MouseEvent| {
                                                            set_drafts.update(|rows| rows.push(attachment_editor::DraftRow::new_matrix(8, 8)));
                                                        };
                                                        let on_component_selected = Callback::new(move |(template_id, name): (String, String)| {
                                                            set_drafts.update(|rows| {
                                                                rows.push(attachment_editor::DraftRow::from_component(template_id, name));
                                                            });
                                                        });
                                                        let picker_templates = all_templates.clone();

                                                        // Save handler — creates templates for custom strips/matrices, then saves bindings
                                                        let layouts_resource = ctx.layouts_resource;
                                                        let do_save = move |_: web_sys::MouseEvent| {
                                                            let rows = drafts.get_untracked();
                                                            let slot_id = slot_id_for_save.get_value();
                                                            let did = did_for_save.get_value();
                                                            let device_for_sync = device.get_untracked();
                                                            let templates = templates_for_summary.with_value(|ts| ts.clone());
                                                            set_save_in_flight.set(true);
                                                            leptos::task::spawn_local(async move {
                                                                let result = async {
                                                                    // Create templates for inline strips/matrices
                                                                    let mut template_ids: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
                                                                    for (i, row) in rows.iter().enumerate() {
                                                                        if !row.needs_template_creation() { continue; }
                                                                        let (name, cat, topo, desc) = match &row.kind {
                                                                            attachment_editor::ComponentDraft::Strip { led_count } => {
                                                                                let n = if row.name.is_empty() { format!("Custom Strip - {led_count} LEDs") } else { row.name.clone() };
                                                                                (n, hypercolor_types::attachment::AttachmentCategory::Strip,
                                                                                 hypercolor_types::spatial::LedTopology::Strip { count: *led_count, direction: hypercolor_types::spatial::StripDirection::LeftToRight },
                                                                                 format!("Custom strip, {led_count} LEDs"))
                                                                            }
                                                                            attachment_editor::ComponentDraft::Matrix { cols, rows } => {
                                                                                let n = if row.name.is_empty() { format!("Custom Matrix - {cols}\u{00d7}{rows}") } else { row.name.clone() };
                                                                                (n, hypercolor_types::attachment::AttachmentCategory::Matrix,
                                                                                 hypercolor_types::spatial::LedTopology::Matrix { width: *cols, height: *rows, serpentine: true, start_corner: hypercolor_types::spatial::Corner::TopLeft },
                                                                                 format!("Custom {cols}\u{00d7}{rows} matrix"))
                                                                            }
                                                                            _ => continue,
                                                                        };
                                                                        let id = format!("custom-{}-{}-{}", cat.as_str(), js_sys::Date::now() as u64, i);
                                                                        let tmpl = hypercolor_types::attachment::AttachmentTemplate {
                                                                            id: id.clone(), name, category: cat,
                                                                            origin: hypercolor_types::attachment::AttachmentOrigin::User,
                                                                            description: desc, vendor: "Custom".to_string(),
                                                                            default_size: hypercolor_types::attachment::AttachmentCanvasSize { width: 0.24, height: 0.08 },
                                                                            topology: topo, compatible_slots: Vec::new(),
                                                                            tags: vec!["custom".to_string()],
                                                                            led_names: None, led_mapping: None, image_url: None, physical_size_mm: None,
                                                                        };
                                                                        api::create_attachment_template(&tmpl).await?;
                                                                        template_ids.insert(i, id);
                                                                    }
                                                                    // Build bindings
                                                                    let current = api::fetch_device_attachments(&did).await?;
                                                                    let mut bindings: Vec<api::AttachmentBindingRequest> = current.bindings.iter()
                                                                        .filter(|b| b.slot_id != slot_id)
                                                                        .map(|b| api::AttachmentBindingRequest {
                                                                            slot_id: b.slot_id.clone(), template_id: b.template_id.clone(),
                                                                            name: b.name.clone(), enabled: b.enabled, instances: b.instances, led_offset: b.led_offset,
                                                                        }).collect();
                                                                    let mut offset = 0_u32;
                                                                    for (i, row) in rows.iter().enumerate() {
                                                                        let tid = match &row.kind {
                                                                            attachment_editor::ComponentDraft::Component { template_id } => template_id.clone(),
                                                                            _ => template_ids.get(&i).cloned().unwrap_or_default(),
                                                                        };
                                                                        let count = row.led_count(&templates).unwrap_or(0);
                                                                        bindings.push(api::AttachmentBindingRequest {
                                                                            slot_id: slot_id.clone(), template_id: tid,
                                                                            name: if row.name.is_empty() { None } else { Some(row.name.clone()) },
                                                                            enabled: true, instances: 1, led_offset: offset,
                                                                        });
                                                                        offset += count;
                                                                    }
                                                                    api::update_device_attachments(&did, &api::UpdateAttachmentsRequest { bindings }).await
                                                                }.await;
                                                                set_save_in_flight.set(false);
                                                                match result {
                                                                    Ok(updated) => {
                                                                        toasts::toast_success("Saved");
                                                                        set_refetch_tick.update(|t| *t += 1);
                                                                        if let Some(dev) = device_for_sync
                                                                            && !updated.suggested_zones.is_empty() {
                                                                                sync_wiring_to_layout(dev, updated.suggested_zones, layouts_resource);
                                                                            }
                                                                    }
                                                                    Err(e) => toasts::toast_error(&format!("Save failed: {e}")),
                                                                }
                                                            });
                                                        };

                                                    view! {
                                                    <div class="border-t border-edge-subtle/50 px-3 py-2.5 space-y-2">
                                                        // Component rows with inline editing
                                                        {move || {
                                                            let rows = drafts.get();
                                                            if rows.is_empty() {
                                                                view! {
                                                                    <div class="text-[10px] text-fg-tertiary/40 text-center py-2">"No components"</div>
                                                                }.into_any()
                                                            } else {
                                                                let did_id = did_for_identify.get_value();
                                                                let sid_id = slot_id_for_identify.get_value();
                                                                rows.into_iter().enumerate().map(|(i, row)| {
                                                                    let is_custom = row.needs_template_creation();
                                                                    let type_label = match &row.kind {
                                                                        attachment_editor::ComponentDraft::Strip { .. } => "Strip",
                                                                        attachment_editor::ComponentDraft::Matrix { .. } => "Matrix",
                                                                        attachment_editor::ComponentDraft::Component { .. } => "Component",
                                                                    };
                                                                    let placeholder = if row.name.is_empty() { type_label.to_string() } else { row.name.clone() };
                                                                    let did_c = did_id.clone();
                                                                    let sid_c = sid_id.clone();

                                                                    view! {
                                                                        <div class="flex items-center gap-2 px-3 py-2.5 rounded-lg bg-surface-overlay/10
                                                                                    border border-edge-subtle/50 hover:border-edge-subtle transition-all group/row">
                                                                            // Type icon
                                                                            <div class="w-5 h-5 rounded flex items-center justify-center shrink-0"
                                                                                 style="color: rgba(128, 255, 234, 0.6)">
                                                                                <Icon icon={match &row.kind {
                                                                                    attachment_editor::ComponentDraft::Strip { .. } => LuMinus,
                                                                                    attachment_editor::ComponentDraft::Matrix { .. } => LuGrid2x2,
                                                                                    attachment_editor::ComponentDraft::Component { .. } => LuCircleDot,
                                                                                }} width="14px" height="14px" />
                                                                            </div>

                                                                            // Name field
                                                                            {if is_custom {
                                                                                view! {
                                                                                    <input
                                                                                        type="text"
                                                                                        placeholder=placeholder
                                                                                        class="flex-1 min-w-0 bg-transparent text-[12px] text-fg-primary
                                                                                               placeholder-fg-tertiary/30 focus:outline-none border-none p-0"
                                                                                        prop:value=row.name.clone()
                                                                                        on:change=move |ev| {
                                                                                            let val = event_target_value(&ev);
                                                                                            set_drafts.update(|rows| {
                                                                                                if let Some(r) = rows.get_mut(i) { r.name = val; }
                                                                                            });
                                                                                        }
                                                                                    />
                                                                                }.into_any()
                                                                            } else {
                                                                                view! {
                                                                                    <span class="flex-1 min-w-0 text-[12px] text-fg-primary truncate">{placeholder}</span>
                                                                                }.into_any()
                                                                            }}

                                                                            // LED count / dimensions (editable for custom, static for library)
                                                                            {match &row.kind {
                                                                                attachment_editor::ComponentDraft::Strip { led_count } => {
                                                                                    let count = *led_count;
                                                                                    view! {
                                                                                        <input
                                                                                            type="number" min="1" max="2000"
                                                                                            class="w-14 bg-surface-base/40 border border-edge-subtle rounded px-1.5 py-0.5
                                                                                                   text-[11px] font-mono tabular-nums text-right shrink-0
                                                                                                   focus:outline-none focus:border-neon-cyan/30"
                                                                                            style="color: rgba(128, 255, 234, 0.8)"
                                                                                            prop:value=count.to_string()
                                                                                            on:change=move |ev| {
                                                                                                if let Ok(v) = event_target_value(&ev).parse::<u32>() {
                                                                                                    let v = v.clamp(1, 2000);
                                                                                                    set_drafts.update(|rows| {
                                                                                                        if let Some(r) = rows.get_mut(i) {
                                                                                                            r.kind = attachment_editor::ComponentDraft::Strip { led_count: v };
                                                                                                        }
                                                                                                    });
                                                                                                }
                                                                                            }
                                                                                        />
                                                                                    }.into_any()
                                                                                }
                                                                                attachment_editor::ComponentDraft::Matrix { cols, rows } => {
                                                                                    let c = *cols;
                                                                                    let r = *rows;
                                                                                    view! {
                                                                                        <div class="flex items-center gap-0.5 shrink-0">
                                                                                            <input
                                                                                                type="number" min="1" max="64"
                                                                                                class="w-10 bg-surface-base/40 border border-edge-subtle rounded px-1 py-0.5
                                                                                                       text-[11px] font-mono tabular-nums text-right
                                                                                                       focus:outline-none focus:border-neon-cyan/30"
                                                                                                style="color: rgba(128, 255, 234, 0.8)"
                                                                                                prop:value=c.to_string()
                                                                                                on:change=move |ev| {
                                                                                                    if let Ok(v) = event_target_value(&ev).parse::<u32>() {
                                                                                                        set_drafts.update(|rows| {
                                                                                                            if let Some(r) = rows.get_mut(i)
                                                                                                                && let attachment_editor::ComponentDraft::Matrix { cols, .. } = &mut r.kind {
                                                                                                                    *cols = v.clamp(1, 64);
                                                                                                                }
                                                                                                        });
                                                                                                    }
                                                                                                }
                                                                                            />
                                                                                            <span class="text-[9px] text-fg-tertiary/30">{"\u{00d7}"}</span>
                                                                                            <input
                                                                                                type="number" min="1" max="64"
                                                                                                class="w-10 bg-surface-base/40 border border-edge-subtle rounded px-1 py-0.5
                                                                                                       text-[11px] font-mono tabular-nums text-right
                                                                                                       focus:outline-none focus:border-neon-cyan/30"
                                                                                                style="color: rgba(128, 255, 234, 0.8)"
                                                                                                prop:value=r.to_string()
                                                                                                on:change=move |ev| {
                                                                                                    if let Ok(v) = event_target_value(&ev).parse::<u32>() {
                                                                                                        set_drafts.update(|rows| {
                                                                                                            if let Some(r) = rows.get_mut(i)
                                                                                                                && let attachment_editor::ComponentDraft::Matrix { rows, .. } = &mut r.kind {
                                                                                                                    *rows = v.clamp(1, 64);
                                                                                                                }
                                                                                                        });
                                                                                                    }
                                                                                                }
                                                                                            />
                                                                                        </div>
                                                                                    }.into_any()
                                                                                }
                                                                                attachment_editor::ComponentDraft::Component { template_id } => {
                                                                                    let count = templates_for_summary.with_value(|ts| ts.iter().find(|t| t.id == *template_id).map(|t| t.led_count)).unwrap_or(0);
                                                                                    view! {
                                                                                        <span class="text-[10px] font-mono tabular-nums shrink-0 px-1.5 py-0.5 rounded bg-surface-overlay/20"
                                                                                              style="color: rgba(128, 255, 234, 0.5)">
                                                                                            {count} " LEDs"
                                                                                        </span>
                                                                                    }.into_any()
                                                                                }
                                                                            }}

                                                                            // Identify
                                                                            <button
                                                                                class="w-5 h-5 flex items-center justify-center rounded shrink-0
                                                                                       opacity-0 group-hover/row:opacity-100 transition-opacity
                                                                                       text-fg-tertiary/40 hover:text-accent btn-press"
                                                                                title="Identify component"
                                                                                on:click=move |_| {
                                                                                    let d = did_c.clone();
                                                                                    let s = sid_c.clone();
                                                                                    leptos::task::spawn_local(async move {
                                                                                        if let Err(e) = api::identify_attachment(&d, &s, Some(i)).await {
                                                                                            toasts::toast_error(&format!("Identify failed: {e}"));
                                                                                        } else {
                                                                                            toasts::toast_success("Flashing component");
                                                                                        }
                                                                                    });
                                                                                }
                                                                            >
                                                                                <Icon icon=LuZap width="10px" height="10px" />
                                                                            </button>

                                                                            // Delete
                                                                            <button
                                                                                class="w-5 h-5 flex items-center justify-center rounded shrink-0
                                                                                       opacity-0 group-hover/row:opacity-100 transition-opacity
                                                                                       text-fg-tertiary/40 hover:text-error-red btn-press"
                                                                                on:click=move |_| { set_drafts.update(|rows| { if i < rows.len() { rows.remove(i); } }); }
                                                                            >
                                                                                <Icon icon=LuX width="10px" height="10px" />
                                                                            </button>
                                                                        </div>
                                                                    }
                                                                }).collect_view().into_any()
                                                            }
                                                        }}

                                                        // Add + save buttons
                                                        <div class="flex items-center justify-between pt-1">
                                                            <div class="flex items-center gap-1.5">
                                                                <button
                                                                    class="text-[10px] font-medium px-2.5 py-1 rounded-lg transition-all btn-press flex items-center gap-1"
                                                                    style="color: rgba(128, 255, 234, 0.7); background: rgba(128, 255, 234, 0.06); border: 1px solid rgba(128, 255, 234, 0.12)"
                                                                    on:click=add_strip
                                                                >
                                                                    <Icon icon=LuPlus width="10px" height="10px" />
                                                                    "Strip"
                                                                </button>
                                                                <button
                                                                    class="text-[10px] font-medium px-2.5 py-1 rounded-lg transition-all btn-press flex items-center gap-1"
                                                                    style="color: rgba(128, 255, 234, 0.7); background: rgba(128, 255, 234, 0.06); border: 1px solid rgba(128, 255, 234, 0.12)"
                                                                    on:click=add_matrix
                                                                >
                                                                    <Icon icon=LuPlus width="10px" height="10px" />
                                                                    "Matrix"
                                                                </button>
                                                                <ComponentPicker
                                                                    components=picker_templates
                                                                    on_select=on_component_selected
                                                                />
                                                            </div>
                                                            <div class="flex items-center gap-2">
                                                                {move || {
                                                                    let s = summary.get();
                                                                    (s.overflow_leds > 0).then(|| view! {
                                                                        <span class="text-[9px] font-mono" style="color: rgb(255, 99, 99)">
                                                                            {s.overflow_leds} " over"
                                                                        </span>
                                                                    })
                                                                }}
                                                                <Show when=move || is_dirty.get()>
                                                                    <button
                                                                        class="text-[10px] font-medium px-2.5 py-1 rounded-lg transition-all btn-press disabled:opacity-30
                                                                               flex items-center gap-1"
                                                                        style="color: rgba(128, 255, 234, 0.8); background: rgba(128, 255, 234, 0.1); border: 1px solid rgba(128, 255, 234, 0.2)"
                                                                        disabled=move || { let s = summary.get(); save_in_flight.get() || !s.is_valid() }
                                                                        on:click=do_save
                                                                    >
                                                                        <Icon icon=LuCheck width="10px" height="10px" />
                                                                        {move || if save_in_flight.get() { "Saving..." } else { "Save" }}
                                                                    </button>
                                                                </Show>
                                                            </div>
                                                        </div>
                                                    </div>
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

// ── Auto-sync wiring to layout ──────────────────────────────────────────────

pub fn sync_wiring_to_layout(
    device: api::DeviceSummary,
    suggested_zones: Vec<AttachmentSuggestedZone>,
    layouts_resource: LocalResource<Result<Vec<api::LayoutSummary>, String>>,
) {
    leptos::task::spawn_local(async move {
        let result: Result<usize, String> = async {
            let mut layout = api::fetch_active_layout().await?;
            let layout_id = layout.id.clone();
            let seeded = layout_geometry::seeded_attachment_layout(
                &device.layout_device_id,
                &device.name,
                &suggested_zones,
                0,
            );
            let imported_count = seeded.zones.len();
            crate::layout_utils::replace_attachment_layout(
                &mut layout,
                &device.layout_device_id,
                seeded,
            );
            let req = api::UpdateLayoutApiRequest {
                name: None,
                description: None,
                canvas_width: None,
                canvas_height: None,
                zones: Some(layout.zones),
                groups: Some(layout.groups),
            };
            api::update_layout(&layout_id, &req).await?;
            api::apply_layout(&layout_id).await?;
            Ok(imported_count)
        }
        .await;

        if let Ok(count) = result
            && count > 0 {
                layouts_resource.refetch();
                let noun = if count == 1 { "zone" } else { "zones" };
                toasts::toast_info(&format!("Layout synced ({count} {noun})"));
            }
    });
}
