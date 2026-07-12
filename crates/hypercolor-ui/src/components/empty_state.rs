//! Shared empty-state placeholder — glowing icon tile, title, optional hint
//! and action row.
//!
//! One implementation for every grid/list empty across the app (Media,
//! Devices, Assets). Colors ride the `fg-*` semantic tokens so both themes
//! stay correct; the tile's halo reads `--glow-rgb` (§4.2), defaulting to
//! electric purple, so a caller can retint it with an `.accent-*` class on
//! an ancestor without touching this component.

use leptos::prelude::*;
use leptos_icons::Icon;

/// Centered empty-state block for grids and lists. Optional `children`
/// render as an action row under the hint (e.g. a "Clear filters" button).
#[component]
pub fn EmptyState(
    /// Icon shown in the tile above the title.
    icon: icondata_core::Icon,
    /// One-line headline.
    #[prop(into)]
    title: String,
    /// Optional supporting hint under the title.
    #[prop(into, optional)]
    hint: String,
    /// Optional action row (buttons) under the hint.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="flex flex-col items-center justify-center py-20 text-center animate-enter-up">
            <div
                class="relative flex h-14 w-14 items-center justify-center rounded-xl border border-edge-subtle bg-surface-overlay/70 text-fg-tertiary/80"
                style="box-shadow: 0 0 26px rgba(var(--glow-rgb, 225, 53, 255), 0.10)"
            >
                <div
                    class="pointer-events-none absolute -inset-4 rounded-full"
                    style="background: radial-gradient(closest-side, rgba(var(--glow-rgb, 225, 53, 255), 0.08), transparent)"
                ></div>
                <Icon icon=icon width="22px" height="22px" />
            </div>
            <div class="mt-4 text-sm font-semibold text-fg-primary">{title}</div>
            {(!hint.is_empty()).then(|| view! {
                <div class="mt-1.5 max-w-xs text-xs leading-relaxed text-fg-tertiary">{hint}</div>
            })}
            {children.map(|children| view! {
                <div class="mt-5 flex items-center justify-center gap-2">{children()}</div>
            })}
        </div>
    }
}
