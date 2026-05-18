//! The Studio Stage — the center preview of the selected surface.
//!
//! Wave 4 ships the Output view: a Light shows the live composited LED
//! canvas via `CanvasPreview`; a Screen shows that device's live face
//! preview via `DisplayPreviewSurface`, the same component the Displays
//! page uses. Wave 5 adds the Layout view and the Output/Layout toggle;
//! Wave 10 adds per-zone preview frames.

use leptos::prelude::*;

use crate::api;
use crate::app::{DisplaysContext, WsContext};
use crate::components::canvas_preview::CanvasPreview;
use crate::components::display_preview_surface::DisplayPreviewSurface;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::display_preview_state::use_display_preview_subscription;
use crate::ws::CanvasFrame;

use super::StudioContext;
use super::surface::{SurfaceKind, surfaces_from_groups};

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

    // `display_preview_frame` is one shared, untagged signal: a direct
    // screen-to-screen switch would otherwise show the previous screen's
    // last face under the new name until its stream delivers a frame.
    // Latch the frame and drop it the instant the target device changes,
    // so the gap falls back to the new screen's still image instead.
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
                <span class=label_class(LabelSize::Micro, LabelTone::Default)>"Output"</span>
            </div>

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
                                    fallback_src=api::display_preview_url(&display.id, None)
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
                                        aria_label="Studio stage live output".to_string()
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
        </div>
    }
}
