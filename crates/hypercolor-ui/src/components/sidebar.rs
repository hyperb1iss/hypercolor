//! Fixed navigation sidebar — nav + now-playing section with player controls.
//! The Now Playing panel renders a live canvas thumbnail of the running effect
//! and extracts a color palette for ambient glow styling.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;
use leptos_router::components::A;
use leptos_router::hooks::use_location;
use leptos_use::use_throttle_fn_with_arg;

use crate::api;
use crate::app::{EffectsContext, FrameAnalysisContext, StudioFlag, WsContext};
use crate::async_helpers::spawn_api_call;
use crate::color::{self, CanvasPalette};
use crate::components::canvas_preview::CanvasPreview;
use crate::components::scene_switcher::{
    SceneSwitcherMenu, active_scene_label, active_scene_locked,
};
use crate::components::zone_now_playing::{SidebarZoneRows, set_zone_enabled};
use crate::config_state::ConfigContext;
use crate::icons::*;
use crate::route_ui::{NowPlayingCanvasMode, now_playing_canvas_mode};
use crate::style_utils::category_accent_rgb;
use crate::tauri_bridge;
use hypercolor_leptos_ext::events::Input;
use hypercolor_leptos_ext::prelude::random_unit;

const SPONSOR_URL: &str = "https://github.com/sponsors/hyperb1iss";

// ── Sidebar Component ──────────────────────────────────────────────────────

