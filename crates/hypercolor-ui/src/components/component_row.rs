//! Single component row — inline editor for strip, matrix, or library component.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::api;
use crate::components::attachment_editor::{ComponentDraft, DraftRow};
use crate::icons::*;
use crate::toasts;

/// Renders a single component in the channel editor.
///
/// - **Strip**: editable name + LED count
/// - **Matrix**: editable name + cols × rows
/// - **Component**: shows library component name + fixed LED count
#[component]
pub fn ComponentRow(
    row: DraftRow,
    index: usize,
    #[prop(into)] device_id: String,
    #[prop(into)] slot_id: String,
    #[prop(into)] on_update: Callback<(usize, DraftRow)>,
    #[prop(into)] on_delete: Callback<usize>,
    templates: Vec<api::TemplateSummary>,
) -> impl IntoView {
    let accent = "128, 255, 234";

    // Resolve display info from the row kind
    let (type_icon, type_label, led_count_display) = match &row.kind {
        ComponentDraft::Strip { led_count } => (LuMinus, "Strip", format!("{led_count}")),
        ComponentDraft::Matrix { cols, rows } => {
            (LuGrid2x2, "Matrix", format!("{cols}\u{00d7}{rows}"))
        }
        ComponentDraft::Component { template_id } => {
            let tmpl = templates.iter().find(|t| t.id == *template_id);
            let label = tmpl
                .map(|t| t.name.as_str())
                .unwrap_or("Unknown");
            let count = tmpl.map(|t| t.led_count).unwrap_or(0);
            (LuCircleDot, label, format!("{count}"))
        }
    };
    let type_label = type_label.to_string();

    let is_editable = row.needs_template_creation();
    let initial_name = row.name.clone();
    let initial_kind = row.kind.clone();

    // Local editing state
    let (name_value, set_name_value) = signal(initial_name.clone());
    let (led_count, set_led_count) = signal(match &initial_kind {
        ComponentDraft::Strip { led_count } => *led_count,
        _ => 0,
    });
    let (matrix_cols, set_matrix_cols) = signal(match &initial_kind {
        ComponentDraft::Matrix { cols, .. } => *cols,
        _ => 8,
    });
    let (matrix_rows, set_matrix_rows) = signal(match &initial_kind {
        ComponentDraft::Matrix { rows, .. } => *rows,
        _ => 8,
    });

    // Push changes back to parent
    let initial_kind_for_match = initial_kind.clone();
    let initial_kind_for_render = initial_kind.clone();
    let push_update = move || {
        let name = name_value.get_untracked();
        let kind = match &initial_kind_for_match {
            ComponentDraft::Strip { .. } => ComponentDraft::Strip {
                led_count: led_count.get_untracked(),
            },
            ComponentDraft::Matrix { .. } => ComponentDraft::Matrix {
                cols: matrix_cols.get_untracked(),
                rows: matrix_rows.get_untracked(),
            },
            ComponentDraft::Component { template_id } => ComponentDraft::Component {
                template_id: template_id.clone(),
            },
        };
        on_update.run((index, DraftRow { kind, name }));
    };
    let push_update = StoredValue::new(push_update);

    let dev_id = device_id.clone();
    let sid = slot_id.clone();

    view! {
        <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-surface-overlay/10 group/row
                    border border-edge-subtle/50 hover:border-edge-subtle transition-all">
            // Type icon
            <div class="w-5 h-5 rounded flex items-center justify-center shrink-0"
                 style=format!("color: rgba({accent}, 0.6)")>
                <Icon icon=type_icon width="14px" height="14px" />
            </div>

            // Name + type label
            <div class="flex-1 min-w-0">
                {if is_editable {
                    view! {
                        <input
                            type="text"
                            placeholder=type_label.clone()
                            class="w-full bg-transparent text-[12px] text-fg-primary placeholder-fg-tertiary/30
                                   focus:outline-none border-none p-0 leading-tight"
                            prop:value=move || name_value.get()
                            on:input=move |ev| {
                                let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                if let Some(el) = target { set_name_value.set(el.value()); }
                            }
                            on:blur=move |_| push_update.with_value(|f| f())
                        />
                    }.into_any()
                } else {
                    let display = if initial_name.is_empty() { type_label.clone() } else { initial_name.clone() };
                    view! {
                        <span class="text-[12px] text-fg-primary leading-tight">{display}</span>
                    }.into_any()
                }}
                <div class="text-[9px] text-fg-tertiary/40 capitalize">{type_label}</div>
            </div>

            // LED count editor (strips) or dimensions (matrices) or static count (components)
            {match &initial_kind_for_render {
                ComponentDraft::Strip { .. } => view! {
                    <div class="flex items-center gap-1 shrink-0">
                        <input
                            type="number" min="1" max="2000"
                            class="w-14 bg-surface-base/40 border border-edge-subtle rounded px-1.5 py-0.5
                                   text-[11px] font-mono tabular-nums text-right
                                   focus:outline-none focus:border-neon-cyan/30"
                            style=format!("color: rgba({accent}, 0.8)")
                            prop:value=move || led_count.get().to_string()
                            on:input=move |ev| {
                                let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                if let Some(el) = target {
                                    if let Ok(v) = el.value().parse::<u32>() {
                                        set_led_count.set(v.clamp(1, 2000));
                                    }
                                }
                            }
                            on:blur=move |_| push_update.with_value(|f| f())
                        />
                        <span class="text-[9px] text-fg-tertiary/30">"LEDs"</span>
                    </div>
                }.into_any(),
                ComponentDraft::Matrix { .. } => view! {
                    <div class="flex items-center gap-1 shrink-0">
                        <input
                            type="number" min="1" max="64"
                            class="w-10 bg-surface-base/40 border border-edge-subtle rounded px-1 py-0.5
                                   text-[11px] font-mono tabular-nums text-right
                                   focus:outline-none focus:border-neon-cyan/30"
                            style=format!("color: rgba({accent}, 0.8)")
                            prop:value=move || matrix_cols.get().to_string()
                            on:input=move |ev| {
                                let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                if let Some(el) = target {
                                    if let Ok(v) = el.value().parse::<u32>() {
                                        set_matrix_cols.set(v.clamp(1, 64));
                                    }
                                }
                            }
                            on:blur=move |_| push_update.with_value(|f| f())
                        />
                        <span class="text-[9px] text-fg-tertiary/30">{"\u{00d7}"}</span>
                        <input
                            type="number" min="1" max="64"
                            class="w-10 bg-surface-base/40 border border-edge-subtle rounded px-1 py-0.5
                                   text-[11px] font-mono tabular-nums text-right
                                   focus:outline-none focus:border-neon-cyan/30"
                            style=format!("color: rgba({accent}, 0.8)")
                            prop:value=move || matrix_rows.get().to_string()
                            on:input=move |ev| {
                                let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                if let Some(el) = target {
                                    if let Ok(v) = el.value().parse::<u32>() {
                                        set_matrix_rows.set(v.clamp(1, 64));
                                    }
                                }
                            }
                            on:blur=move |_| push_update.with_value(|f| f())
                        />
                        <span class="text-[9px] font-mono text-fg-tertiary/30">
                            "=" {move || matrix_cols.get() * matrix_rows.get()}
                        </span>
                    </div>
                }.into_any(),
                ComponentDraft::Component { .. } => view! {
                    <span class="text-[10px] font-mono tabular-nums shrink-0 px-1.5 py-0.5 rounded bg-surface-overlay/20"
                          style=format!("color: rgba({accent}, 0.5)")>
                        {led_count_display} " LEDs"
                    </span>
                }.into_any(),
            }}

            // Identify
            <button
                class="w-5 h-5 flex items-center justify-center rounded shrink-0
                       opacity-0 group-hover/row:opacity-100 transition-opacity
                       text-fg-tertiary/40 hover:text-accent btn-press"
                title="Identify component"
                on:click={
                    let did = dev_id.clone();
                    let sid = sid.clone();
                    move |_| {
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
                <Icon icon=LuZap width="10px" height="10px" />
            </button>

            // Delete
            <button
                class="w-5 h-5 flex items-center justify-center rounded shrink-0
                       opacity-0 group-hover/row:opacity-100 transition-opacity
                       text-fg-tertiary/40 hover:text-error-red btn-press"
                title="Remove component"
                on:click=move |_| on_delete.run(index)
            >
                <Icon icon=LuX width="10px" height="10px" />
            </button>
        </div>
    }
}
