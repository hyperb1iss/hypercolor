//! Dropdown renderer — enum-style selects (`ControlType::Dropdown`).

use leptos::portal::Portal;
use leptos::prelude::*;
use serde_json::json;

use hypercolor_types::effect::{ControlDefinition, ControlValue};

use super::{ControlDropdownDismissHandlers, dropdown_panel_style};
use leptos_icons::Icon;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_dropdown(
    def: &ControlDefinition,
    name: String,
    control_id: String,
    tooltip: Option<String>,
    icon: icondata::Icon,
    icon_style: String,
    value: Signal<ControlValue>,
    on_change: Callback<(String, serde_json::Value)>,
) -> impl IntoView {
    let labels = def.labels.clone();
    let initial = match value.get_untracked() {
        ControlValue::Enum(s) | ControlValue::Text(s) => s,
        _ => labels.first().cloned().unwrap_or_default(),
    };
    let labels_for_sync = labels.clone();
    let dropdown_labels = StoredValue::new(labels.clone());
    let (selected, set_selected) = signal(initial);
    let (dropdown_open, set_dropdown_open) = signal(false);
    let control_name = control_id;
    let dropdown_control_name = StoredValue::new(control_name.clone());
    let dropdown_class = format!("control-dropdown-{}", control_name);
    let dropdown_wrapper_class = dropdown_class.clone();
    let dropdown_class_value = StoredValue::new(dropdown_class.clone());
    let trigger_ref = NodeRef::<leptos::html::Button>::new();

    Effect::new(move |_| {
        let next = match value.get() {
            ControlValue::Enum(s) | ControlValue::Text(s) => s,
            _ => labels_for_sync.first().cloned().unwrap_or_default(),
        };
        if selected.get_untracked() != next {
            set_selected.set(next);
        }
    });

    view! {
        <div
            class="flex items-center gap-2.5 rounded-lg px-3 py-2 hover:bg-surface-hover/20 transition-colors duration-200 group"
            class=("relative", move || dropdown_open.get())
            class=("z-[100]", move || dropdown_open.get())
            title=tooltip.unwrap_or_default()
        >
            <Icon icon=icon width="15px" height="15px" style=icon_style.clone() />
            <label class="text-xs text-fg-secondary font-medium shrink-0 min-w-[80px] max-w-[120px] truncate">{name.clone()}</label>
            <div class=format!("relative flex-1 min-w-0 {dropdown_wrapper_class}")>
                <button
                    type="button"
                    node_ref=trigger_ref
                    class="w-full flex items-center gap-1.5 bg-surface-sunken border px-2.5 py-1.5
                           text-xs cursor-pointer select-silk-trigger"
                    class=("rounded-t-lg", move || dropdown_open.get())
                    class=("rounded-lg", move || !dropdown_open.get())
                    class=("border-accent-muted", move || dropdown_open.get())
                    class=("border-edge-subtle", move || !dropdown_open.get())
                    on:click=move |_| set_dropdown_open.update(|v| *v = !*v)
                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                        if ev.key() == "Escape" && dropdown_open.get_untracked() {
                            set_dropdown_open.set(false);
                            ev.prevent_default();
                        }
                    }
                >
                    <span class="flex-1 min-w-0 text-left truncate text-fg-primary">
                        {move || selected.get()}
                    </span>
                    <svg
                        class="w-3 h-3 shrink-0 transition-transform duration-200"
                        class=("rotate-180", move || dropdown_open.get())
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                    >
                        <path d="m6 9 6 6 6-6" />
                    </svg>
                </button>

                <Show when=move || dropdown_open.get()>
                    <ControlDropdownDismissHandlers
                        class_name=dropdown_class.clone()
                        is_open=dropdown_open
                        set_open=set_dropdown_open
                    />
                    <Portal>
                        <div class=move || dropdown_class_value.get_value()>
                            <div
                                class="fixed z-[9999]
                                       rounded-b-xl overflow-hidden
                                       bg-surface-overlay/98 backdrop-blur-xl
                                       border border-t-0 border-edge-subtle
                                       dropdown-glow animate-slide-down
                                       overflow-y-auto scrollbar-dropdown"
                                style=move || dropdown_panel_style(trigger_ref.get())
                                on:mousedown=|ev: leptos::ev::MouseEvent| ev.stop_propagation()
                            >
                                {move || dropdown_labels.with_value(|labels| {
                                    labels.iter().map(|label| {
                                        let label_val = label.clone();
                                        let display = label.clone();
                                        let m1 = label.clone();
                                        let m2 = label.clone();
                                        let m3 = label.clone();
                                        let m4 = label.clone();
                                        let control_name = dropdown_control_name.get_value();
                                        view! {
                                            <button
                                                type="button"
                                                class="dropdown-option w-full text-left px-3 py-[7px] text-xs cursor-pointer
                                                       flex items-center gap-2"
                                                class=("dropdown-option-active", move || selected.get() == m1)
                                                class=("text-fg-tertiary", move || selected.get() != m2)
                                                on:click=move |_| {
                                                    set_selected.set(label_val.clone());
                                                    on_change.run((control_name.clone(), json!(label_val.clone())));
                                                    set_dropdown_open.set(false);
                                                }
                                            >
                                                <span
                                                    class="w-1 h-1 rounded-full shrink-0 transition-all duration-200"
                                                    class=("bg-accent-muted scale-100 opacity-100", move || selected.get() == m3)
                                                    class=("scale-0 opacity-0", move || selected.get() != m4)
                                                />
                                                <span class="truncate">{display}</span>
                                            </button>
                                        }
                                    }).collect_view()
                                })}
                            </div>
                        </div>
                    </Portal>
                </Show>
            </div>
        </div>
    }
}
