//! Channel editor — manages components for a single device channel (slot).
#![allow(dead_code)]

use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_types::attachment::AttachmentSlot;

use crate::api;
use crate::components::attachment_editor::{ComponentDraft, DraftRow};
use crate::components::component_picker::ComponentPicker;
use crate::icons::*;

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
    let _ = (&slot, &device_id, &device, &on_saved);

    let (drafts, set_drafts) = signal(initial_drafts);

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

    view! {
        <div class="space-y-2 px-2.5 pb-2.5 pt-1">
            {move || {
                let rows = drafts.get();
                if rows.is_empty() {
                    view! {
                        <div class="text-[10px] text-fg-tertiary/40 text-center py-2">"No components"</div>
                    }.into_any()
                } else {
                    rows.into_iter().enumerate().map(|(i, row)| {
                        let led_display = match &row.kind {
                            ComponentDraft::Strip { led_count } => format!("{led_count} LEDs"),
                            ComponentDraft::Matrix { cols, rows } => format!("{cols}\u{00d7}{rows}"),
                            ComponentDraft::Component { template_id } => format!("Component: {}", &template_id[..template_id.len().min(12)]),
                        };
                        let name = if row.name.is_empty() {
                            match &row.kind {
                                ComponentDraft::Strip { .. } => "Strip".to_string(),
                                ComponentDraft::Matrix { .. } => "Matrix".to_string(),
                                ComponentDraft::Component { .. } => "Component".to_string(),
                            }
                        } else {
                            row.name.clone()
                        };
                        view! {
                            <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-surface-overlay/10 border border-edge-subtle/50">
                                <span class="text-[11px] text-fg-primary flex-1">{name}</span>
                                <span class="text-[10px] font-mono text-neon-cyan/60">{led_display}</span>
                                <button
                                    class="text-[9px] text-fg-tertiary/40 hover:text-error-red"
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
                    components=all_templates
                    on_select=on_component_selected
                />
            </div>
        </div>
    }
}
