use leptos::prelude::*;

use crate::components::canvas_preview::CanvasPreview;
use crate::ws::{CanvasFrame, CanvasPixelFormat};
use hypercolor_leptos_ext::canvas::{
    blob_url_from_bytes, revoke_blob_url, supports_bitmap_worker_canvas,
};

fn supports_display_preview_canvas() -> bool {
    supports_bitmap_worker_canvas()
}

fn jpeg_frame_blob_url(frame: &CanvasFrame) -> Option<String> {
    if frame.pixel_format() != CanvasPixelFormat::Jpeg {
        return None;
    }

    blob_url_from_bytes(frame.pixels_js(), "image/jpeg").ok()
}

/// Empty state shown while the display has not produced a preview frame yet.
fn preview_pending_state() -> impl IntoView {
    view! {
        <div class="flex h-full w-full flex-col items-center justify-center gap-1.5 bg-black/40 text-center">
            <span class="text-[11px] font-medium tracking-wide text-fg-tertiary">
                "No preview yet"
            </span>
            <span class="px-4 text-[10px] leading-relaxed text-fg-tertiary/60">
                "Assign a face to start rendering"
            </span>
        </div>
    }
}

#[component]
pub fn DisplayPreviewSurface(
    #[prop(into)] frame: Signal<Option<CanvasFrame>>,
    fallback_src: String,
    aspect_ratio: String,
    #[prop(into)] aria_label: String,
    #[prop(into)] container_class: String,
) -> impl IntoView {
    let prefer_canvas_presenter = supports_display_preview_canvas();
    let canvas_aspect_ratio = aspect_ratio.clone();
    let fallback_alt = aria_label.clone();
    let canvas_aria_label = aria_label.clone();
    let fallback_src_for_canvas = fallback_src.clone();
    let fallback_alt_for_canvas = fallback_alt.clone();
    let fallback_src_for_blob = fallback_src.clone();
    let (live_blob_url, set_live_blob_url) = signal(None::<String>);
    // The demand-driven preview endpoint 404s until the display produces a
    // frame; a failed <img> load swaps to a designed empty state instead of
    // the browser's broken-image glyph.
    let (fallback_failed, set_fallback_failed) = signal(false);

    Effect::new(move |previous: Option<Option<String>>| {
        if let Some(Some(old_url)) = previous.as_ref() {
            revoke_blob_url(old_url);
        }

        if prefer_canvas_presenter {
            set_live_blob_url.set(None);
            return None;
        }

        let next_url = frame.get().as_ref().and_then(jpeg_frame_blob_url);
        set_live_blob_url.set(next_url.clone());
        next_url
    });
    on_cleanup(move || {
        if let Some(url) = live_blob_url.get_untracked() {
            revoke_blob_url(&url);
        }
    });

    view! {
        <div
            class=container_class
            style=move || format!("aspect-ratio: {aspect_ratio};")
        >
            {move || {
                if prefer_canvas_presenter {
                    let fallback_src = fallback_src_for_canvas.clone();
                    let fallback_alt = fallback_alt_for_canvas.clone();
                    let canvas_aspect_ratio = canvas_aspect_ratio.clone();
                    let canvas_aria_label = canvas_aria_label.clone();
                    return view! {
                        <Show
                            when=move || frame.get().is_some()
                            fallback=move || {
                                let fallback_src = fallback_src.clone();
                                let fallback_alt = fallback_alt.clone();
                                view! {
                                    <Show
                                        when=move || !fallback_failed.get()
                                        fallback=preview_pending_state
                                    >
                                        <img
                                            class="h-full w-full object-cover"
                                            src=fallback_src.clone()
                                            alt=fallback_alt.clone()
                                            loading="eager"
                                            decoding="async"
                                            draggable="false"
                                            on:error=move |_| set_fallback_failed.set(true)
                                            on:load=move |_| set_fallback_failed.set(false)
                                        />
                                    </Show>
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
                    }
                    .into_any();
                }

                let src = live_blob_url
                    .get()
                    .unwrap_or_else(|| fallback_src_for_blob.clone());
                let fallback_alt = fallback_alt.clone();
                view! {
                    <Show
                        when=move || !fallback_failed.get()
                        fallback=preview_pending_state
                    >
                        <img
                            class="h-full w-full object-cover"
                            src=src.clone()
                            alt=fallback_alt.clone()
                            loading="eager"
                            decoding="async"
                            draggable="false"
                            on:error=move |_| set_fallback_failed.set(true)
                            on:load=move |_| set_fallback_failed.set(false)
                        />
                    </Show>
                }
                .into_any()
            }}
        </div>
    }
}
