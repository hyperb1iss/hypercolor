//! Fixed navigation sidebar — nav + now-playing section with player controls.
//! The Now Playing panel renders a live canvas thumbnail of the running effect
//! and extracts a color palette for ambient glow styling.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::components::A;
use leptos_router::hooks::use_location;
use leptos_use::use_throttle_fn_with_arg;

use crate::api;
use crate::app::{EffectsContext, WsContext};
use crate::components::canvas_preview::CanvasPreview;
use crate::icons::*;
use crate::style_utils::category_accent_rgb;
use crate::toasts;
use crate::ws::ConnectionState;

/// Sidebar collapsed state, shared via context so the shell can react.
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct SidebarState {
    pub collapsed: ReadSignal<bool>,
    pub set_collapsed: WriteSignal<bool>,
}

// ── Live Palette Extraction ────────────────────────────────────────────────

/// Two-color palette extracted from live canvas frame pixels.
#[derive(Clone, Copy)]
struct LivePalette {
    primary: (f64, f64, f64),
    secondary: (f64, f64, f64),
}

/// Extract the 1-2 most dominant vibrant colors from RGBA pixel data.
///
/// Samples ~200 pixels, groups by hue sector (12 sectors of 30 degrees each),
/// skips dark/desaturated pixels, and returns averaged RGB for the top sectors.
fn extract_palette(frame: &crate::ws::CanvasFrame) -> Option<LivePalette> {
    let pixel_count = frame.pixel_count();
    if pixel_count < 4 {
        return None;
    }

    let step = (pixel_count / 200).max(1);

    // 12 hue sectors (30 deg each): (r_sum, g_sum, b_sum, count)
    let mut sectors = [(0.0_f64, 0.0_f64, 0.0_f64, 0_u32); 12];

    for i in (0..pixel_count).step_by(step) {
        let Some([r, g, b, _]) = frame.rgba_at(i) else {
            continue;
        };
        let r = f64::from(r);
        let g = f64::from(g);
        let b = f64::from(b);

        let rf = r / 255.0;
        let gf = g / 255.0;
        let bf = b / 255.0;

        let max = rf.max(gf).max(bf);
        let min = rf.min(gf).min(bf);
        let chroma = max - min;
        let lightness = (max + min) / 2.0;

        if chroma < 0.15 || lightness < 0.08 {
            continue;
        }

        let hue = if (max - rf).abs() < f64::EPSILON {
            60.0 * (((gf - bf) / chroma) % 6.0)
        } else if (max - gf).abs() < f64::EPSILON {
            60.0 * (((bf - rf) / chroma) + 2.0)
        } else {
            60.0 * (((rf - gf) / chroma) + 4.0)
        };
        let hue = if hue < 0.0 { hue + 360.0 } else { hue };

        let sector = ((hue / 30.0) as usize).min(11);
        sectors[sector].0 += r;
        sectors[sector].1 += g;
        sectors[sector].2 += b;
        sectors[sector].3 += 1;
    }

    let mut ranked: Vec<(usize, u32)> = sectors
        .iter()
        .enumerate()
        .filter(|(_, s)| s.3 >= 3)
        .map(|(i, s)| (i, s.3))
        .collect();

    if ranked.is_empty() {
        return None;
    }

    ranked.sort_by(|a, b| b.1.cmp(&a.1));

    let avg = |idx: usize| -> (f64, f64, f64) {
        let s = &sectors[idx];
        let n = f64::from(s.3);
        (s.0 / n, s.1 / n, s.2 / n)
    };

    let primary = avg(ranked[0].0);
    let secondary = if ranked.len() > 1 {
        avg(ranked[1].0)
    } else {
        primary
    };

    Some(LivePalette { primary, secondary })
}

