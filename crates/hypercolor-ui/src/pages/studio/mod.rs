//! `/studio` — the unified surface-centric composition workspace (Spec 65).
//!
//! Wave 2 ships this as a routable stub gated behind the `studio_ui_beta`
//! flag; Wave 4 builds the three-rail Lights / Stage / Layers shell here.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::page_header::{PageAccent, PageHeader};
use crate::icons::*;

#[component]
pub fn StudioPage() -> impl IntoView {
    view! {
        <div class="flex h-full flex-col">
            <PageHeader
                icon=LuLayers
                title="Studio"
                tagline="Composition workspace"
                accent=PageAccent::Purple
            />
            <div class="flex flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
                <Icon icon=LuLayers width="40px" height="40px" style="color: rgba(225, 53, 255, 0.3)" />
                <div class="text-sm font-semibold text-fg-secondary">"Studio is taking shape"</div>
                <div class="max-w-sm text-xs text-fg-tertiary/70">
                    "The unified Lights, Stage, and Layers workspace lands in an upcoming wave."
                </div>
            </div>
        </div>
    }
}
