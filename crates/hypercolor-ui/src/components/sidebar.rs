//! Fixed navigation sidebar — open by default, manually collapsible.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

/// Sidebar collapsed state, shared via context so the shell can react.
#[derive(Clone, Copy)]
pub struct SidebarState {
    pub collapsed: ReadSignal<bool>,
    pub set_collapsed: WriteSignal<bool>,
}

/// Navigation sidebar with manual toggle.
#[component]
pub fn Sidebar() -> impl IntoView {
    let (collapsed, set_collapsed) = signal(false);
    provide_context(SidebarState {
        collapsed,
        set_collapsed,
    });

    let location = use_location();

    let nav_items = vec![
        NavItem {
            path: "/",
            label: "Dashboard",
            icon: icon_dashboard(),
        },
        NavItem {
            path: "/effects",
            label: "Effects",
            icon: icon_effects(),
        },
    ];

    view! {
        <nav
            class="flex flex-col h-full bg-layer-1 border-r border-white/[0.04] shrink-0 transition-[width] duration-250 ease-out relative"
            class:w-56=move || !collapsed.get()
            class:w-14=move || collapsed.get()
        >
            // Logo section
            <div class="h-14 flex items-center px-4 border-b border-white/[0.04]">
                <div class="w-7 h-7 rounded-lg bg-gradient-to-br from-electric-purple via-coral to-neon-cyan flex items-center justify-center shadow-[0_0_12px_rgba(225,53,255,0.3)]">
                    <span class="text-[11px] font-bold text-white">"H"</span>
                </div>
                <span
                    class="ml-3 text-sm font-semibold tracking-wider text-zinc-200 whitespace-nowrap overflow-hidden transition-opacity duration-200"
                    class:opacity-0=move || collapsed.get()
                    class:opacity-100=move || !collapsed.get()
                    class:w-0=move || collapsed.get()
                >
                    "Hypercolor"
                </span>
            </div>

            // Nav items
            <div class="flex-1 py-3 space-y-0.5 px-2">
                {nav_items.into_iter().map(|item| {
                    let is_active = {
                        let path = item.path;
                        Memo::new(move |_| {
                            let current = location.pathname.get();
                            if path == "/" {
                                current == "/"
                            } else {
                                current.starts_with(path)
                            }
                        })
                    };

                    view! {
                        <A
                            href=item.path
                            attr:class=move || {
                                let base = "flex items-center h-10 px-3 rounded-lg transition-all duration-150 group relative";
                                if is_active.get() {
                                    format!("{base} bg-electric-purple/[0.08] text-zinc-100")
                                } else {
                                    format!("{base} text-zinc-500 hover:text-zinc-200 hover:bg-white/[0.03]")
                                }
                            }
                        >
                            // Active indicator bar
                            <div
                                class="absolute left-0 top-1/2 -translate-y-1/2 w-[3px] h-5 rounded-r-full bg-electric-purple transition-all duration-200"
                                class:opacity-0=move || !is_active.get()
                                class:opacity-100=move || is_active.get()
                                style:box-shadow=move || if is_active.get() { "0 0 8px rgba(225, 53, 255, 0.5)" } else { "none" }
                            />
                            <span
                                class="w-[18px] h-[18px] flex items-center justify-center shrink-0"
                                class:text-electric-purple=move || is_active.get()
                                inner_html=item.icon
                            />
                            <span
                                class="ml-3 text-sm whitespace-nowrap overflow-hidden transition-all duration-200"
                                class:opacity-0=move || collapsed.get()
                                class:opacity-100=move || !collapsed.get()
                                class:w-0=move || collapsed.get()
                            >
                                {item.label}
                            </span>
                        </A>
                    }
                }).collect_view()}
            </div>

            // Collapse toggle at bottom
            <div class="px-2 py-3 border-t border-white/[0.04]">
                <button
                    class="flex items-center justify-center w-full h-8 rounded-lg text-zinc-600 hover:text-zinc-400
                           hover:bg-white/[0.03] transition-all duration-150"
                    on:click=move |_| set_collapsed.update(|v| *v = !*v)
                    title=move || if collapsed.get() { "Expand sidebar" } else { "Collapse sidebar" }
                >
                    <span
                        class="w-4 h-4 flex items-center justify-center transition-transform duration-200"
                        class:rotate-180=move || collapsed.get()
                        inner_html=icon_chevron_left()
                    />
                </button>
            </div>
        </nav>
    }
}

struct NavItem {
    path: &'static str,
    label: &'static str,
    icon: String,
}

fn icon_dashboard() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="9" rx="1"/><rect x="14" y="3" width="7" height="5" rx="1"/><rect x="14" y="12" width="7" height="9" rx="1"/><rect x="3" y="16" width="7" height="5" rx="1"/></svg>"#.to_string()
}

fn icon_effects() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2L2 7l10 5 10-5-10-5z"/><path d="M2 17l10 5 10-5"/><path d="M2 12l10 5 10-5"/></svg>"#.to_string()
}

fn icon_chevron_left() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 18l-6-6 6-6"/></svg>"#.to_string()
}
