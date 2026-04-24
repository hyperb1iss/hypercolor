//! Browser-only canvas helpers.

use wasm_bindgen::{Clamped, JsCast, JsValue};

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

pub fn set_canvas_size(canvas: &web_sys::HtmlCanvasElement, width: u32, height: u32) -> bool {
    let mut resized = false;
    if canvas.width() != width {
        canvas.set_width(width);
        resized = true;
    }
    if canvas.height() != height {
        canvas.set_height(height);
        resized = true;
    }
    resized
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

pub fn supports_offscreen_canvas_2d_bitmap() -> bool {
    let Ok(offscreen) = web_sys::OffscreenCanvas::new(1, 1) else {
        return false;
    };
    let has_context = offscreen.get_context("2d").ok().flatten().is_some();
    let has_bitmap = offscreen
        .transfer_to_image_bitmap()
        .map(|bitmap| {
            bitmap.close();
        })
        .is_ok();

    has_context && has_bitmap
}

pub fn message_image_bitmap(event: &web_sys::MessageEvent) -> Option<web_sys::ImageBitmap> {
    event.data().dyn_into().ok()
}

pub fn image_data_rgba(
    pixels: &[u8],
    width: u32,
    height: u32,
) -> Result<web_sys::ImageData, JsValue> {
    web_sys::ImageData::new_with_u8_clamped_array_and_sh(Clamped(pixels), width, height)
}

pub fn blob_url_from_bytes(bytes: &js_sys::Uint8Array, mime_type: &str) -> Result<String, JsValue> {
    let parts = js_sys::Array::new();
    parts.push(bytes);
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime_type);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)?;
    web_sys::Url::create_object_url_with_blob(&blob)
}

pub fn script_blob_url(source: &str) -> Result<String, JsValue> {
    let parts = js_sys::Array::new();
    parts.push(&JsValue::from_str(source));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("text/javascript");
    let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts.into(), &options)?;
    web_sys::Url::create_object_url_with_blob(&blob)
}

pub fn revoke_blob_url(url: &str) -> bool {
    web_sys::Url::revoke_object_url(url).is_ok()
}

pub fn buffer_data_f32(
    gl: &web_sys::WebGlRenderingContext,
    target: u32,
    values: &[f32],
    usage: u32,
) {
    let array = js_sys::Float32Array::from(values);
    gl.buffer_data_with_array_buffer_view(target, &array, usage);
}

pub fn allocate_texture_u8(
    gl: &web_sys::WebGlRenderingContext,
    width: i32,
    height: i32,
    format: u32,
    pixels: &js_sys::Uint8Array,
) -> Result<(), JsValue> {
    let internal_format =
        i32::try_from(format).map_err(|_| JsValue::from_str("webgl texture format exceeds i32"))?;
    gl.tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_opt_js_u8_array(
        web_sys::WebGlRenderingContext::TEXTURE_2D,
        0,
        internal_format,
        width,
        height,
        0,
        format,
        web_sys::WebGlRenderingContext::UNSIGNED_BYTE,
        Some(pixels),
    )
}

pub fn update_texture_u8_or_reallocate(
    gl: &web_sys::WebGlRenderingContext,
    width: i32,
    height: i32,
    format: u32,
    pixels: &js_sys::Uint8Array,
) -> Result<(), JsValue> {
    gl.tex_sub_image_2d_with_i32_and_i32_and_u32_and_type_and_opt_js_u8_array(
        web_sys::WebGlRenderingContext::TEXTURE_2D,
        0,
        0,
        0,
        width,
        height,
        format,
        web_sys::WebGlRenderingContext::UNSIGNED_BYTE,
        Some(pixels),
    )
    .or_else(|_| allocate_texture_u8(gl, width, height, format, pixels))
}
