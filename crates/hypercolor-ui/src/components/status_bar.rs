//! Status bar — connection state, FPS, device count.

use leptos::prelude::*;

use crate::ws::ConnectionState;

/// Compact status indicators in the header bar.
#[component]
pub fn StatusBar() -> impl IntoView {
    // These will be wired to real WS signals in Wave 5
    let (connection, _set_connection) = signal(ConnectionState::Disconnected);
    let (fps, _set_fps) = signal(0.0_f32);
    let (devices, _set_devices) = signal(0_usize);

    view! {
        <div class="flex items-center gap-4 text-xs font-mono text-zinc-500">
            // Connection indicator
            <div class="flex items-center gap-1.5">
                <div
                    class="w-1.5 h-1.5 rounded-full transition-colors duration-300"
                    class=("bg-success-green", move || connection.get() == ConnectionState::Connected)
                    class=("bg-error-red", move || connection.get() == ConnectionState::Error)
                    class=("bg-electric-yellow", move || connection.get() == ConnectionState::Connecting)
                    class=("bg-zinc-600", move || connection.get() == ConnectionState::Disconnected)
                />
                <span>{move || connection.get().to_string()}</span>
            </div>

            // FPS counter
            <div class="flex items-center gap-1">
                <span class="text-zinc-600">"FPS"</span>
                <span class="tabular-nums text-zinc-400">
                    {move || format!("{:.0}", fps.get())}
                </span>
            </div>

            // Device count
            <div class="flex items-center gap-1">
                <span class="text-zinc-600">"Devices"</span>
                <span class="tabular-nums text-zinc-400">
                    {move || devices.get()}
                </span>
            </div>
        </div>
    }
}