/// Navigation sidebar with manual toggle.
#[component]
pub fn Sidebar() -> impl IntoView {
    let (collapsed, set_collapsed) = signal(false);

    let location = use_location();
    let fx = expect_context::<EffectsContext>();
    let studio_flag = expect_context::<StudioFlag>();
    let zones_ctx = expect_context::<crate::zones::ZonesContext>();

    // Multi-zone scenes keep the panel alive whenever any zone is
    // showing something, even while the primary zone sits idle — the
    // per-zone rows are the content then, not the singular effect.
    let has_active = Memo::new(move |_| {
        fx.active_effect_id.get().is_some()
            || (zones_ctx.multi_zone.get()
                && fx.zone_effects.with(|zones| {
                    zones
                        .iter()
                        .any(|state| state.effect_id.is_some() || state.zone.top_layer.is_some())
                }))
    });
    let canvas_mode = Signal::derive(move || now_playing_canvas_mode(&location.pathname.get()));

    // ── Live canvas + palette from WebSocket frames ────────────────────
    let ws = use_context::<WsContext>();
    // Avoid Signal::derive re-wrap — pass ReadSignals directly when WS is available
    let (canvas_frame, preview_fps, preview_target_fps): (
        Signal<Option<crate::ws::CanvasFrame>>,
        Signal<f32>,
        Signal<u32>,
    ) = match ws {
        Some(ctx) => (
            ctx.canvas_frame.into(),
            ctx.preview_fps.into(),
            ctx.preview_target_fps.into(),
        ),
        None => (
            Signal::derive(|| None),
            Signal::derive(|| 0.0_f32),
            Signal::derive(|| 0_u32),
        ),
    };
    let frame_analysis = use_context::<FrameAnalysisContext>();
    let (live_palette, set_live_palette) = signal(None::<CanvasPalette>);
    let global_brightness_resource = LocalResource::new(api::fetch_global_brightness);
    let (global_brightness, set_global_brightness) = signal(100_u8);

    Effect::new(move |_| {
        if let Some(Ok(brightness)) = global_brightness_resource.get() {
            set_global_brightness.set(brightness);
        }
    });

    let push_global_brightness = use_throttle_fn_with_arg(
        move |brightness: u8| {
            spawn_api_call(
                "Global brightness update failed",
                api::set_global_brightness(brightness),
            );
        },
        50.0,
    );

    if let Some(frame_analysis) = frame_analysis {
        Effect::new(move |_| {
            if canvas_mode.get() != NowPlayingCanvasMode::Palette {
                return;
            }

            let Some(analysis) = frame_analysis.live_canvas.get() else {
                return;
            };

            let smoothed = live_palette
                .get_untracked()
                .map_or(analysis.palette, |old| {
                    color::lerp_palette(old, analysis.palette, 0.3)
                });
            set_live_palette.set(Some(smoothed));
        });
    }

    // Navigate effects list (for prev/next)
    let navigate_effect = move |direction: i32| {
        let list = fx
            .effects_index
            .get()
            .into_iter()
            .map(|entry| entry.effect)
            .filter(|effect| effect.runnable)
            .collect::<Vec<_>>();
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
            .effects_index
            .get()
            .into_iter()
            .map(|entry| entry.effect)
            .filter(|effect| effect.runnable)
            .collect::<Vec<_>>();
        if list.is_empty() {
            return;
        }
        let current = fx.active_effect_id.get();
        let rand = random_unit();
        let mut idx = (rand * list.len() as f64) as usize;
        if list.len() > 1
            && let Some(ref cur) = current
            && list.get(idx).is_some_and(|e| &e.id == cur)
        {
            idx = (idx + 1) % list.len();
        }
        if let Some(effect) = list.get(idx) {
            fx.apply_effect(effect.id.clone());
        }
    };

    // Memoized palette colors. Published once per live_palette tick as
    // --np-primary / --np-secondary / --np-tertiary CSS custom properties on
    // the Now Playing root, so every downstream style binding is a static
    // `rgb(var(--np-*))` string instead of a reactive format! closure that
    // wakes on every tick. All live-palette colors pass through harmonize_rgb
    // so the three hues sit in a cohesive L/S band.
    let primary_rgb = Memo::new(move |_| {
        let cat = fx.active_effect_category.get();
        if canvas_mode.get() != NowPlayingCanvasMode::Palette {
            category_accent_rgb(&cat).to_string()
        } else {
            live_palette.get().map_or_else(
                || category_accent_rgb(&cat).to_string(),
                |p| color::rgb_string(color::harmonize_rgb(p.primary)),
            )
        }
    });
    let secondary_rgb = Memo::new(move |_| {
        let cat = fx.active_effect_category.get();
        if canvas_mode.get() != NowPlayingCanvasMode::Palette {
            category_accent_rgb(&cat).to_string()
        } else {
            live_palette.get().map_or_else(
                || category_accent_rgb(&cat).to_string(),
                |p| color::rgb_string(color::harmonize_rgb(p.secondary)),
            )
        }
    });
    let tertiary_rgb = Memo::new(move |_| {
        let cat = fx.active_effect_category.get();
        if canvas_mode.get() != NowPlayingCanvasMode::Palette {
            category_accent_rgb(&cat).to_string()
        } else {
            live_palette.get().map_or_else(
                || category_accent_rgb(&cat).to_string(),
                |p| color::rgb_string(color::harmonize_rgb(p.tertiary)),
            )
        }
    });
    let open_sponsor = move |ev: leptos::ev::MouseEvent| {
        if !tauri_bridge::is_tauri_available() {
            return;
        }

        ev.prevent_default();
        spawn_local(async move {
            if let Err(error) = tauri_bridge::open_external_url(SPONSOR_URL).await {
                log::warn!("Sponsor link native open failed: {error}");
            }
        });
    };

    view! {
        <nav
            class="flex flex-col h-full bg-surface-raised border-r border-edge-subtle shrink-0 transition-[width] duration-250 ease-out relative"
            class:w-56=move || !collapsed.get()
            class:w-14=move || collapsed.get()
        >
            // Logo — canonical mark, static. The glow layer behind it drifts
            // through brand hues; the mark itself doesn't move.
            <div
                class="w-full border-b border-edge-subtle transition-[height] duration-300"
                class:h-14=move || collapsed.get()
                class:h-32=move || !collapsed.get()
            >
                // Collapsed: static 32px trinity.
                <div
                    class="items-center justify-center h-full logo-container"
                    style:display=move || if collapsed.get() { "flex" } else { "none" }
                >
                    <img
                        src="/assets/brand/mark-color.png"
                        alt="Hypercolor"
                        class="w-8 h-8 select-none logo-mark-image"
                        draggable="false"
                    />
                </div>

                // Expanded: full vertical lockup with chill aura behind.
                <div
                    class="flex-col items-center justify-center h-full px-3 overflow-hidden logo-container"
                    style:display=move || if collapsed.get() { "none" } else { "flex" }
                >
                    <div class="logo-bg logo-bg-mark" />
                    <img
                        src="/assets/brand/lockup-vertical-color.png"
                        alt="Hypercolor"
                        class="h-24 w-auto select-none object-contain logo-mark-image"
                        draggable="false"
                    />
                </div>
            </div>

            // Nav items — the set swaps with the studio_ui_beta flag (§5.1).
            <div class="flex-1 py-3 space-y-0.5 px-2">
                {move || nav_items(studio_flag.enabled.get()).into_iter().map(|item| {
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

                    let link = view! {
                        <A
                            href=item.path
                            attr:class=move || {
                                let base = "flex items-center h-10 px-3 rounded-lg nav-item-hover group relative";
                                if is_active.get() {
                                    format!("{base} text-fg-primary")
                                } else {
                                    format!("{base} text-fg-tertiary hover:text-fg-primary hover:bg-surface-hover/30")
                                }
                            }
                        >
                            // Active indicator bar
                            <div
                                class="absolute left-0 top-1/2 -translate-y-1/2 w-[3px] h-5 rounded-r-full bg-accent transition-opacity duration-200"
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
                                class="ml-3 text-sm whitespace-nowrap overflow-hidden transition-[opacity,width] duration-200"
                                class:opacity-0=move || collapsed.get()
                                class:opacity-100=move || !collapsed.get()
                                class:w-0=move || collapsed.get()
                            >
                                {item.label}
                            </span>
                        </A>
                    };

                    if item.divider_before {
                        view! {
                            <div class="h-px bg-edge-subtle/30 my-2 mx-1" />
                            {link}
                        }.into_any()
                    } else {
                        link.into_any()
                    }
                }).collect_view()}
            </div>

            // Sponsor link — above Now Playing, accent-styled
            <a
                href=SPONSOR_URL
                target="_blank"
                rel="noopener"
                on:click=open_sponsor
                class="flex items-center mx-2
                       text-fg-tertiary hover:text-fg-primary
                       transition-colors duration-200"
                class:justify-center=move || collapsed.get()
                class:gap-2=move || !collapsed.get()
                class:px-3=move || !collapsed.get()
                class:py-2=move || !collapsed.get()
                class:p-2=move || collapsed.get()
                title="Support Hypercolor development"
            >
                <span class="text-accent text-sm font-bold">{"\u{2665}"}</span>
                <span
                    class="text-[11px] whitespace-nowrap"
                    style:display=move || if collapsed.get() { "none" } else { "inline" }
                >"Sponsor the project"</span>
            </a>

            // Now Playing — live thumbnail + palette-styled panel
            //
            // IMPORTANT: The outer closure ONLY reads has_active + collapsed so that
            // palette/name/category updates don't rebuild the DOM (which would destroy
            // the canvas element and cause flicker). All dynamic values use fine-grained
            // reactive style:/class: bindings or inner {move || ...} text nodes.
            {move || {
                if !has_active.get() || collapsed.get() {
                    return None;
                }
                let push_global_brightness = push_global_brightness.clone();

                Some(view! {
                    <div
                        class="shrink-0 border-t border-edge-subtle py-3 space-y-3 animate-enter-fade"
                        style:--np-primary=move || primary_rgb.get()
                        style:--np-secondary=move || secondary_rgb.get()
                        style:--np-tertiary=move || tertiary_rgb.get()
                        style:box-shadow="inset 3px 0 0 rgb(var(--np-primary)), \
                                          inset 4px 0 12px rgba(var(--np-primary), 0.15), \
                                          inset 0 -1px 20px rgba(var(--np-secondary), 0.06)"
                        style:background="linear-gradient(180deg, \
                                          rgba(var(--np-primary), 0.04) 0%, \
                                          rgba(var(--np-secondary), 0.03) 60%, \
                                          transparent 100%)"
                    >
                        // Now Playing label
                        <div
                            class="px-4 text-[9px] font-mono uppercase tracking-[0.15em]"
                            style:color="rgba(var(--np-primary), 0.85)"
                        >
                            {move || if fx.is_playing.get() { "Now Playing" } else { "Paused" }}
                        </div>

                        // Live canvas thumbnail — only on pages without their own preview
                        {move || {
                            (canvas_mode.get() == NowPlayingCanvasMode::Preview).then(|| view! {
                                <div class="px-3 animate-enter-fade">
                                    <div
                                        class="relative rounded-lg overflow-hidden bg-black/40"
                                        style:box-shadow="0 4px 20px rgba(var(--np-primary), 0.25), \
                                                          0 0 40px rgba(var(--np-secondary), 0.08)"
                                    >
                                        <CanvasPreview
                                            frame=canvas_frame
                                            fps=preview_fps
                                            fps_target=preview_target_fps
                                            max_width="100%".to_string()
                                        />
                                    </div>
                                </div>
                            })
                        }}

                        // Effect name + category + audio toggle. Multi-zone
                        // scenes swap the singular metadata for one honest
                        // row per zone (capped, overflow links to Studio).
                        {move || if zones_ctx.multi_zone.get() {
                            view! { <SidebarZoneRows /> }.into_any()
                        } else {
                            view! {
                                <div class="px-4 flex items-center gap-2.5 min-w-0">
                                    <div
                                        class=move || if fx.is_playing.get() {
                                            "w-2 h-2 rounded-full dot-alive shrink-0"
                                        } else {
                                            "w-2 h-2 rounded-full shrink-0 opacity-50"
                                        }
                                        style:background="rgb(var(--np-primary))"
                                        style:box-shadow=move || if fx.is_playing.get() {
                                            "0 0 8px rgba(var(--np-primary), 0.7)".to_string()
                                        } else {
                                            String::new()
                                        }
                                    />
                                    <div class="min-w-0 flex-1">
                                        <div class="text-[11px] font-medium text-fg-primary truncate leading-tight">
                                            {move || fx.active_effect_name.get().unwrap_or_default()}
                                        </div>
                                        <div
                                            class="text-[10px] capitalize mt-0.5"
                                            style:color="rgba(var(--np-secondary), 0.85)"
                                        >
                                            {move || fx.active_effect_category.get()}
                                        </div>
                                    </div>
                                    <SidebarAudioToggle />
                                </div>
                            }.into_any()
                        }}

                        // Palette strip — shows extracted colors as a smooth gradient
                        <div class="px-4">
                            <div
                                class="h-[3px] rounded-full"
                                style:background="linear-gradient(90deg, \
                                                  rgb(var(--np-primary)) 0%, \
                                                  rgb(var(--np-secondary)) 50%, \
                                                  rgb(var(--np-tertiary)) 100%)"
                                style:opacity="0.7"
                                style:box-shadow="0 0 8px rgba(var(--np-primary), 0.3)"
                            />
                        </div>

                        // Player controls
                        <div class="px-4 flex items-center justify-between">
                            <button
                                class="p-2 rounded-lg text-fg-tertiary hover:text-fg-primary hover:bg-surface-hover/40 player-btn"
                                title="Previous effect"
                                aria-label="Previous effect"
                                on:click=move |_| navigate_effect(-1)
                            >
                                <Icon icon=LuSkipBack width="16px" height="16px" />
                            </button>
                            // In a multi-zone scene the pause toggle acts on
                            // the focused zone (primary when none is focused)
                            // and says so — it never silently stops only the
                            // primary while other zones keep rendering.
                            {move || if zones_ctx.multi_zone.get() {
                                let Some(state) = fx.focused_zone_effect.get() else {
                                    return ().into_any();
                                };
                                let zone_id = state.zone.id.clone();
                                let zone_name = state.zone.name.clone();
                                let enabled = state.zone.enabled;
                                let label = if enabled {
                                    format!("Pause {zone_name}")
                                } else {
                                    format!("Resume {zone_name}")
                                };
                                let icon_class = if enabled {
                                    "p-2 rounded-lg text-neon-cyan hover:text-neon-cyan hover:bg-neon-cyan/[0.08] player-btn"
                                } else {
                                    "p-2 rounded-lg text-neon-cyan/40 hover:text-neon-cyan hover:bg-neon-cyan/[0.06] player-btn"
                                };
                                view! {
                                    <button
                                        class=icon_class
                                        title=label.clone()
                                        aria-label=label
                                        on:click=move |_| set_zone_enabled(
                                            zones_ctx,
                                            zone_id.clone(),
                                            !enabled,
                                        )
                                    >
                                        {if enabled {
                                            view! { <Icon icon=LuPause width="16px" height="16px" /> }.into_any()
                                        } else {
                                            view! { <Icon icon=LuPlay width="16px" height="16px" /> }.into_any()
                                        }}
                                    </button>
                                }.into_any()
                            } else if fx.is_playing.get() {
                                view! {
                                    <button
                                        class="p-2 rounded-lg text-neon-cyan hover:text-neon-cyan hover:bg-neon-cyan/[0.08] player-btn"
                                        title="Pause effect"
                                        aria-label="Pause effect"
                                        on:click=move |_| fx.stop_effect()
                                    >
                                        <Icon icon=LuPause width="16px" height="16px" />
                                    </button>
                                }.into_any()
                            } else {
                                view! {
                                    <button
                                        class="p-2 rounded-lg text-neon-cyan/40 hover:text-neon-cyan hover:bg-neon-cyan/[0.06] player-btn"
                                        title="Resume effect"
                                        aria-label="Resume effect"
                                        on:click=move |_| fx.resume_effect()
                                    >
                                        <Icon icon=LuPlay width="16px" height="16px" />
                                    </button>
                                }.into_any()
                            }}
                            <button
                                class="p-2 rounded-lg text-fg-tertiary hover:text-fg-primary hover:bg-surface-hover/40 player-btn"
                                title="Next effect"
                                aria-label="Next effect"
                                on:click=move |_| navigate_effect(1)
                            >
                                <Icon icon=LuSkipForward width="16px" height="16px" />
                            </button>
                            <button
                                class="p-2 rounded-lg text-fg-tertiary hover:text-fg-primary hover:bg-surface-hover/40 player-btn"
                                title="Random effect"
                                aria-label="Random effect"
                                on:click=move |_| random_effect()
                            >
                                <Icon icon=LuShuffle width="16px" height="16px" />
                            </button>
                        </div>

                        <div class="px-4 flex items-center gap-2">
                            <input
                                type="range"
                                min="0"
                                max="100"
                                step="1"
                                aria-label="Global brightness"
                                class="min-w-0 flex-1 h-1 rounded-full appearance-none cursor-pointer"
                                style:accent-color="rgb(var(--np-primary))"
                                style:background="rgba(139, 133, 160, 0.15)"
                                prop:value=move || global_brightness.get().to_string()
                                on:input=move |ev| {
                                    if let Some(brightness) = Input::from_event(ev).value::<u8>() {
                                        set_global_brightness.set(brightness);
                                        push_global_brightness(brightness);
                                    }
                                }
                            />
                            <span class="shrink-0 text-fg-primary font-medium tabular-nums w-9 text-right text-[11px]">
                                {move || format!("{}%", global_brightness.get())}
                            </span>
                        </div>
                    </div>
                })
            }}

            // Scene chip — names the active scene and opens the switcher.
            // Rendered only when there is somewhere to switch to, and only
            // expanded (a 56px rail has no room for a scene name).
            {move || (!collapsed.get()).then(|| view! { <SidebarSceneChip /> })}

            // Bottom bar — collapse toggle only
            <div class="shrink-0 border-t border-edge-subtle px-2 py-2">
                <button
                    class="flex items-center justify-center w-full h-8 rounded-lg text-fg-tertiary hover:text-fg-secondary
                           hover:bg-surface-hover/30 btn-press"
                    on:click=move |_| set_collapsed.update(|v| *v = !*v)
                    title=move || if collapsed.get() { "Expand sidebar" } else { "Collapse sidebar" }
                    aria-label=move || if collapsed.get() { "Expand sidebar" } else { "Collapse sidebar" }
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
    divider_before: bool,
}

/// Navigation set for the sidebar. With the `studio_ui_beta` flag on,
/// Studio and Media replace Assets, Layout, and Displays (Spec 65 §5.1);
/// with it off, the nav is unchanged from before the redesign.
fn nav_items(studio_ui: bool) -> Vec<NavItem> {
    let dashboard = NavItem {
        path: "/",
        label: "Dashboard",
        icon: LuLayoutDashboard,
        divider_before: false,
    };
    let effects = NavItem {
        path: "/effects",
        label: "Effects",
        icon: LuLayers,
        divider_before: false,
    };
    let devices = NavItem {
        path: "/devices",
        label: "Devices",
        icon: LuCpu,
        divider_before: false,
    };
    let settings = NavItem {
        path: "/settings",
        label: "Settings",
        icon: LuSettings,
        divider_before: true,
    };

    if studio_ui {
        vec![
            dashboard,
            effects,
            NavItem {
                path: "/studio",
                label: "Studio",
                icon: LuLayoutTemplate,
                divider_before: false,
            },
            NavItem {
                path: "/media",
                label: "Media",
                icon: LuFolder,
                divider_before: false,
            },
            devices,
            settings,
        ]
    } else {
        vec![
            dashboard,
            effects,
            NavItem {
                path: "/assets",
                label: "Assets",
                icon: LuFolder,
                divider_before: false,
            },
            NavItem {
                path: "/layout",
                label: "Layout",
                icon: LuLayoutTemplate,
                divider_before: false,
            },
            devices,
            NavItem {
                path: "/displays",
                label: "Displays",
                icon: LuMonitor,
                divider_before: false,
            },
            settings,
        ]
    }
}

// ── Sidebar Scene Chip ─────────────────────────────────────────────────────

/// Compact scene indicator above the sidebar footer: active scene name
/// (or "Default"), a lock glyph for snapshot-locked scenes, and a
/// popover scene switcher on click. Rendered only when the user has
/// more than one scene to switch between. No optimistic flip — the
/// label changes when the shared scene resource confirms the switch.
#[component]
fn SidebarSceneChip() -> impl IntoView {
    let scenes_ctx = expect_context::<crate::zones::ScenesContext>();
    let (open, set_open) = signal(false);

    let show = Memo::new(move |_| scenes_ctx.has_multiple());
    let label = Memo::new(move |_| {
        scenes_ctx
            .active
            .with(|active| active_scene_label(active.as_ref()))
    });
    let locked = Memo::new(move |_| {
        scenes_ctx
            .active
            .with(|active| active_scene_locked(active.as_ref()))
    });

    view! {
        <Show when=move || show.get()>
            <div class="shrink-0 border-t border-edge-subtle px-2 pt-2 relative sidebar-scene-chip">
                <button
                    type="button"
                    class="flex w-full items-center gap-2 rounded-lg border border-edge-subtle/60 \
                           bg-surface-overlay/40 px-2.5 py-1.5 text-left transition-colors \
                           hover:border-accent-muted hover:bg-surface-overlay/70 \
                           focus-visible:outline-none focus-visible:ring-1 \
                           focus-visible:ring-accent/50 btn-press"
                    title="Switch scene"
                    aria-haspopup="menu"
                    aria-expanded=move || open.get().to_string()
                    on:click=move |_| set_open.update(|value| *value = !*value)
                >
                    <span class="text-[9px] font-mono uppercase tracking-[0.15em] text-fg-tertiary shrink-0">
                        "Scene"
                    </span>
                    <span class="flex-1 min-w-0 truncate text-[11px] font-medium text-fg-primary">
                        {move || label.get()}
                    </span>
                    {move || locked.get().then(|| view! {
                        <span
                            class="flex shrink-0 text-electric-yellow/70"
                            title="Snapshot-locked scene"
                        >
                            <Icon icon=LuLock width="11px" height="11px" />
                        </span>
                    })}
                    <span class="flex shrink-0 text-fg-tertiary">
                        <Icon icon=LuChevronUp width="12px" height="12px" />
                    </span>
                </button>
                <SceneSwitcherMenu
                    anchor_class="sidebar-scene-chip"
                    is_open=open
                    set_open=set_open
                    placement="left-2 right-2 bottom-full mb-1"
                />
            </div>
        </Show>
    }
}

// ── Sidebar Audio Toggle ───────────────────────────────────────────────────

/// Tiny icon button in the Now Playing metadata row.
///
/// - Audio on: waveform icon, glows coral (purple pulse on beat). Click to disable.
/// - Audio off + audio-reactive effect: muted icon, dim. Click to enable.
/// - Audio off + non-reactive: hidden entirely.
#[component]
fn SidebarAudioToggle() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let fx = expect_context::<EffectsContext>();
    let config_ctx = expect_context::<ConfigContext>();

    let active_is_audio_reactive = Memo::new(move |_| {
        let Some(active_id) = fx.active_effect_id.get() else {
            return false;
        };
        fx.effects_index.with(|effects| {
            effects
                .iter()
                .any(|entry| entry.effect.id == active_id && entry.effect.audio_reactive)
        })
    });

    let toggle_audio = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        let new_state = !config_ctx.audio_enabled.get();
        config_ctx.set_config.update(|config| {
            if let Some(current) = config {
                current.audio.enabled = new_state;
            }
        });
        spawn_api_call("Failed to toggle audio", async move {
            let result =
                api::set_config_value("audio.enabled", &serde_json::json!(new_state)).await;
            if result.is_err() {
                config_ctx.refresh.run(());
            }
            result
        });
    };

    // Quantized glow tier for the audio-on state. The memo dedupes the
    // ~10 Hz audio stream down to actual tier changes, so the static
    // button DOM below only re-patches its style string when the tier
    // flips — never per audio tick.
    let audio_glow = Memo::new(move |_| {
        let al = ws.audio_level.get();
        if al.beat {
            ("rgb(225, 53, 255)", "0 0 6px rgba(225, 53, 255, 0.5)")
        } else if al.level > 0.01 {
            ("rgba(255, 106, 193, 0.7)", "none")
        } else {
            ("rgba(255, 106, 193, 0.4)", "none")
        }
    });

    view! {
        {move || {
            let audio_on = config_ctx.audio_enabled.get();
            let is_reactive = active_is_audio_reactive.get();

            if audio_on {
                Some(view! {
                    <button
                        class="shrink-0 p-1 rounded"
                        style=move || {
                            let (color, shadow) = audio_glow.get();
                            format!("color: {color}; filter: drop-shadow({shadow})")
                        }
                        title="Disable audio"
                        on:click=toggle_audio
                    >
                        <Icon icon=LuAudioLines width="13px" height="13px" />
                    </button>
                }.into_any())
            } else if is_reactive {
                Some(view! {
                    <button
                        class="shrink-0 p-1 rounded text-fg-tertiary/30 hover:text-coral/70 transition-colors"
                        title="Enable audio"
                        on:click=toggle_audio
                    >
                        <Icon icon=LuVolumeX width="13px" height="13px" />
                    </button>
                }.into_any())
            } else {
                None
            }
        }}
    }
}
