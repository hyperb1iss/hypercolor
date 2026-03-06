//! Fixed navigation sidebar — nav + now-playing section with player controls.
//! The Now Playing panel renders a live canvas thumbnail of the running effect
//! and extracts a color palette for ambient glow styling.

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::components::A;
use leptos_router::hooks::use_location;
use wasm_bindgen::Clamped;
use wasm_bindgen::JsCast;

use crate::app::{EffectsContext, WsContext};
use crate::icons::*;

/// Sidebar collapsed state, shared via context so the shell can react.
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct SidebarState {
    pub collapsed: ReadSignal<bool>,
    pub set_collapsed: WriteSignal<bool>,
}

/// Category -> accent RGB string for inline styles (fallback when no canvas data).
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
fn extract_palette(pixels: &[u8]) -> Option<LivePalette> {
    if pixels.len() < 16 {
        return None;
    }

    let pixel_count = pixels.len() / 4;
    let step = (pixel_count / 200).max(1);

    // 12 hue sectors (30 deg each): (r_sum, g_sum, b_sum, count)
    let mut sectors = [(0.0_f64, 0.0_f64, 0.0_f64, 0_u32); 12];

    for i in (0..pixel_count).step_by(step) {
        let off = i * 4;
        let r = f64::from(pixels[off]);
        let g = f64::from(pixels[off + 1]);
        let b = f64::from(pixels[off + 2]);

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

    // ── Live canvas + palette from WebSocket frames ────────────────────
    let ws = use_context::<WsContext>();
    let np_canvas_ref = NodeRef::<leptos::html::Canvas>::new();
    let (live_palette, set_live_palette) = signal(None::<LivePalette>);
    let (last_palette_time, set_last_palette_time) = signal(0.0_f64);

    if let Some(ws) = ws {
        // Palette extraction — throttled ~2x/sec for ambient styling
        Effect::new(move |_| {
            let Some(frame) = ws.canvas_frame.get() else {
                return;
            };

            let now = js_sys::Date::now();
            if now - last_palette_time.get_untracked() < 500.0 {
                return;
            }
            set_last_palette_time.set(now);

            if let Some(new_palette) = extract_palette(&frame.pixels) {
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

        // Canvas painting — every frame, for smooth live thumbnail
        Effect::new(move |_| {
            let Some(frame) = ws.canvas_frame.get() else {
                return;
            };
            let Some(canvas) = np_canvas_ref.get() else {
                return;
            };

            if canvas.width() != frame.width || canvas.height() != frame.height {
                canvas.set_width(frame.width);
                canvas.set_height(frame.height);
            }

            let ctx = canvas
                .get_context("2d")
                .ok()
                .flatten()
                .and_then(|ctx| ctx.dyn_into::<web_sys::CanvasRenderingContext2d>().ok());

            let Some(ctx) = ctx else { return };

            if let Ok(data) = web_sys::ImageData::new_with_u8_clamped_array_and_sh(
                Clamped(&frame.pixels),
                frame.width,
                frame.height,
            ) {
                let _ = ctx.put_image_data(&data, 0.0, 0.0);
            }
        });
    }

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

            // Now Playing — live thumbnail + palette-styled panel
            {move || {
                if !has_active.get() || collapsed.get() {
                    return None;
                }
                let name = fx.active_effect_name.get().unwrap_or_default();
                let cat = fx.active_effect_category.get();
                let fallback_rgb = category_accent_rgb(&cat).to_string();

                // Live palette colors with category fallback
                let palette = live_palette.get();
                let primary = palette.map_or_else(
                    || fallback_rgb.clone(),
                    |p| rgb_string(p.primary),
                );
                let secondary = palette.map_or_else(
                    || fallback_rgb.clone(),
                    |p| rgb_string(p.secondary),
                );

                // Panel: left edge glow from primary color
                let panel_style = format!(
                    "box-shadow: inset 3px 0 0 rgb({primary}), inset 4px 0 12px rgba({primary}, 0.15)"
                );

                // Canvas thumbnail glow — the thumbnail radiates the effect's colors
                let thumb_glow = format!(
                    "box-shadow: 0 4px 20px rgba({primary}, 0.25), 0 0 40px rgba({secondary}, 0.08)"
                );

                // Status dot
                let dot_style = format!(
                    "background: rgb({primary}); box-shadow: 0 0 8px rgba({primary}, 0.7)"
                );

                Some(view! {
                    <div
                        class="border-t border-edge-subtle py-3 space-y-3 animate-fade-in"
                        style=panel_style
                    >
                        // Now Playing label
                        <div class="px-4 text-[9px] font-mono uppercase tracking-[0.15em] text-fg-tertiary/60">
                            "Now Playing"
                        </div>

                        // Live canvas thumbnail — only on pages without their own preview
                        {move || {
                            let path = location.pathname.get();
                            let has_preview = path == "/" || path.starts_with("/effects");
                            (!has_preview).then(|| view! {
                                <div class="px-3 animate-fade-in">
                                    <div
                                        class="relative rounded-lg overflow-hidden bg-black/40"
                                        style=thumb_glow.clone()
                                    >
                                        <canvas
                                            node_ref=np_canvas_ref
                                            class="w-full h-auto block"
                                            style="image-rendering: pixelated;"
                                        />
                                    </div>
                                </div>
                            })
                        }}

                        // Effect name + category
                        <div class="px-4 flex items-center gap-2.5 min-w-0">
                            <div class="w-2 h-2 rounded-full dot-alive shrink-0" style=dot_style />
                            <div class="min-w-0 flex-1">
                                <div class="text-[13px] font-medium text-fg-primary truncate leading-tight">{name}</div>
                                <div class="text-[10px] text-fg-tertiary capitalize mt-0.5">{cat}</div>
                            </div>
                        </div>

                        // Player controls
                        <div class="px-4 flex items-center justify-between">
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