fn lerp_rgb(a: (f64, f64, f64), b: (f64, f64, f64), t: f64) -> (f64, f64, f64) {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

fn rgb_string(c: (f64, f64, f64)) -> String {
    format!("{:.0}, {:.0}, {:.0}", c.0, c.1, c.2)
}

// ── Sidebar Component ──────────────────────────────────────────────────────

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
    let uses_sidebar_preview = Signal::derive(move || {
        let path = location.pathname.get();
        !(path == "/" || path.starts_with("/effects") || path.starts_with("/layout"))
    });

    let nav_items = vec![
        NavItem {
            path: "/",
            label: "Dashboard",
            icon: LuLayoutDashboard,
            divider_before: false,
        },
        NavItem {
            path: "/effects",
            label: "Effects",
            icon: LuLayers,
            divider_before: false,
        },
        NavItem {
            path: "/layout",
            label: "Layout",
            icon: LuLayoutTemplate,
            divider_before: false,
        },
        NavItem {
            path: "/devices",
            label: "Devices",
            icon: LuCpu,
            divider_before: false,
        },
        NavItem {
            path: "/settings",
            label: "Settings",
            icon: LuSettings,
            divider_before: true,
        },
    ];

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
    let (live_palette, set_live_palette) = signal(None::<LivePalette>);
    let (last_palette_time, set_last_palette_time) = signal(0.0_f64);
    let global_brightness_resource = LocalResource::new(api::fetch_global_brightness);
    let (global_brightness, set_global_brightness) = signal(100_u8);

    Effect::new(move |_| {
        if let Some(Ok(brightness)) = global_brightness_resource.get() {
            set_global_brightness.set(brightness);
        }
    });

    let push_global_brightness = use_throttle_fn_with_arg(
        move |brightness: u8| {
            leptos::task::spawn_local(async move {
                if let Err(error) = api::set_global_brightness(brightness).await {
                    toasts::toast_error(&format!("Global brightness update failed: {error}"));
                }
            });
        },
        50.0,
    );

    if let Some(ws) = ws {
        // Palette extraction — throttled ~2x/sec for ambient styling
        Effect::new(move |_| {
            if uses_sidebar_preview.get() {
                return;
            }

            let Some(frame) = ws.canvas_frame.get() else {
                return;
            };

            let now = js_sys::Date::now();
            if now - last_palette_time.get_untracked() < 500.0 {
                return;
            }
            set_last_palette_time.set(now);

            if let Some(new_palette) = extract_palette(&frame) {
                let smoothed = match live_palette.get_untracked() {
                    Some(old) => LivePalette {
                        primary: lerp_rgb(old.primary, new_palette.primary, 0.3),
                        secondary: lerp_rgb(old.secondary, new_palette.secondary, 0.3),
                    },
                    None => new_palette,
                };
                set_live_palette.set(Some(smoothed));
            }
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
            // Logo — click to cycle through modes, persisted to localStorage
            {
                let logo_mode_count = 9_usize;
                let default_mode = 4_usize; // Prism
                let initial_mode = web_sys::window()
                    .and_then(|w| w.local_storage().ok().flatten())
                    .and_then(|s| s.get_item("hc-logo-mode").ok().flatten())
                    .and_then(|v| v.parse::<usize>().ok())
                    .filter(|m| *m < logo_mode_count)
                    .unwrap_or(default_mode);
                let (logo_mode, set_logo_mode) = signal(initial_mode);
                let cycle_logo = move |_| set_logo_mode.update(|m| {
                    *m = (*m + 1) % logo_mode_count;
                    if let Some(storage) = web_sys::window()
                        .and_then(|w| w.local_storage().ok().flatten())
                    {
                        let _ = storage.set_item("hc-logo-mode", &m.to_string());
                    }
                });

                let mode_names = [
                    "circuit", "silk", "bloom", "whisper", "prism",
                    "script", "editorial", "neon", "glitch",
                ];

                view! {
                    <div
                        class="w-full border-b border-edge-subtle transition-all duration-300"
                        class:h-14=move || collapsed.get()
                        class:h-32=move || !collapsed.get()
                    >
                        // Collapsed state: gradient mark
                        <div
                            class="items-center justify-center h-full logo-container"
                            style:display=move || if collapsed.get() { "flex" } else { "none" }
                            on:click=cycle_logo
                            title="Click to change logo style"
                        >
                            <div class="w-8 h-8 rounded-lg logo-mark flex items-center justify-center animate-breathe" style="--glow-rgb: 225, 53, 255">
                                <span class="text-xs font-bold text-white">"H"</span>
                            </div>
                        </div>

                        // Expanded state: cycling logo modes
                        <div
                            class="flex-col items-center justify-center h-full px-3 overflow-hidden logo-container"
                            style:display=move || if collapsed.get() { "none" } else { "flex" }
                            on:click=cycle_logo
                            title="Click to change logo style"
                        >
                            // Ambient background glow — changes per mode
                            <div class=move || {
                                let bg = match logo_mode.get() {
                                    0 => "logo-bg-circuit",
                                    1 => "logo-bg-silk",
                                    2 => "logo-bg-bloom",
                                    3 => "logo-bg-whisper",
                                    4 => "logo-bg-prism",
                                    5 => "logo-bg-script",
                                    6 => "logo-bg-editorial",
                                    7 => "logo-bg-neon",
                                    _ => "logo-bg-glitch",
                                };
                                format!("logo-bg {bg}")
                            } />

                            {move || {
                                let mode = logo_mode.get();
                                match mode {
                                    // 0: Circuit — PCB silkscreen, trace separator, technical precision
                                    0 => view! {
                                        <div class="logo-circuit flex flex-col items-center leading-none gap-1.5">
                                            <span class="logo-gradient-text text-[20px] font-semibold tracking-[0.45em]">"HYPER"</span>
                                            <div class="logo-circuit-trace" />
                                            <span class="logo-gradient-text text-[20px] font-semibold tracking-[0.45em]">"COLOR"</span>
                                        </div>
                                    }.into_any(),

                                    // 1: Silk — elegant weight contrast, thin over bold
                                    1 => view! {
                                        <div class="logo-silk flex flex-col items-center leading-none">
                                            <span class="logo-gradient-text text-[26px] font-normal tracking-[0.25em]">"Hyper"</span>
                                            <span class="logo-gradient-text text-[28px] font-bold tracking-[0.15em] -mt-0.5">"color"</span>
                                        </div>
                                    }.into_any(),

                                    // 2: Bloom — sparkle divider, coral-pink breathe
                                    2 => view! {
                                        <div class="logo-bloom flex flex-col items-center leading-none gap-1">
                                            <span class="logo-gradient-text text-[24px] font-semibold tracking-[0.2em]">"HYPER"</span>
                                            <span class="logo-sparkle text-[14px] leading-none">"✦"</span>
                                            <span class="logo-gradient-text text-[24px] font-semibold tracking-[0.2em]">"COLOR"</span>
                                        </div>
                                    }.into_any(),

                                    // 3: Whisper — lowercase, ultra-wide, decorative lines
                                    3 => view! {
                                        <div class="logo-whisper flex flex-col items-center leading-none gap-2.5">
                                            <div class="logo-whisper-line" />
                                            <span class="logo-gradient-text text-[14px] font-normal tracking-[0.45em]">"hypercolor"</span>
                                            <div class="logo-whisper-line" />
                                        </div>
                                    }.into_any(),

                                    // 4: Prism — dramatic size contrast
                                    4 => view! {
                                        <div class="logo-prism flex flex-col items-center leading-none">
                                            <span class="logo-gradient-text text-[14px] font-normal tracking-[0.5em]">"HYPER"</span>
                                            <span class="logo-gradient-text text-[38px] font-black tracking-[0.08em] -mt-1">"COLOR"</span>
                                        </div>
                                    }.into_any(),

                                    // 5: Script — Dancing Script cursive, full femme
                                    5 => view! {
                                        <div class="logo-script flex flex-col items-center leading-none">
                                            <span class="logo-gradient-text text-[44px] font-bold tracking-[0.02em]">"Hyper"</span>
                                            <span class="logo-gradient-text text-[34px] font-semibold tracking-[0.05em] -mt-3">"color"</span>
                                        </div>
                                    }.into_any(),

                                    // 6: Editorial — Playfair Display, ruled lines
                                    6 => view! {
                                        <div class="logo-editorial flex flex-col items-center leading-none gap-1">
                                            <div class="logo-editorial-rule" />
                                            <span class="logo-gradient-text text-[38px] font-bold italic tracking-[0.04em]">"Hyper"</span>
                                            <span class="logo-gradient-text text-[18px] font-normal tracking-[0.45em] -mt-1">"COLOR"</span>
                                            <div class="logo-editorial-rule" />
                                        </div>
                                    }.into_any(),

                                    // 7: Neon Mono — split-color hacker femme + cursor
                                    7 => view! {
                                        <div class="logo-neon flex flex-col items-center leading-none">
                                            <span class="logo-neon-hyper text-[28px] font-semibold tracking-[0.12em]">"hyper"</span>
                                            <div class="flex items-center mt-0.5">
                                                <span class="logo-neon-color text-[28px] font-semibold tracking-[0.12em]">"color"</span>
                                                <span class="logo-neon-cursor" />
                                            </div>
                                        </div>
                                    }.into_any(),

                                    // 8: Glitch — chromatic aberration, chaotic weight/offset
                                    _ => view! {
                                        <div class="logo-glitch flex flex-col items-start leading-none">
                                            <span class="logo-gradient-text text-[32px] font-black tracking-[0.06em]">"HYPER"</span>
                                            <span class="logo-gradient-text text-[18px] font-light tracking-[0.5em] -mt-1 ml-4">"COLOR"</span>
                                        </div>
                                    }.into_any(),
                                }
                            }}

                            // Mode name hint — absolutely positioned, doesn't affect centering
                            <div class="logo-mode-label text-fg-tertiary">
                                {move || mode_names[logo_mode.get()]}
                            </div>
                        </div>
                    </div>
                }
            }

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
                    };

                    if item.divider_before {
                        view! {
                            <div class="h-px bg-border-subtle/30 my-2 mx-1" />
                            {link}
                        }.into_any()
                    } else {
                        link.into_any()
                    }
                }).collect_view()}
            </div>

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

                // Derived signals for palette RGB — read inside style: closures, not here
                let primary_rgb = move || {
                    let cat = fx.active_effect_category.get();
                    if uses_sidebar_preview.get() {
                        category_accent_rgb(&cat).to_string()
                    } else {
                        live_palette.get().map_or_else(
                            || category_accent_rgb(&cat).to_string(),
                            |p| rgb_string(p.primary),
                        )
                    }
                };
                let secondary_rgb = move || {
                    let cat = fx.active_effect_category.get();
                    if uses_sidebar_preview.get() {
                        category_accent_rgb(&cat).to_string()
                    } else {
                        live_palette.get().map_or_else(
                            || category_accent_rgb(&cat).to_string(),
                            |p| rgb_string(p.secondary),
                        )
                    }
                };

                Some(view! {
                    <div
                        class="border-t border-edge-subtle py-3 space-y-3 animate-fade-in"
                        style:box-shadow=move || {
                            let p = primary_rgb();
                            format!("inset 3px 0 0 rgb({p}), inset 4px 0 12px rgba({p}, 0.15)")
                        }
                    >
                        // Now Playing label
                        <div class="px-4 text-[9px] font-mono uppercase tracking-[0.15em] text-fg-tertiary/60">
                            "Now Playing"
                        </div>

                        // Live canvas thumbnail — only on pages without their own preview
                        {move || {
                            uses_sidebar_preview.get().then(|| view! {
                                <div class="px-3 animate-fade-in">
                                    <div
                                        class="relative rounded-lg overflow-hidden bg-black/40"
                                        style:box-shadow=move || {
                                            let p = primary_rgb();
                                            let s = secondary_rgb();
                                            format!("0 4px 20px rgba({p}, 0.25), 0 0 40px rgba({s}, 0.08)")
                                        }
                                    >
                                        <CanvasPreview
                                            frame=canvas_frame
                                            fps=preview_fps
                                            fps_target=preview_target_fps
                                            max_width="100%".to_string()
                                            aspect_ratio="320 / 200".to_string()
                                        />
                                    </div>
                                </div>
                            })
                        }}

                        // Effect name + category + audio toggle
                        <div class="px-4 flex items-center gap-2.5 min-w-0">
                            <div
                                class="w-2 h-2 rounded-full dot-alive shrink-0"
                                style:background=move || format!("rgb({})", primary_rgb())
                                style:box-shadow=move || format!("0 0 8px rgba({}, 0.7)", primary_rgb())
                            />
                            <div class="min-w-0 flex-1">
                                <div class="text-[11px] font-medium text-fg-primary truncate leading-tight">
                                    {move || fx.active_effect_name.get().unwrap_or_default()}
                                </div>
                                <div class="text-[10px] text-fg-tertiary capitalize mt-0.5">
                                    {move || fx.active_effect_category.get()}
                                </div>
                            </div>
                            <SidebarAudioToggle />
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
                            <button
                                class="p-2 rounded-lg text-error-red/40 hover:text-error-red hover:bg-error-red/[0.06] player-btn"
                                title="Stop effect"
                                aria-label="Stop effect"
                                on:click=move |_| fx.stop_effect()
                            >
                                <Icon icon=LuSquare width="16px" height="16px" />
                            </button>
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
                                class="min-w-0 flex-1 h-1 rounded-full appearance-none cursor-pointer"
                                style="accent-color: rgb(225, 53, 255); background: rgba(139, 133, 160, 0.15)"
                                prop:value=move || global_brightness.get().to_string()
                                on:input=move |ev| {
                                    let value = event_target_value(&ev);
                                    if let Ok(brightness) = value.parse::<u8>() {
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

            // Bottom bar — status, theme, search, collapse
            {
                let theme_ctx = use_context::<crate::components::shell::ThemeContext>();
                let palette_ctx = use_context::<crate::components::shell::PaletteContext>();
                let ws_ctx = use_context::<WsContext>();

                view! {
                    <div class="border-t border-edge-subtle px-2 py-2 space-y-1">
                        // Status + actions row (expanded only)
                        <div
                            class="flex items-center justify-between px-1"
                            style:display=move || if collapsed.get() { "none" } else { "flex" }
                        >
                            // Connection status
                            <div class="flex items-center gap-1.5 text-[10px] font-mono text-fg-tertiary">
                                {move || {
                                    ws_ctx.map(|ws| {
                                        view! {
                                            <div
                                                class="w-[5px] h-[5px] rounded-full"
                                                style=move || {
                                                    match ws.connection_state.get() {
                                                        ConnectionState::Connected => "background: rgb(80, 250, 123); box-shadow: 0 0 6px rgba(80, 250, 123, 0.5)",
                                                        ConnectionState::Error => "background: rgb(255, 99, 99); box-shadow: 0 0 6px rgba(255, 99, 99, 0.5)",
                                                        ConnectionState::Connecting => "background: rgb(241, 250, 140)",
                                                        ConnectionState::Disconnected => "background: rgb(82, 82, 91)",
                                                    }
                                                }
                                            />
                                            <span>{move || ws.connection_state.get().to_string()}</span>
                                            <span class="text-fg-tertiary/50 ml-1">
                                                {move || format!("preview {:.0}/{}", ws.preview_fps.get(), ws.preview_target_fps.get())}
                                            </span>
                                            <span class="text-fg-tertiary/50">
                                                {move || {
                                                    ws.metrics
                                                        .get()
                                                        .map(|metrics| format!("engine {:.0}/{}", metrics.fps.actual, metrics.fps.target))
                                                        .unwrap_or_else(|| "engine ...".to_string())
                                                }}
                                            </span>
                                        }
                                    })
                                }}
                            </div>

                            // Right side: theme + search
                            <div class="flex items-center gap-0.5">
                                // Search (command palette)
                                {move || {
                                    palette_ctx.map(|ctx| {
                                        let open = ctx.open;
                                        view! {
                                            <button
                                                class="p-1.5 rounded-md text-fg-tertiary hover:text-fg-primary hover:bg-surface-hover/40 btn-press"
                                                title="Search effects (⌘K)"
                                                aria-label="Search effects"
                                                on:click=move |_| open.run(())
                                            >
                                                <Icon icon=LuSearch width="14px" height="14px" />
                                            </button>
                                        }
                                    })
                                }}

                                // Theme toggle
                                {move || {
                                    theme_ctx.map(|ctx| {
                                        let toggle = ctx.toggle;
                                        let is_dark = ctx.is_dark;
                                        view! {
                                            <button
                                                class="p-1.5 rounded-md text-fg-tertiary hover:text-fg-primary hover:bg-surface-hover/40 btn-press"
                                                title=move || if is_dark.get() { "Light mode" } else { "Dark mode" }
                                                aria-label=move || if is_dark.get() { "Light mode" } else { "Dark mode" }
                                                on:click=move |_| toggle.run(())
                                            >
                                                {move || if is_dark.get() {
                                                    view! { <Icon icon=LuSun width="14px" height="14px" style="color: inherit" /> }.into_any()
                                                } else {
                                                    view! { <Icon icon=LuMoon width="14px" height="14px" style="color: inherit" /> }.into_any()
                                                }}
                                            </button>
                                        }
                                    })
                                }}
                            </div>
                        </div>

                        // Collapse toggle
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
                }
            }
        </nav>
    }
}

struct NavItem {
    path: &'static str,
    label: &'static str,
    icon: icondata_core::Icon,
    divider_before: bool,
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
        let new_state = !ws.audio_enabled.get();
        ws.set_audio_enabled.set(new_state);
        leptos::task::spawn_local(async move {
            if let Err(error) =
                api::set_config_value("audio.enabled", &serde_json::json!(new_state)).await
            {
                toasts::toast_error(&format!("Failed to toggle audio: {error}"));
            }
        });
    };

    view! {
        {move || {
            let audio_on = ws.audio_enabled.get();
            let is_reactive = active_is_audio_reactive.get();

            if audio_on {
                let al = ws.audio_level.get();
                let (color, shadow) = if al.beat {
                    ("rgb(225, 53, 255)", "0 0 6px rgba(225, 53, 255, 0.5)")
                } else if al.level > 0.01 {
                    ("rgba(255, 106, 193, 0.7)", "none")
                } else {
                    ("rgba(255, 106, 193, 0.4)", "none")
                };
                Some(view! {
                    <button
                        class="shrink-0 p-1 rounded transition-all duration-75"
                        style=format!("color: {color}; filter: drop-shadow({shadow})")
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
