//! Capture page — the screen capture control room.
//!
//! Live screen preview wrapped in a real ambilight glow ring driven by the
//! `screen_zones` WebSocket channel (the smoothed, color-tuned zone colors
//! that screen-reactive effects consume), with the full capture pipeline
//! tunable live from the right rail: color tuning, smoothing, grid shape,
//! letterbox detection, and portal source selection.

use leptos::html::Canvas;
use leptos::prelude::*;
use wasm_bindgen::{Clamped, JsCast};

use crate::api;
use crate::app::WsContext;
use crate::components::canvas_preview::CanvasPreview;
use crate::components::page_header::{PageAccent, PageHeader};
use crate::components::settings_controls::{
    SectionHeader, SectionReset, SettingDropdown, SettingSlider, SettingToggle,
};
use crate::config_state::{ConfigContext, apply_config_key};
use crate::icons::*;
use crate::ws::ScreenZonesFrame;
use hypercolor_types::config::HypercolorConfig;
use leptos_icons::Icon;

fn read_config<T>(
    config: Signal<Option<HypercolorConfig>>,
    read: impl Fn(&HypercolorConfig) -> T,
    fallback: T,
) -> T {
    config.with(|cfg| cfg.as_ref().map(&read)).unwrap_or(fallback)
}

#[component]
pub fn CapturePage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let config_ctx = expect_context::<ConfigContext>();
    let config = config_ctx.config;
    let set_config = config_ctx.set_config;

    // The zone stream subscription lives for the whole page visit; the
    // screen preview registers itself through CanvasPreview.
    Effect::new(move |_| {
        ws.set_screen_zones_consumers
            .update(|count| *count = count.saturating_add(1));
    });
    on_cleanup(move || {
        ws.set_screen_zones_consumers
            .update(|count| *count = count.saturating_sub(1));
    });

    let enabled = Signal::derive(move || {
        read_config(config.into(), |cfg| cfg.capture.enabled, false)
    });

    // Optimistic config writes, mirroring the Settings page contract.
    let on_change = Callback::new(move |(key, value): (String, serde_json::Value)| {
        set_config.update(|cfg| {
            if let Some(cfg) = cfg {
                apply_config_key(cfg, &key, &value);
            }
        });
        leptos::task::spawn_local(async move {
            if let Err(e) = api::set_config_value(&key, &value).await {
                leptos::logging::warn!("Capture config set failed: {e}");
                config_ctx.refresh.run(());
            }
        });
    });

    let on_reset = Callback::new(move |()| {
        leptos::task::spawn_local(async move {
            if let Err(e) = api::reset_config_key("capture").await {
                leptos::logging::warn!("Capture config reset failed: {e}");
            }
            config_ctx.refresh.run(());
        });
    });

    view! {
        <div class="flex flex-col h-full">
            <PageHeader
                icon=LuMonitorPlay
                title="Capture"
                tagline="Screen capture, tuned for light"
                accent=PageAccent::Cyan
            />
            <div class="flex flex-1 min-h-0 gap-4 p-4">
                <div class="flex flex-col flex-1 min-w-0 gap-3">
                    <CaptureStatusRow zones=ws.screen_zones_frame enabled=enabled />
                    <AmbilightStage
                        zones=ws.screen_zones_frame
                        enabled=enabled
                        on_change=on_change
                    />
                </div>
                <div class="w-[340px] shrink-0 overflow-y-auto pr-1">
                    <CaptureControls
                        config=config
                        enabled=enabled
                        on_change=on_change
                        on_reset=on_reset
                    />
                </div>
            </div>
        </div>
    }
}

// ── Status Row ─────────────────────────────────────────────────────────────

