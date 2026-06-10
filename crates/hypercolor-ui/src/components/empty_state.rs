//! Shared empty-state placeholder — icon, title, optional hint.
//!
//! One implementation for the byte-identical empties that Assets and the
//! media grid shipped separately, plus the divergent one on Devices. Colors
//! ride the `fg-*` semantic tokens so both themes stay correct.

use leptos::prelude::*;
use leptos_icons::Icon;

/// Centered empty-state block for grids and lists.
#[component]
pub fn EmptyState(
    /// Icon shown above the title.
    icon: icondata_core::Icon,
    /// One-line headline.
    #[prop(into)]
    title: String,
    /// Optional supporting hint under the title.
    #[prop(into, optional)]
    hint: String,
) -> impl IntoView {
    view! {
        <div class="flex flex-col items-center justify-center py-20 text-center">
            <span class="text-fg-tertiary/35">
                <Icon icon=icon width="36px" height="36px" />
            </span>
            <div class="mt-3 text-sm font-semibold text-fg-secondary">{title}</div>
            {(!hint.is_empty()).then(|| view! {
                <div class="mt-1 max-w-xs text-xs text-fg-tertiary/70">{hint}</div>
            })}
        </div>
    }
}
