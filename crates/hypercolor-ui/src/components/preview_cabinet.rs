//! Unified preview cabinet — cinematic canvas preview with integrated
//! preset strip, shared between the dashboard and the effects page.
//!
//! Layout: a rounded cabinet containing the canvas on top (scrim, top
//! accent wash, bottom info overlay with title / author / description /
//! category dot / source+audio badges) and a preset row at the bottom
//! separated only by a thin accent-tinted divider. Category accent is
//! palette-aware — it prefers the active effect's thumbnail palette
//! primary so overlay text harmonises with the rendered frame.
//!
//! Two sizing modes via `fill_height`:
//!
//! - `fill_height = true` — cabinet is `h-full flex flex-col` and the
//!   canvas region grows to fill whatever space remains after the
//!   preset strip. Used on the dashboard where the cabinet sits in a
//!   bounded-height hero row.
//! - `fill_height = false` (default) — cabinet is a plain `flex flex-col`
//!   and the canvas self-sizes via its own aspect ratio. Used on the
//!   effects page where the preview sits in an unbounded sidebar.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use hypercolor_leptos_ext::raf::Scheduler;
use hypercolor_types::effect::ControlValue;
use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::app::{EffectsContext, WsContext};
use crate::color;
use crate::components::canvas_preview::CanvasPreview;
use crate::components::preset_panel::PresetToolbar;
use crate::icons::*;
use crate::style_utils::category_accent_rgb;
use crate::thumbnails::ThumbnailStore;

