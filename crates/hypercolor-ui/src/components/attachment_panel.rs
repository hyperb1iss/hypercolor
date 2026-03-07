//! Device attachment summary panel with active-layout import support.

use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_types::attachment::{AttachmentCategory, AttachmentSuggestedZone};
use hypercolor_types::spatial::{
    DeviceZone, LedTopology, NormalizedPosition, Orientation, ZoneAttachment, ZoneShape,
};

use crate::api;
use crate::app::DevicesContext;
use crate::icons::*;
use crate::toasts;

/// Read-only attachment panel for a selected device.
#[component]
pub fn AttachmentPanel(
    #[prop(into)] device_id: Signal<String>,
    #[prop(into)] device: Signal<Option<api::DeviceSummary>>,
) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();
    let (import_in_flight, set_import_in_flight) = signal(false);

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

    let import_to_layout = move || {
        if import_in_flight.get_untracked() {
            return;
        }

        let Some(device) = device.get_untracked() else {
            return;
        };

        set_import_in_flight.set(true);
        let layouts_resource = ctx.layouts_resource;
        let set_import_in_flight = set_import_in_flight;
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
                                                let slot_bindings: Vec<_> = bindings
                                                    .iter()
                                                    .filter(|binding| binding.slot_id == slot.id)
                                                    .cloned()
                                                    .collect();
                                                let used_leds: u32 = slot_bindings
                                                    .iter()
                                                    .map(|binding| binding.effective_led_count)
                                                    .sum();

                                                view! {
                                                    <div class="rounded-lg border border-edge-subtle bg-surface-overlay/20 px-3 py-2.5">
                                                        <div class="flex items-start justify-between gap-3">
                                                            <div>
                                                                <div class="text-xs font-medium text-fg-primary">{slot.name.clone()}</div>
                                                                <div class="text-[11px] font-mono text-fg-tertiary">
                                                                    {slot.id.clone()} " • " {used_leds} "/" {slot.led_count} " LEDs"
                                                                </div>
                                                            </div>
                                                            <span class="rounded-full border border-edge-subtle bg-surface-overlay/40 px-2 py-0.5 text-[10px] font-mono text-fg-tertiary">
                                                                {slot.suggested_categories.len()} " hints"
                                                            </span>
                                                        </div>

                                                        <div class="mt-2 space-y-1.5">
                                                            {if slot_bindings.is_empty() {
                                                                view! {
                                                                    <div class="text-xs text-fg-tertiary">
                                                                        "No attachment configured"
                                                                    </div>
                                                                }.into_any()
                                                            } else {
                                                                view! {
                                                                    <>
                                                                        {slot_bindings.into_iter().map(|binding| {
                                                                            let display_name = binding
                                                                                .name
                                                                                .clone()
                                                                                .unwrap_or_else(|| binding.template_name.clone());
                                                                            view! {
                                                                                <div class="flex items-center justify-between gap-3 rounded-md bg-surface-overlay/30 px-2.5 py-1.5">
                                                                                    <div class="min-w-0">
                                                                                        <div class="truncate text-xs text-fg-primary">{display_name}</div>
                                                                                        <div class="text-[11px] font-mono text-fg-tertiary">
                                                                                            {binding.template_name}
                                                                                        </div>
                                                                                    </div>
                                                                                    <span class="shrink-0 text-[11px] font-mono text-fg-tertiary">
                                                                                        "x" {binding.instances} " • " {binding.effective_led_count}
                                                                                    </span>
                                                                                </div>
                                                                            }
                                                                        }).collect_view()}
                                                                    </>
                                                                }.into_any()
                                                            }}
                                                        </div>
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

            DeviceZone {
                id: attachment_zone_id(&device.layout_device_id, suggested),
                name: suggested.name.clone(),
                device_id: device.layout_device_id.clone(),
                zone_name: Some(suggested.slot_id.clone()),
                group_id: None,
                position,
                size: normalized_size(suggested),
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
                    led_mapping: suggested.led_mapping.clone(),
                }),
            }
        })
        .collect()
}

fn normalized_size(suggested: &AttachmentSuggestedZone) -> NormalizedPosition {
    NormalizedPosition::new(
        suggested.default_size.width.clamp(0.14, 0.36),
        suggested.default_size.height.clamp(0.12, 0.30),
    )
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
