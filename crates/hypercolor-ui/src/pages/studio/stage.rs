//! The Studio Stage — the center preview of the selected surface.
//!
//! Wave 4 ships the Output view: the live composited canvas plus its
//! resolution and FPS. Wave 5 adds the Layout view and the Output/Layout
//! toggle; Wave 10 adds per-zone preview frames.

use leptos::prelude::*;

use crate::app::WsContext;
use crate::components::canvas_preview::CanvasPreview;
use crate::components::section_label::{LabelSize, LabelTone, label_class};

use super::StudioContext;
use super::surface::surfaces_from_groups;

/// The center Stage. Reads the selected surface from [`StudioContext`] and
/// the live canvas stream from [`WsContext`].
#[component]
pub fn Stage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let studio = expect_context::<StudioContext>();

    let surface_name = Memo::new(move |_| {
        let id = studio.selected_surface_id.get()?;
        let scene = studio.active_scene.get()?;
        surfaces_from_groups(&scene.groups)
            .into_iter()
            .find(|surface| surface.id == id)
            .map(|surface| surface.name)
    });

    let caption = Memo::new(move |_| {
        let resolution = ws
            .canvas_frame
            .get()
            .map(|frame| format!("{}×{}", frame.width, frame.height))
            .unwrap_or_else(|| "—".to_owned());
        format!("{resolution} · {:.0} fps", ws.preview_fps.get())
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
                    <div class="font-mono text-[11px] tabular-nums text-fg-tertiary/70">
                        {move || caption.get()}
                    </div>
                </div>
            </div>
        </div>
    }
}