/// Unified cinematic preview widget with integrated preset strip.
#[component]
pub fn PreviewCabinet(
    /// Set to `true` on the page whose preview owns the performance
    /// telemetry stream. Only one `PreviewCabinet` should enable this
    /// per session — the telemetry context expects a single reporter.
    #[prop(default = false)]
    report_telemetry: bool,
    /// When `true`, the cabinet fills its parent's height and the canvas
    /// area grows to consume whatever space is left after the preset
    /// strip. When `false`, the canvas self-sizes via its aspect ratio
    /// and the cabinet's total height follows from the canvas plus the
    /// preset row.
    #[prop(default = false)]
    fill_height: bool,
    /// Pass `Some(callback)` to render a maximize/minimize button in the
    /// canvas's top-right corner. The callback fires on click; the host
    /// page is responsible for updating `is_fullscreen` in response.
    #[prop(optional)]
    on_toggle_fullscreen: Option<Callback<()>>,
    /// Drives the fullscreen button's icon (maximize vs minimize) and
    /// tooltip. Ignored when `on_toggle_fullscreen` is `None`.
    #[prop(into, optional)]
    is_fullscreen: MaybeProp<bool>,
) -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let fx = expect_context::<EffectsContext>();
    let thumb_store = use_context::<ThumbnailStore>();

    // Palette-aware accent — prefer the thumbnail's extracted primary so
    // the overlay text harmonises with the rendered frame, falling back
    // to the category accent before any thumbnail has been captured.
    let accent_rgb = Memo::new(move |_| {
        if let Some(store) = thumb_store
            && let Some(id) = fx.active_effect_id.get()
        {
            let palette_primary = fx.effects_index.with(|effects| {
                effects
                    .iter()
                    .find(|e| e.effect.id == id)
                    .and_then(|e| store.get(&id, &e.effect.version).map(|t| t.palette.primary))
            });
            if let Some(primary) = palette_primary {
                return primary;
            }
        }
        category_accent_rgb(&fx.active_effect_category.get()).to_string()
    });
    let accent_signal: Signal<String> = accent_rgb.into();

    let title_tint = Memo::new(move |_| color::accent_text_tint(&accent_rgb.get(), 0.86, 0.65));
    let body_tint = Memo::new(move |_| color::accent_text_tint(&accent_rgb.get(), 0.78, 0.22));
    let meta_tint = Memo::new(move |_| color::accent_text_tint(&accent_rgb.get(), 0.68, 0.65));
    let label_tint = Memo::new(move |_| color::accent_text_tint(&accent_rgb.get(), 0.84, 0.7));

    let effect_meta = Memo::new(move |_| {
        fx.active_effect_id.get().and_then(|id| {
            fx.effects_index.with(|effects| {
                effects
                    .iter()
                    .find(|e| e.effect.id == id)
                    .map(|e| e.effect.clone())
            })
        })
    });

    let control_values: Signal<HashMap<String, ControlValue>> = fx.active_control_values.into();
    let effect_id = Signal::derive(move || fx.active_effect_id.get());
    let active_preset_id_signal = Signal::derive(move || fx.active_preset_id.get());
    let has_effect = Memo::new(move |_| fx.active_effect_id.get().is_some());

    // Ignition pulse — toggles briefly whenever the active effect changes so
    // keyframe-based swap animations restart. Setting false then re-enabling
    // via `requestAnimationFrame` ensures the class is removed and re-added
    // across a single paint boundary, so the element snaps to the keyframe
    // start state (opacity:0, blurred) without flashing the visible default
    // in between. Prev-id is stored so initial mount doesn't animate.
    let igniting = RwSignal::new(false);
    let prev_effect_id = StoredValue::new(fx.active_effect_id.get_untracked());
    let ignition_scheduler = Rc::new(RefCell::new(None::<Scheduler>));

    Effect::new({
        let ignition_scheduler = Rc::clone(&ignition_scheduler);
        move |_| {
            let current = fx.active_effect_id.get();
            let previous = prev_effect_id.get_value();
            if previous != current {
                igniting.set(false);
                let scheduler = Scheduler::new(move |_| igniting.set(true));
                scheduler.schedule();
                ignition_scheduler.borrow_mut().replace(scheduler);
            }
            prev_effect_id.set_value(current);
        }
    });

    // Sizing classes switch between the two modes.
    //
    // The outer cabinet deliberately does NOT set `overflow-hidden` — the
    // preset dropdown needs to escape the cabinet bottom and float over
    // whatever sits below (controls panel on the effects page, stats on
    // the dashboard). Clipping lives on the inner canvas wrapper instead,
    // which rounds its top corners so the canvas and scrim still clip to
    // the cabinet's rounded silhouette.
    // `cabinet-accent-transition` adds a 2px transparent top border with a
    // `border-top-color` transition, so the dynamic `style:border-top-color`
    // below crossfades to the new accent instead of snap-changing. The
    // ignite pulse layers an animated box-shadow on top for extra drama.
    let cabinet_class = if fill_height {
        "cabinet-accent-transition relative rounded-xl border border-edge-subtle bg-black edge-glow \
         h-full flex flex-col"
    } else {
        "cabinet-accent-transition relative rounded-xl border border-edge-subtle bg-black edge-glow flex flex-col"
    };
    let canvas_wrapper_class = if fill_height {
        "relative flex-1 min-h-0 overflow-hidden rounded-t-xl"
    } else {
        "relative overflow-hidden rounded-t-xl"
    };

    view! {
        <div
            class=cabinet_class
            class:animate-cabinet-ignite=move || igniting.get()
            style:--glow-rgb=move || accent_rgb.get()
            style:border-top-color=move || format!("rgba({}, 0.45)", accent_rgb.get())
        >
            // ── Top: canvas + scrim + overlay info ─────────────────────────
            <div class=canvas_wrapper_class>
                <CanvasPreview
                    frame=ws.canvas_frame
                    fps=ws.preview_fps
                    show_fps=false
                    fps_target=ws.preview_target_fps
                    report_presenter_telemetry=report_telemetry
                />

                // Maximize / exit-fullscreen button — floats above the scrim
                // so the overlay info row's `pointer-events-none` wrapper
                // doesn't swallow clicks. Only rendered when the host page
                // actually wires up `on_toggle_fullscreen`; on pages that
                // don't care (effects sidebar), the control disappears.
                {on_toggle_fullscreen.map(|cb| view! {
                    <button
                        type="button"
                        class="absolute top-3 right-3 z-20 p-1.5 rounded-lg \
                               bg-black/45 backdrop-blur-sm border border-edge-subtle/60 \
                               text-fg-secondary hover:text-fg-primary \
                               hover:bg-black/65 hover:border-edge-default \
                               transition-all"
                        title=move || if is_fullscreen.get().unwrap_or(false) {
                            "Exit fullscreen (Esc)"
                        } else {
                            "Fullscreen preview"
                        }
                        on:click=move |ev: ev::MouseEvent| {
                            ev.stop_propagation();
                            cb.run(());
                        }
                    >
                        {move || if is_fullscreen.get().unwrap_or(false) {
                            view! { <Icon icon=LuMinimize width="13px" height="13px" /> }.into_any()
                        } else {
                            view! { <Icon icon=LuMaximize width="13px" height="13px" /> }.into_any()
                        }}
                    </button>
                })}

                // Ignition curtain — absolute overlay that fades out over the
                // canvas on each effect swap. Uses opacity animation only so
                // it never triggers a stacking-context change on the canvas
                // itself (applying `filter` to the wrapper caused layer
                // reshuffling that made the preset strip jump a few frames).
                <div
                    class="absolute inset-0 pointer-events-none opacity-0"
                    class:animate-canvas-ignite=move || igniting.get()
                />

                // Scrim — transparent at top, fades dark at bottom for legible overlay text
                <div
                    class="absolute inset-0 pointer-events-none"
                    style="background: linear-gradient(180deg, \
                           rgba(0, 0, 0, 0) 0%, \
                           rgba(0, 0, 0, 0) 42%, \
                           rgba(0, 0, 0, 0.78) 78%, \
                           rgba(0, 0, 0, 0.95) 100%)"
                />

                // Top accent wash — colored highlight along the top edge
                <div
                    class="absolute top-0 left-0 right-0 h-px pointer-events-none"
                    style=move || format!(
                        "background: linear-gradient(90deg, transparent 0%, rgba({0}, 0.8) 50%, transparent 100%); \
                         box-shadow: 0 0 14px rgba({0}, 0.55)",
                        accent_rgb.get()
                    )
                />

                // Info overlay — title + author, description, category + source/audio badges
                <div class="absolute left-0 right-0 bottom-0 px-4 pb-3 pt-10 pointer-events-none">
                    {move || {
                        let name = fx.active_effect_name.get();
                        let meta = effect_meta.get();
                        name.map(|effect_name| {
                            let description = meta.as_ref().map(|m| m.description.clone()).unwrap_or_default();
                            let category = meta.as_ref().map(|m| m.category.clone()).unwrap_or_default();
                            let author = meta.as_ref().map(|m| m.author.clone()).unwrap_or_default();
                            let audio_reactive = meta.as_ref().is_some_and(|m| m.audio_reactive);
                            let source = meta.as_ref().map(|m| m.source.clone()).unwrap_or_default();
                            let is_calibration = meta.as_ref().is_some_and(|m| {
                                m.name.eq_ignore_ascii_case("Calibration")
                                    || m.tags.iter().any(|tag| tag.eq_ignore_ascii_case("calibration"))
                            });
                            let is_html = source == "html";
                            let show_source = source != "native";

                            view! {
                                <div
                                    class="flex items-baseline justify-between gap-3 mb-1"
                                    class:animate-effect-swap=move || igniting.get()
                                >
                                    <h3
                                        class="text-[15px] font-semibold line-clamp-1 leading-tight \
                                               drop-shadow-[0_2px_8px_rgba(0,0,0,0.85)]"
                                        style:color=move || format!("rgb({})", title_tint.get())
                                    >
                                        {effect_name}
                                    </h3>
                                    {(!author.is_empty()).then(|| view! {
                                        <span
                                            class="text-[10px] font-mono uppercase tracking-wider shrink-0 truncate \
                                                   max-w-[140px] drop-shadow-[0_1px_4px_rgba(0,0,0,0.85)]"
                                            style:color=move || format!("rgba({}, 0.65)", meta_tint.get())
                                        >
                                            {author}
                                        </span>
                                    })}
                                </div>

                                {(!description.is_empty()).then(|| view! {
                                    <p
                                        class="text-[11px] line-clamp-2 leading-relaxed mb-2 \
                                               drop-shadow-[0_1px_4px_rgba(0,0,0,0.85)]"
                                        class:animate-effect-swap-2=move || igniting.get()
                                        style:color=move || format!("rgba({}, 0.88)", body_tint.get())
                                    >
                                        {description}
                                    </p>
                                })}

                                <div
                                    class="flex items-center justify-between gap-2"
                                    class:animate-effect-swap-3=move || igniting.get()
                                >
                                    <div class="flex items-center gap-1.5 min-w-0">
                                        <div
                                            class="w-1.5 h-1.5 rounded-full shrink-0 dot-alive"
                                            style:background=move || format!("rgb({})", accent_rgb.get())
                                            style:box-shadow=move || format!("0 0 6px rgba({}, 0.75)", accent_rgb.get())
                                        />
                                        <span
                                            class="text-[10px] font-mono uppercase tracking-wider capitalize truncate \
                                                   drop-shadow-[0_1px_3px_rgba(0,0,0,0.85)]"
                                            style:color=move || format!("rgb({})", meta_tint.get())
                                        >
                                            {category}
                                        </span>
                                    </div>
                                    <div class="flex items-center gap-1.5 shrink-0">
                                        {is_calibration.then(|| view! {
                                            <span
                                                class="inline-flex items-center gap-1 text-[9px] font-mono px-1.5 py-0.5 rounded-full bg-neon-cyan/14 text-neon-cyan backdrop-blur-sm"
                                                title="Layout setup and calibration tool"
                                            >
                                                <Icon icon=LuRadar width="11px" height="11px" />
                                                <span>"Setup"</span>
                                            </span>
                                        })}
                                        {show_source.then(|| {
                                            let icon = if is_html { LuGlobe } else { LuCode };
                                            view! {
                                                <span
                                                    class="inline-flex items-center text-[9px] font-mono px-1.5 py-0.5 \
                                                           rounded-full bg-white/5 backdrop-blur-sm"
                                                    style:color=move || format!("rgba({}, 0.85)", meta_tint.get())
                                                >
                                                    <Icon icon=icon width="11px" height="11px" />
                                                </span>
                                            }
                                        })}
                                        {audio_reactive.then(|| view! {
                                            <span
                                                class="inline-flex items-center text-coral/90 px-1.5 py-0.5 \
                                                       rounded-full bg-coral/15 backdrop-blur-sm"
                                                title="Audio-reactive"
                                            >
                                                <Icon icon=LuAudioLines width="11px" height="11px" />
                                            </span>
                                        })}
                                    </div>
                                </div>
                            }
                        })
                    }}
                </div>
            </div>

            // ── Bottom: preset strip — part of the cabinet, not its own box ─
            //
            // Note: this strip deliberately has NO `overflow-hidden` so the
            // preset dropdown can float down past the cabinet bottom and
            // layer over whatever sits below. `rounded-b-xl` clips the
            // gradient background to match the cabinet's bottom silhouette
            // without clipping descendant elements.
            <div
                class="shrink-0 relative z-50 rounded-b-xl"
                style=move || {
                    let rgb = accent_rgb.get();
                    format!(
                        "border-top: 1px solid rgba({rgb}, 0.22); \
                         background: linear-gradient(180deg, \
                           rgba({rgb}, 0.06) 0%, \
                           rgba(0, 0, 0, 0.55) 100%)"
                    )
                }
            >
                <div class="flex items-center gap-3 px-3 py-2">
                    <div class="flex items-center gap-1.5 shrink-0">
                        <span
                            class="inline-flex items-center justify-center"
                            style=move || {
                                let rgb = accent_rgb.get();
                                format!(
                                    "color: rgb({rgb}); \
                                     filter: drop-shadow(0 0 6px rgba({rgb}, 0.75))"
                                )
                            }
                        >
                            <Icon icon=LuZap width="12px" height="12px" />
                        </span>
                        <span
                            class="text-[9px] font-mono uppercase tracking-[0.18em] font-semibold"
                            style=move || format!("color: rgb({})", label_tint.get())
                        >
                            "Preset"
                        </span>
                    </div>

                    <div
                        class="w-px h-4 shrink-0"
                        style=move || format!(
                            "background: linear-gradient(180deg, \
                               transparent 0%, \
                               rgba({rgb}, 0.35) 50%, \
                               transparent 100%)",
                            rgb = accent_rgb.get()
                        )
                    />

                    <div class="flex-1 min-w-0">
                        {move || if has_effect.get() {
                            view! {
                                <PresetToolbar
                                    effect_id=effect_id
                                    control_values=control_values
                                    accent_rgb=accent_signal
                                    on_preset_applied=Callback::new(move |()| fx.refresh_active_effect())
                                    active_preset_id_signal=active_preset_id_signal
                                />
                            }.into_any()
                        } else {
                            view! {
                                <div class="flex items-center h-[30px] text-[10px] font-mono uppercase tracking-[0.14em] text-fg-tertiary/60">
                                    "Pick an effect to load presets"
                                </div>
                            }.into_any()
                        }}
                    </div>
                </div>
            </div>
        </div>
    }
}
