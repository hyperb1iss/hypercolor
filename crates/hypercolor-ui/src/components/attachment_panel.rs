//! Channel panel — device channel listing with inline component editors.
//!
//! Each channel (hardware slot) shows its topology, name, LED count, identify button,
//! and a `ChannelEditor` for managing its components.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use hypercolor_types::attachment::AttachmentSuggestedZone;
use hypercolor_types::spatial::{
    DeviceZone, LedTopology, NormalizedPosition, Orientation, ZoneAttachment,
};

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

    let on_saved = Callback::new(move |()| {
        set_refetch_tick.update(|t| *t += 1);
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
                                                        let (drafts, set_drafts) = signal(initial_drafts);
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
                                                    view! {
                                                    <div class="border-t border-edge-subtle/50 px-3 py-2 space-y-2">
                                                        {move || {
                                                            let rows = drafts.get();
                                                            if rows.is_empty() {
                                                                view! {
                                                                    <div class="text-[10px] text-fg-tertiary/40 text-center py-2">"No components"</div>
                                                                }.into_any()
                                                            } else {
                                                                rows.into_iter().enumerate().map(|(i, row)| {
                                                                    let led_display = match &row.kind {
                                                                        attachment_editor::ComponentDraft::Strip { led_count } => format!("{led_count} LEDs"),
                                                                        attachment_editor::ComponentDraft::Matrix { cols, rows } => format!("{cols}\u{00d7}{rows}"),
                                                                        attachment_editor::ComponentDraft::Component { template_id } => {
                                                                            format!("Component: {}", &template_id[..template_id.len().min(16)])
                                                                        }
                                                                    };
                                                                    let name = if row.name.is_empty() {
                                                                        match &row.kind {
                                                                            attachment_editor::ComponentDraft::Strip { .. } => "Strip".to_string(),
                                                                            attachment_editor::ComponentDraft::Matrix { .. } => "Matrix".to_string(),
                                                                            attachment_editor::ComponentDraft::Component { .. } => "Component".to_string(),
                                                                        }
                                                                    } else {
                                                                        row.name.clone()
                                                                    };
                                                                    view! {
                                                                        <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-surface-overlay/10 border border-edge-subtle/50 group/row">
                                                                            <span class="text-[11px] text-fg-primary flex-1">{name}</span>
                                                                            <span class="text-[10px] font-mono tabular-nums shrink-0" style="color: rgba(128, 255, 234, 0.6)">{led_display}</span>
                                                                            <button
                                                                                class="w-4 h-4 flex items-center justify-center rounded shrink-0
                                                                                       opacity-0 group-hover/row:opacity-100 transition-opacity
                                                                                       text-fg-tertiary/40 hover:text-error-red"
                                                                                on:click=move |_| { set_drafts.update(|rows| { if i < rows.len() { rows.remove(i); } }); }
                                                                            >
                                                                                <Icon icon=LuX width="10px" height="10px" />
                                                                            </button>
                                                                        </div>
                                                                    }
                                                                }).collect_view().into_any()
                                                            }
                                                        }}
                                                        <div class="flex items-center gap-1.5 pt-1">
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
