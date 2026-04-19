//! Layout editor page — spatial zone arrangement with live effect preview.

use leptos::prelude::*;

use crate::app::WsContext;
use crate::components::layout_builder::LayoutBuilder;

const LAYOUT_PREVIEW_FPS_CAP: u32 = 60;

/// Dedicated layout editor page at `/layout`.
#[component]
pub fn LayoutPage() -> impl IntoView {
    let ws = expect_context::<WsContext>();

    Effect::new(move |_| {
        ws.set_preview_cap.set(LAYOUT_PREVIEW_FPS_CAP);
        ws.set_preview_width_cap.set(0);
    });
    on_cleanup(move || {
        ws.set_preview_cap.set(crate::ws::DEFAULT_PREVIEW_FPS_CAP);
        ws.set_preview_width_cap.set(0);
    });

    view! {
        <div class="flex h-full min-h-0 flex-col overflow-hidden">
            <LayoutBuilder />
        </div>
    }
}
