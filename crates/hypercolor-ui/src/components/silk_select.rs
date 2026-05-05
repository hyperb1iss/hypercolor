//! Canonical dropdown used across the app.
//!
//! Replaces native `<select>` elements, whose option popup can't be styled
//! to match the dark theme on Linux/Firefox. Wraps a trigger button + a
//! portaled option panel with consistent SilkCircuit styling and reuses
//! the shared dismiss / positioning helpers from `control_panel`.
//!
//! Callers supply `(value, label)` pairs and a value signal; the component
//! owns open/close state, click-outside dismissal, scroll-close, and
//! Escape-to-close.

use std::sync::atomic::{AtomicU64, Ordering};

use leptos::portal::Portal;
use leptos::prelude::*;

use crate::components::control_panel::{ControlDropdownDismissHandlers, dropdown_panel_style};

fn next_silk_select_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[component]
pub fn SilkSelect(
    /// Currently-selected value. Must match one option's value, or be
    /// empty / unmatched to display the placeholder.
    #[prop(into)]
    value: Signal<String>,
    /// Options as `(value, label)` pairs. First is the internal value,
    /// second is the user-facing label.
    #[prop(into)]
    options: Signal<Vec<(String, String)>>,
    /// Fires with the new value when the user picks an option.
    on_change: Callback<String>,
    /// Shown when no option matches the current value.
    #[prop(into, optional)]
    placeholder: MaybeProp<String>,
    #[prop(into, optional)] disabled: MaybeProp<bool>,
    /// Extra classes appended to the trigger button's class list. The
    /// base provides layout + chevron + dropdown mechanics; visual
    /// styling (surface, border, padding, text) lives here.
    #[prop(into, optional)]
    class: String,
    /// Extra classes appended to the trigger's label span (font / size
    /// tweaks, e.g. `font-mono text-[10px]`).
    #[prop(into, optional)]
    label_class: String,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Button>::new();

    let unique_class = format!("silk-select-{}", next_silk_select_id());
    let wrapper_class = format!("relative min-w-0 {unique_class}");
    // Stored values are Copy so they can be freely captured in the Show /
    // Portal closures, which must be Fn.
    let dismiss_class = StoredValue::new(unique_class.clone());
    let portal_class = StoredValue::new(unique_class);

    let display_label = Memo::new(move |_| {
        let current = value.get();
        options
            .with(|opts| {
                opts.iter()
                    .find(|(v, _)| v == &current)
                    .map(|(_, label)| label.clone())
            })
            .unwrap_or_else(|| placeholder.get().unwrap_or_default())
    });

    let has_value = Memo::new(move |_| {
        let current = value.get();
        options.with(|opts| opts.iter().any(|(v, _)| v == &current))
    });

    let trigger_class =
        format!("w-full flex items-center gap-1.5 select-silk-trigger transition-all {class}");
    let label_class = format!("flex-1 min-w-0 text-left truncate {label_class}");

    Effect::new(move |_| {
        if disabled.get().unwrap_or(false) && open.get() {
            set_open.set(false);
        }
    });

    view! {
        <div class=wrapper_class>
            <button
                type="button"
                node_ref=trigger_ref
                class=trigger_class
                class=("rounded-t-lg", move || open.get())
                class=("rounded-lg", move || !open.get())
                class=("border-accent-muted", move || open.get())
                class=("cursor-pointer", move || !disabled.get().unwrap_or(false))
                class=("cursor-not-allowed", move || disabled.get().unwrap_or(false))
                class=("opacity-60", move || disabled.get().unwrap_or(false))
                disabled=move || disabled.get().unwrap_or(false)
                on:click=move |_| {
                    if disabled.get().unwrap_or(false) {
                        return;
                    }
                    set_open.update(|v| *v = !*v);
                }
                on:keydown=move |ev: web_sys::KeyboardEvent| {
                    if disabled.get().unwrap_or(false) {
                        return;
                    }
                    if ev.key() == "Escape" && open.get_untracked() {
                        set_open.set(false);
                        ev.prevent_default();
                    }
                }
            >
                <span
                    class=label_class
                    class=("text-fg-tertiary", move || !has_value.get())
                >
                    {move || display_label.get()}
                </span>
                <svg
                    class="w-3 h-3 shrink-0 transition-transform duration-200"
                    class=("rotate-180", move || open.get())
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    style="opacity: 0.55"
                >
                    <path d="m6 9 6 6 6-6" />
                </svg>
            </button>

            <Show when=move || open.get() && !disabled.get().unwrap_or(false)>
                <ControlDropdownDismissHandlers
                    class_name=dismiss_class.get_value()
                    is_open=open
                    set_open=set_open
                />
                <Portal>
                    <div class=move || portal_class.get_value()>
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
                            {move || options.get().into_iter().map(|(val, label)| {
                                let on_pick = val.clone();
                                let v_a = val.clone();
                                let v_b = val.clone();
                                let v_c_scale = val.clone();
                                let v_c_opacity = val.clone();
                                let v_d_scale = val.clone();
                                let v_d_opacity = val.clone();
                                view! {
                                    <button
                                        type="button"
                                        class="dropdown-option w-full text-left px-3 py-[7px] text-xs cursor-pointer
                                               flex items-center gap-2"
                                        class=("dropdown-option-active", move || value.get() == v_a)
                                        class=("text-fg-tertiary", move || value.get() != v_b)
                                        on:click=move |_| {
                                            on_change.run(on_pick.clone());
                                            set_open.set(false);
                                        }
                                    >
                                        <span
                                            class="w-1 h-1 rounded-full shrink-0 transition-all duration-200 bg-accent-muted"
                                            class=("scale-100", move || value.get() == v_c_scale)
                                            class=("opacity-100", move || value.get() == v_c_opacity)
                                            class=("scale-0", move || value.get() != v_d_scale)
                                            class=("opacity-0", move || value.get() != v_d_opacity)
                                        />
                                        <span class="truncate">{label}</span>
                                    </button>
                                }
                            }).collect_view()}
                        </div>
                    </div>
                </Portal>
            </Show>
        </div>
    }
}
