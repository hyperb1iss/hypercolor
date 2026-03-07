//! Layout editor page — spatial zone arrangement with live effect preview.

use leptos::prelude::*;

use crate::components::layout_builder::LayoutBuilder;

/// Dedicated layout editor page at `/layout`.
#[component]
pub fn LayoutPage() -> impl IntoView {
    view! {
        <div class="flex h-full min-h-0 flex-col overflow-hidden animate-fade-in">
            <LayoutBuilder />
        </div>
    }
}
