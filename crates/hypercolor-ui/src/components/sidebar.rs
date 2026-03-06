//! Fixed navigation sidebar — nav + now-playing section with player controls.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

use crate::app::EffectsContext;
use crate::icons::*;

/// Sidebar collapsed state, shared via context so the shell can react.
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct SidebarState {
    pub collapsed: ReadSignal<bool>,
    pub set_collapsed: WriteSignal<bool>,
}

/// Category -> accent RGB string for inline styles.
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
            icon: LuLayoutDashboard,
        },
        NavItem {
            path: "/effects",
            label: "Effects",
            icon: LuLayers,
        },
        NavItem {
            path: "/devices",
            label: "Devices",
            icon: LuCpu,
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
            class="flex flex-col h-full bg-surface-raised border-r border-edge-subtle shrink-0 transition-[width] duration-250 ease-out relative"
            class:w-56=move || !collapsed.get()
            class:w-14=move || collapsed.get()
        >
            // Logo section
            <div class="h-14 flex items-center px-4 border-b border-edge-subtle">
                <div class="w-7 h-7 rounded-lg bg-gradient-to-br from-electric-purple via-coral to-neon-cyan flex items-center justify-center animate-breathe" style="--glow-rgb: 225, 53, 255">
                    <span class="text-[11px] font-bold text-white">"H"</span>
                </div>
                <span
                    class="ml-3 text-sm font-semibold tracking-wider text-fg-primary whitespace-nowrap overflow-hidden transition-opacity duration-200"
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
                                    format!("{base} bg-accent-muted text-fg-primary")
                                } else {
                                    format!("{base} text-fg-tertiary hover:text-fg-primary hover:bg-surface-hover/30")
                                }
                            }
                        >
                            // Active indicator bar
                            <div
                                class="absolute left-0 top-1/2 -translate-y-1/2 w-[3px] h-5 rounded-r-full bg-accent transition-all duration-200"
                                class:opacity-0=move || !is_active.get()
                                class:opacity-100=move || is_active.get()
                                style:box-shadow=move || if is_active.get() { "0 0 8px rgba(225, 53, 255, 0.5)" } else { "none" }
                            />
                            <span
                                class="w-[18px] h-[18px] flex items-center justify-center shrink-0"
                                class:text-accent=move || is_active.get()
                            >
                                <Icon icon=item.icon width="18px" height="18px" />
                            </span>
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

            // Now Playing — full-width panel pinned to bottom, no rounded corners
            {move || {
                if !has_active.get() || collapsed.get() {
                    return None;
                }
                let name = fx.active_effect_name.get().unwrap_or_default();
                let cat = fx.active_effect_category.get();
                let rgb = category_accent_rgb(&cat).to_string();

                // Category accent: left edge glow strip + subtle tinted background
                let panel_style = format!(
                    "background: linear-gradient(90deg, rgba({rgb}, 0.10) 0%, rgba({rgb}, 0.02) 40%, transparent 100%); \
                     box-shadow: inset 3px 0 0 rgb({rgb}), inset 4px 0 12px rgba({rgb}, 0.15)"
                );
                let dot_style = format!(
                    "background: rgb({rgb}); box-shadow: 0 0 8px rgba({rgb}, 0.7)"
                );

                Some(view! {
                    <div
                        class="border-t border-edge-subtle px-4 py-4 space-y-4 animate-fade-in"
                        style=panel_style
                    >
                        // Now playing label
                        <div class="text-[9px] font-mono uppercase tracking-[0.15em] text-fg-tertiary/60">"Now Playing"</div>

                        // Effect name + category
                        <div class="flex items-center gap-2.5 min-w-0">
                            <div class="w-2.5 h-2.5 rounded-full dot-alive shrink-0" style=dot_style />
                            <div class="min-w-0 flex-1">
                                <div class="text-sm font-medium text-fg-primary truncate leading-tight">{name}</div>
                                <div class="text-[10px] text-fg-tertiary capitalize mt-0.5">{cat}</div>
                            </div>
                        </div>

                        // Player controls — full-width row with bigger touch targets
                        <div class="flex items-center justify-between">
                            <button
                                class="p-2 rounded-lg text-fg-tertiary hover:text-fg-primary hover:bg-surface-hover/40 player-btn"
                                title="Previous effect"
                                on:click=move |_| navigate_effect(-1)
                            >
                                <Icon icon=LuSkipBack width="16px" height="16px" />
                            </button>
                            <button
                                class="p-2 rounded-lg text-error-red/40 hover:text-error-red hover:bg-error-red/[0.06] player-btn"
                                title="Stop effect"
                                on:click=move |_| fx.stop_effect()
                            >
                                <Icon icon=LuSquare width="16px" height="16px" />
                            </button>
                            <button
                                class="p-2 rounded-lg text-fg-tertiary hover:text-fg-primary hover:bg-surface-hover/40 player-btn"
                                title="Next effect"
                                on:click=move |_| navigate_effect(1)
                            >
                                <Icon icon=LuSkipForward width="16px" height="16px" />
                            </button>
                            <button
                                class="p-2 rounded-lg text-fg-tertiary hover:text-fg-primary hover:bg-surface-hover/40 player-btn"
                                title="Random effect"
                                on:click=move |_| random_effect()
                            >
                                <Icon icon=LuShuffle width="16px" height="16px" />
                            </button>
                        </div>
                    </div>
                })
            }}

            // Collapse toggle at bottom
            <div class="px-2 py-3 border-t border-edge-subtle">
                <button
                    class="flex items-center justify-center w-full h-8 rounded-lg text-fg-tertiary hover:text-fg-secondary
                           hover:bg-surface-hover/30 btn-press"
                    on:click=move |_| set_collapsed.update(|v| *v = !*v)
                    title=move || if collapsed.get() { "Expand sidebar" } else { "Collapse sidebar" }
                >
                    <span
                        class="w-4 h-4 flex items-center justify-center transition-transform duration-200"
                        class:rotate-180=move || collapsed.get()
                    >
                        <Icon icon=LuChevronLeft width="16px" height="16px" />
                    </span>
                </button>
            </div>
        </nav>
    }
}

struct NavItem {
    path: &'static str,
    label: &'static str,
    icon: icondata_core::Icon,
}
