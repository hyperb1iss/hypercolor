//! Status bar — connection state, FPS, device count.

use leptos::prelude::*;

use crate::app::WsContext;
use crate::ws::ConnectionState;

/// Compact status indicators in the header bar.
#[component]
pub fn StatusBar() -> impl IntoView {
    let ws = expect_context::<WsContext>();

    view! {
        <div class="flex items-center gap-4 text-xs font-mono text-zinc-500">
            // Active effect name
            {move || ws.active_effect.get().map(|name| {
                view! {
                    <div class="flex items-center gap-1.5 text-zinc-300">
                        <div class="w-1.5 h-1.5 rounded-full bg-electric-purple animate-pulse" />
                        <span class="max-w-[140px] truncate">{name}</span>
                    </div>
                }
            })}

            <div class="w-px h-3 bg-white/5" />

            // Connection indicator
            <div class="flex items-center gap-1.5">
                <div
                    class="w-1.5 h-1.5 rounded-full transition-colors duration-300"
                    class=("bg-success-green", move || ws.connection_state.get() == ConnectionState::Connected)
                    class=("bg-error-red", move || ws.connection_state.get() == ConnectionState::Error)
                    class=("bg-electric-yellow", move || ws.connection_state.get() == ConnectionState::Connecting)
                    class=("bg-zinc-600", move || ws.connection_state.get() == ConnectionState::Disconnected)
                />
                <span>{move || ws.connection_state.get().to_string()}</span>
            </div>

            // FPS counter
            <div class="flex items-center gap-1">
                <span class="text-zinc-600">"FPS"</span>
                <span class="tabular-nums text-zinc-400">
                    {move || format!("{:.0}", ws.fps.get())}
                </span>
            </div>
        </div>
    }
}
