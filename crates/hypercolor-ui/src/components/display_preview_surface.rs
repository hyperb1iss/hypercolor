use leptos::prelude::*;
use wasm_bindgen::{JsCast, JsValue};

use crate::components::canvas_preview::CanvasPreview;
use crate::ws::{CanvasFrame, CanvasPixelFormat};

fn supports_display_preview_canvas() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let Some(document) = window.document() else {
        return false;
    };
    let Ok(canvas) = document.create_element("canvas") else {
        return false;
    };
    let Ok(canvas) = canvas.dyn_into::<web_sys::HtmlCanvasElement>() else {
        return false;
    };

    let has_bitmap_renderer = canvas
        .get_context("bitmaprenderer")
        .ok()
        .flatten()
        .is_some();
    let global = js_sys::global();
    let has_create_image_bitmap =
        js_sys::Reflect::has(&global, &JsValue::from_str("createImageBitmap")).unwrap_or(false);
    let has_worker = js_sys::Reflect::has(&global, &JsValue::from_str("Worker")).unwrap_or(false);

    has_bitmap_renderer && has_create_image_bitmap && has_worker
}

fn jpeg_frame_blob_url(frame: &CanvasFrame) -> Option<String> {
    if frame.pixel_format() != CanvasPixelFormat::Jpeg {
        return None;
    }

    let parts = js_sys::Array::new();
    parts.push(frame.pixels_js());
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("image/jpeg");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options).ok()?;
    web_sys::Url::create_object_url_with_blob(&blob).ok()
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

    Effect::new(move |previous: Option<Option<String>>| {
        if let Some(Some(old_url)) = previous.as_ref() {
            let _ = web_sys::Url::revoke_object_url(old_url);
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
            let _ = web_sys::Url::revoke_object_url(&url);
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
                    }
                    .into_any();
                }

                let src = live_blob_url
                    .get()
                    .unwrap_or_else(|| fallback_src_for_blob.clone());
                view! {
                    <img
                        class="h-full w-full object-cover"
                        src=src
                        alt=fallback_alt.clone()
                        loading="eager"
                        decoding="async"
                        draggable="false"
                    />
                }
                .into_any()
            }}
        </div>
    }
}
