//! The Studio Stage — the center workspace for the selected surface.
//!
//! The Stage has two views. **Output** is the live preview: a Light shows
//! the composited LED canvas via `CanvasPreview`, a Screen shows that
//! device's face via `DisplayPreviewSurface`. **Layout** embeds the
//! spatial device-placement editor lifted from the retired `/layout`
//! page. The Output/Layout toggle is hidden for Screens — a single LCD
//! has no spatial placement. Wave 10 adds per-zone preview frames.

use leptos::prelude::*;

use crate::api;
use crate::app::{DisplaysContext, WsContext};
use crate::components::canvas_preview::CanvasPreview;
use crate::components::display_preview_surface::DisplayPreviewSurface;
use crate::components::layout_builder::LayoutBuilder;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::display_preview_state::use_display_preview_subscription;
use crate::ws::CanvasFrame;

use super::StudioContext;
use super::stage_view::{StageView, resolve_stage_view};
use super::surface::{SurfaceKind, surfaces_from_groups};

/// Preview FPS ceiling while the Layout editor is on the Stage, matching
/// the retired `/layout` page so spatial editing stays smooth.
const LAYOUT_PREVIEW_FPS_CAP: u32 = 60;

/// The center Stage. Reads the selected surface from [`StudioContext`] and
/// the live preview streams from [`WsContext`].
#[component]
pub fn Stage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let studio = expect_context::<StudioContext>();
    let displays = expect_context::<DisplaysContext>().displays_resource;

    let selected_surface = Memo::new(move |_| {
        let id = studio.selected_surface_id.get()?;
        let scene = studio.active_scene.get()?;
        surfaces_from_groups(&scene.groups)
            .into_iter()
            .find(|surface| surface.id == id)
    });

    let surface_name = Memo::new(move |_| selected_surface.get().map(|surface| surface.name));
    let is_screen =
        Memo::new(move |_| selected_surface.get().map(|s| s.kind) == Some(SurfaceKind::Screen));

    // The toggle latches the last requested view; `resolved_view` applies
    // the §6.3 rule that a Screen has no Layout view.
    let requested_view = RwSignal::new(StageView::default());
    let resolved_view =
        Memo::new(move |_| resolve_stage_view(requested_view.get(), is_screen.get()));

    // The Layout editor wants the same preview headroom the `/layout`
    // page reserved; Output falls back to the shared default.
    Effect::new(move |_| {
        let cap = if resolved_view.get() == StageView::Layout {
            LAYOUT_PREVIEW_FPS_CAP
        } else {
            crate::ws::DEFAULT_PREVIEW_FPS_CAP
        };
        ws.set_preview_cap.set(cap);
        ws.set_preview_width_cap.set(0);
    });
    on_cleanup(move || {
        ws.set_preview_cap.set(crate::ws::DEFAULT_PREVIEW_FPS_CAP);
        ws.set_preview_width_cap.set(0);
    });

    // A Screen surface drives the per-display face-preview stream; a Light
    // leaves the target `None`, which unsubscribes. The subscription
    // retargets reactively and clears on unmount.
    let display_device = Signal::derive(move || {
        selected_surface
            .get()
            .filter(|surface| surface.kind == SurfaceKind::Screen)
            .and_then(|surface| surface.display_device_id)
    });
    use_display_preview_subscription(ws, display_device);

    // The selected screen's device record — its dimensions and shape size
    // the preview frame.
    let selected_display = Memo::new(move |_| {
        let device_id = display_device.get()?;
        let snapshot = displays.get();
        let items = snapshot.as_ref()?.as_ref().ok()?;
        items
            .iter()
            .find(|display| display.id == device_id)
            .cloned()
    });

    let screen_frame = RwSignal::new(None::<CanvasFrame>);
    Effect::new(move |_| {
        display_device.track();
        screen_frame.set(None);
    });
    Effect::new(move |_| {
        let frame = ws.display_preview_frame.get();
        // The channel carries no device id, so accept a frame only when
        // its resolution matches the selected screen. That rejects an
        // in-flight frame from the previously selected screen; two
        // identically sized screens still need daemon-side frame tagging
        // to be fully distinguishable.
        let belongs_to_target = match (&frame, selected_display.get()) {
            (Some(frame), Some(display)) => {
                frame.width == display.width && frame.height == display.height
            }
            (None, _) => true,
            (Some(_), None) => false,
        };
        if belongs_to_target {
            screen_frame.set(frame);
        }
    });

    // The display-preview stream carries no FPS, so the Screen caption is
    // resolution only; the LED canvas reports both.
    let caption = Memo::new(move |_| {
        if is_screen.get() {
            selected_display
                .get()
                .map(|display| format!("{}×{}", display.width, display.height))
                .unwrap_or_else(|| "—".to_owned())
        } else {
            let resolution = ws
                .canvas_frame
                .get()
                .map(|frame| format!("{}×{}", frame.width, frame.height))
                .unwrap_or_else(|| "—".to_owned());
            format!("{resolution} · {:.0} fps", ws.preview_fps.get())
        }
    });

    view! {
        <div class="flex h-full flex-col bg-surface-sunken/20">
            <div class="flex items-center justify-between gap-3 border-b border-edge-subtle/60 px-5 py-3">
                <div class="flex items-baseline gap-2">
                    <span class=label_class(LabelSize::Small, LabelTone::Default)>"Stage"</span>
                    <span class="text-sm font-semibold text-fg-primary">
                        {move || surface_name.get().unwrap_or_else(|| "No surface".to_owned())}
                    </span>
                </div>
                {move || {
                    if is_screen.get() {
                        view! {
                            <span class=label_class(
                                LabelSize::Micro,
                                LabelTone::Default,
                            )>"Output"</span>
                        }
                            .into_any()
                    } else {
                        view! { <StageViewToggle requested=requested_view /> }.into_any()
                    }
                }}
            </div>

            {move || match resolved_view.get() {
                StageView::Layout => {
                    view! {
                        <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
                            <LayoutBuilder />
                        </div>
                    }
                        .into_any()
                }
                StageView::Output => {
                    view! {
                        <div class="flex flex-1 items-center justify-center overflow-hidden p-6">
                            <div class="flex max-w-full flex-col items-center gap-3">
                                {move || {
                                    if is_screen.get() {
                                        let Some(display) = selected_display.get() else {
                                            return view! {
                                                <div class="flex h-64 w-64 items-center justify-center rounded-xl border border-dashed border-edge-subtle/45 text-[11px] text-fg-tertiary/55">
                                                    "Preparing screen preview…"
                                                </div>
                                            }
                                                .into_any();
                                        };
                                        let aspect = format!(
                                            "{} / {}",
                                            display.width.max(1),
                                            display.height.max(1),
                                        );
                                        let shape = if display.circular {
                                            "rounded-full"
                                        } else {
                                            "rounded-xl"
                                        };
                                        let container_class = format!(
                                            "w-full max-w-[520px] overflow-hidden border \
                                             border-edge-subtle/70 bg-black shadow-2xl {shape}",
                                        );
                                        view! {
                                            <DisplayPreviewSurface
                                                frame=screen_frame
                                                fallback_src=api::display_preview_url(
                                                    &display.id,
                                                    None,
                                                )
                                                aspect_ratio=aspect
                                                aria_label=format!(
                                                    "Studio stage preview of {}",
                                                    display.name,
                                                )
                                                container_class=container_class
                                            />
                                        }
                                            .into_any()
                                    } else {
                                        view! {
                                            <div
                                                class="overflow-hidden rounded-xl border border-edge-subtle/70 bg-black/45"
                                                style="box-shadow: 0 0 44px rgba(225, 53, 255, 0.09)"
                                            >
                                                <CanvasPreview
                                                    frame=ws.canvas_frame
                                                    fps=ws.preview_fps
                                                    fps_target=ws.preview_target_fps
                                                    max_width="min(640px, 100%)".to_string()
                                                    aria_label="Studio stage live output"
                                                        .to_string()
                                                />
                                            </div>
                                        }
                                            .into_any()
                                    }
                                }}
                                <div class="font-mono text-[11px] tabular-nums text-fg-tertiary/70">
                                    {move || caption.get()}
                                </div>
                            </div>
                        </div>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

/// The Output/Layout segmented toggle in the Stage header. Shown only for
/// Light surfaces; a Screen has no Layout view.
#[component]
fn StageViewToggle(requested: RwSignal<StageView>) -> impl IntoView {
    view! {
        <div class="flex items-center gap-0.5 rounded-lg border border-edge-subtle/60 bg-surface-sunken/40 p-0.5">
            <StageTab label="Output" value=StageView::Output requested=requested />
            <StageTab label="Layout" value=StageView::Layout requested=requested />
        </div>
    }
}

#[component]
fn StageTab(
    label: &'static str,
    value: StageView,
    requested: RwSignal<StageView>,
) -> impl IntoView {
    let selected = move || requested.get() == value;
    view! {
        <button
            type="button"
            class="rounded-md px-2.5 py-1 text-[11px] font-medium uppercase tracking-wide transition-colors"
            class=("bg-accent/12", selected)
            class=("text-fg-primary", selected)
            class=("text-fg-tertiary/65", move || !selected())
            on:click=move |_| requested.set(value)
        >
            {label}
        </button>
    }
}
