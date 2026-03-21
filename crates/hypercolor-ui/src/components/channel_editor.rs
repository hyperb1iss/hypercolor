//! Channel editor — manages components for a single device channel (slot).
//!
//! Renders the list of component rows with inline editing, plus add buttons
//! and a save button. Custom strips/matrices have their templates created
//! at save time, not on add.

use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_types::attachment::AttachmentSlot;

use crate::api;
use crate::app::DevicesContext;
use crate::components::attachment_editor::{
    self, ChannelDraftSummary, ComponentDraft, DraftRow,
};
use crate::components::attachment_panel;
use crate::components::component_picker::ComponentPicker;
use crate::components::component_row::ComponentRow;
use crate::icons::*;
use crate::toasts;

/// Editor for a single channel's components.
#[component]
pub fn ChannelEditor(
    slot: AttachmentSlot,
    initial_drafts: Vec<DraftRow>,
    all_templates: Vec<api::TemplateSummary>,
    #[prop(into)] device_id: Signal<String>,
    #[prop(into)] device: Signal<Option<api::DeviceSummary>>,
    #[prop(into)] on_saved: Callback<()>,
) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();
    let (save_in_flight, set_save_in_flight) = signal(false);

    let initial_store = StoredValue::new(initial_drafts.clone());
    let (drafts, set_drafts) = signal(initial_drafts);
    let templates_store = StoredValue::new(all_templates.clone());
    let slot_store = StoredValue::new(slot.clone());

    let summary = Signal::derive(move || {
        let slot = slot_store.get_value();
        let rows = drafts.get();
        templates_store.with_value(|ts| attachment_editor::summarize_channel(&slot, &rows, ts))
    });

    let is_dirty = Signal::derive(move || {
        initial_store.with_value(|saved| drafts.get() != *saved)
    });

    let did_stored = StoredValue::new(String::new());
    Effect::new(move |_| {
        did_stored.set_value(device_id.get());
    });

    // ── Add handlers ────────────────────────────────────────────────────────

    let add_strip = move |_: web_sys::MouseEvent| {
        set_drafts.update(|rows| rows.push(DraftRow::new_strip(60)));
    };

    let add_matrix = move |_: web_sys::MouseEvent| {
        set_drafts.update(|rows| rows.push(DraftRow::new_matrix(8, 8)));
    };

    let on_component_selected = Callback::new(move |(template_id, name): (String, String)| {
        set_drafts.update(|rows| {
            rows.push(DraftRow::from_component(template_id, name));
        });
    });

    let on_row_update = Callback::new(move |(index, updated): (usize, DraftRow)| {
        set_drafts.update(|rows| {
            if let Some(row) = rows.get_mut(index) {
                *row = updated;
            }
        });
    });

    let on_row_delete = Callback::new(move |index: usize| {
        set_drafts.update(|rows| {
            if index < rows.len() {
                rows.remove(index);
            }
        });
    });

    // ── Save handler ────────────────────────────────────────────────────────

    let do_save = move |_: web_sys::MouseEvent| {
        let rows = drafts.get_untracked();
        let slot = slot_store.get_value();
        let slot_id = slot.id.clone();
        let did = did_stored.get_value();
        let layouts_resource = ctx.layouts_resource;
        let device_for_sync = device.get_untracked();
        let templates = templates_store.with_value(|ts| ts.clone());

        set_save_in_flight.set(true);

        leptos::task::spawn_local(async move {
            let result = async {
                // Create templates for any custom strips/matrices
                let mut template_id_map: std::collections::HashMap<usize, String> =
                    std::collections::HashMap::new();

                for (i, row) in rows.iter().enumerate() {
                    if !row.needs_template_creation() {
                        continue;
                    }
                    let (name, category, topology, desc) = match &row.kind {
                        ComponentDraft::Strip { led_count } => {
                            let name = if row.name.is_empty() {
                                format!("Custom Strip - {led_count} LEDs")
                            } else {
                                row.name.clone()
                            };
                            (
                                name,
                                hypercolor_types::attachment::AttachmentCategory::Strip,
                                hypercolor_types::spatial::LedTopology::Strip {
                                    count: *led_count,
                                    direction: hypercolor_types::spatial::StripDirection::LeftToRight,
                                },
                                format!("Custom LED strip, {led_count} LEDs"),
                            )
                        }
                        ComponentDraft::Matrix { cols, rows } => {
                            let total = cols * rows;
                            let name = if row.name.is_empty() {
                                format!("Custom Matrix - {cols}\u{00d7}{rows}")
                            } else {
                                row.name.clone()
                            };
                            (
                                name,
                                hypercolor_types::attachment::AttachmentCategory::Matrix,
                                hypercolor_types::spatial::LedTopology::Matrix {
                                    width: *cols,
                                    height: *rows,
                                    serpentine: true,
                                    start_corner: hypercolor_types::spatial::Corner::TopLeft,
                                },
                                format!("Custom {cols}\u{00d7}{rows} LED matrix, {total} LEDs"),
                            )
                        }
                        ComponentDraft::Component { .. } => continue,
                    };

                    let id = format!(
                        "custom-{}-{}-{}",
                        category.as_str(),
                        js_sys::Date::now() as u64,
                        i
                    );

                    let template = hypercolor_types::attachment::AttachmentTemplate {
                        id: id.clone(),
                        name,
                        category,
                        origin: hypercolor_types::attachment::AttachmentOrigin::User,
                        description: desc,
                        vendor: "Custom".to_string(),
                        default_size: hypercolor_types::attachment::AttachmentCanvasSize {
                            width: 0.24,
                            height: 0.08,
                        },
                        topology,
                        compatible_slots: Vec::new(),
                        tags: vec!["custom".to_string()],
                        led_names: None,
                        led_mapping: None,
                        image_url: None,
                        physical_size_mm: None,
                    };

                    api::create_attachment_template(&template).await?;
                    template_id_map.insert(i, id);
                }

                // Build the attachment bindings
                let current = api::fetch_device_attachments(&did).await?;
                let mut api_bindings: Vec<api::AttachmentBindingRequest> = current
                    .bindings
                    .iter()
                    .filter(|b| b.slot_id != slot_id)
                    .map(|b| api::AttachmentBindingRequest {
                        slot_id: b.slot_id.clone(),
                        template_id: b.template_id.clone(),
                        name: b.name.clone(),
                        enabled: b.enabled,
                        instances: b.instances,
                        led_offset: b.led_offset,
                    })
                    .collect();

                let mut led_offset = 0_u32;
                for (i, row) in rows.iter().enumerate() {
                    let template_id = match &row.kind {
                        ComponentDraft::Component { template_id } => template_id.clone(),
                        _ => {
                            template_id_map.get(&i).cloned().unwrap_or_default()
                        }
                    };

                    let led_count = row.led_count(&templates).unwrap_or(0);
                    let name = if row.name.is_empty() {
                        None
                    } else {
                        Some(row.name.clone())
                    };

                    api_bindings.push(api::AttachmentBindingRequest {
                        slot_id: slot_id.clone(),
                        template_id,
                        name,
                        enabled: true,
                        instances: 1,
                        led_offset,
                    });

                    led_offset += led_count;
                }

                api::update_device_attachments(
                    &did,
                    &api::UpdateAttachmentsRequest {
                        bindings: api_bindings,
                    },
                )
                .await
            }
            .await;

            set_save_in_flight.set(false);

            match result {
                Ok(updated) => {
                    toasts::toast_success("Saved");
                    on_saved.run(());

                    if let Some(dev) = device_for_sync {
                        if !updated.suggested_zones.is_empty() {
                            attachment_panel::sync_wiring_to_layout(
                                dev,
                                updated.suggested_zones,
                                layouts_resource,
                            );
                        }
                    }
                }
                Err(e) => toasts::toast_error(&format!("Save failed: {e}")),
            }
        });
    };

    // ── Render ───────────────────────────────────────────────────────────────

    let did_for_rows = StoredValue::new(String::new());
    Effect::new(move |_| {
        did_for_rows.set_value(device_id.get());
    });
    let slot_id_for_rows = slot.id.clone();

    view! {
        <div class="space-y-2 px-2.5 pb-2.5 pt-1">
            // Component rows
            {move || {
                let rows = drafts.get();
                let ts = templates_store.with_value(|t| t.clone());
                let did = did_for_rows.get_value();
                let sid = slot_id_for_rows.clone();

                if rows.is_empty() {
                    view! {
                        <div class="text-[10px] text-fg-tertiary/40 text-center py-3">"No components"</div>
                    }.into_any()
                } else {
                    rows.into_iter().enumerate().map(|(i, row)| {
                        view! {
                            <ComponentRow
                                row=row
                                index=i
                                device_id=did.clone()
                                slot_id=sid.clone()
                                on_update=on_row_update
                                on_delete=on_row_delete
                                templates=ts.clone()
                            />
                        }
                    }).collect_view().into_any()
                }
            }}

            // Add buttons + save
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
                        components=all_templates
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
    }
}
