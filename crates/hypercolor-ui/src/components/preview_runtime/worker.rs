use std::cell::{Cell, RefCell};
use std::rc::Rc;

use hypercolor_leptos_ext::canvas::{bitmap_renderer_context, message_image_bitmap};
use hypercolor_leptos_ext::events::WorkerMessageHandler;
use js_sys::Array;
use wasm_bindgen::JsValue;
use web_sys::{
    Blob, BlobPropertyBag, HtmlCanvasElement, ImageBitmapRenderingContext, MessageEvent,
    OffscreenCanvas, Url, Worker,
};

use crate::ws::{CanvasFrame, CanvasPixelFormat};

use super::PreviewRenderOutcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchDecision {
    DispatchNow,
    Deferred,
}

struct FrameDispatchState<T> {
    in_flight: bool,
    queued: Option<T>,
}

impl<T> Default for FrameDispatchState<T> {
    fn default() -> Self {
        Self {
            in_flight: false,
            queued: None,
        }
    }
}

impl<T> FrameDispatchState<T> {
    fn push_or_defer(&mut self, frame: T) -> DispatchDecision {
        if self.in_flight {
            self.queued = Some(frame);
            DispatchDecision::Deferred
        } else {
            self.in_flight = true;
            self.queued = Some(frame);
            DispatchDecision::DispatchNow
        }
    }

    fn take_for_dispatch(&mut self) -> Option<T> {
        self.queued.take()
    }

    fn next_after_present(&mut self) -> Option<T> {
        if self.queued.is_some() {
            self.in_flight = true;
            return self.queued.take();
        }

        self.in_flight = false;
        None
    }
}

const PREVIEW_WORKER_SOURCE: &str = r#"
let canvas = null;
let ctx = null;
let latestFrame = null;
let framePending = false;
let scratchRgba = null;

self.onmessage = (event) => {
  const frame = decodeFrame(event.data);
  if (!frame) {
    return;
  }

  latestFrame = frame;
  if (!framePending) {
    framePending = true;
    scheduleFlush();
  }
};

function decodeFrame(data) {
  if (!Array.isArray(data) || data.length !== 4) {
    return null;
  }

  const width = data[0] >>> 0;
  const height = data[1] >>> 0;
  const format = data[2] | 0;
  const pixels = data[3];
  if (!(pixels instanceof Uint8Array)) {
    return null;
  }

  return { width, height, format, pixels };
}

function scheduleFlush() {
  if (typeof self.requestAnimationFrame === "function") {
    self.requestAnimationFrame(flushFrame);
    return;
  }

  self.setTimeout(flushFrame, 0);
}

