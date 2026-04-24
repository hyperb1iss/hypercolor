//! Channel panel — device channel listing with inline component editors.
//!
//! Each channel (hardware slot) shows its topology, name, LED count, identify button,
//! and inline attachment draft controls for managing its components.

use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_leptos_ext::events::Input;
use hypercolor_types::attachment::AttachmentSuggestedZone;

use crate::api;
use crate::app::DevicesContext;
use crate::async_helpers::spawn_identify;
use crate::channel_names;
use crate::components::attachment_editor;
use crate::components::component_picker::ComponentPicker;
use crate::components::device_card::topology_shape_svg;
use crate::icons::*;
use crate::layout_geometry;
use crate::toasts;

// ── Channel panel ───────────────────────────────────────────────────────────

/// Channel panel — lists device channels with inline component editing.
#[component]
pub fn WiringPanel(
    #[prop(into)] device_id: Signal<String>,
    #[prop(into)] device: Signal<Option<api::DeviceSummary>>,
) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();

    // Note: we intentionally don't wire a refetch tick here. After a save, the
    // response contains the updated bindings so we patch local state in place
    // rather than refetching — that's what preserves scroll position and input
    // focus in the component editor. The resources still re-trigger naturally
    // when `device_id` changes (device switch).
    let attachments = LocalResource::new(move || {
        let id = device_id.get();
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

    let templates = LocalResource::new(move || async move {
        api::fetch_attachment_templates(None)
            .await
            .unwrap_or_default()
    });

    // Layout zone display-names — used as fallback when localStorage has no
    // custom channel name.  This picks up renames the user made in the layout
    // zone properties panel.
    let layout_zone_names = LocalResource::new(move || {
        let dev = device.get();
        async move {
            let Some(dev) = dev else {
                return std::collections::HashMap::<String, String>::new();
            };
            let layout_did = dev.layout_device_id.clone();
            let dev_name = dev.name.clone();
            let prefix = format!("{dev_name} \u{00b7} "); // "Device · "
            match api::fetch_active_layout().await {
                Ok(layout) => layout
                    .zones
                    .iter()
                    .filter(|z| z.device_id == layout_did)
                    .filter_map(|z| {
                        let slot_id = z.zone_name.as_ref()?;
                        // Strip the "DeviceName · " prefix that seeded layouts add,
                        // leaving just the user's channel display name.
                        let name = z.name.strip_prefix(&prefix).unwrap_or(&z.name).to_string();
                        Some((slot_id.clone(), name))
                    })
                    .collect(),
                Err(_) => std::collections::HashMap::new(),
            }
        }
    });

    // Palette cycled per channel — each slot gets its own accent so a long
    // channel list reads as a spectrum, not a grey stack.
    const CHANNEL_PALETTE: &[&str] = &[
        "128, 255, 234", // cyan
        "255, 106, 193", // coral
        "80, 250, 123",  // green
        "241, 250, 140", // yellow
        "225, 53, 255",  // purple
        "110, 180, 255", // blue
    ];

    view! {
        <div class="rounded-xl bg-surface-raised border border-edge-subtle overflow-hidden edge-glow">
            <div class="relative flex items-center justify-between px-4 py-2.5 border-b border-edge-subtle">
                <div class="flex items-center gap-2">
                    <Icon icon=LuLayers width="12px" height="12px" style="color: rgba(128, 255, 234, 0.85)" />
                    <h3 class="text-[10px] font-mono uppercase tracking-[0.16em] font-semibold text-fg-secondary">"Channels"</h3>
                </div>
                // Animated accent bar across the header bottom
                <div class="absolute bottom-0 left-0 right-0 h-[1px]"
                     style="background: linear-gradient(90deg, rgba(128, 255, 234, 0.35), rgba(225, 53, 255, 0.25), rgba(255, 106, 193, 0.20), transparent)" />
            </div>

            <div>
                <Suspense fallback=|| view! {
                    <div class="text-[10px] text-fg-tertiary animate-pulse py-3 px-4">"Loading..."</div>
                }>
                    {move || {
                        let all_templates = templates.get().map(|t| t.to_vec()).unwrap_or_default();
                        let device_zones = device.get().map(|d| d.zones.clone()).unwrap_or_default();
                        let zone_names = layout_zone_names.get().unwrap_or_default();
                        let did = device_id.get();

                        attachments.get().map(|result| match result {
                            Ok(profile) => {
                                let slots = profile.slots.clone();
                                let bindings = profile.bindings.clone();

                                if slots.is_empty() {
                                    return view! {
                                        <div class="text-[10px] text-fg-tertiary/50 text-center py-4 px-4">
                                            "No channels"
                                        </div>
                                    }.into_any();
                                }

                                view! {
                                    // Stronger dividers — each channel reads as its own band
                                    <div class="divide-y divide-edge-subtle/60">
                                        {slots.into_iter().enumerate().map(|(slot_idx, slot)| {
                                            let slot_id = slot.id.clone();

                                            // Match zone for topology + identify
                                            let zone_match = device_zones.iter()
                                                .find(|z| z.name == slot.name || z.id == slot.id)
                                                .cloned();
                                            let zone_svg = zone_match.as_ref()
                                                .map(|z| topology_shape_svg(&z.topology))
                                                .unwrap_or_else(|| topology_shape_svg("strip"));
                                            let zone_id = zone_match.as_ref().map(|z| z.id.clone());

                                            // Channel name: localStorage → layout zone name → driver default
                                            let default_name = slot.name.clone();
                                            let layout_name = zone_names.get(&slot.id).cloned();
                                            let display_name = channel_names::load_channel_name(&did, &slot.id)
                                                .or(layout_name)
                                                .unwrap_or_else(|| default_name.clone());
                                            let (channel_name, set_channel_name) = signal(display_name);
                                            let (editing, set_editing) = signal(false);
                                            let (name_input, set_name_input) = signal(String::new());

                                            let save_name = {
                                                let did = did.clone();
                                                let slot_id = slot_id.clone();
                                                let default_name = default_name.clone();
                                                move || {
                                                    set_editing.set(false);
                                                    let previous_name = channel_name.get_untracked();
                                                    let new_name = name_input.get();
                                                    let name = if new_name.trim().is_empty() {
                                                        default_name.clone()
                                                    } else {
                                                        new_name.trim().to_string()
                                                    };
                                                    channel_names::save_channel_name(
                                                        &did,
                                                        &slot_id,
                                                        &default_name,
                                                        &name,
                                                    );
                                                    set_channel_name.set(name.clone());
                                                    if let Some(device) = device.get_untracked() {
                                                        sync_channel_name_to_active_layout(
                                                            device,
                                                            slot_id.clone(),
                                                            default_name.clone(),
                                                            previous_name,
                                                            name,
                                                        );
                                                    }
                                                }
                                            };

                                            // Expand bindings into new draft rows
                                            let initial_drafts = attachment_editor::expand_bindings_to_drafts(
                                                &slot.id, &bindings, &all_templates,
                                            );

                                            let accent = CHANNEL_PALETTE[slot_idx % CHANNEL_PALETTE.len()];
                                            let did_for_identify = did.clone();

                                            // Expand channels with existing components, collapse empties.
                                            let has_components = !initial_drafts.is_empty();
                                            let (expanded, set_expanded) = signal(has_components);

                                            view! {
                                                <div class="relative group/channel">
                                                    // Left accent bar — brand the channel without a nested card
                                                    <div class="absolute left-0 top-0 bottom-0 w-[3px] transition-opacity"
                                                         style=format!(
                                                             "background: linear-gradient(180deg, rgba({accent}, 0.75), rgba({accent}, 0.35)); \
                                                              box-shadow: 0 0 8px rgba({accent}, 0.35)"
                                                         ) />
                                                    // Channel header — flat row, no border, subtle hover tint
                                                    <div class="pl-4 pr-3 py-2.5 flex items-center gap-2 group/slot cursor-pointer select-none
                                                                transition-colors hover:bg-white/[0.02]"
                                                         on:click=move |_| set_expanded.update(|v| *v = !*v)>
                                                        // Expand/collapse chevron
                                                        <div
                                                            class="w-3 h-3 shrink-0 text-fg-tertiary/40 transition-transform duration-150"
                                                            class=("rotate-90", move || expanded.get())
                                                        >
                                                            <Icon icon=LuChevronRight width="12px" height="12px" />
                                                        </div>
                                                        // Topology icon — glows in the channel accent
                                                        <div class="w-5 h-5 shrink-0 flex items-center justify-center rounded"
                                                             style=format!(
                                                                 "color: rgba({accent}, 0.95); \
                                                                  background: rgba({accent}, 0.08); \
                                                                  box-shadow: inset 0 0 8px rgba({accent}, 0.12)"
                                                             )
                                                             inner_html=format!(r#"<svg viewBox="0 0 16 16" width="14" height="14">{zone_svg}</svg>"#) />


                                                        // Editable name
                                                        <div class="flex-1 min-w-0">
                                                            {move || if editing.get() {
                                                                view! {
                                                                    <input
                                                                        type="text"
                                                                        class="bg-surface-overlay border border-edge-subtle rounded px-1.5 py-0.5 text-[11px] text-fg-primary
                                                                               focus:outline-none focus:border-accent-muted w-full"
                                                                        prop:value=move || name_input.get()
                                                                        on:click=move |ev: web_sys::MouseEvent| ev.stop_propagation()
                                                                        on:input=move |ev| {
                                                                            let event = Input::from_event(ev);
                                                                            if let Some(value) = event.value_string() {
                                                                                set_name_input.set(value);
                                                                            }
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
                                                                        on:click=move |ev: web_sys::MouseEvent| {
                                                                            ev.stop_propagation();
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

                                                        // LED count — glows in the channel accent
                                                        <span class="text-[10px] font-mono tabular-nums font-semibold shrink-0"
                                                              style=format!(
                                                                  "color: rgba({accent}, 0.85); \
                                                                   text-shadow: 0 0 8px rgba({accent}, 0.25)"
                                                              )>
                                                            {slot.led_count}
                                                            <span class="text-[8px] opacity-60 ml-0.5">"LED"</span>
                                                        </span>

                                                        // Component count badge (collapsed only)
                                                        {
                                                            let component_count = initial_drafts.len();
                                                            (component_count > 0).then(|| view! {
                                                                <Show when=move || !expanded.get()>
                                                                    <span class="text-[9px] font-mono tabular-nums px-1.5 py-0.5 rounded-full shrink-0"
                                                                          style=format!(
                                                                              "color: rgba({accent}, 0.7); \
                                                                               background: rgba({accent}, 0.10); \
                                                                               border: 1px solid rgba({accent}, 0.18)"
                                                                          )>
                                                                        {component_count}
                                                                    </span>
                                                                </Show>
                                                            })
                                                        }

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
                                                                        move |ev: web_sys::MouseEvent| {
                                                                            ev.stop_propagation();
                                                                            let did = did.clone();
                                                                            let zid = zid.clone();
                                                                            spawn_identify(
                                                                                "channel",
                                                                                async move { api::identify_zone(&did, &zid).await },
                                                                            );
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
                                                        // Per-channel hardware LED cap (e.g. PrismRGB Prism 8 = 126/channel).
                                                        // Used to clamp the custom-strip input so users can't draft a strip
                                                        // that exceeds the channel's physical capacity.
                                                        let slot_max_leds = slot.led_count.max(1);
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
                                                                        // Patch drafts in place so row_ids stay stable — this is
                                                                        // what keeps scroll position, input focus, and the <For>
                                                                        // keyed list intact across a save. We also backfill
                                                                        // persisted_target on rows that were just saved so the
                                                                        // identify button starts working without a refetch.
                                                                        let slot_id_local = slot_id_for_save.get_value();
                                                                        set_drafts.update(|rows| {
                                                                            let mut slot_bindings: Vec<(usize, &api::AttachmentBindingSummary)> = updated
                                                                                .bindings
                                                                                .iter()
                                                                                .enumerate()
                                                                                .filter(|(_, binding)| binding.slot_id == slot_id_local)
                                                                                .collect();
                                                                            slot_bindings.sort_by_key(|(_, binding)| binding.led_offset);
                                                                            for (row, (binding_index, _)) in rows.iter_mut().zip(slot_bindings.iter()) {
                                                                                row.persisted_target = Some(attachment_editor::PersistedAttachmentTarget {
                                                                                    binding_index: *binding_index,
                                                                                    instance: 0,
                                                                                });
                                                                            }
                                                                        });
                                                                        // Reset the dirty baseline — is_dirty becomes false and
                                                                        // the save button hides until the next edit.
                                                                        initial_store.set_value(drafts.get_untracked());
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
                                                    <div
                                                        class="pl-9 pr-3 pb-3 pt-1"
                                                        class=("hidden", move || !expanded.get())
                                                    >
                                                        // Component rows — <For> keyed on row_id keeps existing row DOM
                                                        // (and input focus/scroll position) stable across drafts mutations.
                                                        // Handlers look rows up by row_id instead of draft index so
                                                        // reorders and deletes can't desync them.
                                                        <For
                                                            each=move || drafts.get()
                                                            key=|row: &attachment_editor::DraftRow| row.row_id
                                                            children=move |row: attachment_editor::DraftRow| {
                                                                let row_id = row.row_id;
                                                                let initial_kind = row.kind.clone();
                                                                let initial_name = row.name.clone();
                                                                let is_custom = row.needs_template_creation();
                                                                let type_label = match &initial_kind {
                                                                    attachment_editor::ComponentDraft::Strip { .. } => "Strip",
                                                                    attachment_editor::ComponentDraft::Matrix { .. } => "Matrix",
                                                                    attachment_editor::ComponentDraft::Component { .. } => "Component",
                                                                };
                                                                let placeholder = if initial_name.is_empty() {
                                                                    type_label.to_string()
                                                                } else {
                                                                    initial_name.clone()
                                                                };
                                                                let did_c = did_for_identify.get_value();
                                                                let sid_c = slot_id_for_identify.get_value();

                                                                // Per-row reactive views of `drafts`, looked up by row_id so
                                                                // reorders or deletes can't point at the wrong row.
                                                                let name_sig = Memo::new(move |_| {
                                                                    drafts
                                                                        .with(|rows| {
                                                                            rows.iter()
                                                                                .find(|r| r.row_id == row_id)
                                                                                .map(|r| r.name.clone())
                                                                        })
                                                                        .unwrap_or_default()
                                                                });
                                                                let strip_led_count = Memo::new(move |_| {
                                                                    drafts
                                                                        .with(|rows| {
                                                                            rows.iter().find(|r| r.row_id == row_id).and_then(|r| match &r.kind {
                                                                                attachment_editor::ComponentDraft::Strip { led_count } => Some(*led_count),
                                                                                _ => None,
                                                                            })
                                                                        })
                                                                        .unwrap_or(0)
                                                                });
                                                                let matrix_cols_sig = Memo::new(move |_| {
                                                                    drafts
                                                                        .with(|rows| {
                                                                            rows.iter().find(|r| r.row_id == row_id).and_then(|r| match &r.kind {
                                                                                attachment_editor::ComponentDraft::Matrix { cols, .. } => Some(*cols),
                                                                                _ => None,
                                                                            })
                                                                        })
                                                                        .unwrap_or(0)
                                                                });
                                                                let matrix_rows_sig = Memo::new(move |_| {
                                                                    drafts
                                                                        .with(|rows| {
                                                                            rows.iter().find(|r| r.row_id == row_id).and_then(|r| match &r.kind {
                                                                                attachment_editor::ComponentDraft::Matrix { rows: r, .. } => Some(*r),
                                                                                _ => None,
                                                                            })
                                                                        })
                                                                        .unwrap_or(0)
                                                                });
                                                                let persisted_sig = Memo::new(move |_| {
                                                                    drafts.with(|rows| {
                                                                        rows.iter()
                                                                            .find(|r| r.row_id == row_id)
                                                                            .and_then(|r| r.persisted_target)
                                                                    })
                                                                });

                                                                view! {
                                                                    <div class="flex items-center gap-2 py-1.5 transition-colors group/row">
                                                                        // Type icon (hollow bullet, tinted in channel accent)
                                                                        <div class="w-4 h-4 shrink-0 flex items-center justify-center"
                                                                             style=format!("color: rgba({accent}, 0.65)")>
                                                                            <Icon icon={match &initial_kind {
                                                                                attachment_editor::ComponentDraft::Strip { .. } => LuMinus,
                                                                                attachment_editor::ComponentDraft::Matrix { .. } => LuGrid2x2,
                                                                                attachment_editor::ComponentDraft::Component { .. } => LuCircleDot,
                                                                            }} width="13px" height="13px" />
                                                                        </div>

                                                                        // Name field
                                                                        {if is_custom {
                                                                            view! {
                                                                                <input
                                                                                    type="text"
                                                                                    placeholder=placeholder
                                                                                    class="flex-1 min-w-0 bg-transparent text-[12px] text-fg-primary
                                                                                           placeholder-fg-tertiary/30 focus:outline-none border-none p-0"
                                                                                    prop:value=move || name_sig.get()
                                                                                    on:input=move |ev| {
                                                                                        let val = Input::from_event(ev).value_string().unwrap_or_default();
                                                                                        set_drafts.update(|rows| {
                                                                                            if let Some(r) = rows.iter_mut().find(|r| r.row_id == row_id) {
                                                                                                r.name = val;
                                                                                            }
                                                                                        });
                                                                                    }
                                                                                />
                                                                            }.into_any()
                                                                        } else {
                                                                            view! {
                                                                                <span class="flex-1 min-w-0 text-[12px] text-fg-primary truncate">{placeholder}</span>
                                                                            }.into_any()
                                                                        }}

                                                                        // LED count / dimensions — editable for custom, static for library.
                                                                        // `on:input` commits every keystroke/arrow so values can't get
                                                                        // lost between blur and click. The strip input is clamped to
                                                                        // `slot_max_leds` (the channel's hardware capacity).
                                                                        {match &initial_kind {
                                                                            attachment_editor::ComponentDraft::Strip { .. } => view! {
                                                                                <input
                                                                                    type="number" min="1" max=slot_max_leds.to_string()
                                                                                    class="w-14 bg-surface-base/40 border border-edge-subtle rounded px-1.5 py-0.5
                                                                                           text-[11px] font-mono tabular-nums text-right shrink-0
                                                                                           focus:outline-none focus:border-neon-cyan/30"
                                                                                    style="color: rgba(128, 255, 234, 0.8)"
                                                                                    prop:value=move || strip_led_count.get().to_string()
                                                                                    on:input=move |ev| {
                                                                                        if let Some(v) = Input::from_event(ev).value::<u32>() {
                                                                                            let clamped = v.clamp(1, slot_max_leds);
                                                                                            set_drafts.update(|rows| {
                                                                                                if let Some(r) = rows.iter_mut().find(|r| r.row_id == row_id) {
                                                                                                    r.kind = attachment_editor::ComponentDraft::Strip { led_count: clamped };
                                                                                                }
                                                                                            });
                                                                                        }
                                                                                    }
                                                                                />
                                                                            }.into_any(),
                                                                            attachment_editor::ComponentDraft::Matrix { .. } => view! {
                                                                                <div class="flex items-center gap-0.5 shrink-0">
                                                                                    <input
                                                                                        type="number" min="1" max="64"
                                                                                        class="w-10 bg-surface-base/40 border border-edge-subtle rounded px-1 py-0.5
                                                                                               text-[11px] font-mono tabular-nums text-right
                                                                                               focus:outline-none focus:border-neon-cyan/30"
                                                                                        style="color: rgba(128, 255, 234, 0.8)"
                                                                                        prop:value=move || matrix_cols_sig.get().to_string()
                                                                                        on:input=move |ev| {
                                                                                            if let Some(v) = Input::from_event(ev).value::<u32>() {
                                                                                                let clamped = v.clamp(1, 64);
                                                                                                set_drafts.update(|rows| {
                                                                                                    if let Some(r) = rows.iter_mut().find(|r| r.row_id == row_id)
                                                                                                        && let attachment_editor::ComponentDraft::Matrix { cols, .. } = &mut r.kind {
                                                                                                        *cols = clamped;
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
                                                                                        prop:value=move || matrix_rows_sig.get().to_string()
                                                                                        on:input=move |ev| {
                                                                                            if let Some(v) = Input::from_event(ev).value::<u32>() {
                                                                                                let clamped = v.clamp(1, 64);
                                                                                                set_drafts.update(|rows| {
                                                                                                    if let Some(r) = rows.iter_mut().find(|r| r.row_id == row_id)
                                                                                                        && let attachment_editor::ComponentDraft::Matrix { rows, .. } = &mut r.kind {
                                                                                                        *rows = clamped;
                                                                                                    }
                                                                                                });
                                                                                            }
                                                                                        }
                                                                                    />
                                                                                </div>
                                                                            }.into_any(),
                                                                            attachment_editor::ComponentDraft::Component { template_id } => {
                                                                                let count = templates_for_summary.with_value(|ts| ts.iter().find(|t| t.id == *template_id).map(|t| t.led_count)).unwrap_or(0);
                                                                                view! {
                                                                                    <span class="text-[10px] font-mono tabular-nums shrink-0"
                                                                                          style=format!("color: rgba({accent}, 0.7)")>
                                                                                        {count} " LEDs"
                                                                                    </span>
                                                                                }.into_any()
                                                                            }
                                                                        }}

                                                                        // Identify — only enabled once the row has been saved
                                                                        // (and therefore has a real binding_index to target).
                                                                        <button
                                                                            class="w-5 h-5 flex items-center justify-center rounded shrink-0
                                                                                   opacity-0 group-hover/row:opacity-100 transition-opacity
                                                                                   text-fg-tertiary/40 hover:text-accent btn-press
                                                                                   disabled:opacity-0 disabled:pointer-events-none"
                                                                            title="Identify component"
                                                                            disabled=move || persisted_sig.get().is_none()
                                                                            on:click=move |_| {
                                                                                if let Some(target) = persisted_sig.get_untracked() {
                                                                                    let d = did_c.clone();
                                                                                    let s = sid_c.clone();
                                                                                    spawn_identify(
                                                                                        "component",
                                                                                        async move {
                                                                                            api::identify_attachment(
                                                                                                &d,
                                                                                                &s,
                                                                                                Some(target.binding_index),
                                                                                                Some(target.instance),
                                                                                            )
                                                                                            .await
                                                                                        },
                                                                                    );
                                                                                }
                                                                            }
                                                                        >
                                                                            <Icon icon=LuZap width="10px" height="10px" />
                                                                        </button>

                                                                        // Delete
                                                                        <button
                                                                            class="w-5 h-5 flex items-center justify-center rounded shrink-0
                                                                                   opacity-0 group-hover/row:opacity-100 transition-opacity
                                                                                   text-fg-tertiary/40 hover:text-error-red btn-press"
                                                                            on:click=move |_| {
                                                                                set_drafts.update(|rows| { rows.retain(|r| r.row_id != row_id); });
                                                                            }
                                                                        >
                                                                            <Icon icon=LuX width="10px" height="10px" />
                                                                        </button>
                                                                    </div>
                                                                }
                                                            }
                                                        />
                                                        <Show when=move || drafts.with(Vec::is_empty)>
                                                            <div class="text-[10px] text-fg-tertiary/40 py-1.5">"No components"</div>
                                                        </Show>

                                                        // Add + save buttons — compact inline strip
                                                        <div class="flex items-center justify-between pt-1.5 mt-1 border-t border-edge-subtle/20">
                                                            <div class="flex items-center gap-1">
                                                                <button
                                                                    class="text-[10px] font-medium px-2 py-1 rounded-md transition-all btn-press flex items-center gap-1"
                                                                    style=format!("color: rgba({accent}, 0.75); background: rgba({accent}, 0.06)")
                                                                    on:click=add_strip
                                                                >
                                                                    <Icon icon=LuPlus width="10px" height="10px" />
                                                                    "Strip"
                                                                </button>
                                                                <button
                                                                    class="text-[10px] font-medium px-2 py-1 rounded-md transition-all btn-press flex items-center gap-1"
                                                                    style=format!("color: rgba({accent}, 0.75); background: rgba({accent}, 0.06)")
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
                                                                        <span
                                                                            class="text-[9px] font-mono"
                                                                            style="color: rgb(255, 99, 99)"
                                                                            title="This channel's hardware capacity is exceeded. Reduce the component(s) or remove one."
                                                                        >
                                                                            {s.total_leds} "/" {s.available_leds} " LEDs"
                                                                        </span>
                                                                    })
                                                                }}
                                                                <Show when=move || is_dirty.get()>
                                                                    <button
                                                                        class="text-[10px] font-semibold px-2.5 py-1 rounded-md transition-all btn-press disabled:opacity-30
                                                                               flex items-center gap-1"
                                                                        style=format!(
                                                                            "color: rgb({accent}); \
                                                                             background: rgba({accent}, 0.14); \
                                                                             border: 1px solid rgba({accent}, 0.30); \
                                                                             box-shadow: 0 0 10px rgba({accent}, 0.15)"
                                                                        )
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
            let mut seeded = layout_geometry::seeded_attachment_layout(
                &device.layout_device_id,
                &device.name,
                &suggested_zones,
                0,
            );
            let slot_display_names = suggested_zones
                .iter()
                .filter_map(|zone| {
                    channel_names::load_channel_name(&device.id, &zone.slot_id)
                        .map(|display_name| (zone.slot_id.clone(), display_name))
                })
                .collect::<std::collections::HashMap<_, _>>();
            crate::layout_utils::apply_slot_display_names_to_seeded_attachment_layout(
                &mut seeded,
                &device.name,
                &slot_display_names,
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
            };
            api::update_layout(&layout_id, &req).await?;
            api::apply_layout(&layout_id).await?;
            Ok(imported_count)
        }
        .await;

        if let Ok(count) = result
            && count > 0
        {
            layouts_resource.refetch();
            let noun = if count == 1 { "zone" } else { "zones" };
            toasts::toast_info(&format!("Layout synced ({count} {noun})"));
        }
    });
}

fn sync_channel_name_to_active_layout(
    device: api::DeviceSummary,
    slot_id: String,
    default_name: String,
    previous_name: String,
    new_name: String,
) {
    if previous_name == new_name {
        return;
    }

    leptos::task::spawn_local(async move {
        let result: Result<bool, String> = async {
            let mut layout = api::fetch_active_layout().await?;
            let layout_id = layout.id.clone();
            let changed = crate::layout_utils::sync_channel_display_name_in_layout(
                &mut layout,
                &device.layout_device_id,
                &device.name,
                &slot_id,
                &default_name,
                &previous_name,
                &new_name,
            );
            if !changed {
                return Ok(false);
            }

            let req = api::UpdateLayoutApiRequest {
                name: None,
                description: None,
                canvas_width: None,
                canvas_height: None,
                zones: Some(layout.zones),
            };
            api::update_layout(&layout_id, &req).await?;
            api::apply_layout(&layout_id).await?;
            Ok(true)
        }
        .await;

        if let Err(error) = result {
            toasts::toast_error(&format!("Channel rename sync failed: {error}"));
        }
    });
}
