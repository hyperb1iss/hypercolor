//! Status bar — connection state, FPS, active effect.

use leptos::prelude::*;

use crate::app::WsContext;
use crate::ws::ConnectionState;

/// Compact status indicators for the header bar.
#[component]
pub fn StatusBar() -> impl IntoView {
    let ws = expect_context::<WsContext>();

    view! {
        <div class="flex items-center gap-3 text-[11px] font-mono text-fg-dim">
            // Connection indicator
            <div class="flex items-center gap-1.5">
                <div
                    class="w-[5px] h-[5px] rounded-full transition-colors duration-300"
                    class=("bg-success-green shadow-[0_0_6px_rgba(80,250,123,0.5)] dot-alive", move || ws.connection_state.get() == ConnectionState::Connected)
                    class=("bg-error-red shadow-[0_0_6px_rgba(255,99,99,0.5)]", move || ws.connection_state.get() == ConnectionState::Error)
                    class=("bg-electric-yellow animate-pulse", move || ws.connection_state.get() == ConnectionState::Connecting)
                    class=("bg-zinc-600", move || ws.connection_state.get() == ConnectionState::Disconnected)
                />
                <span class="text-fg-dim">{move || ws.connection_state.get().to_string()}</span>
            </div>

            <div class="w-px h-3 bg-white/[0.05]" />

            // FPS counter
            <div class="flex items-center gap-1.5">
                <span class="text-fg-dim/60">"FPS"</span>
                <span class="tabular-nums text-fg-muted">
                    {move || format!("{:.0}", ws.fps.get())}
                </span>
            </div>
        </div>
    }
}