#[component]
fn CaptureStatusRow(
    zones: ReadSignal<Option<ScreenZonesFrame>>,
    #[prop(into)] enabled: Signal<bool>,
) -> impl IntoView {
    let live = Memo::new(move |_| {
        zones.with(|frame| {
            frame
                .as_ref()
                .is_some_and(|zones| zones.grid_cols > 0 && zones.grid_rows > 0)
        })
    });
    let source_label = Memo::new(move |_| {
        zones.with(|frame| {
            frame
                .as_ref()
                .filter(|zones| zones.source_width > 0)
                .map(|zones| format!("{}\u{00d7}{}", zones.source_width, zones.source_height))
        })
    });
    let grid_label = Memo::new(move |_| {
        zones.with(|frame| {
            frame
                .as_ref()
                .filter(|zones| zones.grid_cols > 0)
                .map(|zones| format!("{}\u{00d7}{} zones", zones.grid_cols, zones.grid_rows))
        })
    });
    let letterbox_label = Memo::new(move |_| {
        zones.with(|frame| {
            frame.as_ref().and_then(|zones| {
                let [top, bottom, left, right] = zones.letterbox;
                (u32::from(top) + u32::from(bottom) + u32::from(left) + u32::from(right) > 0)
                    .then(|| "letterboxed".to_owned())
            })
        })
    });

    view! {
        <div class="flex items-center gap-2 text-[11px]">
            {move || if live.get() {
                view! {
                    <span class="inline-flex items-center gap-1.5 rounded-full border border-edge-subtle bg-surface-overlay/60 px-2.5 py-1 text-fg-secondary">
                        <span class="h-1.5 w-1.5 rounded-full" style="background: rgb(80, 250, 123); box-shadow: 0 0 6px rgba(80, 250, 123, 0.8)"></span>
                        "Live"
                    </span>
                }.into_any()
            } else if enabled.get() {
                view! {
                    <span class="inline-flex items-center gap-1.5 rounded-full border border-edge-subtle bg-surface-overlay/60 px-2.5 py-1 text-fg-tertiary">
                        <span class="h-1.5 w-1.5 rounded-full bg-fg-tertiary/50"></span>
                        "Idle"
                    </span>
                }.into_any()
            } else {
                view! {
                    <span class="inline-flex items-center gap-1.5 rounded-full border border-edge-subtle bg-surface-overlay/60 px-2.5 py-1 text-fg-tertiary">
                        <span class="h-1.5 w-1.5 rounded-full bg-fg-tertiary/30"></span>
                        "Off"
                    </span>
                }.into_any()
            }}
            {move || source_label.get().map(|label| view! {
                <span class="rounded-full border border-edge-subtle bg-surface-overlay/40 px-2.5 py-1 text-fg-tertiary font-mono">{label}</span>
            })}
            {move || grid_label.get().map(|label| view! {
                <span class="rounded-full border border-edge-subtle bg-surface-overlay/40 px-2.5 py-1 text-fg-tertiary font-mono">{label}</span>
            })}
            {move || letterbox_label.get().map(|label| view! {
                <span class="rounded-full border border-edge-subtle bg-surface-overlay/40 px-2.5 py-1" style="color: rgba(241, 250, 140, 0.8)">{label}</span>
            })}
        </div>
    }
}

// ── Ambilight Stage ────────────────────────────────────────────────────────

