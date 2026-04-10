use std::cell::Cell;
use std::rc::Rc;

use js_sys::{Array, Object, Reflect};
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use web_sys::{
    Blob, BlobPropertyBag, HtmlCanvasElement, ImageBitmap, ImageBitmapRenderingContext,
    MessageEvent, OffscreenCanvas, Url, Worker,
};

use crate::ws::{CanvasFrame, CanvasPixelFormat};

use super::PreviewRenderOutcome;

const PREVIEW_WORKER_SOURCE: &str = r#"
let canvas = null;
let ctx = null;
let latestFrame = null;
let framePending = false;
let scratchRgba = null;

self.onmessage = (event) => {
  const data = event.data;
  if (!data || data.kind !== "frame") {
    return;
  }

  latestFrame = data;
  if (!framePending) {
    framePending = true;
    scheduleFlush();
  }
};

function scheduleFlush() {
  if (typeof self.requestAnimationFrame === "function") {
    self.requestAnimationFrame(flushFrame);
    return;
  }

  self.setTimeout(flushFrame, 0);
}

function flushFrame() {
  framePending = false;
  const frame = latestFrame;
  if (!frame) {
    return;
  }

  if (!ensureCanvas(frame.width, frame.height)) {
    return;
  }

  const imageData = createImageData(frame);
  if (!imageData) {
    return;
  }

  ctx.putImageData(imageData, 0, 0);
  const bitmap = canvas.transferToImageBitmap();
  self.postMessage({ kind: "present", frameNumber: frame.frameNumber, bitmap }, [bitmap]);
}

function ensureCanvas(width, height) {
  if (!canvas) {
    if (typeof OffscreenCanvas !== "function") {
      return false;
    }

    canvas = new OffscreenCanvas(width, height);
    ctx = canvas.getContext("2d", { alpha: false, desynchronized: true });
    if (!ctx) {
      canvas = null;
      return false;
    }
  }

  if (canvas.width !== width) {
    canvas.width = width;
  }
  if (canvas.height !== height) {
    canvas.height = height;
  }

  return true;
}

function createImageData(frame) {
  const pixels = frame.pixels;
  if (!(pixels instanceof Uint8Array)) {
    return null;
  }

  if (frame.format === "rgba") {
    return new ImageData(
      new Uint8ClampedArray(pixels.buffer, pixels.byteOffset, pixels.byteLength),
      frame.width,
      frame.height,
    );
  }

  if (frame.format !== "rgb") {
    return null;
  }

  const requiredLength = frame.width * frame.height * 4;
  if (!scratchRgba || scratchRgba.length !== requiredLength) {
    scratchRgba = new Uint8ClampedArray(requiredLength);
  }

  for (let src = 0, dst = 0; src + 2 < pixels.length; src += 3, dst += 4) {
    scratchRgba[dst] = pixels[src];
    scratchRgba[dst + 1] = pixels[src + 1];
    scratchRgba[dst + 2] = pixels[src + 2];
    scratchRgba[dst + 3] = 255;
  }

  return new ImageData(scratchRgba, frame.width, frame.height);
}
"#;

pub(super) struct PreviewWorkerRuntime {
    worker: Worker,
    worker_url: String,
    failed: Rc<Cell<bool>>,
    last_shape: Option<(u32, u32, CanvasPixelFormat)>,
    _onmessage: Closure<dyn FnMut(MessageEvent)>,
}

