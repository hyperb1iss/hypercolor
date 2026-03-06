//! Status bar — connection state, FPS, active effect.

use leptos::prelude::*;

use crate::app::WsContext;
use crate::ws::ConnectionState;

/// Compact status indicators for the header bar.
#[component]
pub fn StatusBar() -> impl IntoView {
    let ws = expect_context::<WsContext>();

    view! {
        <div class="flex items-center gap-3 text-[11px] font-mono text-text-tertiary">
            // Connection indicator
            <div class="flex items-center gap-1.5">
                <div
                    class="w-[5px] h-[5px] rounded-full transition-colors duration-300"
                    style=move || {
                        match ws.connection_state.get() {
                            ConnectionState::Connected => "background: rgb(80, 250, 123); box-shadow: 0 0 6px rgba(80, 250, 123, 0.5); animation: dot-alive 2s ease-in-out infinite",
                            ConnectionState::Error => "background: rgb(255, 99, 99); box-shadow: 0 0 6px rgba(255, 99, 99, 0.5)",
                            ConnectionState::Connecting => "background: rgb(241, 250, 140); animation: pulse 2s cubic-bezier(0.4, 0, 0.6, 1) infinite",
                            ConnectionState::Disconnected => "background: rgb(82, 82, 91)",
                        }
                    }
                />
                <span class="text-text-tertiary">{move || ws.connection_state.get().to_string()}</span>
            </div>

            <div class="w-px h-3 bg-border-subtle" />

            // FPS counter
            <div class="flex items-center gap-1.5">
                <span class="text-text-tertiary/60">"FPS"</span>
                <span class="tabular-nums text-text-secondary">
                    {move || format!("{:.0}", ws.fps.get())}
                </span>
            </div>
        </div>
    }
}