/// The hero: the live screen preview floating on a glow ring painted from
/// the actual zone colors. The glow canvas is the zone grid itself,
/// CSS-scaled past the preview bounds and heavily blurred — exactly the
/// halo an ambilight rig would cast behind the display.
#[component]
fn AmbilightStage(
    zones: ReadSignal<Option<ScreenZonesFrame>>,
    #[prop(into)] enabled: Signal<bool>,
    on_change: Callback<(String, serde_json::Value)>,
) -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let glow_canvas = NodeRef::<Canvas>::new();
    let overlay_canvas = NodeRef::<Canvas>::new();
    let (show_glow, set_show_glow) = signal(true);
    let (show_zones, set_show_zones) = signal(false);

    let has_signal = Memo::new(move |_| {
        zones.with(|frame| {
            frame
                .as_ref()
                .is_some_and(|zones| zones.grid_cols > 0 && zones.grid_rows > 0)
        })
    });

    Effect::new(move |_| {
        let frame = zones.get();
        let glow_on = show_glow.get();
        if let Some(canvas) = glow_canvas.get() {
            paint_zone_canvas(&canvas, glow_on.then_some(frame.as_ref()).flatten());
        }
    });
    Effect::new(move |_| {
        let frame = zones.get();
        let zones_on = show_zones.get();
        if let Some(canvas) = overlay_canvas.get() {
            paint_zone_canvas(&canvas, zones_on.then_some(frame.as_ref()).flatten());
        }
    });

    let toggle_class = |active: bool| {
        if active {
            "inline-flex items-center gap-1.5 rounded-md border border-edge-subtle bg-surface-overlay px-2 py-1 text-[11px] text-fg-primary"
        } else {
            "inline-flex items-center gap-1.5 rounded-md border border-edge-subtle/60 bg-surface-overlay/40 px-2 py-1 text-[11px] text-fg-tertiary hover:text-fg-secondary"
        }
    };

    view! {
        <div class="relative flex-1 min-h-0 rounded-xl border border-edge-subtle bg-surface-base overflow-hidden">
            <div class="absolute inset-0 flex items-center justify-center p-12">
                <div class="relative w-full max-w-[860px]">
                    <canvas
                        node_ref=glow_canvas
                        class="absolute inset-0 h-full w-full transition-opacity duration-300"
                        style="transform: scale(1.18); filter: blur(42px) saturate(1.25); opacity: 0.9; image-rendering: auto;"
                        aria-hidden="true"
                    ></canvas>
                    <div class="relative rounded-lg overflow-hidden border border-edge-subtle/80" style="box-shadow: 0 8px 40px rgba(0, 0, 0, 0.45)">
                        <Show
                            when=move || has_signal.get()
                            fallback=move || view! {
                                <CaptureIdleState enabled=enabled on_change=on_change />
                            }
                        >
                            <CanvasPreview
                                frame=Signal::derive(move || ws.screen_canvas_frame.get())
                                fps=Signal::derive(|| 0.0_f32)
                                fps_target=Signal::derive(|| 0_u32)
                                register_main_preview_consumer=false
                                consumer_count=ws.set_screen_preview_consumers
                                image_rendering="auto".to_string()
                                aspect_ratio="16 / 9".to_string()
                                aria_label="Live screen capture preview".to_string()
                            />
                        </Show>
                        <canvas
                            node_ref=overlay_canvas
                            class="pointer-events-none absolute inset-0 h-full w-full transition-opacity duration-200"
                            style=move || format!(
                                "image-rendering: pixelated; opacity: {};",
                                if show_zones.get() { "0.85" } else { "0" }
                            )
                            aria-hidden="true"
                        ></canvas>
                    </div>
                </div>
            </div>
            <div class="absolute top-3 right-3 flex items-center gap-1.5">
                <button
                    type="button"
                    class=move || toggle_class(show_glow.get())
                    on:click=move |_| set_show_glow.update(|on| *on = !*on)
                >
                    <Icon icon=LuSun width="12px" height="12px" />
                    "Glow"
                </button>
                <button
                    type="button"
                    class=move || toggle_class(show_zones.get())
                    on:click=move |_| set_show_zones.update(|on| *on = !*on)
                >
                    <Icon icon=LuGrid2x2 width="12px" height="12px" />
                    "Zones"
                </button>
            </div>
        </div>
    }
}

/// Paint a zone grid frame onto a canvas at native grid resolution; CSS
/// handles all scaling. Clearing happens when `frame` is `None`.
fn paint_zone_canvas(canvas: &web_sys::HtmlCanvasElement, frame: Option<&ScreenZonesFrame>) {
    let Some(context) = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|ctx| ctx.dyn_into::<web_sys::CanvasRenderingContext2d>().ok())
    else {
        return;
    };

    let Some(frame) = frame.filter(|frame| frame.grid_cols > 0 && frame.grid_rows > 0) else {
        context.clear_rect(
            0.0,
            0.0,
            f64::from(canvas.width()),
            f64::from(canvas.height()),
        );
        return;
    };

    let cols = u32::from(frame.grid_cols);
    let rows = u32::from(frame.grid_rows);
    if canvas.width() != cols {
        canvas.set_width(cols);
    }
    if canvas.height() != rows {
        canvas.set_height(rows);
    }

    let mut rgba = vec![0_u8; (cols * rows * 4) as usize];
    for (zone, pixel) in frame.payload.chunks_exact(3).zip(rgba.chunks_exact_mut(4)) {
        pixel[0] = zone[0];
        pixel[1] = zone[1];
        pixel[2] = zone[2];
        pixel[3] = 255;
    }

    let Ok(image_data) =
        web_sys::ImageData::new_with_u8_clamped_array_and_sh(Clamped(&rgba), cols, rows)
    else {
        return;
    };
    let _ = context.put_image_data(&image_data, 0.0, 0.0);
}

