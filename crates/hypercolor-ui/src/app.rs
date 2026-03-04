use leptos::prelude::*;
use leptos_meta::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use crate::components::shell::Shell;
use crate::pages::dashboard::DashboardPage;
use crate::pages::effects::EffectsPage;
use crate::ws::{CanvasFrame, ConnectionState, WsManager};

/// Global WebSocket state provided via Leptos context.
#[derive(Clone, Copy)]
pub struct WsContext {
    pub canvas_frame: ReadSignal<Option<CanvasFrame>>,
    pub connection_state: ReadSignal<ConnectionState>,
    pub fps: ReadSignal<f32>,
    pub active_effect: ReadSignal<Option<String>>,
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    // Global WebSocket connection — shared across all pages via context
    let ws = WsManager::new();
    let ws_ctx = WsContext {
        canvas_frame: ws.canvas_frame,
        connection_state: ws.connection_state,
        fps: ws.fps,
        active_effect: ws.active_effect,
    };
    provide_context(ws_ctx);

    view! {
        <Meta charset="UTF-8" />
        <Meta name="viewport" content="width=device-width, initial-scale=1.0" />
        <Title text="Hypercolor" />

        <Router>
            <Shell>
                <Routes fallback=|| view! { <p class="text-zinc-400 p-8">"Not found"</p> }>
                    <Route path=path!("/") view=DashboardPage />
                    <Route path=path!("/effects") view=EffectsPage />
                    <Route path=path!("/effects/:id") view=EffectsPage />
                </Routes>
            </Shell>
        </Router>
    }
}
