//! App shell — sidebar + header + content area.

use leptos::prelude::*;

use crate::components::sidebar::Sidebar;
use crate::components::status_bar::StatusBar;

/// Top-level layout shell. Sidebar left, header + content right.
#[component]
pub fn Shell(children: Children) -> impl IntoView {
    view! {
        <div class="flex h-screen bg-layer-0 text-zinc-100 overflow-hidden">
            <Sidebar />
            <div class="flex flex-col flex-1 min-w-0">
                <header class="h-12 flex items-center justify-between px-4 border-b border-white/5 bg-layer-1">
                    <div class="text-sm text-zinc-400 font-mono tracking-wide">
                        "HYPERCOLOR"
                    </div>
                    <StatusBar />
                </header>
                <main class="flex-1 overflow-auto p-6">
                    {children()}
                </main>
            </div>
        </div>
    }
}
