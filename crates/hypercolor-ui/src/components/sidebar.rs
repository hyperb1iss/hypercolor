//! Fixed navigation sidebar — nav + now-playing section with player controls.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

use crate::app::EffectsContext;

/// Sidebar collapsed state, shared via context so the shell can react.
#[derive(Clone, Copy)]
pub struct SidebarState {
    pub collapsed: ReadSignal<bool>,
    pub set_collapsed: WriteSignal<bool>,
}

/// Category → accent RGB string for inline styles.
fn category_accent_rgb(category: &str) -> &'static str {
    match category {
        "ambient" => "128, 255, 234",
        "audio" => "255, 106, 193",
        "gaming" => "225, 53, 255",
        "reactive" => "241, 250, 140",
        "generative" => "80, 250, 123",
        "interactive" => "130, 170, 255",
        "productivity" => "255, 153, 255",
        "utility" => "139, 133, 160",
        _ => "225, 53, 255",
    }
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
    let fx = expect_context::<EffectsContext>();

    let has_active = Memo::new(move |_| fx.active_effect_id.get().is_some());

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

    // Navigate effects list (for prev/next)
    let navigate_effect = move |direction: i32| {
        let list = fx
            .effects_resource
            .get()
            .and_then(|r| r.ok())
            .unwrap_or_default();
        if list.is_empty() {
            return;
        }
        let current = fx.active_effect_id.get();
        let idx = current
            .as_ref()
            .and_then(|id| list.iter().position(|e| &e.id == id))
            .unwrap_or(0);
        let next_idx = ((idx as i32 + direction).rem_euclid(list.len() as i32)) as usize;
        fx.apply_effect(list[next_idx].id.clone());
    };

    // Random effect
    let random_effect = move || {
        let list = fx
            .effects_resource
            .get()
            .and_then(|r| r.ok())
            .unwrap_or_default();
        if list.is_empty() {
            return;
        }
        let current = fx.active_effect_id.get();
        let rand = js_sys::Math::random();
        let mut idx = (rand * list.len() as f64) as usize;
        if list.len() > 1 {
            if let Some(ref cur) = current {
                if list.get(idx).is_some_and(|e| &e.id == cur) {
                    idx = (idx + 1) % list.len();
                }
            }
        }
        if let Some(effect) = list.get(idx) {
            fx.apply_effect(effect.id.clone());
        }
    };

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
                                let base = "flex items-center h-10 px-3 rounded-lg nav-item-hover group relative";
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

            // Now Playing section — shows when an effect is active and sidebar is expanded
            {move || {
                if !has_active.get() || collapsed.get() {
                    return None;
                }
                let name = fx.active_effect_name.get().unwrap_or_default();
                let cat = fx.active_effect_category.get();
                let rgb = category_accent_rgb(&cat).to_string();
                let bg_style = format!(
                    "background: linear-gradient(135deg, rgba({rgb}, 0.08) 0%, rgba({rgb}, 0.02) 100%); \
                     border-color: rgba({rgb}, 0.1)"
                );
                let dot_style = format!(
                    "background: rgb({rgb}); box-shadow: 0 0 6px rgba({rgb}, 0.6)"
                );

                Some(view! {
                    <div
                        class="mx-2 mb-2 rounded-xl border p-3 space-y-3 animate-pop-in"
                        style=bg_style
                    >
                        // Effect name + category dot
                        <div class="flex items-center gap-2 min-w-0">
                            <div class="w-2 h-2 rounded-full dot-alive shrink-0" style=dot_style />
                            <div class="min-w-0 flex-1">
                                <div class="text-xs font-medium text-fg truncate">{name}</div>
                                <div class="text-[10px] text-fg-dim capitalize">{cat}</div>
                            </div>
                        </div>

                        // Player controls — compact row
                        <div class="flex items-center justify-center gap-1">
                            // Previous
                            <button
                                class="p-1.5 rounded-lg text-fg-dim hover:text-fg hover:bg-white/[0.08] player-btn"
                                title="Previous effect"
                                on:click=move |_| navigate_effect(-1)
                            >
                                <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="currentColor">
                                    <path d="M6 6h2v12H6zm3.5 6 8.5 6V6z"/>
                                </svg>
                            </button>
                            // Stop
                            <button
                                class="p-1.5 rounded-lg text-error-red/40 hover:text-error-red hover:bg-error-red/[0.08] player-btn"
                                title="Stop effect"
                                on:click=move |_| fx.stop_effect()
                            >
                                <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="currentColor">
                                    <rect x="6" y="6" width="12" height="12" rx="1"/>
                                </svg>
                            </button>
                            // Next
                            <button
                                class="p-1.5 rounded-lg text-fg-dim hover:text-fg hover:bg-white/[0.08] player-btn"
                                title="Next effect"
                                on:click=move |_| navigate_effect(1)
                            >
                                <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="currentColor">
                                    <path d="M6 18l8.5-6L6 6v12zM16 6v12h2V6h-2z"/>
                                </svg>
                            </button>
                            // Shuffle
                            <button
                                class="p-1.5 rounded-lg text-fg-dim hover:text-fg hover:bg-white/[0.08] player-btn"
                                title="Random effect"
                                on:click=move |_| random_effect()
                            >
                                <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                                    <polyline points="16 3 21 3 21 8"/>
                                    <line x1="4" y1="20" x2="21" y2="3"/>
                                    <polyline points="21 16 21 21 16 21"/>
                                    <line x1="15" y1="15" x2="21" y2="21"/>
                                    <line x1="4" y1="4" x2="9" y2="9"/>
                                </svg>
                            </button>
                        </div>
                    </div>
                })
            }}

            // Collapse toggle at bottom
            <div class="px-2 py-3 border-t border-white/[0.04]">
                <button
                    class="flex items-center justify-center w-full h-8 rounded-lg text-zinc-600 hover:text-zinc-400
                           hover:bg-white/[0.03] btn-press"
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
