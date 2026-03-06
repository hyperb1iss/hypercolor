//! Canvas preview — renders live RGBA frames from WebSocket binary data.

use leptos::html::Canvas;
use leptos::prelude::*;
use wasm_bindgen::Clamped;
use wasm_bindgen::JsCast;

use crate::ws::CanvasFrame;

/// Live canvas preview that paints RGBA pixel data from WebSocket frames.
#[component]
pub fn CanvasPreview(
    #[prop(into)] frame: Signal<Option<CanvasFrame>>,
    #[prop(into)] fps: Signal<f32>,
    #[prop(default = false)] show_fps: bool,
    #[prop(default = "100%".to_string())] max_width: String,
) -> impl IntoView {
    let canvas_ref = NodeRef::<Canvas>::new();

    // Paint frames to canvas whenever new data arrives
    Effect::new(move |_| {
        let Some(frame) = frame.get() else { return };
        let Some(canvas) = canvas_ref.get() else {
            return;
        };

        // Only resize canvas when dimensions actually change — setting width/height
        // every frame forces layout recalc and causes the sidebar to jitter.
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

        let image_data = web_sys::ImageData::new_with_u8_clamped_array_and_sh(
            Clamped(&frame.pixels),
            frame.width,
            frame.height,
        );

        if let Ok(image_data) = image_data {
            let _ = ctx.put_image_data(&image_data, 0.0, 0.0);
        }
    });

    let style = format!(
        "max-width: {max_width}; width: 100%; aspect-ratio: 320 / 200; image-rendering: pixelated;"
    );

    view! {
        <div class="relative bg-black animate-scale-in">
            <canvas
                node_ref=canvas_ref
                class="w-full h-auto block bg-black"
                style=style
            />
            {if show_fps {
                Some(view! {
                    <div class="absolute top-2 right-2 bg-black/70 backdrop-blur-sm px-2 py-0.5 rounded text-[10px] font-mono text-fg-tertiary
                                transition-all duration-300 animate-fade-in">
                        {move || format!("{:.0} fps", fps.get())}
                    </div>
                })
            } else {
                None
            }}
        </div>
    }
}
