//! Browser-only canvas helpers.

use wasm_bindgen::{JsCast, JsValue};

pub fn create_canvas() -> Result<web_sys::HtmlCanvasElement, JsValue> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("document unavailable"))?;

    document
        .create_element("canvas")?
        .dyn_into()
        .map_err(|_| JsValue::from_str("created element is not a canvas"))
}

pub fn context_2d(
    canvas: &web_sys::HtmlCanvasElement,
) -> Option<web_sys::CanvasRenderingContext2d> {
    canvas
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|context| context.dyn_into().ok())
}

pub fn bitmap_renderer_context(
    canvas: &web_sys::HtmlCanvasElement,
) -> Option<web_sys::ImageBitmapRenderingContext> {
    canvas
        .get_context("bitmaprenderer")
        .ok()
        .flatten()
        .and_then(|context| context.dyn_into().ok())
}

pub fn webgl_context(
    canvas: &web_sys::HtmlCanvasElement,
) -> Option<web_sys::WebGlRenderingContext> {
    canvas
        .get_context("webgl")
        .ok()
        .flatten()
        .or_else(|| canvas.get_context("experimental-webgl").ok().flatten())
        .and_then(|context| context.dyn_into().ok())
}

pub fn supports_global(name: &str) -> bool {
    js_sys::Reflect::has(&js_sys::global(), &JsValue::from_str(name)).unwrap_or(false)
}

pub fn supports_bitmap_worker_canvas() -> bool {
    create_canvas()
        .ok()
        .is_some_and(|canvas| bitmap_renderer_context(&canvas).is_some())
        && supports_global("createImageBitmap")
        && supports_global("Worker")
}

pub fn message_image_bitmap(event: &web_sys::MessageEvent) -> Option<web_sys::ImageBitmap> {
    event.data().dyn_into().ok()
}
