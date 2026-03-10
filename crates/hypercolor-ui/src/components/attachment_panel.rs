//! Device attachment configuration panel — configure templates per slot, then import to layout.

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

/// Attachment panel for a selected device — configure bindings and import to layout.
#[component]
pub fn AttachmentPanel(
    #[prop(into)] device_id: Signal<String>,
    #[prop(into)] device: Signal<Option<api::DeviceSummary>>,
) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();
    let (import_in_flight, set_import_in_flight) = signal(false);
    let (expanded_slot, set_expanded_slot) = signal(Option::<String>::None);
    let (save_in_flight, set_save_in_flight) = signal(false);

    // Bump this to force-refetch attachments after a save.
    let (refetch_tick, set_refetch_tick) = signal(0_u32);

    let attachments = LocalResource::new(move || {
        let id = device_id.get();
        refetch_tick.get(); // subscribe to refetch trigger
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
                Ok((0, _)) => toasts::toast_info("No attachment zones ready to import"),
                Ok((count, layout_name)) => {
                    layouts_resource.refetch();
                    let noun = if count == 1 { "zone" } else { "zones" };
                    toasts::toast_success(&format!(
                        "Imported {count} attachment {noun} into {layout_name}"
                    ));
                }
                Err(error) => {
                    toasts::toast_error(&format!("Attachment import failed: {error}"));
                }
            }
        });
    };

    view! {
        <div class="rounded-xl bg-surface-raised border border-edge-subtle overflow-hidden edge-glow">
            <div class="flex items-center justify-between px-4 py-3 border-b border-edge-subtle">
                <div class="flex items-center gap-2">
                    <Icon icon=LuLayoutTemplate width="14px" height="14px" style="color: rgba(128, 255, 234, 0.95)" />
                    <h3 class="text-xs font-mono uppercase tracking-[0.12em] text-fg-tertiary">
                        "Attachments"
                    </h3>
                </div>
                <button
                    class="px-2 py-1 rounded-md text-[10px] font-medium transition-all btn-press disabled:opacity-50 disabled:cursor-not-allowed"
                    style="background: rgba(128, 255, 234, 0.08); border: 1px solid rgba(128, 255, 234, 0.16); color: rgb(128, 255, 234)"
                    disabled=move || import_in_flight.get()
                    on:click=move |_| import_to_layout()
                >
                    {move || if import_in_flight.get() { "Importing..." } else { "Import" }}
                </button>
            </div>

            <div class="p-4">
                <Suspense fallback=|| view! {
                    <div class="text-xs text-fg-tertiary animate-pulse">"Loading attachments..."</div>
                }>
                    {move || {
                        let all_templates = templates.get().map(|loaded| loaded.to_vec()).unwrap_or_default();

                        attachments.get().map(|result| match result {
                            Ok(profile) => {
                                let slots = profile.slots.clone();
                                let bindings = profile.bindings.clone();
                                let total_attachments: u32 = bindings
                                    .iter()
                                    .map(|binding| binding.instances.max(1))
                                    .sum();
                                let ready_count = profile.suggested_zones.len();

                                if slots.is_empty() {
                                    return view! {
                                        <div class="text-xs text-fg-tertiary text-center py-3">
                                            "No attachment slots reported for this device"
                                        </div>
                                    }
                                    .into_any();
                                }

                                view! {
                                    <div class="space-y-3">
                                        <div class="flex items-center justify-between text-[11px] font-mono text-fg-tertiary">
                                            <span>{total_attachments} " attached"</span>
                                            <span>{ready_count} " ready for layout"</span>
                                        </div>

                                        <div class="space-y-2">
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
                                                let binding_summary = slot_bindings
                                                    .iter()
                                                    .map(|binding| {
                                                        let name = binding
                                                            .name
                                                            .clone()
                                                            .unwrap_or_else(|| binding.template_name.clone());
                                                        if binding.instances > 1 {
                                                            format!("{name} ×{}", binding.instances)
                                                        } else {
                                                            name
                                                        }
                                                    })
                                                    .collect::<Vec<_>>()
                                                    .join(", ");

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

                                                let reset_rows = {
                                                    move |_: web_sys::MouseEvent| {
                                                        initial_rows_store.with_value(|saved| {
                                                            set_draft_rows.set(saved.clone());
                                                        });
                                                    }
                                                };
                                                let add_row = {
                                                    move |_: web_sys::MouseEvent| {
                                                        set_draft_rows.update(|rows| {
                                                            rows.push(
                                                                attachment_editor::AttachmentDraftRow::empty(),
                                                            );
                                                        });
                                                    }
                                                };
                                                view! {
                                                    <div class="rounded-lg border border-edge-subtle bg-surface-overlay/20 overflow-hidden transition-all">
                                                        <button
                                                            class="w-full px-3 py-2.5 text-left hover:bg-surface-hover/30 transition-colors"
                                                            on:click=toggle_slot
                                                        >
                                                            <div class="flex items-start justify-between gap-3">
                                                                <div class="min-w-0">
                                                                    <div class="text-xs font-medium text-fg-primary">
                                                                        {slot.name.clone()}
                                                                    </div>
                                                                    <div class="text-[11px] font-mono text-fg-tertiary">
                                                                        {slot.id.clone()} " · " {used_leds} "/" {slot.led_count} " LEDs"
                                                                    </div>
                                                                </div>
                                                                <div class="flex items-center gap-2 shrink-0">
                                                                    {if attachment_count > 0 {
                                                                        view! {
                                                                            <span class="rounded-full bg-[rgba(128,255,234,0.1)] border border-[rgba(128,255,234,0.2)] px-2 py-0.5 text-[10px] font-mono"
                                                                                style="color: rgb(128, 255, 234)"
                                                                            >
                                                                                {attachment_count} " attached"
                                                                            </span>
                                                                        }
                                                                            .into_any()
                                                                    } else {
                                                                        view! {
                                                                            <span class="rounded-full border border-edge-subtle bg-surface-overlay/40 px-2 py-0.5 text-[10px] font-mono text-fg-tertiary">
                                                                                {slot.suggested_categories.len()} " hints"
                                                                            </span>
                                                                        }
                                                                            .into_any()
                                                                    }}
                                                                    {if is_expanded() {
                                                                        view! {
                                                                            <Icon icon=LuChevronUp width="12px" height="12px" style="color: rgba(139, 133, 160, 0.7)" />
                                                                        }
                                                                            .into_any()
                                                                    } else {
                                                                        view! {
                                                                            <Icon icon=LuChevronDown width="12px" height="12px" style="color: rgba(139, 133, 160, 0.7)" />
                                                                        }
                                                                            .into_any()
                                                                    }}
                                                                </div>
                                                            </div>

                                                            {if attachment_count > 0 && !is_expanded() {
                                                                Some(view! {
                                                                    <div class="mt-1.5 text-[11px] text-fg-tertiary truncate">
                                                                        {binding_summary.clone()}
                                                                    </div>
                                                                }
                                                                .into_any())
                                                            } else if !is_expanded() {
                                                                Some(view! {
                                                                    <div class="mt-1.5 text-[11px] text-fg-tertiary">
                                                                        "Add attachments in order. Hypercolor will pack the LEDs automatically."
                                                                    </div>
                                                                }
                                                                .into_any())
                                                            } else {
                                                                None
                                                            }}
                                                        </button>

                                                        <Show when=is_expanded>
                                                            <div class="px-3 pb-3 space-y-3 border-t border-edge-subtle bg-surface-sunken/30 animate-fade-in">
                                                                <div class="pt-3 flex items-center justify-between gap-3">
                                                                    <div class="text-[11px] text-fg-tertiary">
                                                                        "Add attachments in order. LED ranges are packed automatically."
                                                                    </div>
                                                                    <div class="text-[10px] font-mono text-fg-tertiary whitespace-nowrap">
                                                                        {move || {
                                                                            let summary = draft_summary.get();
                                                                            format!(
                                                                                "{} / {} LEDs",
                                                                                summary.total_leds,
                                                                                summary.available_leds
                                                                            )
                                                                        }}
                                                                    </div>
                                                                </div>

                                                                <div class="space-y-2">
                                                                    {move || {
                                                                        let rows = draft_rows.get();
                                                                        let summary = draft_summary.get();

                                                                        if rows.is_empty() {
                                                                            return view! {
                                                                                <div class="rounded-lg border border-dashed border-edge-subtle bg-surface-base/30 px-3 py-3 text-[11px] text-fg-tertiary">
                                                                                    "No attachments on this channel yet."
                                                                                </div>
                                                                            }
                                                                            .into_any();
                                                                        }

                                                                        rows.into_iter()
                                                                            .enumerate()
                                                                            .map(|(index, row)| {
                                                                                let placement = summary
                                                                                    .rows
                                                                                    .get(index)
                                                                                    .cloned()
                                                                                    .flatten();
                                                                                let placement_label =
                                                                                    placement
                                                                                        .clone();
                                                                                let placement_style =
                                                                                    placement
                                                                                        .clone();
                                                                                let template_info = templates_store
                                                                                    .with_value(|templates| {
                                                                                        templates
                                                                                            .iter()
                                                                                            .find(|template| {
                                                                                                template.id
                                                                                                    == row.template_id
                                                                                            })
                                                                                            .cloned()
                                                                                    });
                                                                                let options = templates_store
                                                                                    .with_value(|templates| {
                                                                                        templates.clone()
                                                                                    });

                                                                                view! {
                                                                                    <div class="rounded-lg border border-edge-subtle bg-surface-base/40 px-2.5 py-2 space-y-2">
                                                                                        <div class="flex items-center justify-between gap-2">
                                                                                            <div class="text-[10px] font-mono text-fg-tertiary">
                                                                                                "Attachment " {index + 1}
                                                                                            </div>
                                                                                            <div class="flex items-center gap-1">
                                                                                                <button
                                                                                                    class="w-6 h-6 flex items-center justify-center rounded border border-edge-subtle bg-surface-overlay/40 text-fg-tertiary hover:text-fg-primary hover:border-edge-default transition-colors btn-press disabled:opacity-30"
                                                                                                    disabled=index == 0
                                                                                                    on:click={
                                                                                                        let set_rows = set_draft_rows;
                                                                                                        move |ev: web_sys::MouseEvent| {
                                                                                                            ev.stop_propagation();
                                                                                                            set_rows.update(|rows| {
                                                                                                                if index > 0 {
                                                                                                                    rows.swap(index - 1, index);
                                                                                                                }
                                                                                                            });
                                                                                                        }
                                                                                                    }
                                                                                                >
                                                                                                    <Icon icon=LuChevronUp width="10px" height="10px" style="color: inherit" />
                                                                                                </button>
                                                                                                <button
                                                                                                    class="w-6 h-6 flex items-center justify-center rounded border border-edge-subtle bg-surface-overlay/40 text-fg-tertiary hover:text-fg-primary hover:border-edge-default transition-colors btn-press disabled:opacity-30"
                                                                                                    disabled=move || { index + 1 >= draft_rows.get().len() }
                                                                                                    on:click={
                                                                                                        let set_rows = set_draft_rows;
                                                                                                        move |ev: web_sys::MouseEvent| {
                                                                                                            ev.stop_propagation();
                                                                                                            set_rows.update(|rows| {
                                                                                                                if index + 1 < rows.len() {
                                                                                                                    rows.swap(index, index + 1);
                                                                                                                }
                                                                                                            });
                                                                                                        }
                                                                                                    }
                                                                                                >
                                                                                                    <Icon icon=LuChevronDown width="10px" height="10px" style="color: inherit" />
                                                                                                </button>
                                                                                                <button
                                                                                                    class="w-6 h-6 flex items-center justify-center rounded border border-[rgba(255,99,99,0.16)] bg-[rgba(255,99,99,0.06)] text-[rgb(255,99,99)] transition-colors btn-press"
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
                                                                                                    <Icon icon=LuX width="10px" height="10px" style="color: inherit" />
                                                                                                </button>
                                                                                            </div>
                                                                                        </div>

                                                                                        <select
                                                                                            class="w-full bg-surface-overlay/60 border border-edge-subtle rounded-lg px-3 py-1.5 text-xs text-fg-primary focus:outline-none focus:border-accent-muted cursor-pointer"
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
                                                                                            <option value="">
                                                                                                "Select an attachment..."
                                                                                            </option>
                                                                                            {if category_count > 0 && category_count < options.len() {
                                                                                                let suggested =
                                                                                                    options[..category_count]
                                                                                                        .to_vec();
                                                                                                let others =
                                                                                                    options[category_count..]
                                                                                                        .to_vec();
                                                                                                view! {
                                                                                                    <optgroup label="Suggested">
                                                                                                        {suggested.into_iter().map(|template| {
                                                                                                            let id = template.id.clone();
                                                                                                            let label = format!(
                                                                                                                "{} — {} LEDs",
                                                                                                                template.name,
                                                                                                                template.led_count
                                                                                                            );
                                                                                                            view! { <option value=id>{label}</option> }
                                                                                                        }).collect_view()}
                                                                                                    </optgroup>
                                                                                                    <optgroup label="All Templates">
                                                                                                        {others.into_iter().map(|template| {
                                                                                                            let id = template.id.clone();
                                                                                                            let label = format!(
                                                                                                                "{} — {} LEDs",
                                                                                                                template.name,
                                                                                                                template.led_count
                                                                                                            );
                                                                                                            view! { <option value=id>{label}</option> }
                                                                                                        }).collect_view()}
                                                                                                    </optgroup>
                                                                                                }
                                                                                                    .into_any()
                                                                                            } else {
                                                                                                options.into_iter().map(|template| {
                                                                                                    let id = template.id.clone();
                                                                                                    let label = format!(
                                                                                                        "{} — {} LEDs",
                                                                                                        template.name,
                                                                                                        template.led_count
                                                                                                    );
                                                                                                    view! { <option value=id>{label}</option> }
                                                                                                }).collect_view().into_any()
                                                                                            }}
                                                                                        </select>

                                                                                        <div class="flex items-center justify-between gap-3 text-[11px] font-mono text-fg-tertiary">
                                                                                            <div class="min-w-0 truncate">
                                                                                                {match template_info {
                                                                                                    Some(template) => {
                                                                                                        let vendor = if template.vendor.is_empty() {
                                                                                                            "Custom".to_owned()
                                                                                                        } else {
                                                                                                            template.vendor
                                                                                                        };
                                                                                                        format!(
                                                                                                            "{} · {} LEDs · {}",
                                                                                                            vendor,
                                                                                                            template.led_count,
                                                                                                            template.category.as_str()
                                                                                                        )
                                                                                                    }
                                                                                                    None if row.template_id.is_empty() => {
                                                                                                        "Choose an attachment template".to_owned()
                                                                                                    }
                                                                                                    None => {
                                                                                                        "Template is unavailable".to_owned()
                                                                                                    }
                                                                                                }}
                                                                                            </div>
                                                                                            <div class="shrink-0"
                                                                                                style=move || {
                                                                                                    if placement_style.is_some() {
                                                                                                        if summary.overflow_leds > 0 {
                                                                                                            "color: rgb(255, 99, 99)".to_owned()
                                                                                                        } else {
                                                                                                            "color: rgba(128, 255, 234, 0.95)".to_owned()
                                                                                                        }
                                                                                                    } else {
                                                                                                        "color: rgba(139, 133, 160, 0.8)".to_owned()
                                                                                                    }
                                                                                                }
                                                                                            >
                                                                                                {match placement_label {
                                                                                                    Some(placement) => format!(
                                                                                                        "LED {}-{}",
                                                                                                        placement.led_offset,
                                                                                                        placement.led_end.saturating_sub(1)
                                                                                                    ),
                                                                                                    None => "Pending".to_owned(),
                                                                                                }}
                                                                                            </div>
                                                                                        </div>
                                                                                    </div>
                                                                                }
                                                                            })
                                                                            .collect_view()
                                                                            .into_any()
                                                                    }}
                                                                </div>

                                                                <div class="flex items-center justify-between gap-2 pt-1">
                                                                    <div class="flex items-center gap-2">
                                                                        <button
                                                                            class="flex items-center gap-1 px-2 py-1 rounded-md text-[10px] font-medium transition-all btn-press"
                                                                            style="background: rgba(128, 255, 234, 0.06); border: 1px solid rgba(128, 255, 234, 0.14); color: rgb(128, 255, 234)"
                                                                            on:click=add_row
                                                                        >
                                                                            <Icon icon=LuPlus width="10px" height="10px" style="color: inherit" />
                                                                            "Add Attachment"
                                                                        </button>
                                                                        <div class="text-[10px] font-mono"
                                                                            style=move || {
                                                                                let summary = draft_summary.get();
                                                                                if summary.overflow_leds > 0 {
                                                                                    "color: rgb(255, 99, 99)".to_owned()
                                                                                } else if summary.has_incomplete_rows {
                                                                                    "color: rgb(241, 250, 140)".to_owned()
                                                                                } else {
                                                                                    "color: rgba(139, 133, 160, 0.8)".to_owned()
                                                                                }
                                                                            }
                                                                        >
                                                                            {move || {
                                                                                let summary = draft_summary.get();
                                                                                if summary.overflow_leds > 0 {
                                                                                    format!(
                                                                                        "{} LEDs over budget",
                                                                                        summary.overflow_leds
                                                                                    )
                                                                                } else if summary.has_incomplete_rows {
                                                                                    "Select a template for every row".to_owned()
                                                                                } else {
                                                                                    "Ready to apply".to_owned()
                                                                                }
                                                                            }}
                                                                        </div>
                                                                    </div>

                                                                    <div class="flex items-center gap-2">
                                                                        <Show when=move || draft_is_dirty.get()>
                                                                            <button
                                                                                class="px-2 py-1 rounded-md text-[10px] font-medium transition-all btn-press"
                                                                                style="background: rgba(139, 133, 160, 0.08); border: 1px solid rgba(139, 133, 160, 0.16); color: rgba(214, 208, 232, 0.9)"
                                                                                on:click=reset_rows
                                                                            >
                                                                                "Reset"
                                                                            </button>
                                                                        </Show>
                                                                        <button
                                                                            class="flex items-center gap-1 px-2 py-1 rounded-md text-[10px] font-medium transition-all btn-press disabled:opacity-40 disabled:cursor-not-allowed"
                                                                            style="background: rgba(128, 255, 234, 0.1); border: 1px solid rgba(128, 255, 234, 0.2); color: rgb(128, 255, 234)"
                                                                            disabled=move || {
                                                                                let summary = draft_summary.get();
                                                                                save_in_flight.get()
                                                                                    || !draft_is_dirty.get()
                                                                                    || !summary.is_valid()
                                                                            }
                                                                            on:click={
                                                                                let slot = slot.clone();
                                                                                let slot_id = slot_id.clone();
                                                                                move |_: web_sys::MouseEvent| {
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
                                                                                                    binding.slot_id
                                                                                                        != slot_id
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
                                                                                                set_draft_rows
                                                                                                    .set(saved_rows);
                                                                                                toasts::toast_success(
                                                                                                    if updated
                                                                                                        .bindings
                                                                                                        .iter()
                                                                                                        .any(|binding| {
                                                                                                            binding.slot_id
                                                                                                                == slot_id
                                                                                                        }) {
                                                                                                        "Attachments updated"
                                                                                                    } else {
                                                                                                        "Attachments cleared"
                                                                                                    },
                                                                                                );
                                                                                                set_refetch_tick
                                                                                                    .update(|tick| *tick += 1);
                                                                                            }
                                                                                            Err(error) => {
                                                                                                toasts::toast_error(
                                                                                                    &format!(
                                                                                                        "Update failed: {error}"
                                                                                                    ),
                                                                                                );
                                                                                            }
                                                                                        }
                                                                                    });
                                                                                }
                                                                            }
                                                                        >
                                                                            <Icon icon=LuCheck width="10px" height="10px" style="color: inherit" />
                                                                            {move || if save_in_flight.get() { "Applying..." } else { "Apply" }}
                                                                        </button>
                                                                    </div>
                                                                </div>
                                                            </div>
                                                        </Show>
                                                    </div>
                                                }
                                            }).collect_view()}
                                        </div>
                                    </div>
                                }
                                    .into_any()
                            }
                            Err(error) => view! {
                                <div class="text-xs text-error-red py-2">{error}</div>
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