async function flushFrame() {
  framePending = false;
  const frame = latestFrame;
  if (!frame) {
    return;
  }

  if (frame.format === 2) {
    const bitmap = await createJpegBitmap(frame);
    if (!bitmap) {
      return;
    }

    self.postMessage(bitmap, [bitmap]);
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
  self.postMessage(bitmap, [bitmap]);
}

async function createJpegBitmap(frame) {
  if (typeof createImageBitmap !== "function") {
    return null;
  }

  try {
    const blob = new Blob([frame.pixels], { type: "image/jpeg" });
    return await createImageBitmap(blob);
  } catch {
    return null;
  }
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

  if (frame.format === 1) {
    return new ImageData(
      new Uint8ClampedArray(pixels.buffer, pixels.byteOffset, pixels.byteLength),
      frame.width,
      frame.height,
    );
  }

  if (frame.format !== 0) {
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
    dispatch_state: Rc<RefCell<FrameDispatchState<CanvasFrame>>>,
    last_shape: Option<(u32, u32, CanvasPixelFormat)>,
    onmessage: WorkerMessageHandler,
}

impl PreviewWorkerRuntime {
    pub(super) fn new(canvas: &HtmlCanvasElement, frame: &CanvasFrame) -> Result<Self, ()> {
        if canvas.width() != frame.width {
            canvas.set_width(frame.width);
        }
        if canvas.height() != frame.height {
            canvas.set_height(frame.height);
        }

        let bitmap_ctx = bitmap_renderer_context(canvas).ok_or(())?;
        probe_worker_support(frame.pixel_format())?;

        let worker_url = create_worker_url().map_err(|_| ())?;
        let worker = Worker::new(&worker_url).map_err(|_| ())?;
        let failed = Rc::new(Cell::new(false));
        let dispatch_state = Rc::new(RefCell::new(FrameDispatchState::default()));
        let failed_handle = Rc::clone(&failed);
        let dispatch_state_handle = Rc::clone(&dispatch_state);
        let canvas_handle = canvas.clone();
        let bitmap_ctx_handle = bitmap_ctx.clone();
        let worker_handle = worker.clone();

        let onmessage = WorkerMessageHandler::attach(&worker, move |event| {
            if !present_bitmap(&canvas_handle, &bitmap_ctx_handle, &event) {
                failed_handle.set(true);
                return;
            }

            let next_frame = dispatch_state_handle.borrow_mut().next_after_present();
            if let Some(frame) = next_frame
                && post_frame(&worker_handle, &frame).is_err()
            {
                failed_handle.set(true);
            }
        });

        Ok(Self {
            worker,
            worker_url,
            failed,
            dispatch_state,
            last_shape: None,
            onmessage,
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

        let decision = self
            .dispatch_state
            .borrow_mut()
            .push_or_defer(frame.clone());
        if decision == DispatchDecision::DispatchNow {
            let next_frame = self
                .dispatch_state
                .borrow_mut()
                .take_for_dispatch()
                .expect("dispatch-now state should hold the frame being sent");
            if post_frame(&self.worker, &next_frame).is_err() {
                self.failed.set(true);
                return PreviewRenderOutcome::Reinitialize;
            }
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

fn probe_worker_jpeg_support() -> Result<(), ()> {
    js_sys::Reflect::has(&js_sys::global(), &JsValue::from_str("createImageBitmap"))
        .map_err(|_| ())
        .and_then(|supported| supported.then_some(()).ok_or(()))
}

fn probe_worker_support(format: CanvasPixelFormat) -> Result<(), ()> {
    match format {
        CanvasPixelFormat::Jpeg => probe_worker_jpeg_support(),
        CanvasPixelFormat::Rgb | CanvasPixelFormat::Rgba => probe_worker_canvas_support(),
    }
}

impl Drop for PreviewWorkerRuntime {
    fn drop(&mut self) {
        self.onmessage.detach_from(&self.worker);
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
    let message = Array::new();
    message.push(&JsValue::from_f64(f64::from(frame.width)));
    message.push(&JsValue::from_f64(f64::from(frame.height)));
    message.push(&JsValue::from_f64(f64::from(pixel_format_code(
        frame.pixel_format(),
    ))));
    message.push(frame.pixels_js());
    worker.post_message(&message)
}

fn pixel_format_code(format: CanvasPixelFormat) -> u8 {
    match format {
        CanvasPixelFormat::Rgb => 0,
        CanvasPixelFormat::Rgba => 1,
        CanvasPixelFormat::Jpeg => 2,
    }
}

fn present_bitmap(
    canvas: &HtmlCanvasElement,
    bitmap_ctx: &ImageBitmapRenderingContext,
    event: &MessageEvent,
) -> bool {
    let Some(bitmap) = message_image_bitmap(event) else {
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

#[cfg(test)]
mod tests {
    use crate::ws::CanvasPixelFormat;

    use super::{DispatchDecision, FrameDispatchState, pixel_format_code};

    #[test]
    fn dispatch_state_coalesces_frames_while_one_is_in_flight() {
        let mut state = FrameDispatchState::default();

        assert_eq!(state.push_or_defer(1), DispatchDecision::DispatchNow);
        assert_eq!(state.take_for_dispatch(), Some(1));
        assert_eq!(state.push_or_defer(2), DispatchDecision::Deferred);
        assert_eq!(state.push_or_defer(3), DispatchDecision::Deferred);
        assert_eq!(state.next_after_present(), Some(3));
        assert_eq!(state.next_after_present(), None);
    }

    #[test]
    fn pixel_format_code_handles_jpeg() {
        assert_eq!(pixel_format_code(CanvasPixelFormat::Jpeg), 2);
    }
}
