//! Device attachment panel — visual hardware attachment configuration with auto-save.

use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_types::attachment::AttachmentSuggestedZone;
use hypercolor_types::spatial::{
    DeviceZone, LedTopology, NormalizedPosition, Orientation, ZoneAttachment,
};

use crate::api;
use crate::app::DevicesContext;
use crate::components::attachment_editor;
use crate::icons::*;
use crate::layout_geometry;
use crate::toasts;

/// Category → shape SVG for inline attachment visualization.
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

/// Attachment panel for a selected device.
#[component]
pub fn AttachmentPanel(
    #[prop(into)] device_id: Signal<String>,
    #[prop(into)] device: Signal<Option<api::DeviceSummary>>,
) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();
    let (import_in_flight, set_import_in_flight) = signal(false);
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

    let templates = LocalResource::new(move || async move {
        api::fetch_attachment_templates(None)
            .await
            .unwrap_or_default()
    });

    let import_to_layout = move || {
        if import_in_flight.get_untracked() {
            return;
        }
        let Some(device) = device.get_untracked() else {
            return;
        };
        set_import_in_flight.set(true);
        let layouts_resource = ctx.layouts_resource;
        leptos::task::spawn_local(async move {
            let result: Result<(usize, String), String> = async {
                let attachments = api::fetch_device_attachments(&device.id).await?;
                if attachments.suggested_zones.is_empty() {
                    return Ok((0_usize, String::new()));
                }
                let mut layout = api::fetch_active_layout().await?;
                let layout_name = layout.name.clone();
                let layout_id = layout.id.clone();
                let imported_zones =
                    build_attachment_layout_zones(&device, &attachments.suggested_zones);
                let imported_count = imported_zones.len();
                layout.zones.retain(|zone| {
                    !(zone.device_id == device.layout_device_id && zone.attachment.is_some())
                });
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
                Ok((imported_count, layout_name))
            }
            .await;

            set_import_in_flight.set(false);
            match result {
                Ok((0, _)) => toasts::toast_info("No attachment zones to import"),
                Ok((count, layout_name)) => {
                    layouts_resource.refetch();
                    let noun = if count == 1 { "zone" } else { "zones" };
                    toasts::toast_success(&format!("Imported {count} {noun} into {layout_name}"));
                }
                Err(error) => {
                    toasts::toast_error(&format!("Import failed: {error}"));
                }
            }
        });
    };

    view! {
        <div class="rounded-xl bg-surface-raised border border-edge-subtle overflow-hidden edge-glow">
            // Header
            <div class="flex items-center justify-between px-4 py-2.5 border-b border-edge-subtle">
                <div class="flex items-center gap-2">
                    <Icon icon=LuLayoutTemplate width="12px" height="12px" style="color: rgba(128, 255, 234, 0.7)" />
                    <h3 class="text-[10px] font-mono uppercase tracking-[0.12em] text-fg-tertiary">"Attachments"</h3>
                </div>
                <button
                    class="px-2 py-0.5 rounded-md text-[9px] font-medium transition-all btn-press disabled:opacity-40"
                    style="color: rgba(128, 255, 234, 0.7); background: rgba(128, 255, 234, 0.06)"
                    disabled=move || import_in_flight.get()
                    on:click=move |_| import_to_layout()
                >
                    {move || if import_in_flight.get() { "Importing..." } else { "Sync to Layout" }}
                </button>
            </div>

            <div class="p-3">
                <Suspense fallback=|| view! {
                    <div class="text-[10px] text-fg-tertiary animate-pulse py-2">"Loading..."</div>
                }>
                    {move || {
                        let all_templates = templates.get().map(|loaded| loaded.to_vec()).unwrap_or_default();

                        attachments.get().map(|result| match result {
                            Ok(profile) => {
                                let slots = profile.slots.clone();
                                let bindings = profile.bindings.clone();

                                if slots.is_empty() {
                                    return view! {
                                        <div class="text-[10px] text-fg-tertiary/50 text-center py-3">
                                            "No attachment slots"
                                        </div>
                                    }
                                    .into_any();
                                }

                                view! {
                                    <div class="space-y-1.5">
                                        {slots.into_iter().map(|slot| {
                                            let slot_id = slot.id.clone();
                                            let slot_bindings = bindings
                                                .iter()
                                                .filter(|binding| binding.slot_id == slot.id)
                                                .cloned()
                                                .collect::<Vec<_>>();
                                            let used_leds: u32 = slot_bindings
                                                .iter()
                                                .map(|binding| binding.effective_led_count)
                                                .sum();
                                            let attachment_count: u32 = slot_bindings
                                                .iter()
                                                .map(|binding| binding.instances.max(1))
                                                .sum();

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

                                            let initial_rows =
                                                attachment_editor::expand_slot_bindings(
                                                    &slot.id,
                                                    &bindings,
                                                );
                                            let initial_rows_store =
                                                StoredValue::new(initial_rows.clone());
                                            let (draft_rows, set_draft_rows) =
                                                signal(initial_rows.clone());

                                            let slot_categories = slot.suggested_categories.clone();
                                            let relevant_templates = if slot_categories.is_empty() {
                                                all_templates.clone()
                                            } else {
                                                let matching = all_templates
                                                    .iter()
                                                    .filter(|template| {
                                                        slot_categories.iter().any(|category| {
                                                            category.matches_category(
                                                                &template.category,
                                                            )
                                                        })
                                                    })
                                                    .cloned()
                                                    .collect::<Vec<_>>();
                                                let matching_ids = matching
                                                    .iter()
                                                    .map(|template| template.id.clone())
                                                    .collect::<std::collections::HashSet<_>>();
                                                let mut ordered = matching;
                                                ordered.extend(
                                                    all_templates
                                                        .iter()
                                                        .filter(|template| {
                                                            !matching_ids.contains(
                                                                &template.id,
                                                            )
                                                        })
                                                        .cloned(),
                                                );
                                                ordered
                                            };
                                            let category_count = relevant_templates
                                                .iter()
                                                .take_while(|template| {
                                                    slot_categories.is_empty()
                                                        || slot_categories.iter().any(|category| {
                                                            category.matches_category(
                                                                &template.category,
                                                            )
                                                        })
                                                })
                                                .count();
                                            let templates_store =
                                                StoredValue::new(relevant_templates.clone());

                                            let draft_summary = Signal::derive({
                                                let slot = slot.clone();
                                                move || {
                                                    templates_store.with_value(|templates| {
                                                        attachment_editor::summarize_slot_rows(
                                                            &slot,
                                                            &draft_rows.get(),
                                                            templates,
                                                        )
                                                    })
                                                }
                                            });
                                            let draft_is_dirty = Signal::derive(move || {
                                                initial_rows_store.with_value(|saved| {
                                                    draft_rows.get() != *saved
                                                })
                                            });

                                            let add_row = {
                                                move |_: web_sys::MouseEvent| {
                                                    set_draft_rows.update(|rows| {
                                                        rows.push(
                                                            attachment_editor::AttachmentDraftRow::empty(),
                                                        );
                                                    });
                                                }
                                            };

                                            // Visual summary of attached items when collapsed
                                            let bound_shapes: Vec<(String, String, u32)> = slot_bindings
                                                .iter()
                                                .map(|binding| {
                                                    let cat = all_templates
                                                        .iter()
                                                        .find(|t| t.id == binding.template_id)
                                                        .map(|t| t.category.as_str().to_string())
                                                        .unwrap_or_else(|| "other".to_string());
                                                    let name = binding.name.clone()
                                                        .unwrap_or_else(|| binding.template_name.clone());
                                                    (cat, name, binding.instances)
                                                })
                                                .collect();

                                            view! {
                                                <div class="rounded-lg border border-edge-subtle bg-surface-overlay/15 overflow-hidden transition-all">
                                                    // Slot header — always visible
                                                    <button
                                                        class="w-full px-3 py-2 text-left hover:bg-surface-hover/20 transition-colors"
                                                        on:click=toggle_slot
                                                    >
                                                        <div class="flex items-center justify-between gap-2">
                                                            <div class="flex items-center gap-2 min-w-0">
                                                                <span class="text-[11px] font-medium text-fg-primary truncate">{slot.name.clone()}</span>
                                                                {(used_leds > 0).then(|| view! {
                                                                    <span class="text-[9px] font-mono text-fg-tertiary/40 tabular-nums shrink-0">
                                                                        {used_leds} "/" {slot.led_count}
                                                                    </span>
                                                                })}
                                                            </div>
                                                            <div class="flex items-center gap-1.5 shrink-0">
                                                                {if is_expanded() {
                                                                    view! {
                                                                        <Icon icon=LuChevronUp width="11px" height="11px" style="color: rgba(139, 133, 160, 0.5)" />
                                                                    }.into_any()
                                                                } else {
                                                                    view! {
                                                                        <Icon icon=LuChevronDown width="11px" height="11px" style="color: rgba(139, 133, 160, 0.5)" />
                                                                    }.into_any()
                                                                }}
                                                            </div>
                                                        </div>

                                                        // Collapsed: show attached shapes inline
                                                        {(!is_expanded() && attachment_count > 0).then(|| {
                                                            let shapes = bound_shapes.clone();
                                                            view! {
                                                                <div class="flex items-center gap-2 mt-1.5" style="color: rgba(128, 255, 234, 0.5)">
                                                                    {shapes.into_iter().map(|(cat, name, instances)| {
                                                                        let svg = category_shape_svg(&cat, 16);
                                                                        let label = if instances > 1 {
                                                                            format!("{name} \u{00d7}{instances}")
                                                                        } else {
                                                                            name
                                                                        };
                                                                        view! {
                                                                            <div class="flex items-center gap-1">
                                                                                <div class="w-4 h-4 shrink-0" inner_html=svg />
                                                                                <span class="text-[10px] text-fg-tertiary/60 truncate max-w-[100px]">{label}</span>
                                                                            </div>
                                                                        }
                                                                    }).collect_view()}
                                                                </div>
                                                            }
                                                        })}

                                                        // Collapsed: empty hint
                                                        {(!is_expanded() && attachment_count == 0).then(|| {
                                                            let hint_count = slot.suggested_categories.len();
                                                            let hint_shapes: Vec<String> = slot.suggested_categories
                                                                .iter()
                                                                .take(4)
                                                                .map(|cat| category_shape_svg(cat.as_str(), 14))
                                                                .collect();
                                                            view! {
                                                                <div class="flex items-center gap-1.5 mt-1.5" style="color: rgba(139, 133, 160, 0.25)">
                                                                    {hint_shapes.into_iter().map(|svg| view! {
                                                                        <div class="w-3.5 h-3.5" inner_html=svg />
                                                                    }).collect_view()}
                                                                    {(hint_count > 0).then(|| view! {
                                                                        <span class="text-[9px] font-mono text-fg-tertiary/25">
                                                                            {hint_count} " suggested"
                                                                        </span>
                                                                    })}
                                                                </div>
                                                            }
                                                        })}
                                                    </button>

                                                    // Expanded: attachment rows
                                                    {
                                                    let slot_for_save = StoredValue::new(slot.clone());
                                                    let slot_id_for_save = StoredValue::new(slot_id.clone());
                                                    view! {
                                                    <Show when=is_expanded>
                                                        <div class="px-3 pb-2.5 space-y-2 border-t border-edge-subtle bg-surface-sunken/20 animate-fade-in">
                                                            <div class="pt-2 space-y-1.5">
                                                                {move || {
                                                                    let rows = draft_rows.get();
                                                                    let summary = draft_summary.get();

                                                                    if rows.is_empty() {
                                                                        return view! {
                                                                            <div class="text-[10px] text-fg-tertiary/40 text-center py-2">
                                                                                "No attachments"
                                                                            </div>
                                                                        }.into_any();
                                                                    }

                                                                    rows.into_iter()
                                                                        .enumerate()
                                                                        .map(|(index, row)| {
                                                                            let placement = summary
                                                                                .rows
                                                                                .get(index)
                                                                                .cloned()
                                                                                .flatten();
                                                                            let template_info = templates_store
                                                                                .with_value(|templates| {
                                                                                    templates
                                                                                        .iter()
                                                                                        .find(|template| {
                                                                                            template.id == row.template_id
                                                                                        })
                                                                                        .cloned()
                                                                                });
                                                                            let options = templates_store
                                                                                .with_value(|templates| {
                                                                                    templates.clone()
                                                                                });
                                                                            let cat_str = template_info
                                                                                .as_ref()
                                                                                .map(|t| t.category.as_str().to_string())
                                                                                .unwrap_or_else(|| "other".to_string());
                                                                            let shape_svg = category_shape_svg(&cat_str, 20);

                                                                            view! {
                                                                                <div class="flex items-center gap-2 group/row">
                                                                                    // Shape icon
                                                                                    <div class="w-5 h-5 shrink-0" style="color: rgba(128, 255, 234, 0.4)" inner_html=shape_svg />

                                                                                    // Template selector
                                                                                    <select
                                                                                        class="flex-1 bg-surface-overlay/40 border border-edge-subtle rounded px-2 py-1 text-[11px] text-fg-primary
                                                                                               focus:outline-none focus:border-accent-muted cursor-pointer min-w-0"
                                                                                        prop:value=row.template_id.clone()
                                                                                        on:change={
                                                                                            let set_rows = set_draft_rows;
                                                                                            move |ev| {
                                                                                                let value = event_target_value(&ev);
                                                                                                set_rows.update(|rows| {
                                                                                                    if let Some(row) = rows.get_mut(index) {
                                                                                                        row.template_id = value.clone();
                                                                                                    }
                                                                                                });
                                                                                            }
                                                                                        }
                                                                                    >
                                                                                        <option value="">"Select..."</option>
                                                                                        {if category_count > 0 && category_count < options.len() {
                                                                                            let suggested = options[..category_count].to_vec();
                                                                                            let others = options[category_count..].to_vec();
                                                                                            view! {
                                                                                                <optgroup label="Suggested">
                                                                                                    {suggested.into_iter().map(|template| {
                                                                                                        let id = template.id.clone();
                                                                                                        let label = format!("{} \u{2014} {}", template.name, template.led_count);
                                                                                                        view! { <option value=id>{label}</option> }
                                                                                                    }).collect_view()}
                                                                                                </optgroup>
                                                                                                <optgroup label="All">
                                                                                                    {others.into_iter().map(|template| {
                                                                                                        let id = template.id.clone();
                                                                                                        let label = format!("{} \u{2014} {}", template.name, template.led_count);
                                                                                                        view! { <option value=id>{label}</option> }
                                                                                                    }).collect_view()}
                                                                                                </optgroup>
                                                                                            }.into_any()
                                                                                        } else {
                                                                                            options.into_iter().map(|template| {
                                                                                                let id = template.id.clone();
                                                                                                let label = format!("{} \u{2014} {}", template.name, template.led_count);
                                                                                                view! { <option value=id>{label}</option> }
                                                                                            }).collect_view().into_any()
                                                                                        }}
                                                                                    </select>

                                                                                    // LED range (only when placed)
                                                                                    {placement.map(|p| view! {
                                                                                        <span class="text-[8px] font-mono text-fg-tertiary/30 tabular-nums shrink-0 w-10 text-right">
                                                                                            {p.led_offset} "-" {p.led_end.saturating_sub(1)}
                                                                                        </span>
                                                                                    })}

                                                                                    // Delete — visible on hover
                                                                                    <button
                                                                                        class="w-5 h-5 flex items-center justify-center rounded shrink-0
                                                                                               opacity-0 group-hover/row:opacity-100 transition-opacity
                                                                                               text-fg-tertiary/40 hover:text-error-red"
                                                                                        on:click={
                                                                                            let set_rows = set_draft_rows;
                                                                                            move |ev: web_sys::MouseEvent| {
                                                                                                ev.stop_propagation();
                                                                                                set_rows.update(|rows| {
                                                                                                    if index < rows.len() {
                                                                                                        rows.remove(index);
                                                                                                    }
                                                                                                });
                                                                                            }
                                                                                        }
                                                                                    >
                                                                                        <Icon icon=LuX width="10px" height="10px" />
                                                                                    </button>
                                                                                </div>
                                                                            }
                                                                        })
                                                                        .collect_view()
                                                                        .into_any()
                                                                }}
                                                            </div>

                                                            // Footer: add + save
                                                            <div class="flex items-center justify-between pt-1">
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
                                                                        if summary.overflow_leds > 0 {
                                                                            Some(view! {
                                                                                <span class="text-[9px] font-mono" style="color: rgb(255, 99, 99)">
                                                                                    {summary.overflow_leds} " over"
                                                                                </span>
                                                                            })
                                                                        } else {
                                                                            None
                                                                        }
                                                                    }}
                                                                    <Show when=move || draft_is_dirty.get()>
                                                                        <button
                                                                            class="text-[9px] font-medium px-1.5 py-0.5 rounded transition-all btn-press disabled:opacity-30"
                                                                            style="color: rgba(128, 255, 234, 0.7); background: rgba(128, 255, 234, 0.08)"
                                                                            disabled=move || {
                                                                                let summary = draft_summary.get();
                                                                                save_in_flight.get() || !summary.is_valid()
                                                                            }
                                                                            on:click={
                                                                                move |_: web_sys::MouseEvent| {
                                                                                let slot = slot_for_save.get_value();
                                                                                let slot_id = slot_id_for_save.get_value();
                                                                                    let packed_rows = match templates_store
                                                                                        .with_value(|templates| {
                                                                                            attachment_editor::pack_slot_rows(
                                                                                                &slot,
                                                                                                &draft_rows.get_untracked(),
                                                                                                templates,
                                                                                            )
                                                                                        }) {
                                                                                        Ok(packed_rows) => packed_rows,
                                                                                        Err(error) => {
                                                                                            toasts::toast_error(&error);
                                                                                            return;
                                                                                        }
                                                                                    };

                                                                                    set_save_in_flight.set(true);
                                                                                    let did = device_id.get_untracked();
                                                                                    let slot_id = slot_id.clone();
                                                                                    leptos::task::spawn_local(async move {
                                                                                        let result = async {
                                                                                            let current =
                                                                                                api::fetch_device_attachments(&did)
                                                                                                    .await?;
                                                                                            let mut api_bindings = current
                                                                                                .bindings
                                                                                                .iter()
                                                                                                .filter(|binding| {
                                                                                                    binding.slot_id != slot_id
                                                                                                })
                                                                                                .map(|binding| {
                                                                                                    api::AttachmentBindingRequest {
                                                                                                        slot_id: binding.slot_id.clone(),
                                                                                                        template_id: binding.template_id.clone(),
                                                                                                        name: binding.name.clone(),
                                                                                                        enabled: binding.enabled,
                                                                                                        instances: binding.instances,
                                                                                                        led_offset: binding.led_offset,
                                                                                                    }
                                                                                                })
                                                                                                .collect::<Vec<_>>();

                                                                                            api_bindings.extend(
                                                                                                packed_rows.into_iter().map(
                                                                                                    |binding| {
                                                                                                        api::AttachmentBindingRequest {
                                                                                                            slot_id: slot_id.clone(),
                                                                                                            template_id: binding.template_id,
                                                                                                            name: binding.name,
                                                                                                            enabled: true,
                                                                                                            instances: 1,
                                                                                                            led_offset: binding.led_offset,
                                                                                                        }
                                                                                                    },
                                                                                                ),
                                                                                            );

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
                                                                                                let saved_rows =
                                                                                                    attachment_editor::expand_slot_bindings(
                                                                                                        &slot_id,
                                                                                                        &updated.bindings,
                                                                                                    );
                                                                                                initial_rows_store
                                                                                                    .set_value(saved_rows.clone());
                                                                                                set_draft_rows.set(saved_rows);
                                                                                                toasts::toast_success("Saved");
                                                                                                set_refetch_tick
                                                                                                    .update(|tick| *tick += 1);
                                                                                            }
                                                                                            Err(error) => {
                                                                                                toasts::toast_error(
                                                                                                    &format!("Save failed: {error}"),
                                                                                                );
                                                                                            }
                                                                                        }
                                                                                    });
                                                                                }
                                                                            }
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
                                }
                                    .into_any()
                            }
                            Err(error) => view! {
                                <div class="text-[10px] text-error-red py-2">{error}</div>
                            }
                            .into_any(),
                        })
                    }}
                </Suspense>
            </div>
        </div>
    }
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
