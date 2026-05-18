//! `/media` — the media catalog page (Spec 65 §7).
//!
//! Wave 2 ships this as a routable stub gated behind the `studio_ui_beta`
//! flag; Wave 3 builds the catalog grid, upload, search, and detail panel.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::page_header::{PageAccent, PageHeader};
use crate::icons::*;

#[component]
pub fn MediaPage() -> impl IntoView {
    view! {
        <div class="flex h-full flex-col">
            <PageHeader
                icon=LuFolder
                title="Media"
                tagline="Media catalog"
                accent=PageAccent::Coral
            />
            <div class="flex flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
                <Icon icon=LuFolder width="40px" height="40px" style="color: rgba(255, 106, 193, 0.3)" />
                <div class="text-sm font-semibold text-fg-secondary">"Media catalog is on the way"</div>
                <div class="max-w-sm text-xs text-fg-tertiary/70">
                    "Upload, search, and organize media for use as composition layers."
                </div>
            </div>
        </div>
    }
}
