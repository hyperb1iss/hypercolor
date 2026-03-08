//! Device attachment configuration panel — configure templates per slot, then import to layout.

use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_types::attachment::{AttachmentCategory, AttachmentSuggestedZone};
use hypercolor_types::spatial::{
    DeviceZone, LedTopology, NormalizedPosition, Orientation, ZoneAttachment, ZoneShape,
};

use crate::api;
use crate::app::DevicesContext;
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
                    class="px-2.5 py-1 rounded-lg text-[10px] font-medium transition-all btn-press disabled:opacity-50 disabled:cursor-not-allowed"
                    style="background: rgba(128, 255, 234, 0.08); border: 1px solid rgba(128, 255, 234, 0.16); color: rgb(128, 255, 234)"
                    disabled=move || import_in_flight.get()
                    on:click=move |_| import_to_layout()
                >
                    {move || if import_in_flight.get() { "Importing..." } else { "Import To Layout" }}
                </button>
            </div>

            <div class="p-4">
                <Suspense fallback=|| view! {
                    <div class="text-xs text-fg-tertiary animate-pulse">"Loading attachments..."</div>
                }>
                    {move || {
                        let all_templates = templates.get().map(|t| t.to_vec()).unwrap_or_default();

                        attachments.get().map(|result| match result {
                            Ok(profile) => {
                                let slots = profile.slots.clone();
                                let bindings = profile.bindings.clone();
                                let ready_count = profile.suggested_zones.len();

                                if slots.is_empty() {
                                    return view! {
                                        <div class="text-xs text-fg-tertiary text-center py-3">
                                            "No attachment slots reported for this device"
                                        </div>
                                    }.into_any();
                                }

                                view! {
                                    <div class="space-y-3">
                                        <div class="flex items-center justify-between text-[11px] font-mono text-fg-tertiary">
                                            <span>{bindings.len()} " bindings"</span>
                                            <span>{ready_count} " zones ready"</span>
                                        </div>

                                        <div class="space-y-2">
                                            {slots.into_iter().map(|slot| {
                                                let slot_id = slot.id.clone();
                                                let slot_bindings: Vec<_> = bindings
                                                    .iter()
                                                    .filter(|binding| binding.slot_id == slot.id)
                                                    .cloned()
                                                    .collect();
                                                let used_leds: u32 = slot_bindings
                                                    .iter()
                                                    .map(|binding| binding.effective_led_count)
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

                                                let has_binding = !slot_bindings.is_empty();
                                                let binding_summary: String = if has_binding {
                                                    slot_bindings.iter().map(|b| {
                                                        b.name.clone().unwrap_or_else(|| b.template_name.clone())
                                                    }).collect::<Vec<_>>().join(", ")
                                                } else {
                                                    String::new()
                                                };

                                                // Config form state
                                                let first_binding = slot_bindings.first().cloned();
                                                let initial_template_id = first_binding
                                                    .as_ref()
                                                    .map(|b| b.template_id.clone())
                                                    .unwrap_or_default();
                                                let initial_instances = first_binding
                                                    .as_ref()
                                                    .map(|b| b.instances)
                                                    .unwrap_or(1);
                                                let initial_offset = first_binding
                                                    .as_ref()
                                                    .map(|b| b.led_offset)
                                                    .unwrap_or(0);

                                                let (selected_template, set_selected_template) = signal(initial_template_id);
                                                let (instances, set_instances) = signal(initial_instances);
                                                let (led_offset, set_led_offset) = signal(initial_offset);

                                                let slot_categories = slot.suggested_categories.clone();
                                                let slot_led_count = slot.led_count;

                                                // Sort templates: matching categories first
                                                let relevant_templates: Vec<_> = if slot_categories.is_empty() {
                                                    all_templates.clone()
                                                } else {
                                                    let mut matching: Vec<_> = all_templates
                                                        .iter()
                                                        .filter(|t| slot_categories.iter().any(|c| c.matches_category(&t.category)))
                                                        .cloned()
                                                        .collect();
                                                    let matching_ids: Vec<_> = matching.iter().map(|t| t.id.clone()).collect();
                                                    let mut others: Vec<_> = all_templates
                                                        .iter()
                                                        .filter(|t| !matching_ids.contains(&t.id))
                                                        .cloned()
                                                        .collect();
                                                    matching.append(&mut others);
                                                    matching
                                                };

                                                let category_count = relevant_templates
                                                    .iter()
                                                    .take_while(|t| {
                                                        slot_categories.is_empty()
                                                            || slot_categories.iter().any(|c| c.matches_category(&t.category))
                                                    })
                                                    .count();

                                                let templates_for_select = relevant_templates.clone();
                                                let stored_templates = StoredValue::new(relevant_templates);

                                                let selected_info = move || {
                                                    let tid = selected_template.get();
                                                    if tid.is_empty() {
                                                        return None;
                                                    }
                                                    stored_templates.with_value(|ts| {
                                                        ts.iter().find(|t| t.id == tid).cloned()
                                                    })
                                                };

                                                let selected_info_preview = selected_info;

                                                let save_binding = {
                                                    let slot_id = slot_id.clone();
                                                    move || {
                                                        let template_id = selected_template.get_untracked();
                                                        if template_id.is_empty() {
                                                            toasts::toast_info("Select a template first");
                                                            return;
                                                        }
                                                        set_save_in_flight.set(true);
                                                        let slot_id = slot_id.clone();
                                                        let did = device_id.get_untracked();
                                                        let inst = instances.get_untracked();
                                                        let off = led_offset.get_untracked();

                                                        leptos::task::spawn_local(async move {
                                                            let result = async {
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

                                                                api_bindings.push(api::AttachmentBindingRequest {
                                                                    slot_id,
                                                                    template_id,
                                                                    name: None,
                                                                    enabled: true,
                                                                    instances: inst,
                                                                    led_offset: off,
                                                                });

                                                                api::update_device_attachments(&did, &api::UpdateAttachmentsRequest {
                                                                    bindings: api_bindings,
                                                                }).await
                                                            }.await;

                                                            set_save_in_flight.set(false);
                                                            match result {
                                                                Ok(_) => {
                                                                    toasts::toast_success("Attachment saved");
                                                                    set_refetch_tick.update(|t| *t += 1);
                                                                }
                                                                Err(error) => {
                                                                    toasts::toast_error(&format!("Save failed: {error}"));
                                                                }
                                                            }
                                                        });
                                                    }
                                                };

                                                let remove_binding = {
                                                    let slot_id = slot_id.clone();
                                                    move || {
                                                        set_save_in_flight.set(true);
                                                        let slot_id = slot_id.clone();
                                                        let did = device_id.get_untracked();

                                                        leptos::task::spawn_local(async move {
                                                            let result = async {
                                                                let current = api::fetch_device_attachments(&did).await?;
                                                                let api_bindings: Vec<api::AttachmentBindingRequest> = current
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

                                                                api::update_device_attachments(&did, &api::UpdateAttachmentsRequest {
                                                                    bindings: api_bindings,
                                                                }).await
                                                            }.await;

                                                            set_save_in_flight.set(false);
                                                            match result {
                                                                Ok(_) => {
                                                                    set_selected_template.set(String::new());
                                                                    set_instances.set(1);
                                                                    set_led_offset.set(0);
                                                                    toasts::toast_success("Attachment removed");
                                                                    set_refetch_tick.update(|t| *t += 1);
                                                                }
                                                                Err(error) => {
                                                                    toasts::toast_error(&format!("Remove failed: {error}"));
                                                                }
                                                            }
                                                        });
                                                    }
                                                };

                                                view! {
                                                    <div class="rounded-lg border border-edge-subtle bg-surface-overlay/20 overflow-hidden transition-all">
                                                        // Slot header — clickable
                                                        <button
                                                            class="w-full px-3 py-2.5 text-left hover:bg-surface-hover/30 transition-colors"
                                                            on:click=toggle_slot
                                                        >
                                                            <div class="flex items-start justify-between gap-3">
                                                                <div class="min-w-0">
                                                                    <div class="text-xs font-medium text-fg-primary">{slot.name.clone()}</div>
                                                                    <div class="text-[11px] font-mono text-fg-tertiary">
                                                                        {slot.id.clone()} " · " {used_leds} "/" {slot.led_count} " LEDs"
                                                                    </div>
                                                                </div>
                                                                <div class="flex items-center gap-2 shrink-0">
                                                                    {if has_binding {
                                                                        view! {
                                                                            <span class="rounded-full bg-[rgba(128,255,234,0.1)] border border-[rgba(128,255,234,0.2)] px-2 py-0.5 text-[10px] font-mono"
                                                                                style="color: rgb(128, 255, 234)"
                                                                            >
                                                                                {slot_bindings.len()} " bound"
                                                                            </span>
                                                                        }.into_any()
                                                                    } else {
                                                                        view! {
                                                                            <span class="rounded-full border border-edge-subtle bg-surface-overlay/40 px-2 py-0.5 text-[10px] font-mono text-fg-tertiary">
                                                                                {slot.suggested_categories.len()} " hints"
                                                                            </span>
                                                                        }.into_any()
                                                                    }}
                                                                    {if is_expanded() {
                                                                        view! { <Icon icon=LuChevronUp width="12px" height="12px" style="color: rgba(139, 133, 160, 0.7)" /> }.into_any()
                                                                    } else {
                                                                        view! { <Icon icon=LuChevronDown width="12px" height="12px" style="color: rgba(139, 133, 160, 0.7)" /> }.into_any()
                                                                    }}
                                                                </div>
                                                            </div>

                                                            // Collapsed summary
                                                            {if has_binding && !is_expanded() {
                                                                Some(view! {
                                                                    <div class="mt-1.5 text-[11px] text-fg-tertiary truncate">
                                                                        {binding_summary.clone()}
                                                                    </div>
                                                                }.into_any())
                                                            } else if !is_expanded() {
                                                                Some(view! {
                                                                    <div class="mt-1.5 text-[11px] text-fg-tertiary">
                                                                        {"Click to configure".to_string()}
                                                                    </div>
                                                                }.into_any())
                                                            } else {
                                                                None
                                                            }}
                                                        </button>

                                                        // Expanded configuration
                                                        <Show when=is_expanded>
                                                            <div class="px-3 pb-3 space-y-3 border-t border-edge-subtle bg-surface-sunken/30 animate-fade-in">
                                                                // Template selector
                                                                <div class="pt-3">
                                                                    <label class="block text-[11px] font-mono text-fg-tertiary mb-1.5">"Template"</label>
                                                                    <select
                                                                        class="w-full bg-surface-overlay/60 border border-edge-subtle rounded-lg px-3 py-1.5 text-xs text-fg-primary focus:outline-none focus:border-accent-muted cursor-pointer"
                                                                        prop:value=move || selected_template.get()
                                                                        on:change=move |ev| {
                                                                            set_selected_template.set(event_target_value(&ev));
                                                                        }
                                                                    >
                                                                        <option value="" disabled=true selected=move || selected_template.get().is_empty()>
                                                                            "Select a template..."
                                                                        </option>
                                                                        {if category_count > 0 && category_count < templates_for_select.len() {
                                                                            let suggested: Vec<_> = templates_for_select[..category_count].to_vec();
                                                                            let others: Vec<_> = templates_for_select[category_count..].to_vec();
                                                                            view! {
                                                                                <optgroup label="Suggested">
                                                                                    {suggested.into_iter().map(|t| {
                                                                                        let id = t.id.clone();
                                                                                        let label = format!("{} — {} LEDs", t.name, t.led_count);
                                                                                        view! { <option value=id>{label}</option> }
                                                                                    }).collect_view()}
                                                                                </optgroup>
                                                                                <optgroup label="All Templates">
                                                                                    {others.into_iter().map(|t| {
                                                                                        let id = t.id.clone();
                                                                                        let label = format!("{} — {} LEDs", t.name, t.led_count);
                                                                                        view! { <option value=id>{label}</option> }
                                                                                    }).collect_view()}
                                                                                </optgroup>
                                                                            }.into_any()
                                                                        } else {
                                                                            templates_for_select.clone().into_iter().map(|t| {
                                                                                let id = t.id.clone();
                                                                                let label = format!("{} — {} LEDs", t.name, t.led_count);
                                                                                view! { <option value=id>{label}</option> }
                                                                            }).collect_view().into_any()
                                                                        }}
                                                                    </select>
                                                                </div>

                                                                // Template info
                                                                {move || selected_info().map(|info| view! {
                                                                    <div class="rounded-md bg-surface-overlay/40 border border-edge-subtle px-2.5 py-2 text-[11px] font-mono text-fg-tertiary space-y-0.5">
                                                                        <div class="flex justify-between">
                                                                            <span>"Category"</span>
                                                                            <span class="text-fg-secondary">{info.category.as_str().to_owned()}</span>
                                                                        </div>
                                                                        <div class="flex justify-between">
                                                                            <span>"LEDs"</span>
                                                                            <span class="text-fg-secondary">{info.led_count}</span>
                                                                        </div>
                                                                        {(!info.vendor.is_empty()).then(|| view! {
                                                                            <div class="flex justify-between">
                                                                                <span>"Vendor"</span>
                                                                                <span class="text-fg-secondary">{info.vendor.clone()}</span>
                                                                            </div>
                                                                        })}
                                                                    </div>
                                                                })}

                                                                // Instances + offset
                                                                <div class="grid grid-cols-2 gap-2">
                                                                    <div>
                                                                        <label class="block text-[11px] font-mono text-fg-tertiary mb-1">"Instances"</label>
                                                                        <div class="flex items-center gap-1">
                                                                            <button
                                                                                class="w-6 h-6 flex items-center justify-center rounded border border-edge-subtle bg-surface-overlay/40 text-fg-tertiary hover:text-fg-primary hover:border-edge-default transition-colors btn-press"
                                                                                on:click=move |_| set_instances.update(|v| *v = (*v).saturating_sub(1).max(1))
                                                                            >
                                                                                <Icon icon=LuMinus width="10px" height="10px" style="color: inherit" />
                                                                            </button>
                                                                            <input
                                                                                type="number"
                                                                                min="1"
                                                                                class="flex-1 w-full bg-surface-base/60 border border-edge-subtle rounded px-2 py-1 text-xs text-fg-primary font-mono text-center focus:outline-none focus:border-accent-muted [appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
                                                                                prop:value=move || instances.get().to_string()
                                                                                on:change=move |ev| {
                                                                                    let val: u32 = event_target_value(&ev).parse().unwrap_or(1);
                                                                                    set_instances.set(val.max(1));
                                                                                }
                                                                            />
                                                                            <button
                                                                                class="w-6 h-6 flex items-center justify-center rounded border border-edge-subtle bg-surface-overlay/40 text-fg-tertiary hover:text-fg-primary hover:border-edge-default transition-colors btn-press"
                                                                                on:click=move |_| set_instances.update(|v| *v += 1)
                                                                            >
                                                                                <Icon icon=LuPlus width="10px" height="10px" style="color: inherit" />
                                                                            </button>
                                                                        </div>
                                                                    </div>
                                                                    <div>
                                                                        <label class="block text-[11px] font-mono text-fg-tertiary mb-1">"LED Offset"</label>
                                                                        <input
                                                                            type="number"
                                                                            min="0"
                                                                            class="w-full bg-surface-base/60 border border-edge-subtle rounded px-2 py-1 text-xs text-fg-primary font-mono text-center focus:outline-none focus:border-accent-muted [appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
                                                                            prop:value=move || led_offset.get().to_string()
                                                                            on:change=move |ev| {
                                                                                let val: u32 = event_target_value(&ev).parse().unwrap_or(0);
                                                                                set_led_offset.set(val.min(slot_led_count));
                                                                            }
                                                                        />
                                                                    </div>
                                                                </div>

                                                                // LED range preview
                                                                {move || {
                                                                    selected_info_preview().map(|t| {
                                                                        let total = t.led_count * instances.get();
                                                                        let offset = led_offset.get();
                                                                        let over = (offset + total) > slot_led_count;
                                                                        view! {
                                                                            <div class="text-[11px] font-mono"
                                                                                style=if over { "color: rgb(255, 99, 99)" } else { "color: rgba(139, 133, 160, 0.8)" }
                                                                            >
                                                                                {format!("Range: LED {} – {} of {}", offset, offset + total, slot_led_count)}
                                                                                {over.then_some(" (exceeds slot)")}
                                                                            </div>
                                                                        }
                                                                    })
                                                                }}

                                                                // Actions
                                                                <div class="flex items-center gap-2 pt-1">
                                                                    <button
                                                                        class="flex-1 flex items-center justify-center gap-1.5 px-3 py-1.5 rounded-lg text-[11px] font-medium transition-all btn-press disabled:opacity-40 disabled:cursor-not-allowed"
                                                                        style="background: rgba(128, 255, 234, 0.1); border: 1px solid rgba(128, 255, 234, 0.2); color: rgb(128, 255, 234)"
                                                                        disabled=move || save_in_flight.get() || selected_template.get().is_empty()
                                                                        on:click={
                                                                            let save = save_binding.clone();
                                                                            move |_| save()
                                                                        }
                                                                    >
                                                                        <Icon icon=LuCheck width="12px" height="12px" style="color: inherit" />
                                                                        {move || if save_in_flight.get() { "Saving..." } else { "Save" }}
                                                                    </button>
                                                                    {has_binding.then(|| {
                                                                        let remove = remove_binding.clone();
                                                                        view! {
                                                                            <button
                                                                                class="flex items-center justify-center gap-1.5 px-3 py-1.5 rounded-lg text-[11px] font-medium transition-all btn-press disabled:opacity-40 disabled:cursor-not-allowed"
                                                                                style="background: rgba(255, 99, 99, 0.08); border: 1px solid rgba(255, 99, 99, 0.16); color: rgb(255, 99, 99)"
                                                                                disabled=move || save_in_flight.get()
                                                                                on:click=move |_| remove()
                                                                            >
                                                                                <Icon icon=LuX width="12px" height="12px" style="color: inherit" />
                                                                                "Remove"
                                                                            </button>
                                                                        }
                                                                    })}
                                                                </div>
                                                            </div>
                                                        </Show>
                                                    </div>
                                                }
                                            }).collect_view()}
                                        </div>
                                    </div>
                                }.into_any()
                            }
                            Err(error) => view! {
                                <div class="text-xs text-error-red py-2">{error}</div>
                            }.into_any(),
                        })
                    }}
                </Suspense>
            </div>
        </div>
    }
}

