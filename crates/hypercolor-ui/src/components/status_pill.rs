//! Header status pill — the dashboard-strip grammar as a shared primitive.
//!
//! A glowing dot, a mono micro-label, and a colored value line. Pages (and
//! UI extensions) compose rows of these inside a `HeaderToolbar` slot,
//! separated by `w-px h-5 bg-edge-subtle/30` divider bars, so every header
//! strip in the app reads as one system.

use leptos::prelude::*;

use crate::components::section_label::{LabelSize, LabelTone, label_class};

#[component]
pub fn StatusPill(
    label: &'static str,
    #[prop(into)] value: String,
    color: &'static str,
    pulsing: bool,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-2.5">
            <div
                class="w-2 h-2 rounded-full shrink-0"
                class=("animate-pulse", pulsing)
                style=format!("background: {color}; box-shadow: 0 0 8px {color}aa")
            />
            <div>
                <div class=label_class(LabelSize::Micro, LabelTone::Default)>{label}</div>
                <div
                    class="text-[14px] font-semibold tabular-nums leading-none mt-0.5"
                    style=format!("color: {color}")
                >
                    {value}
                </div>
            </div>
        </div>
    }
}

/// Divider bar between pills in a header strip.
#[component]
pub fn StatusPillDivider() -> impl IntoView {
    view! { <div class="w-px h-5 bg-edge-subtle/30" /> }
}