impl PreviewWorkerRuntime {
    pub(super) fn new(canvas: &HtmlCanvasElement, frame: &CanvasFrame) -> Result<Self, ()> {
        if canvas.width() != frame.width {
            canvas.set_width(frame.width);
        }
        if canvas.height() != frame.height {
            canvas.set_height(frame.height);
        }

        let bitmap_ctx = canvas
            .get_context("bitmaprenderer")
            .ok()
            .flatten()
            .and_then(|ctx| ctx.dyn_into::<ImageBitmapRenderingContext>().ok())
            .ok_or(())?;
        probe_worker_canvas_support()?;

        let worker_url = create_worker_url().map_err(|_| ())?;
        let worker = Worker::new(&worker_url).map_err(|_| ())?;
        let failed = Rc::new(Cell::new(false));
        let failed_handle = Rc::clone(&failed);
        let canvas_handle = canvas.clone();
        let bitmap_ctx_handle = bitmap_ctx.clone();

        let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(move |event| {
            if !present_bitmap(&canvas_handle, &bitmap_ctx_handle, &event) {
                failed_handle.set(true);
            }
        });

        worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));

        Ok(Self {
            worker,
            worker_url,
            failed,
            last_shape: None,
            _onmessage: onmessage,
        })
    }

    pub(super) fn render(&mut self, frame: &CanvasFrame) -> PreviewRenderOutcome {
        if self.failed.get() {
            return PreviewRenderOutcome::Reinitialize;
        }

        let next_shape = (frame.width, frame.height, frame.pixel_format());
        if self.last_shape.is_some_and(|shape| shape != next_shape) {
            self.failed.set(true);
            return PreviewRenderOutcome::Reinitialize;
        }

        if post_frame(&self.worker, frame).is_err() {
            self.failed.set(true);
            return PreviewRenderOutcome::Reinitialize;
        }

        self.last_shape = Some(next_shape);
        PreviewRenderOutcome::Presented
    }
}

fn probe_worker_canvas_support() -> Result<(), ()> {
    let offscreen = OffscreenCanvas::new(1, 1).map_err(|_| ())?;
    let has_context = offscreen.get_context("2d").ok().flatten().is_some();
    let has_bitmap = offscreen
        .transfer_to_image_bitmap()
        .map(|bitmap| {
            bitmap.close();
        })
        .is_ok();

    if has_context && has_bitmap {
        Ok(())
    } else {
        Err(())
    }
}

impl Drop for PreviewWorkerRuntime {
    fn drop(&mut self) {
        self.worker.set_onmessage(None);
        self.worker.terminate();
        let _ = Url::revoke_object_url(&self.worker_url);
    }
}

fn create_worker_url() -> Result<String, JsValue> {
    let parts = Array::new();
    parts.push(&JsValue::from_str(PREVIEW_WORKER_SOURCE));

    let options = BlobPropertyBag::new();
    options.set_type("text/javascript");

    let blob = Blob::new_with_str_sequence_and_options(&parts.into(), &options)?;
    Url::create_object_url_with_blob(&blob)
}

fn post_frame(worker: &Worker, frame: &CanvasFrame) -> Result<(), JsValue> {
    let message = Object::new();
    Reflect::set(
        &message,
        &JsValue::from_str("kind"),
        &JsValue::from_str("frame"),
    )?;
    Reflect::set(
        &message,
        &JsValue::from_str("frameNumber"),
        &JsValue::from_f64(f64::from(frame.frame_number)),
    )?;
    Reflect::set(
        &message,
        &JsValue::from_str("width"),
        &JsValue::from_f64(f64::from(frame.width)),
    )?;
    Reflect::set(
        &message,
        &JsValue::from_str("height"),
        &JsValue::from_f64(f64::from(frame.height)),
    )?;
    Reflect::set(
        &message,
        &JsValue::from_str("format"),
        &JsValue::from_str(pixel_format_label(frame.pixel_format())),
    )?;
    Reflect::set(&message, &JsValue::from_str("pixels"), frame.pixels_js())?;
    worker.post_message(&message)
}

fn pixel_format_label(format: CanvasPixelFormat) -> &'static str {
    match format {
        CanvasPixelFormat::Rgb => "rgb",
        CanvasPixelFormat::Rgba => "rgba",
    }
}

fn present_bitmap(
    canvas: &HtmlCanvasElement,
    bitmap_ctx: &ImageBitmapRenderingContext,
    event: &MessageEvent,
) -> bool {
    let data = event.data();
    let Ok(kind) = Reflect::get(&data, &JsValue::from_str("kind")) else {
        return false;
    };
    if kind.as_string().as_deref() != Some("present") {
        return true;
    }

    let Ok(bitmap_value) = Reflect::get(&data, &JsValue::from_str("bitmap")) else {
        return false;
    };
    let Ok(bitmap) = bitmap_value.dyn_into::<ImageBitmap>() else {
        return false;
    };

    if canvas.width() != bitmap.width() {
        canvas.set_width(bitmap.width());
    }
    if canvas.height() != bitmap.height() {
        canvas.set_height(bitmap.height());
    }

    bitmap_ctx.transfer_from_image_bitmap(&bitmap);
    bitmap.close();
    true
}