#[allow(clippy::cast_precision_loss)]
fn build_attachment_layout_zones(
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

            DeviceZone {
                id: attachment_zone_id(&device.layout_device_id, suggested),
                name: suggested.name.clone(),
                device_id: device.layout_device_id.clone(),
                zone_name: Some(suggested.slot_id.clone()),
                group_id: None,
                position,
                size: layout_geometry::attachment_zone_size(suggested, max_size),
                rotation: 0.0,
                scale: 1.0,
                orientation: orientation_for_topology(&suggested.topology),
                topology: suggested.topology.clone(),
                led_positions: Vec::new(),
                led_mapping: suggested.led_mapping.clone(),
                sampling_mode: None,
                edge_behavior: None,
                shape: shape_for_category(&suggested.category),
                shape_preset: None,
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

fn shape_for_category(category: &AttachmentCategory) -> Option<ZoneShape> {
    match category {
        AttachmentCategory::Fan
        | AttachmentCategory::Aio
        | AttachmentCategory::Heatsink
        | AttachmentCategory::Ring => Some(ZoneShape::Ring),
        AttachmentCategory::Strip
        | AttachmentCategory::Strimer
        | AttachmentCategory::Case
        | AttachmentCategory::Radiator
        | AttachmentCategory::Matrix => Some(ZoneShape::Rectangle),
        AttachmentCategory::Bulb | AttachmentCategory::Other(_) => None,
    }
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
        "attachment-{}-{}-{}",
        slugify(layout_device_id),
        slugify(&suggested.slot_id),
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
