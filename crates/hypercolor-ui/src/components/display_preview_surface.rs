use leptos::prelude::*;

use crate::components::canvas_preview::CanvasPreview;
use crate::ws::CanvasFrame;

#[component]
pub fn DisplayPreviewSurface(
    #[prop(into)] frame: Signal<Option<CanvasFrame>>,
    fallback_src: String,
    aspect_ratio: String,
    #[prop(into)] aria_label: String,
    #[prop(into)] container_class: String,
) -> impl IntoView {
    let canvas_aspect_ratio = aspect_ratio.clone();
    let fallback_alt = aria_label.clone();
    let canvas_aria_label = aria_label.clone();

    view! {
        <div
            class=container_class
            style=move || format!("aspect-ratio: {aspect_ratio};")
        >
            <Show
                when=move || frame.get().is_some()
                fallback=move || {
                    view! {
                        <img
                            class="h-full w-full object-cover"
                            src=fallback_src.clone()
                            alt=fallback_alt.clone()
                            loading="eager"
                            decoding="async"
                            draggable="false"
                        />
                    }
                }
            >
                <CanvasPreview
                    frame=frame
                    fps=Signal::derive(|| 0.0_f32)
                    show_fps=false
                    fps_target=Signal::derive(|| 0_u32)
                    max_width="100%".to_string()
                    image_rendering="auto".to_string()
                    aspect_ratio=canvas_aspect_ratio.clone()
                    aria_label=canvas_aria_label.clone()
                    register_main_preview_consumer=false
                />
            </Show>
        </div>
    }
}