// ── Idle / Off State ───────────────────────────────────────────────────────

#[component]
fn CaptureIdleState(
    #[prop(into)] enabled: Signal<bool>,
    on_change: Callback<(String, serde_json::Value)>,
) -> impl IntoView {
    view! {
        <div class="flex flex-col items-center justify-center gap-3 py-20 text-center" style="aspect-ratio: 16 / 9;">
            <Icon icon=LuMonitorPlay width="36px" height="36px" style="color: rgba(128, 255, 234, 0.35)" />
            {move || if enabled.get() {
                view! {
                    <div class="space-y-1">
                        <p class="text-sm text-fg-secondary">"Waiting for the capture stream"</p>
                        <p class="text-xs text-fg-tertiary">"If the portal picker is open, choose a screen to share."</p>
                    </div>
                }.into_any()
            } else {
                view! {
                    <div class="space-y-3">
                        <div class="space-y-1">
                            <p class="text-sm text-fg-secondary">"Screen capture is off"</p>
                            <p class="text-xs text-fg-tertiary">"Turn it on to drive ambient lighting from what's on screen."</p>
                        </div>
                        <button
                            type="button"
                            class="rounded-md border border-edge-subtle bg-surface-overlay px-3 py-1.5 text-xs text-fg-primary hover:bg-surface-overlay/80"
                            on:click=move |_| on_change.run(("capture.enabled".to_owned(), serde_json::Value::Bool(true)))
                        >
                            "Enable capture"
                        </button>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

// ── Controls Rail ──────────────────────────────────────────────────────────

#[component]
fn CaptureControls(
    #[prop(into)] config: Signal<Option<HypercolorConfig>>,
    #[prop(into)] enabled: Signal<bool>,
    on_change: Callback<(String, serde_json::Value)>,
    on_reset: Callback<()>,
) -> impl IntoView {
    let source = Signal::derive(move || {
        read_config(config, |cfg| cfg.capture.source.clone(), String::new())
    });
    let capture_fps = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.capture.capture_fps), 30.0)
    });
    let saturation = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.capture.saturation), 1.0)
    });
    let brightness = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.capture.brightness), 1.0)
    });
    let gamma =
        Signal::derive(move || read_config(config, |cfg| f64::from(cfg.capture.gamma), 1.0));
    let smoothing = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.capture.smoothing), 0.3)
    });
    let scene_cut = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.capture.scene_cut_threshold), 100.0)
    });
    let grid_cols = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.capture.grid_cols), 8.0)
    });
    let grid_rows = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.capture.grid_rows), 6.0)
    });
    let letterbox =
        Signal::derive(move || read_config(config, |cfg| cfg.capture.letterbox, true));
    let letterbox_threshold = Signal::derive(move || {
        read_config(config, |cfg| f64::from(cfg.capture.letterbox_threshold), 0.02)
    });

    let source_options = vec![
        ("auto".to_string(), "Auto".to_string()),
        ("pipewire".to_string(), "PipeWire".to_string()),
    ];

    let (picking, set_picking) = signal(false);
    let pick_source = move |_| {
        if picking.get_untracked() {
            return;
        }
        set_picking.set(true);
        leptos::task::spawn_local(async move {
            if let Err(e) = api::pick_capture_source().await {
                leptos::logging::warn!("Source pick failed: {e}");
            }
            set_picking.set(false);
        });
    };

    view! {
        <div class="space-y-0">
            <section class="pb-3 space-y-0">
                <SectionHeader title="Capture" icon=LuMonitorPlay />
                <SettingToggle
                    label="Enabled"
                    description="Capture the screen for ambient lighting"
                    key="capture.enabled"
                    value=enabled
                    on_change=on_change
                />
                <SettingSlider
                    label="Capture FPS"
                    description="Frames per second requested from the compositor"
                    key="capture.capture_fps"
                    value=capture_fps
                    on_change=on_change
                    min=1.0 max=60.0 step=1.0
                    decimals=0
                    integer=true
                />
                <SettingDropdown
                    label="Backend"
                    description="Screen capture backend"
                    key="capture.source"
                    value=source
                    options=Signal::stored(source_options)
                    on_change=on_change
                />
                <div class="flex items-center justify-between gap-4 py-3 border-b border-edge-subtle/40">
                    <div class="min-w-0">
                        <div class="text-[13px] text-fg-primary">"Source"</div>
                        <div class="text-xs text-fg-tertiary">"Re-open the portal picker to capture a different screen"</div>
                    </div>
                    <button
                        type="button"
                        class="inline-flex shrink-0 items-center gap-1.5 rounded-md border border-edge-subtle bg-surface-overlay px-2.5 py-1.5 text-xs text-fg-primary hover:bg-surface-overlay/80 disabled:opacity-50"
                        disabled=move || !enabled.get() || picking.get()
                        on:click=pick_source
                    >
                        <Icon icon=LuMonitor width="12px" height="12px" />
                        {move || if picking.get() { "Opening picker\u{2026}" } else { "Choose source" }}
                    </button>
                </div>
            </section>

            <section class="py-3 space-y-0">
                <SectionHeader title="Ambilight" icon=LuSun />
                <SettingSlider
                    label="Saturation"
                    description="Chroma boost on zone colors — punch past 1.0"
                    key="capture.saturation"
                    value=saturation
                    on_change=on_change
                    min=0.0 max=2.5 step=0.05
                />
                <SettingSlider
                    label="Brightness"
                    description="Linear brightness multiplier on zone colors"
                    key="capture.brightness"
                    value=brightness
                    on_change=on_change
                    min=0.0 max=2.0 step=0.05
                />
                <SettingSlider
                    label="Gamma"
                    description="Midtone shaping — above 1.0 deepens darks"
                    key="capture.gamma"
                    value=gamma
                    on_change=on_change
                    min=0.4 max=2.5 step=0.05
                />
                <SettingSlider
                    label="Smoothing"
                    description="Temporal response: low is cinematic, high is twitchy"
                    key="capture.smoothing"
                    value=smoothing
                    on_change=on_change
                    min=0.05 max=1.0 step=0.05
                />
                <SettingSlider
                    label="Scene cut"
                    description="Frame-change threshold that snaps colors instantly"
                    key="capture.scene_cut_threshold"
                    value=scene_cut
                    on_change=on_change
                    min=0.0 max=500.0 step=10.0
                    decimals=0
                />
                <SettingSlider
                    label="Grid columns"
                    description="Horizontal ambilight zones sampled from the screen"
                    key="capture.grid_cols"
                    value=grid_cols
                    on_change=on_change
                    min=2.0 max=32.0 step=1.0
                    decimals=0
                    integer=true
                />
                <SettingSlider
                    label="Grid rows"
                    description="Vertical ambilight zones sampled from the screen"
                    key="capture.grid_rows"
                    value=grid_rows
                    on_change=on_change
                    min=2.0 max=32.0 step=1.0
                    decimals=0
                    integer=true
                />
            </section>

            <section class="py-3 space-y-0">
                <SectionHeader title="Letterbox" icon=LuMinimize />
                <SettingToggle
                    label="Auto-crop bars"
                    description="Detect and crop black letterbox or pillarbox bars"
                    key="capture.letterbox"
                    value=letterbox
                    on_change=on_change
                />
                <SettingSlider
                    label="Detection threshold"
                    description="Luminance below this counts as a black bar"
                    key="capture.letterbox_threshold"
                    value=letterbox_threshold
                    on_change=on_change
                    min=0.0 max=0.2 step=0.005
                    decimals=3
                />
            </section>

            <SectionReset section_label="Capture" on_reset=on_reset />
        </div>
    }
}
