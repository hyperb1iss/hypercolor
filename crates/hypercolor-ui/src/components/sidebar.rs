//! Fixed navigation sidebar — 56px collapsed, 220px on hover.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

/// Navigation sidebar with expand-on-hover.
#[component]
pub fn Sidebar() -> impl IntoView {
    let (expanded, set_expanded) = signal(false);
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
            class="flex flex-col h-full bg-layer-1 border-r border-white/5 transition-all duration-200 ease-out shrink-0"
            class:w-14=move || !expanded.get()
            class:w-56=move || expanded.get()
            on:mouseenter=move |_| set_expanded.set(true)
            on:mouseleave=move |_| set_expanded.set(false)
        >
            // Logo
            <div class="h-12 flex items-center px-4 border-b border-white/5">
                <div class="w-6 h-6 rounded-md bg-gradient-to-br from-electric-purple to-neon-cyan flex items-center justify-center">
                    <span class="text-xs font-bold text-white">"H"</span>
                </div>
                <span
                    class="ml-3 text-sm font-semibold tracking-wide text-zinc-200 whitespace-nowrap overflow-hidden transition-opacity duration-200"
                    class:opacity-0=move || !expanded.get()
                    class:opacity-100=move || expanded.get()
                >
                    "Hypercolor"
                </span>
            </div>

            // Nav items
            <div class="flex-1 py-2 space-y-1 px-2">
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
                                let base = "flex items-center h-10 px-3 rounded-lg transition-colors duration-150 group relative";
                                if is_active.get() {
                                    format!("{base} bg-white/5 text-zinc-100")
                                } else {
                                    format!("{base} text-zinc-400 hover:text-zinc-200 hover:bg-white/[0.03]")
                                }
                            }
                        >
                            // Active indicator bar
                            <div
                                class="absolute left-0 top-1/2 -translate-y-1/2 w-0.5 h-5 rounded-full bg-electric-purple transition-opacity duration-150"
                                class:opacity-0=move || !is_active.get()
                                class:opacity-100=move || is_active.get()
                            />
                            <span class="w-5 h-5 flex items-center justify-center shrink-0" inner_html=item.icon />
                            <span
                                class="ml-3 text-sm whitespace-nowrap overflow-hidden transition-opacity duration-200"
                                class:opacity-0=move || !expanded.get()
                                class:opacity-100=move || expanded.get()
                            >
                                {item.label}
                            </span>
                        </A>
                    }
                }).collect_view()}
            </div>
        </nav>
    }
}

struct NavItem {
    path: &'static str,
    label: &'static str,
    icon: String,
}

// Simple SVG icons as inline strings

fn icon_dashboard() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="9" rx="1"/><rect x="14" y="3" width="7" height="5" rx="1"/><rect x="14" y="12" width="7" height="9" rx="1"/><rect x="3" y="16" width="7" height="5" rx="1"/></svg>"#.to_string()
}

fn icon_effects() -> String {
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2L2 7l10 5 10-5-10-5z"/><path d="M2 17l10 5 10-5"/><path d="M2 12l10 5 10-5"/></svg>"#.to_string()
}
