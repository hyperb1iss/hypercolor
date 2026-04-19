//! Shared search input for page toolbars.
//!
//! Effects and Devices shipped identical markup with only the placeholder and
//! signal pair differing. Consolidating it here keeps the search row
//! pixel-perfect across pages and leaves one place to evolve the focus-hint
//! key binding if we ever wire "/" up globally.

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::icons::LuSearch;

#[component]
pub fn PageSearchBar(
    #[prop(into)] placeholder: String,
    value: ReadSignal<String>,
    set_value: WriteSignal<String>,
) -> impl IntoView {
    view! {
        <div class="relative flex-1 min-w-0">
            <span class="absolute left-3 top-1/2 -translate-y-1/2 pointer-events-none text-fg-tertiary">
                <Icon icon=LuSearch width="14px" height="14px" />
            </span>
            <input
                type="text"
                placeholder=placeholder
                class="w-full bg-surface-overlay/60 border border-edge-subtle rounded-lg pl-9 pr-10 py-1.5 \
                       text-sm text-fg-primary placeholder-fg-tertiary \
                       focus:outline-none focus:border-accent-muted \
                       search-glow glow-ring transition-all duration-300"
                prop:value=move || value.get()
                on:input=move |ev: ev::Event| {
                    let target = ev.target()
                        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                    if let Some(el) = target {
                        set_value.set(el.value());
                    }
                }
            />
            <kbd class="absolute right-3 top-1/2 -translate-y-1/2 text-[9px] font-mono \
                        text-fg-tertiary bg-surface-overlay/30 px-1.5 py-0.5 rounded border border-edge-subtle">
                "/"
            </kbd>
        </div>
    }
}
